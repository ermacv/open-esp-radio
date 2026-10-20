//! Reproduce stack-word noninterference, including companion machine facts.
//! PCs and a slot select diagnostic input; they do not select summary semantics.
mod support;
use oer_analysis_riscv_analysis::writes::*;
use oer_analysis_riscv_model::*;
use oer_analysis_riscv_program::facts::ImageFacts;
use oer_analysis_riscv_stack::{DebugMetadata, analyze, parse_summaries, read_debug};
use std::{collections::BTreeSet, rc::Rc};

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let bytes: Rc<[u8]> = std::fs::read(args.next().ok_or(
        "usage: stack-call-writes ELF ROM_ELF STACK_SUMMARIES_TOML CALL_PC_HEX SLOT_DECIMAL",
    )?)?
    .into();
    let rom = std::fs::read(args.next().ok_or("missing companion ELF")?)?;
    let summaries = parse_summaries(&std::fs::read_to_string(
        args.next().ok_or("missing stack summaries")?,
    )?)?;
    let site = args.next().ok_or("missing call PC")?;
    let site = u32::from_str_radix(site.trim_start_matches("0x"), 16)?;
    let slot: i64 = args.next().ok_or("missing slot")?.parse()?;
    if args.next().is_some() {
        return Err("too many arguments".into());
    }
    let memory = WorkingMemory::new(512 * 1024 * 1024)?;
    let mut control = || Ok(());
    let image = ImageFacts::new(&bytes, &[], &memory, &mut control)?;
    let rom = ImageFacts::new(&rom, &[], &memory, &mut control)?;
    let mut keep = Ledger::new(&memory, &mut control)?;
    let (dwarf, _) = read_debug(&bytes, DebugMetadata::Standard, &mut keep, &mut control)?;
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
    let calls = stack_calls(
        &index,
        caller.address.into(),
        Some(CallAbi::RiscvInteger),
        &summaries,
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
