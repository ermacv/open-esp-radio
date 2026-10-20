//! Diagnostic orchestration only. Numerical facts are made by the common
//! checked factories, never by the validator's names, offsets or budgets.
use oer_analysis_riscv_analysis::stack_context::*;
use oer_analysis_riscv_model::*;
use oer_analysis_riscv_program::facts::ImageFacts;
use oer_analysis_riscv_stack::*;
use std::{collections::BTreeSet, path::Path};

#[allow(clippy::too_many_arguments)]
pub fn derive<'m>(
    image: &ImageFacts<'_>,
    analysis: &Analysis,
    dwarf: &Dwarf,
    types: &TypeFacts,
    owner: u32,
    memory: &'m WorkingMemory,
    keep: &mut Ledger<'_>,
    c: &mut dyn RunControl,
) -> std::result::Result<Option<StackContext<'m>>, Box<dyn std::error::Error>> {
    use oer_analysis_riscv_lift::StructuralRiscvDecoder;
    use oer_chip_profile::{
        Profile,
        policy::{StackPolicy, Storage},
    };
    let profile = Profile::load(Path::new("."), "esp32s31").map_err(|e| e.to_string())?;
    let policy = StackPolicy::load(Path::new("chips/esp32s31/platform/stack.toml"))
        .map_err(|e| e.to_string())?;
    let task = &policy.stacks().map_err(|e| e.to_string())?.cpu0_task_stack;
    let task_bounds = match task.storage() {
        Storage::Symbol(name) => oer_analysis_elf::StorageBounds::symbol(image.file(), name)?,
        Storage::Range { bottom, top } => {
            oer_analysis_elf::StorageBounds::range(image.file(), bottom, top)?
        }
    }
    .ok_or("missing task storage")?;
    println!("stack-storage\t{:?}\t{task_bounds:?}", task.storage());
    let symbol = |name: &str| {
        image
            .functions()
            .iter()
            .find(|f| f.names.iter().any(|n| n == name))
            .ok_or("missing context function")
    };
    // The existing platform's assembly symbol selects diagnostic input. Its
    // actual return values, not its name, establish both pointers.
    let installer = symbol("_runtime_stack_bootstrap")?;
    let setup_records = super::support::capture_context(image, installer, memory, dwarf, c)?;
    let setup_index = FunctionIndex::new(&setup_records, RecordPolicy::Canonical, memory, c)?;
    let Some(setup) = stack_setup(
        &setup_index,
        installer.address.into(),
        &StructuralRiscvDecoder,
        c,
    )?
    else {
        println!("stack-context-gap\tinstaller machine values are unknown");
        for record in setup_index.records() {
            if matches!(
                record,
                FunctionRecord::ReturnInputs { .. } | FunctionRecord::Value { register: 2, .. }
            ) || !oer_analysis_riscv_analysis::writes::closes_local_graph(record)
            {
                println!("stack-setup-input\t{record:?}");
            }
        }
        return Ok(None);
    };
    println!("stack-setup\t{setup:?}");
    if u64::from(setup.pointer()) != task_bounds.end {
        println!("stack-context-gap\tinstalled sp differs from the task storage top");
        return Ok(None);
    }
    let placement = profile.placement.as_ref().ok_or("missing placement")?;
    let irq = profile
        .interrupts
        .as_ref()
        .ok_or("missing interrupt contract")?;
    let mut irq_storage = None;
    for range in &placement.range {
        let bounds =
            oer_analysis_elf::StorageBounds::range(image.file(), &range.range[0], &range.range[1])?;
        if let Some(bounds) = bounds
            && bounds.end == u64::from(setup.scratch())
            && range.region == "sram"
            && bounds.capacity == u64::from(irq.irq_stack_bytes)
        {
            println!("stack-scratch-storage\t{:?}\t{bounds:?}", range.range);
            irq_storage = Some(bounds);
        }
    }
    if irq_storage.is_none() {
        return Err("installed mscratch matches no placed storage".into());
    }
    let startup = symbol(
        &placement
            .entry
            .as_ref()
            .ok_or("missing image entry")?
            .symbol,
    )?;
    let records = super::support::capture_context(image, startup, memory, dwarf, c)?;
    let startup_index = FunctionIndex::new(&records, RecordPolicy::Canonical, memory, c)?;
    let mut root = None;
    for transfer in &analysis.functions[&startup.address].transfers {
        if transfer.target != Some(installer.address) {
            continue;
        }
        let resumed = resume_stack_context(
            &setup,
            &startup_index,
            startup.address.into(),
            transfer.site.into(),
            memory,
            c,
        )?;
        for tail in &analysis.functions[&startup.address].transfers {
            if tail.target == Some(symbol(&task.root)?.address) {
                root = match resumed.as_ref() {
                    Some(resumed) => call_stack_context(
                        &startup_index,
                        startup.address.into(),
                        tail.site.into(),
                        Some(resumed),
                        memory,
                        c,
                    )?,
                    None => None,
                };
            }
        }
    }
    println!("stack-root\t{}\t{root:?}", task.root);
    let names: Vec<_> = image
        .functions()
        .iter()
        .map(|f| (f.address, oer_analysis_elf::demangle(&f.label())))
        .collect();
    let mut resolutions = image_resolutions(
        image,
        analysis,
        dwarf,
        types,
        names.iter().map(|(a, n)| (*a, n.as_str())),
        keep,
        c,
    )?;
    if let Some(ipc) = &irq.ipc {
        let taken = taken_addresses(image, keep, c)?;
        resolutions.extend(ipc_resolutions(
            analysis,
            &taken,
            names
                .iter()
                .filter(|(_, name)| name == &ipc.post)
                .map(|(address, _)| *address),
            ipc.post_handler_argument,
            names
                .iter()
                .filter(|(_, name)| ipc.dispatch.contains(name))
                .map(|(address, _)| *address),
            keep,
            c,
        )?);
    }
    let vectors = vector_table(image, &irq.vector_table_symbol, c)?
        .into_iter()
        .flatten()
        .map(|entry| trap_entry(image, entry, c))
        .collect::<Result<Vec<_>>>()?;
    let exception = trap_entry(image, symbol(&irq.exception_entry_symbol)?.address, c)?;
    println!(
        "stack-trap-entries\t{}\t{:?}\texception={exception:?}",
        vectors.len(),
        vectors.first()
    );
    let field = |(offset, size): (u32, u32)| Field { offset, size };
    let table = interrupt_table(
        image,
        &irq.table_symbol,
        &TableLayout {
            entry: irq.table_entry_bytes,
            source: field(irq.table_source.into()),
            level: field(irq.table_level.into()),
            core: field(irq.table_core.into()),
            handler: irq.table_handler,
        },
        c,
    )?;
    let sources = u32::try_from(
        image
            .file()
            .address(&irq.source_table_symbol)
            .ok_or("missing IRQ source table")?,
    )?;
    let harts = interrupt_stacks(
        analysis,
        &Stacks {
            table: &table,
            cores: &irq.harts,
            always: &irq.always_levels,
            vectors: &vectors,
            exception,
            sources,
            resolutions: &resolutions,
        },
    )?;
    for hart in harts {
        println!(
            "stack-nesting\thart={}\tbytes={:?}\tpartial={}\tlevel-drops={:?}\tassumptions={:?}",
            hart.core,
            hart.bytes,
            hart.partial,
            hart.level_drops(),
            hart.assumptions()
        );
        for level in hart.levels {
            println!(
                "stack-nesting-level\thart={}\tlevel={}\tbytes={:?}\tpartial={}\tframe={}",
                hart.core, level.level, level.bytes, level.partial, level.frame
            );
        }
    }
    // A prologue certificate measures depth relative to the IRQ position;
    // it does not establish the interrupted sp/mscratch numerical pair.
    println!(
        "stack-irq-gap\ttrap arrival sp/mscratch interval and nesting remain unproved; neither storage capacity nor partial + ? is a numerical root"
    );
    let audit = Paths {
        image,
        analysis,
        dwarf,
        resolutions: &resolutions,
        root: root.as_ref(),
        memory,
    };
    let context = audit.owner(owner, &mut BTreeSet::new(), c)?;
    println!("stack-context-result\t{context:?}");
    Ok(context)
}

