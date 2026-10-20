//! Inspect a call's write summaries. Addresses select input; never semantics.
mod support;
use oer_analysis_riscv_analysis::writes::*;
use oer_analysis_riscv_model::*;
use oer_analysis_riscv_program::facts::ImageFacts;

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let bytes: std::rc::Rc<[u8]> = std::fs::read(
        args.next()
            .ok_or("usage: call-writes ELF CALL_PC_HEX [--no-symbolic-spills]")?,
    )?
    .into();
    let site = args.next().ok_or("missing call PC")?;
    let site = u32::from_str_radix(site.trim_start_matches("0x"), 16)?;
    let symbolic_spills = match args.next().as_deref() {
        None => true,
        Some("--no-symbolic-spills") => false,
        Some(_) => return Err("unknown option".into()),
    };
    if args.next().is_some() {
        return Err("too many arguments".into());
    }
    let memory = WorkingMemory::new(512 * 1024 * 1024)?;
    let mut control = || Ok(());
    let image = ImageFacts::new(&bytes, &[], &memory, &mut control)?;
    let mut keep = Ledger::new(&memory, &mut control)?;
    let (dwarf, _) = oer_analysis_riscv_stack::read_debug(
        &bytes,
        oer_analysis_riscv_stack::DebugMetadata::Standard,
        &mut keep,
        &mut control,
    )?;
    let caller = image
        .functions()
        .iter()
        .find(|f| site >= f.address && site - f.address < f.size)
        .ok_or("no caller")?;
    let records = support::capture(
        &image,
        caller,
        &memory,
        &dwarf,
        symbolic_spills,
        &mut control,
    )?;
    let index = FunctionIndex::new(&records, RecordPolicy::Canonical, &memory, &mut control)?;
    println!(
        "caller\t{:#010x}\t{}",
        caller.address,
        oer_analysis_elf::demangle(&caller.label())
    );
    println!(
        "inputs\t{site:#010x}\t{:?}",
        index
            .call_inputs(site.into(), &mut control)?
            .map(|i| i.registers)
    );
    let mut targets = Vec::new();
    if let Some(transfer) = index.transfer(site.into(), &mut control)? {
        match transfer.target {
            AbstractValue::ImageAddress { address } => targets.push(*address),
            AbstractValue::Alternatives { values } => {
                for value in values.values() {
                    if let ValueAlternative::ImageAddress { address } = value {
                        targets.push(*address);
                    }
                }
            }
            _ => (),
        }
    }
    let mut summaries = Vec::new();
    for target in targets {
        let Some(callee) = image.functions().iter().find(|f| f.address == target) else {
            continue;
        };
        let records = support::capture(
            &image,
            callee,
            &memory,
            &dwarf,
            symbolic_spills,
            &mut control,
        )?;
        let facts = FunctionIndex::new(&records, RecordPolicy::Canonical, &memory, &mut control)?;
        let summary = function_writes(
            &facts,
            target.into(),
            Some(CallAbi::RiscvInteger),
            &memory,
            &mut control,
        )?;
        println!(
            "callee\t{target:#010x}\t{}\tclosed={}\tunknown-effects={}",
            oer_analysis_elf::demangle(&callee.label()),
            summary.closed_graph,
            summary.has_unknown_memory_effects()
        );
        for write in summary.writes.iter() {
            println!("write\t{:#010x}\t{write:?}", write.site);
        }
        for reload in summary.reloads.iter() {
            println!("reload\t{:#010x}\t{reload:?}", reload.site);
        }
        for barrier in summary.barriers.iter() {
            println!("barrier\t{:#010x}\t{:?}", barrier.site, barrier.kind);
        }
        summaries.push(summary);
    }
    let summaries: Vec<_> = summaries.iter().collect();
    let applied = call_writes(
        &index,
        caller.address.into(),
        site.into(),
        Some(CallAbi::RiscvInteger),
        &summaries,
        &memory,
        &mut control,
    )?;
    for write in applied.writes.iter() {
        println!("applied\t{:#010x}\t{write:?}", write.write.site);
    }
    for barrier in applied.barriers.iter() {
        println!("call-barrier\t{:#010x}\t{:?}", barrier.site, barrier.kind);
    }
    Ok(())
}
