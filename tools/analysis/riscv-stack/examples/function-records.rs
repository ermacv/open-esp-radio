//! Inspect the common engine's records for one linked function, independently
//! of callback policy. Numerical addresses only select diagnostic input.
#[path = "support/cli.rs"]
mod cli;
mod support;
// Compile the actual stack observer and sweep into this diagnostic only.
#[allow(dead_code)]
#[path = "../src/image.rs"]
mod image;
#[path = "support/observation.rs"]
mod observation;
#[allow(dead_code)]
#[path = "../src/sweep.rs"]
mod sweep;
use clap::Parser;
use oer_analysis_riscv_analysis::writes::function_writes;
use oer_analysis_riscv_model::*;
use oer_analysis_riscv_program::facts::ImageFacts;
use oer_analysis_riscv_stack::{DebugMetadata, Field, TableLayout, interrupt_table, read_debug};
use serde::Deserialize;
use std::rc::Rc;
use sweep::TransferKind;

#[derive(Debug, Parser)]
#[command(
    name = "function-records",
    about = "Inspect one linked function's records",
    args_override_self = true
)]
struct Cli {
    elf: std::path::PathBuf,
    #[arg(value_name = "PC_HEX", value_parser = |value: &str| u32::from_str_radix(value.trim_start_matches("0x"), 16))]
    address: u32,
    #[arg(long)]
    symbolic_spills: bool,
    #[arg(long)]
    writes: bool,
    #[arg(long)]
    image_facts: bool,
    #[arg(long)]
    inline_context: bool,
    #[arg(long)]
    stack_observation: bool,
    #[arg(long, value_name = "CHIP_TOML", allow_hyphen_values = true)]
    interrupt_contract: Option<std::path::PathBuf>,
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let Cli {
        elf,
        address,
        symbolic_spills,
        writes,
        image_facts,
        inline_context,
        stack_observation,
        interrupt_contract,
    } = cli::parse();
    let bytes: Rc<[u8]> = std::fs::read(elf)?.into();
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
    let debug = (inline_context || stack_observation)
        .then(|| {
            let metadata = if stack_observation {
                DebugMetadata::TypedLocations
            } else {
                DebugMetadata::Standard
            };
            read_debug(&bytes, metadata, &mut keep, &mut control)
        })
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
    let call_behavior = debug
        .as_ref()
        .map_or(&UnknownCallBehavior as &dyn CallBehavior, |(dwarf, _)| {
            dwarf
        });
    if stack_observation {
        observation::print(&image, function, call_behavior, &memory, &mut control)?;
    }
    let records = support::capture(
        &image,
        function,
        &memory,
        call_behavior,
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
            FunctionRecord::ReturnInputs { offset, registers } => {
                println!("return-inputs\t{offset:#010x}\t{registers:?}")
            }
            FunctionRecord::AlignedValue { fact } => println!("aligned-value\t{fact:?}"),
            FunctionRecord::Dispatch {
                offset,
                base,
                index: table_index,
                scale,
                displacement,
            } => {
                println!(
                    "dispatch\t{offset:#010x}\t{base:?}\t{table_index:?}\t{scale}\t{displacement}"
                );
                println!(
                    "dispatch-inputs\t{offset:#010x}\t{:?}",
                    index
                        .call_inputs(*offset, &mut control)?
                        .map(|inputs| inputs.registers)
                );
                if image_facts {
                    let dump = |base: &AbstractValue| {
                        if let AbstractValue::Constant { value: base }
                        | AbstractValue::ImageAddress { address: base } = base
                            && let Some(words) = image.jump_table(*base)
                        {
                            for (i, word) in words.enumerate() {
                                let address = base.wrapping_add(4 * i as u32);
                                println!(
                                    "dispatch-word\t{offset:#010x}\t{address:#010x}\t{word:#010x}\t{:?}\t{:?}",
                                    image.word(address),
                                    image.read_only_word(address)
                                );
                            }
                        }
                    };
                    if let AbstractValue::Alternatives { values } = base {
                        for value in values.values() {
                            dump(&value.as_value());
                        }
                    } else {
                        dump(base);
                    }
                }
            }
            FunctionRecord::Instruction {
                offset, decoded, ..
            } => {
                println!("instruction\t{offset:#010x}\t{}", decoded.text);
                if inline_context && let Some((dwarf, _)) = &debug {
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
    println!(
        "cfg-closed\t{}",
        index
            .records()
            .iter()
            .all(oer_analysis_riscv_analysis::writes::closes_local_graph)
    );
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory as _;

    #[test]
    fn previous_diagnostic_flags_and_repetitions_parse() {
        Cli::command().debug_assert();
        let cli = Cli::try_parse_from(["function-records", "image.elf", "0x2f80d260"])
            .expect("accepted example command line");
        assert_eq!(cli.address, 0x2f80d260);
        assert!(
            !(cli.symbolic_spills
                || cli.writes
                || cli.image_facts
                || cli.inline_context
                || cli.stack_observation)
        );
        assert!(cli.interrupt_contract.is_none());
        let cli = Cli::try_parse_from([
            "function-records",
            "image.elf",
            "2F80D260",
            "--symbolic-spills",
            "--writes",
            "--image-facts",
            "--inline-context",
            "--stack-observation",
            "--writes",
            "--interrupt-contract",
            "old.toml",
            "--interrupt-contract",
            "chip.toml",
        ])
        .expect("accepted example command line");
        assert!(
            cli.symbolic_spills
                && cli.writes
                && cli.image_facts
                && cli.inline_context
                && cli.stack_observation
        );
        assert_eq!(cli.interrupt_contract, Some("chip.toml".into()));
        assert_eq!(cli.address, 0x2f80d260);
        for arguments in [
            vec!["function-records", "image.elf"],
            vec!["function-records", "image.elf", "not-a-pc"],
            vec![
                "function-records",
                "image.elf",
                "1234",
                "--interrupt-contract",
            ],
            vec!["function-records", "image.elf", "1234", "--bad"],
        ] {
            assert!(Cli::try_parse_from(&arguments).is_err(), "{arguments:?}");
        }
    }
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
