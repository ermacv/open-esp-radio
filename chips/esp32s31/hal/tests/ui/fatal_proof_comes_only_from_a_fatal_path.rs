#![no_std]
use oer_esp32s31_hal::cores::{self, Fatal, StopTheWorld};

pub fn paths(slot: &mut Option<Fatal<'static>>) {
    // SAFETY: the case only compiles; nothing calls it.
    unsafe { cores::fatal(|path| drop(path)) };
    #[cfg(ui_reject)]
    cores::fatal(|path| drop(path)); // ui-error: E0133
    #[cfg(ui_reject)]
    // SAFETY: as above.
    unsafe { cores::fatal(|path| *slot = Some(path)) }; // ui-error: E0521
    let _ = slot;
}

pub fn stops(stop: Option<StopTheWorld<'_>>) -> Option<cores::Stopped> {
    let stop = stop?;
    #[cfg(ui_reject)]
    let _forged = StopTheWorld { hart: stop.hart() }; // ui-error: cannot construct `StopTheWorld<'_>` with struct literal syntax due to private fields
    Some(stop.other())
}
