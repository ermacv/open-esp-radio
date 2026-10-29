//! The target core's USB Serial/JTAG console on this chip: the early writer
//! until the console opens, then the duplex endpoint of the core-0
//! executor and its interrupt, which raises the endpoint's static signal.

use oer_esp32s31_compose::console::Pads;
use oer_esp32s31_compose::{Esp32s31, console, platform::InterruptTypes};
use oer_espressif_interrupt_table::{Controller, Router};
pub(crate) type Console = console::Console;
oer_espressif_staged_runtime::placed_static! {
    static INTERRUPT: console::Binding = console::Binding::new::<crate::ConsoleUsb,crate::ExecutorWake>(), in critical_data "console_usb";
}
pub(crate) fn interrupt(part: &mut <Esp32s31 as InterruptTypes>::Console) {
    INTERRUPT.interrupt(part);
}

/// Writes `note` before the console opens.
pub(crate) fn early(note: &oer_diagnostics::Note<'_>) -> bool {
    console::early(note)
}

/// Opens the console over the block the early writer retires: the router
/// moves the interrupt's part to its handler and routes it. The pads stay
/// apart, for the image that switches the console off.
pub(crate) fn open<C: Controller>(
    router: &mut Router<C>,
    token: crate::ConsoleUsb,
) -> (Console, Pads) {
    INTERRUPT
        .open(router, token)
        .unwrap_or_else(|_| crate::fail(crate::fatal::Failure::Console))
}
