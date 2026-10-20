//! Inspect the common engine's records for one linked function, independently
//! of callback policy. Numerical addresses only select diagnostic input.
mod support;
use oer_analysis_riscv_analysis::writes::function_writes;
use oer_analysis_riscv_model::*;
use oer_analysis_riscv_program::facts::ImageFacts;
use oer_analysis_riscv_stack::{DebugMetadata, Field, TableLayout, interrupt_table, read_debug};
use serde::Deserialize;
use std::rc::Rc;

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let bytes: Rc<[u8]> = std::fs::read(args.next().ok_or(
        "usage: function-records ELF PC_HEX [--symbolic-spills] [--writes] [--image-facts] [--inline-context] [--interrupt-contract CHIP_TOML]",
    )?)?
    .into();
    let address = args.next().ok_or("missing function address")?;
    let address = u32::from_str_radix(address.trim_start_matches("0x"), 16)?;
    let (mut symbolic_spills, mut image_facts, mut inline_context) = (false, false, false);
    let mut writes = false;
    let mut interrupt_contract = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--symbolic-spills" => symbolic_spills = true,
            "--writes" => writes = true,
            "--image-facts" => image_facts = true,
            "--inline-context" => inline_context = true,
            "--interrupt-contract" => {
                interrupt_contract = Some(args.next().ok_or("missing chip contract path")?);
            }
            _ => return Err(format!("unknown option: {arg}").into()),
        }
    }
    let memory = WorkingMemory::new(512 * 1024 * 1024)?;
    let mut control = || Ok(());
    let image = ImageFacts::new(&bytes, &[], &memory, &mut control)?;
    if let Some(path) = interrupt_contract {
        let contract: Contract = toml::from_str(&std::fs::read_to_string(path)?)?;
        let irq = contract.interrupts;
        let field = |[offset, size]: [u32; 2]| Field { offset, size };
        let layout = TableLayout {
            entry: irq.table_entry_bytes,
            source: field(irq.table_source),
            level: field(irq.table_level),
            core: field(irq.table_core),
            handler: irq.table_handler,
        };
        let table = image
            .file()
            .symbol(&irq.table_symbol)
            .ok_or("missing IRQ table")?;
        let first = image
            .read_only_word(u32::try_from(table.address)?)
            .ok_or("missing IRQ entries")?;
        println!(
            "irq-table\t{:#010x}\t{first:#010x}\t{:?}",
            table.address, layout
        );
        for (index, entry) in interrupt_table(&image, &irq.table_symbol, &layout, &mut control)?
            .iter()
            .enumerate()
        {
            let word = u64::from(first)
                + index as u64 * u64::from(layout.entry)
                + u64::from(layout.handler);
            println!("irq-entry\t{index}\t{word:#010x}\t{entry:?}");
        }
    }
    if image_facts {
        for symbol in image.file().symbols().filter(|s| s.defined && s.size != 0) {
            println!(
                "symbol\t{:#010x}\t{}\t{:?}\t{}",
                symbol.address,
                symbol.size,
                symbol.kind,
                oer_analysis_elf::demangle(symbol.name)
            );
        }
        for (index, use_) in image.address_uses().iter().enumerate() {
            println!("address-use\t{index}\t{use_:?}");
        }
        for &(address, value) in image.words() {
            println!(
                "image-word\t{address:#010x}\t{value:#010x}\tread-only={:?}",
                image.read_only_word(address)
            );
        }
    }
    let mut keep = Ledger::new(&memory, &mut control)?;
    let debug = inline_context
        .then(|| read_debug(&bytes, DebugMetadata::Standard, &mut keep, &mut control))
        .transpose()?;
    let function = image
        .functions()
        .iter()
        .find(|f| address >= f.address && address - f.address < f.size)
        .ok_or("no containing function")?;
    let address = function.address;
    println!(
        "function\t{address:#010x}\t{}",
        oer_analysis_elf::demangle(&function.label())
    );
    let records = support::capture(
        &image,
        function,
        &memory,
        debug
            .as_ref()
            .map_or(&UnknownCallBehavior as &dyn CallBehavior, |(dwarf, _)| {
                dwarf
            }),
        symbolic_spills,
        &mut control,
    )?;
    let index = FunctionIndex::new(&records, RecordPolicy::Canonical, &memory, &mut control)?;
    for record in index.records() {
        match record {
            FunctionRecord::Transfer { offset, .. } => {
                println!(
                    "transfer\t{offset:#010x}\t{:?}",
                    index.transfer(*offset, &mut control)?.map(|t| t.target)
                );
                println!(
                    "inputs\t{offset:#010x}\t{:?}",
                    index
                        .call_inputs(*offset, &mut control)?
                        .map(|i| i.registers)
                );
                let register = index.records().iter().find_map(|r| match r {
                    FunctionRecord::Instruction {
                        offset: at,
                        decoded,
                        ..
                    } if at == offset => decoded
                        .flow
                        .through_register()
                        .map(|(register, _, _)| register),
                    _ => None,
                });
                if let Some(register) = register {
                    println!(
                        "target-value\t{offset:#010x}\tx{register}\t{:?}",
                        index
                            .call_inputs(*offset, &mut control)?
                            .and_then(|i| i.registers.get(register as usize))
                    );
                }
            }
            FunctionRecord::Expression { id, offset, .. } => println!(
                "expression\t{id}\t{offset:#010x}\t{:?}",
                index.expression(*id, &mut control)?
            ),
            FunctionRecord::AlignedValue { fact } => println!("aligned-value\t{fact:?}"),
            FunctionRecord::Instruction {
                offset, decoded, ..
            } => {
                println!("instruction\t{offset:#010x}\t{}", decoded.text);
                if let Some((dwarf, _)) = &debug {
                    println!(
                        "inline-context\t{offset:#010x}\t{:?}",
                        dwarf.inline_chain(*offset as u32)?
                    );
                }
            }
            FunctionRecord::Value {
                offset,
                register,
                value,
                ..
            } => {
                println!("register-write\t{offset:#010x}\t{register}\t{value:?}");
            }
            FunctionRecord::Edge { .. }
            | FunctionRecord::Gap { .. }
            | FunctionRecord::SemanticGap { .. } => println!("graph\t{record:?}"),
            _ => {}
        }
    }
    index.accesses(&mut control, &mut |access, _| {
        println!(
            "memory\t{:#010x}\t{:?}\t{}\t{:?}\t{:?}",
            access.offset, access.access, access.width, access.address, access.value
        );
        Ok(())
    })?;
    if writes {
        let summary = function_writes(
            &index,
            address.into(),
            Some(CallAbi::RiscvInteger),
            &[],
            &memory,
            &mut control,
        )?;
        println!(
            "write-summary\tclosed={}\tunknown-effects={}",
            summary.closed_graph,
            summary.has_unknown_memory_effects()
        );
        for write in summary.writes.iter() {
            println!("write-event\t{:#010x}\t{write:?}", write.site);
        }
        for reload in summary.reloads.iter() {
            println!("stack-reload\t{:#010x}\t{reload:?}", reload.site);
        }
        for barrier in summary.barriers.iter() {
            println!("write-barrier\t{:#010x}\t{barrier:?}", barrier.site);
        }
    }
    Ok(())
}

// Read only the existing platform table layout; the stack library validates
// the image's immutable slice and every relocated handler word.
#[derive(Deserialize)]
struct Contract {
    interrupts: InterruptContract,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct InterruptContract {
    table_symbol: String,
    table_entry_bytes: u32,
    table_source: [u32; 2],
    table_level: [u32; 2],
    table_core: [u32; 2],
    table_handler: u32,
}
