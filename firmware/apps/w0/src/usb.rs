//! The image's USB Serial/JTAG console: the early writer until the console
//! opens, then the duplex endpoint of the core-0 executor and its interrupt,
//! which raises the endpoint's static signal.

use oer_esp32s31_hal::usb_serial_jtag::{
    ConsoleEndpoint, ConsoleInterrupt, UsbPads, retire_early_writer,
};
use oer_espressif_executor::Executor;
use oer_espressif_interrupt_table::{Controller, Router};
use oer_espressif_staged_runtime::panic::PanicRecord;
use oer_espressif_usb_serial_jtag::{Endpoint, Interrupt};

/// The console's endpoint, polled by the core-0 executor's task.
pub(crate) type Console = Endpoint<Executor<0>, ConsoleEndpoint>;

oer_espressif_staged_runtime::placed_static! {
    /// The console's interrupt: the endpoint's signal.
    static INTERRUPT: Interrupt<Executor<0>, ConsoleInterrupt> = Interrupt::new::<crate::ConsoleUsb, crate::WakeToken>(), in critical_data "console_usb";
}

/// The console's `Hardware` interrupt handler, over the part its entry
/// holds: it acknowledges the events it read and raises them to the
/// console's task, calling no waker.
pub(crate) fn interrupt(part: &mut ConsoleInterrupt) {
    INTERRUPT.service(part);
}

/// Writes `note` before the console opens; the early writer waits for room
/// within its budget and counts what it drops.
pub(crate) fn early(note: &oer_diagnostics::Note<'_>) {
    let _ = oer_esp32s31_hal::usb_serial_jtag::early_note(note);
}

/// Writes the previous boot's panic before the console opens.
pub(crate) fn early_previous_panic(record: &PanicRecord) {
    let _ = oer_espressif_staged_runtime::diagnostics::emit_previous_panic(
        |bytes| {
            if oer_esp32s31_hal::usb_serial_jtag::early_text(bytes) {
                Ok(())
            } else {
                Err(())
            }
        },
        record,
    );
}

/// Opens the console over the block the early writer retires: the router
/// moves the interrupt's part to its handler and routes it. The pads stay
/// apart, held so that nothing switches them off.
pub(crate) fn open<C: Controller>(
    router: &mut Router<C>,
    token: crate::ConsoleUsb,
) -> (Console, UsbPads) {
    let Ok((endpoint, interrupt, pads)) = retire_early_writer() else {
        crate::fail(oer_diagnostics::name!("console-retire"; 32))
    };
    let bound = router
        .bind(token, interrupt)
        .unwrap_or_else(|error| oer_espressif_staged_runtime::interrupts::reset_binding(error));
    (Endpoint::new(endpoint, INTERRUPT.signal(), &bound), pads)
}
