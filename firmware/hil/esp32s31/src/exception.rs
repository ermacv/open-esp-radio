//! Fatal CPU exception reporting independent of the executor and USB logger.

use oer_diagnostics::Note;
use oer_espressif_staged_runtime::trap::{self, TrapFrame};

/// Print the task stack's watchpoint, which the runtime's handoff armed on
/// the stack's bottom word. HAL reserves watchpoint zero for stack monitoring
/// and respects debugger ownership.
pub fn report_stack_guard() {
    let (control, address) = trap::stack_watchpoint();
    let hart = esp_hal::system::Cpu::current() as usize;
    oer_espressif_usb_fifo::note(&Note::stack_guard(
        hart as u32,
        control as u32,
        address as u32,
    ));
}

oer_espressif_staged_runtime::exception_handler!(exception);

/// A fatal exception cannot rely on a live executor or acquire a logger
/// lock that the interrupted code may already hold: it records the
/// machine state without formatting and resets; the next boot reports it.
#[inline(always)]
fn exception(context: &TrapFrame) -> ! {
    crate::fatal::record_exception(context.ra as u32)
}
