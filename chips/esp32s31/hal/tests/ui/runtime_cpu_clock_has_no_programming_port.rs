#![no_std]
use oer_esp32s31_hal::root::Root;

#[cfg(ui_reject)]
fn program<C: oer_espressif_boot::clock::CpuClockRegisters>(_: C) {}

pub fn split(root: Root) {
    let clock = root.into_runtime().cpu_clock;
    #[cfg(ui_reject)]
    program(clock); // ui-error: E0277
    #[cfg(not(ui_reject))]
    let _read_only = clock;
}
