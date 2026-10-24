#![no_std]
use oer_esp32s31_hal::usb_serial_jtag::{UsbEndpoint, UsbPads};
use oer_mmio::Placement;

fn other_hart<T: Send>(_: T) {}

pub fn keep<P: Placement>(endpoint: UsbEndpoint<P>, pads: UsbPads<P>) {
    other_hart(());
    let _ = (&endpoint, &pads);
    #[cfg(ui_reject)]
    other_hart(endpoint); // ui-error: E0277
    #[cfg(ui_reject)]
    other_hart(pads); // ui-error: E0277
}