struct Paths<'a, 'm> {
    image: &'a ImageFacts<'a>,
    analysis: &'a Analysis,
    dwarf: &'a Dwarf,
    resolutions: &'a Resolutions,
    root: Option<&'a StackContext<'m>>,
    memory: &'m WorkingMemory,
}

impl<'m> Paths<'_, 'm> {
    fn owner(
        &self,
        owner: u32,
        visiting: &mut BTreeSet<u32>,
        c: &mut dyn RunControl,
    ) -> Result<Option<StackContext<'m>>> {
        if let Some(root) = self.root
            && root.owner() == u64::from(owner)
        {
            return merge_stack_contexts(&[Some(root)], self.memory, c);
        }
        if !visiting.insert(owner) {
            println!("stack-context-gap\tcycle at {owner:#010x}");
            return Ok(None);
        }
        let mut contexts = Vec::new();
        for (&address, function) in &self.analysis.functions {
            for transfer in &function.transfers {
                c.checkpoint(1)?;
                let resolved = self.resolutions.get(transfer.site);
                if transfer.target != Some(owner)
                    && !(transfer.target.is_none()
                        && resolved.is_some_and(|r| r.targets.contains(&owner)))
                {
                    continue;
                }
                println!(
                    "stack-incoming\t{address:#010x}\t{:#010x}\t{owner:#010x}\t{:?}",
                    transfer.site,
                    resolved.map(|r| &r.facts)
                );
                let parent = self.owner(address, visiting, c)?;
                let records = super::support::capture_context(
                    self.image,
                    &function.function,
                    self.memory,
                    self.dwarf,
                    c,
                )?;
                let index = FunctionIndex::new(&records, RecordPolicy::Canonical, self.memory, c)?;
                let mut closed = true;
                for record in index.records() {
                    c.checkpoint(1)?;
                    if !oer_analysis_riscv_analysis::writes::closes_local_graph(record) {
                        println!("stack-context-open\t{address:#010x}\t{record:?}");
                        closed = false;
                    }
                    if matches!(
                        record,
                        FunctionRecord::SemanticGap {
                            reason: SemanticGapReason::UnsupportedInstruction,
                            ..
                        }
                    ) {
                        println!("stack-context-semantic\t{address:#010x}\t{record:?}");
                    }
                }
                let sp = index
                    .call_inputs(transfer.site.into(), c)?
                    .and_then(|i| i.registers.get(2));
                let context = if parent.is_some() {
                    call_stack_context_with_targets(
                        &index,
                        address.into(),
                        transfer.site.into(),
                        parent.as_ref(),
                        Some(self.resolutions),
                        Some(owner),
                        self.memory,
                        c,
                    )?
                } else {
                    None
                };
                println!(
                    "stack-context-step\t{address:#010x}\t{:#010x}\tclosed={closed}\tsp={sp:?}\t{context:?}",
                    transfer.site
                );
                contexts.push(context);
            }
        }
        visiting.remove(&owner);
        if contexts.is_empty() {
            println!("stack-context-gap\tno checked numerical root for {owner:#010x}");
        }
        merge_stack_contexts(
            &contexts.iter().map(Option::as_ref).collect::<Vec<_>>(),
            self.memory,
            c,
        )
    }
}
