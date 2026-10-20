//! Inspect the common engine's records for one linked function, independently
//! of callback policy. Numerical addresses only select diagnostic input.
use oer_analysis_riscv_analysis::{
    FunctionInput, PreparedReferences, RecognizedTables, research_following_tables,
};
use oer_analysis_riscv_lift::RiscvDecoder;
use oer_analysis_riscv_model::*;
use oer_analysis_riscv_program::{ProgramView, facts::ImageFacts, tables::ImageTables};

struct Saved<'a>(RecordBuffer<'a>);
impl FunctionSink for Saved<'_> {
    fn record(&mut self, record: &FunctionRecord, control: &mut dyn RunControl) -> Result<()> {
        self.0.push(record.clone(), control.position())
    }
}

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
    let section = image.file().section(function.section)?;
    let start = (u64::from(address) - section.address) as usize;
    let code = section
        .data
        .get(start..start + function.size as usize)
        .ok_or("function exceeds section")?;
    let references = PreparedReferences::new(
        &[],
        function.section as u32,
        &RiscvDecoder,
        &memory,
        &mut control,
    )?;
    let view = ProgramView::new(&bytes, image.file(), &[], &memory, &mut control)?;
    let mut saved = Saved(RecordBuffer::new(&memory));
    research_following_tables(
        FunctionInput {
            image: Some(&view),
            section: function.section as u32,
            extent: CodeRange {
                start: address.into(),
                length: function.size.into(),
            },
            bytes: code,
            relocations: &references,
            data_ranges: &[],
            jumps: &[],
            returns: &NoReturns,
            call_behavior: &UnknownCallBehavior,
        },
        &RecognizedTables {
            entries: &ImageTables { image: &image },
            section: function.section as u32,
        },
        &RiscvDecoder,
        &memory,
        &mut control,
        &mut saved,
        Some(CallAbi::RiscvInteger),
    )?;
    let index = FunctionIndex::new(&saved.0, RecordPolicy::Canonical, &memory, &mut control)?;
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
