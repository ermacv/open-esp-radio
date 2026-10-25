#![no_std]
use oer_esp32s31_hal::cores::{Fatal, StopTheWorld};
use oer_esp32s31_hal::usb_serial_jtag::TakenConsole;

fn other_hart<T: Send>(_: T) {}

pub fn keep(path: Fatal<'_>, stop: StopTheWorld<'_>, console: TakenConsole) {
    other_hart(());
    let _ = (&path, &stop, &console);
    #[cfg(ui_reject)]
    other_hart(path); // ui-error: E0277
    #[cfg(ui_reject)]
    other_hart(stop); // ui-error: E0277
    #[cfg(ui_reject)]
    other_hart(console); // ui-error: E0277
}
