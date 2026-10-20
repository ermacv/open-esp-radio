//! Inspect the common engine's records for one linked function, independently
//! of callback policy. Numerical addresses only select diagnostic input.
mod support;
use oer_analysis_riscv_model::*;
use oer_analysis_riscv_program::facts::ImageFacts;

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let bytes = std::fs::read(args.next().ok_or("usage: function-records ELF PC_HEX")?)?;
    let address = args.next().ok_or("missing function address")?;
    let address = u32::from_str_radix(address.trim_start_matches("0x"), 16)?;
    if args.next().is_some() {
        return Err("too many arguments".into());
    }
    let memory = WorkingMemory::new(512 * 1024 * 1024)?;
    let mut control = || Ok(());
    let image = ImageFacts::new(&bytes, &[], &memory, &mut control)?;
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
        &UnknownCallBehavior,
        false,
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
    Ok(())
}
