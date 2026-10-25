#![no_std]
use oer_esp32s31_hal::cores::{Fatal, StopTheWorld};

fn other_hart<T: Send>(_: T) {}

pub fn keep(path: Fatal<'_>, stop: StopTheWorld<'_>) {
    other_hart(());
    let _ = (&path, &stop);
    #[cfg(ui_reject)]
    other_hart(path); // ui-error: E0277
    #[cfg(ui_reject)]
    other_hart(stop); // ui-error: E0277
}
