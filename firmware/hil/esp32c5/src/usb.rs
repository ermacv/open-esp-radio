//! The USB Serial/JTAG console on this chip: the early writer until the
//! console opens, then the duplex endpoint of the executor and its
//! interrupt, which raises the endpoint's static signal.

use oer_esp32c5_hal::usb_serial_jtag::{
    ConsoleEndpoint, ConsoleInterrupt, UsbSerialJtagRegisters, console_parts,
};
use oer_espressif_executor::Executor;
use oer_espressif_usb_serial_jtag::{Endpoint, Interrupt};

/// The console's endpoint, polled by the executor's console task.
pub(crate) type Console = Endpoint<Executor<0>, ConsoleEndpoint>;

/// The console's interrupt: its handler's registers and the endpoint's
/// signal.
static INTERRUPT: Interrupt<Executor<0>, ConsoleInterrupt> = Interrupt::new();

/// The console's `Hardware` interrupt handler: it acknowledges the events
/// it read and raises them to the console's task, calling no waker.
pub(crate) fn interrupt() {
    let _ = INTERRUPT.service();
}

/// Opens the console over the block the esp-hal adapter took from the
/// early writer and esp-hal (`take_usb_serial_jtag`): it gives the handler
/// its registers, then routes the interrupt. The image never switches the
/// pads.
pub(crate) fn open(registers: UsbSerialJtagRegisters, route: crate::ConsoleUsb) -> Console {
    let (endpoint, interrupt, _pads) = console_parts(registers);
    let signal = INTERRUPT.install(interrupt);
    let routed = oer_espressif_esp_hal_irq::enable(&route);
    let (Ok(signal), Ok(())) = (signal, routed) else {
        panic!("the console's USB interrupt");
    };
    Endpoint::new(endpoint, signal)
}
