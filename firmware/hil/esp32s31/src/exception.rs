//! Fatal CPU exception reporting independent of the executor and USB logger.

use oer_esp32s31_board::rom;
use oer_espressif_staged_runtime::trap::{self, TrapFrame};

/// Print the task stack's watchpoint, which the runtime's handoff armed on
/// the stack's bottom word. HAL reserves watchpoint zero for stack monitoring
/// and respects debugger ownership.
pub fn report_stack_guard() {
    let (control, address) = trap::stack_watchpoint();
    let hart = esp_hal::system::Cpu::current() as usize;
    rom::print_fmt(format_args!(
        "OPEN_RADIO_HIL stack_guard hart={hart} control={control:08x} address={address:08x}\r\n"
    ));
}

oer_espressif_staged_runtime::exception_handler!(exception);

oer_espressif_staged_runtime::sram_fn! {
    in rwtext "exception";
    /// A fatal exception cannot rely on a live executor or acquire a logger
    /// lock that the interrupted code may already hold: it records the
    /// machine state without formatting and resets; the next boot reports it.
    #[inline(always)]
    fn exception(context: &TrapFrame) -> ! {
        crate::fatal::record_exception(context.ra as u32)
    }
}
