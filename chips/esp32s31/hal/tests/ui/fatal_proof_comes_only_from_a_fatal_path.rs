#![no_std]
use oer_esp32s31_hal::cores::{self, Fatal, StopTheWorld};

pub fn paths(_slot: &mut Option<Fatal<'static>>) -> ! {
    cores::fatal(|_path| {
        #[cfg(ui_reject)]
        { *_slot = Some(_path); } // ui-error: E0521
    })
}

pub fn stops(stop: Option<StopTheWorld<'_>>) -> Option<oer_espressif_boot::drivers::cores::Stopped> {
    let stop = stop?;
    #[cfg(ui_reject)]
    let _forged = StopTheWorld { hart: stop.hart() }; // ui-error: cannot construct `StopTheWorld<'_>` with struct literal syntax due to private fields
    stop.stopped(cores::BOOT_HART)
}
