#![no_std]
use oer_esp32s31_hal::peripherals::timg0_alarm::{Timg0AlarmInterrupt, Timg0AlarmProgram};

pub fn program_only(program: &mut Timg0AlarmProgram) {
    #[cfg(ui_reject)]
    let _ = program.acknowledge(); // ui-error: E0599
    #[cfg(not(ui_reject))]
    program.disarm();
}

pub fn interrupt_only(interrupt: &mut Timg0AlarmInterrupt) {
    #[cfg(ui_reject)]
    interrupt.disarm(); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let _ = interrupt.acknowledge();
}

pub struct Bundle {
    pub program: Timg0AlarmProgram,
}

pub fn actor_to_bundle(mut program: Timg0AlarmProgram) -> Bundle {
    #[cfg(not(ui_reject))]
    program.disarm();
    let bundle = Bundle { program };
    #[cfg(ui_reject)]
    program.disarm(); // ui-error: E0382
    bundle
}
