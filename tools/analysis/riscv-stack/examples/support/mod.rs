//! Capture through the existing stack pipeline's table proofs and common engine.
use oer_analysis_riscv_model::*;
use oer_analysis_riscv_program::facts::ImageFacts;

struct Saved<'a>(RecordBuffer<'a>);
impl FunctionSink for Saved<'_> {
    fn record(&mut self, record: &FunctionRecord, control: &mut dyn RunControl) -> Result<()> {
        self.0.push(record.clone(), control.position())
    }
}

pub fn capture<'m>(
    image: &ImageFacts<'_>,
    function: &oer_analysis_elf::Function,
    memory: &'m WorkingMemory,
    call_behavior: &dyn CallBehavior,
    symbolic_spills: bool,
    control: &mut dyn RunControl,
) -> Result<RecordBuffer<'m>> {
    let mut saved = Saved(RecordBuffer::new(memory));
    oer_analysis_riscv_stack::inspect_function(
        image,
        function,
        call_behavior,
        oer_analysis_riscv_analysis::ResearchOptions {
            abi: Some(CallAbi::RiscvInteger),
            symbolic_spills,
        },
        memory,
        control,
        &mut saved,
    )?;
    Ok(saved.0)
}
