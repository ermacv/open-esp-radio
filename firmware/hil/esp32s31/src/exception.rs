//! Fatal CPU exception reporting independent of the executor and USB logger.

use oer_diagnostics::Note;
use oer_espressif_staged_runtime::trap::{self, TrapFrame};

/// Print the task stack's watchpoint, which the runtime's handoff armed on
/// the stack's bottom word. HAL reserves watchpoint zero for stack monitoring
/// and respects debugger ownership.
pub fn report_stack_guard() {
    let (control, address) = oer_riscv::selected_trigger_data();
    let hart = trap::TrapRegisters::read().hart;
    crate::usb::early(&Note::stack_guard(
        hart as u32,
        control as u32,
        address as u32,
    ));
}

/// A fatal exception cannot rely on a live executor or acquire a logger
/// lock that the interrupted code may already hold: it records the
/// machine state without formatting, with the other hart stopped, and
/// resets; the next boot reports it.
pub(crate) fn capture(
    claim: &oer_espressif_staged_runtime::retained_fault::Claim<'_>,
    frame: &TrapFrame,
    stop: Result<&oer_esp32s31_hal::cores::StopTheWorld<'_>, oer_esp32s31_hal::cores::StopError>,
) {
    crate::fatal::record_exception(claim, frame, stop);
}
