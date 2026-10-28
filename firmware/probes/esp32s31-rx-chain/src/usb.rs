//! The image's USB Serial/JTAG console: the early writer until the console
//! opens, then the duplex endpoint of the core-0 executor and its interrupt,
//! which raises the endpoint's static signal.

use oer_esp32s31_hal::usb_serial_jtag::{
    ConsoleEndpoint, ConsoleInterrupt, RetireError, UsbPads, retire_early_writer,
};
use oer_espressif_executor::Executor;
use oer_espressif_interrupt_table::{Controller, Router};
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

/// Writes `bytes` as they are on the open console.
pub(crate) async fn text(console: &mut Console, bytes: &[u8]) {
    use embedded_io_async::Write as _;
    let _ = console.write_all(bytes).await;
}

/// Why the console did not open.
pub(crate) enum OpenError {
    /// The early writer kept the block.
    Retire,
}

impl OpenError {
    /// The reason the image's failure names.
    pub(crate) const fn reason(&self) -> oer_diagnostics::Name<32> {
        match self {
            Self::Retire => oer_diagnostics::name!("console-retire"; 32),
        }
    }
}

/// Opens the console over the block the early writer retires: the router
/// moves the interrupt's part to its handler and routes it. The pads stay
/// apart, held so that nothing switches them off.
pub(crate) fn open<C: Controller>(
    router: &mut Router<C>,
    token: crate::ConsoleUsb,
) -> Result<(Console, UsbPads), OpenError> {
    let (endpoint, interrupt, pads) =
        retire_early_writer().map_err(|_held: RetireError| OpenError::Retire)?;
    let bound =
        router.bind(token, interrupt).unwrap_or_else(|error| {
            oer_espressif_staged_runtime::interrupts::reset_binding::<
                oer_esp32s31_hal::platform::Port,
                _,
            >(error)
        });
    Ok((Endpoint::new(endpoint, INTERRUPT.signal(), &bound), pads))
}
