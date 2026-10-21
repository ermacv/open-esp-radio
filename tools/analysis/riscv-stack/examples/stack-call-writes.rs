//! Reproduce stack-word noninterference, including companion machine facts.
//! PCs and a slot select diagnostic input; they do not select summary semantics.
#[path = "support/stack_context.rs"]
mod checked_stack;
#[path = "support/cli.rs"]
mod cli;
mod support;
use clap::Parser;
use oer_analysis_riscv_analysis::{FunctionInput, PreparedReferences, writes::*};
use oer_analysis_riscv_lift::StructuralRiscvDecoder;
use oer_analysis_riscv_model::*;
use oer_analysis_riscv_program::ProgramView;
use oer_analysis_riscv_program::facts::ImageFacts;
use oer_analysis_riscv_stack::{DebugMetadata, analyze, parse_summaries, read_debug};
use std::{collections::BTreeSet, rc::Rc};

#[derive(Debug, Parser)]
#[command(
    name = "stack-call-writes",
    about = "Inspect stack-word noninterference"
)]
struct Cli {
    elf: std::path::PathBuf,
    rom_elf: std::path::PathBuf,
    stack_summaries_toml: std::path::PathBuf,
    #[arg(value_name = "CALL_PC_HEX", value_parser = |value: &str| u32::from_str_radix(value.trim_start_matches("0x"), 16))]
    site: u32,
    #[arg(value_name = "SLOT_DECIMAL", allow_negative_numbers = true)]
    slot: i64,
    /// Query one four-byte normal-return provenance relation.
    #[arg(long)]
    word_offset: Option<u32>,
    /// Derive the current platform's machine stack context; accepts no interval.
    #[arg(long)]
    checked_stack_context: bool,
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let cli: Cli = cli::parse();
    let bytes: Rc<[u8]> = std::fs::read(cli.elf)?.into();
    let rom = std::fs::read(cli.rom_elf)?;
    let summaries = parse_summaries(&std::fs::read_to_string(cli.stack_summaries_toml)?)?;
    let site = cli.site;
    let slot = cli.slot;
    let word_query = cli.word_offset.map(|offset| WordQuery {
        destination: 0,
        source: 1,
        length: 2,
        offset,
    });
    let memory = WorkingMemory::new(512 * 1024 * 1024)?;
    let mut control = || Ok(());
    let image = ImageFacts::new(&bytes, &[], &memory, &mut control)?;
    let rom = ImageFacts::new(&rom, &[], &memory, &mut control)?;
    let mut keep = Ledger::new(&memory, &mut control)?;
    let (dwarf, types) = read_debug(
        &bytes,
        if cli.checked_stack_context {
            DebugMetadata::TypedLocations
        } else {
            DebugMetadata::Standard
        },
        &mut keep,
        &mut control,
    )?;
    // The existing joint stack analysis validates companion identities and
    // reviewed frame/call contracts. These are not memory-effect contracts.
    let analysis = analyze(&image, &[&rom], &summaries, &dwarf, &mut keep, &mut control)?;
    let caller = image
        .functions()
        .iter()
        .find(|f| site >= f.address && site - f.address < f.size)
        .ok_or("no caller")?;
    let before = support::capture(&image, caller, &memory, &dwarf, true, &mut control)?;
    let index = FunctionIndex::new(&before, RecordPolicy::Canonical, &memory, &mut control)?;
    let stack_context = if cli.checked_stack_context {
        checked_stack::derive(
            &image,
            &analysis,
            &dwarf,
            &types,
            caller.address,
            &memory,
            &mut keep,
            &mut control,
        )?
    } else {
        None
    };
    println!(
        "caller\t{:#010x}\t{}",
        caller.address,
        oer_analysis_elf::demangle(&caller.label())
    );
    println!(
        "target\t{site:#010x}\t{:?}",
        index.transfer(site.into(), &mut control)?.map(|t| t.target)
    );
    println!(
        "inputs\t{site:#010x}\t{:?}",
        index
            .call_inputs(site.into(), &mut control)?
            .map(|i| i.registers)
    );
    let mut pending = Vec::new();
    if let Some(transfer) = index.transfer(site.into(), &mut control)? {
        add_targets(transfer.target, &mut pending);
    }
    let mut seen = BTreeSet::new();
    let mut captures = Vec::new();
    while let Some(target) = pending.pop() {
        if !seen.insert(target) {
            continue;
        }
        let Some(function) = rom.functions().iter().find(|f| f.address == target) else {
            println!("missing-callee\t{target:#010x}");
            continue;
        };
        let records = support::capture(
            &rom,
            function,
            &memory,
            &UnknownCallBehavior,
            true,
            &mut control,
        )?;
        if let Some(facts) = analysis.functions.get(&target) {
            println!(
                "stack-facts\t{target:#010x}\tframe={:?}\tsource={:?}\ttransfers={:?}",
                facts.frame, facts.source, facts.transfers
            );
        }
        for record in records.iter() {
            match record {
                FunctionRecord::Instruction {
                    offset,
                    bytes,
                    decoded,
                } => println!(
                    "instruction\t{offset:#010x}\t{bytes:02x?}\t{}\t{:?}",
                    decoded.text, decoded.flow
                ),
                FunctionRecord::Transfer {
                    offset,
                    target,
                    call,
                } => {
                    println!("rom-transfer\t{offset:#010x}\tcall={call}\t{target:?}");
                    add_targets(target, &mut pending);
                }
                FunctionRecord::Value {
                    offset,
                    register,
                    value,
                    ..
                } => println!("register-write\t{offset:#010x}\t{register}\t{value:?}"),
                FunctionRecord::CallInputs { offset, registers } => {
                    println!("rom-inputs\t{offset:#010x}\t{registers:?}");
                }
                FunctionRecord::Edge { .. } => println!("rom-edge\t{target:#010x}\t{record:?}"),
                FunctionRecord::SemanticGap { offset, reason } => {
                    println!("rom-gap\t{offset:#010x}\t{reason:?}");
                }
                FunctionRecord::MemoryAccess {
                    offset,
                    access,
                    width,
                    address,
                    ..
                } => println!("rom-memory\t{offset:#010x}\t{access:?}\t{width}\t{address:?}"),
                FunctionRecord::StoreAddressRange { offset, .. } => {
                    println!("rom-loop-bound\t{offset:#010x}\t{record:?}");
                }
                _ => (),
            }
        }
        captures.push((target, function, records));
    }
    // Capture each function once. Supply already available summaries to the
    // same local summarizer; it independently limits composition to one hop.
    let mut writes = Vec::new();
    for (target, function, records) in captures.iter().rev() {
        let facts = FunctionIndex::new(records, RecordPolicy::Canonical, &memory, &mut control)?;
        let available: Vec<_> = writes.iter().collect();
        let summary = function_writes(
            &facts,
            u64::from(*target),
            Some(CallAbi::RiscvInteger),
            &available,
            &memory,
            &mut control,
        )?;
        println!(
            "callee\t{target:#010x}\t{}\tclosed={}\tunknown-effects={}",
            function.label(),
            summary.closed_graph,
            summary.has_unknown_memory_effects()
        );
        println!("tail-proof\t{target:#010x}\t{:?}", summary.tail);
        for write in summary.writes.iter() {
            println!("rom-write\t{:#010x}\t{target:#010x}\t{write:?}", write.site);
        }
        for barrier in summary.barriers.iter() {
            println!(
                "rom-barrier\t{:#010x}\t{target:#010x}\t{barrier:?}",
                barrier.site
            );
        }
        writes.push(summary);
    }
    let summaries: Vec<_> = writes.iter().collect();
    let applied = call_writes(
        &index,
        caller.address.into(),
        site.into(),
        Some(CallAbi::RiscvInteger),
        &summaries,
        &memory,
        &mut control,
    )?;
    println!(
        "preserves\t{site:#010x}\tslot={slot}\t{}",
        applied.preserves_stack_word(caller.address.into(), slot, &mut control)?
    );
    for write in applied.writes.iter() {
        println!("call-write\t{:#010x}\t{write:?}", write.write.site);
        // Query this event's footprint separately. This cannot establish
        // preservation through the full call, whose other barriers remain.
        let mut event = CallWrites {
            writes: AdmittedVec::new(&memory),
            barriers: AdmittedVec::new(&memory),
        };
        event.writes.push(*write, control.position())?;
        println!(
            "call-write-excludes-spill\t{:#010x}\t{}",
            write.write.site,
            event.preserves_stack_word(caller.address.into(), slot, &mut control)?
        );
    }
    for barrier in applied.barriers.iter() {
        println!("call-barrier\t{:#010x}\t{barrier:?}", barrier.site);
    }
    let mut specialized = Vec::new();
    if let Some(transfer) = index.transfer(site.into(), &mut control)?
        && let AbstractValue::ImageAddress { address } = transfer.target
        && let Some(generic) = writes.iter().find(|s| s.entry == u64::from(*address))
        && let Some(context) = call_context(
            &index,
            caller.address.into(),
            site.into(),
            Some(CallAbi::RiscvInteger),
            generic,
            &memory,
            &mut control,
        )?
    {
        println!("context\t{context:?}");
        let view = ProgramView::new(rom.bytes(), rom.file(), &[], &memory, &mut control)?;
        for (target, function, records) in captures.iter().rev() {
            let available: Vec<_> = specialized.iter().collect();
            let summary = if u64::from(*target) == context.entry() {
                let section = rom.file().section(function.section)?;
                let start = u64::from(function.address)
                    .checked_sub(section.address)
                    .ok_or("function before its section")? as usize;
                let code = section
                    .data
                    .get(start..start + function.size as usize)
                    .ok_or("function exceeds its section")?;
                let input = FunctionInput {
                    traps: oer_analysis_riscv_analysis::TrapSemantics::Unknown,
                    image: Some(&view),
                    section: function.section as u32,
                    extent: CodeRange {
                        start: function.address.into(),
                        length: function.size.into(),
                    },
                    bytes: code,
                    relocations: &PreparedReferences::empty(),
                    data_ranges: &[],
                    jumps: &[],
                    returns: &NoSummaries,
                    call_behavior: &UnknownCallBehavior,
                };
                if let Some(query) = word_query {
                    contextual_word_function_writes(
                        input,
                        &StructuralRiscvDecoder,
                        &context,
                        query,
                        &available,
                        &memory,
                        &mut control,
                    )?
                } else {
                    contextual_function_writes(
                        input,
                        &StructuralRiscvDecoder,
                        &context,
                        &available,
                        &memory,
                        &mut control,
                    )?
                }
            } else {
                let facts =
                    FunctionIndex::new(records, RecordPolicy::Canonical, &memory, &mut control)?;
                function_writes(
                    &facts,
                    u64::from(*target),
                    Some(CallAbi::RiscvInteger),
                    &available,
                    &memory,
                    &mut control,
                )?
            };
            if let Some(proof) = summary.specialization() {
                for branch in proof.branches() {
                    println!("context-branch\t{target:#010x}\t{branch:?}");
                }
                for pc in proof.unreachable() {
                    println!("context-unreachable\t{target:#010x}\t{pc:#010x}");
                }
            }
            if let Some(proof) = summary.word_postcondition() {
                println!("post-word\t{target:#010x}\t{proof:?}");
                for event in proof.events() {
                    println!("word-event\t{target:#010x}\t{event:?}");
                }
            }
            specialized.push(summary);
        }
        let available: Vec<_> = specialized.iter().collect();
        if word_query.is_some()
            && let Some(summary) = specialized
                .iter()
                .find(|summary| summary.entry == u64::from(*address))
        {
            if let Ok(instance) = instantiate_word_with_stack(
                &index,
                caller.address.into(),
                site.into(),
                Some(CallAbi::RiscvInteger),
                summary,
                stack_context.as_ref(),
                &memory,
                &mut control,
            )? {
                println!(
                    "word-instance\t{:#010x}\t{:#010x}\t{:?}\t{:?}",
                    instance.caller(),
                    instance.site(),
                    instance.contents(),
                    instance.barrier()
                );
            }
            println!(
                "word-application\t{:?}",
                call_word_with_stack(
                    &index,
                    caller.address.into(),
                    site.into(),
                    Some(CallAbi::RiscvInteger),
                    summary,
                    stack_context.as_ref(),
                    &memory,
                    &mut control
                )?
            );
        }
        let applied = call_writes(
            &index,
            caller.address.into(),
            site.into(),
            Some(CallAbi::RiscvInteger),
            &available,
            &memory,
            &mut control,
        )?;
        for write in applied.writes.iter() {
            println!("context-call-write\t{:#010x}\t{write:?}", write.write.site);
        }
        for barrier in applied.barriers.iter() {
            println!("context-call-barrier\t{:#010x}\t{barrier:?}", barrier.site);
        }
        println!(
            "context-preserves\t{}",
            applied.preserves_stack_word(caller.address.into(), slot, &mut control)?
        );
    }
    let available: Vec<_> = if specialized.is_empty() {
        writes.iter().collect()
    } else {
        specialized.iter().collect()
    };
    let calls = stack_calls(
        &index,
        caller.address.into(),
        Some(CallAbi::RiscvInteger),
        &available,
        &memory,
        &mut control,
    )?;
    let after = support::capture_with_calls(
        &image,
        caller,
        &memory,
        &dwarf,
        true,
        Some(&calls),
        &mut control,
    )?;
    let after = FunctionIndex::new(&after, RecordPolicy::Canonical, &memory, &mut control)?;
    for (profile, facts) in [("before", &index), ("after", &after)] {
        let summary = function_writes(
            facts,
            caller.address.into(),
            Some(CallAbi::RiscvInteger),
            &[],
            &memory,
            &mut control,
        )?;
        for write in summary.writes.iter() {
            if matches!(write.address, WriteValue::Base { base: WriteBase::Stack { offset, .. }, .. } if offset == slot)
            {
                println!("{profile}-spill\t{:#010x}\t{write:?}", write.site);
            }
        }
        for reload in summary.reloads.iter().filter(|r| r.slot == slot) {
            println!("{profile}-reload\t{:#010x}\t{reload:?}", reload.site);
        }
    }
    Ok(())
}

