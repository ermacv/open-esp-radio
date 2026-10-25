//! The image's USB Serial/JTAG console: the early writer until the console
//! opens, then the duplex endpoint of the core-0 executor and its interrupt,
//! which raises the endpoint's static signal.

use oer_esp32s31_hal::usb_serial_jtag::{
    ConsoleEndpoint, ConsoleInterrupt, UsbPads, UsbSerialJtagRegisters, console_parts,
};
use oer_espressif_executor::Executor;
use oer_espressif_staged_runtime::panic::PanicRecord;
use oer_espressif_usb_serial_jtag::{Endpoint, Interrupt};

/// The console's endpoint, polled by the core-0 executor's task.
pub(crate) type Console = Endpoint<Executor<0>, ConsoleEndpoint>;

oer_espressif_staged_runtime::placed_static! {
    /// The console's interrupt: its handler's registers and the endpoint's
    /// signal.
    static INTERRUPT: Interrupt<Executor<0>, ConsoleInterrupt> = Interrupt::new(), in critical_data "console_usb";
}

/// The console's `Hardware` interrupt handler: it acknowledges the events
/// it read and raises them to the console's task, calling no waker.
pub(crate) fn interrupt() {
    let _ = INTERRUPT.service();
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

/// Opens the console over the block the esp-hal adapter took from the early
/// writer and esp-hal (`take_usb_serial_jtag`): it gives the handler its
/// registers, then routes the interrupt. The pads stay apart, held so that
/// nothing switches them off.
pub(crate) fn open(
    registers: UsbSerialJtagRegisters,
    route: crate::ConsoleUsb,
) -> (Console, UsbPads) {
    let (endpoint, interrupt, pads) = console_parts(registers);
    let signal = INTERRUPT.install(interrupt);
    let routed = oer_espressif_esp_hal_irq::enable(&route);
    let (Ok(signal), Ok(())) = (signal, routed) else {
        panic!("the console's USB interrupt");
    };
    (Endpoint::new(endpoint, signal), pads)
}
