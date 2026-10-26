//! The image's USB Serial/JTAG console: the early writer until the console
//! opens, then the duplex endpoint of the core-0 executor and its interrupt,
//! which raises the endpoint's static signal.

use oer_esp32s31_hal::usb_serial_jtag::{
    ConsoleEndpoint, ConsoleInterrupt, UsbPads, retire_early_writer,
};
use oer_espressif_executor::Executor;
use oer_espressif_staged_runtime::panic::PanicRecord;
use oer_espressif_usb_serial_jtag::{Endpoint, Interrupt};
use oer_interrupt_table::{Controller, Router};

/// The console's endpoint, polled by the core-0 executor's task.
pub(crate) type Console = Endpoint<Executor<0>, ConsoleEndpoint>;

oer_espressif_staged_runtime::placed_static! {
    /// The console's interrupt: the endpoint's signal.
    static INTERRUPT: Interrupt<Executor<0>, ConsoleInterrupt> = Interrupt::new(), in critical_data "console_usb";
}

/// The console's `Hardware` interrupt handler, over the part its entry
/// holds: it acknowledges the events it read and raises them to the
/// console's task, calling no waker.
pub(crate) fn interrupt(part: &mut ConsoleInterrupt) {
    INTERRUPT.service(part);
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

/// Writes `bytes` as they are before the console opens.
pub(crate) fn early_text(bytes: &[u8]) {
    let _ = oer_esp32s31_hal::usb_serial_jtag::early_text(bytes);
}

/// Opens the console over the block the early writer retires: the router
/// moves the interrupt's part to its handler and routes it. The pads stay
/// apart, held so that nothing switches them off.
pub(crate) fn open<C: Controller>(
    router: &mut Router<C>,
    token: crate::ConsoleUsb,
) -> Result<(Console, UsbPads), &'static str> {
    let (endpoint, interrupt, pads) = retire_early_writer().map_err(|_| "console")?;
    let bound = router
        .bind(token, interrupt)
        .map_err(|_| "console interrupt")?;
    Ok((Endpoint::new(endpoint, INTERRUPT.signal(), &bound), pads))
}
