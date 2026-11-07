#![no_std]
use oer_chips_esp32s31_hal::system::root::Root;

pub fn split(root: Root) {
    let owners = root.into_runtime();
    #[cfg(ui_reject)]
    let _armed: oer_chips_esp32s31_hal::peripherals::watchdog::ArmedRtcWatchdog<oer_chips_esp32s31_hal::peripherals::watchdog::RwdtRegisters> = owners.watchdogs.rtc; // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _unstarted = owners.watchdogs.rtc;
}