fn add_targets(value: &AbstractValue, pending: &mut Vec<u32>) {
    match value {
        AbstractValue::ImageAddress { address } => pending.push(*address),
        AbstractValue::Alternatives { values } => {
            for value in values.values() {
                if let ValueAlternative::ImageAddress { address } = value {
                    pending.push(*address);
                }
            }
        }
        _ => (), // call_writes retains unknown or mixed alternatives as barriers.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory as _;

    #[test]
    fn previous_hex_pc_and_signed_decimal_slot_forms_parse() {
        Cli::command().debug_assert();
        for (pc, slot, expected) in [("0x2f80d260", "-16", -16), ("2F80D260", "+24", 24)] {
            let cli = Cli::try_parse_from([
                "stack-call-writes",
                "image.elf",
                "rom.elf",
                "summaries.toml",
                pc,
                slot,
            ])
            .expect("accepted example command line");
            assert_eq!(cli.elf, std::path::Path::new("image.elf"));
            assert_eq!(cli.rom_elf, std::path::Path::new("rom.elf"));
            assert_eq!(
                cli.stack_summaries_toml,
                std::path::Path::new("summaries.toml")
            );
            assert_eq!(cli.site, 0x2f80d260);
            assert_eq!(cli.slot, expected);
        }
        for tail in [
            vec![],
            vec!["bad-pc", "0"],
            vec!["1234", "bad-slot"],
            vec!["1234", "0", "extra"],
        ] {
            assert!(
                Cli::try_parse_from(
                    [
                        "stack-call-writes",
                        "image.elf",
                        "rom.elf",
                        "summaries.toml"
                    ]
                    .into_iter()
                    .chain(tail)
                )
                .is_err()
            );
        }
    }
}
