#![no_std]
use oer_esp32s31_hal::peripherals::{system_timer::SystimerAlarm0, timg0_alarm::{Timg0AlarmProgram, Timg0AlarmInterrupt}};
use oer_espressif_interrupt_table::Silent;
use oer_time::Duration;

pub fn arm(
    timer: &mut Timg0AlarmProgram,
    _own: &Silent<Timg0AlarmInterrupt>,
    _other: &Silent<SystimerAlarm0>,
) {
    #[cfg(ui_reject)]
    let _ = timer.arm_after(Duration::from_micros(40), _other); // ui-error: E0277
    #[cfg(not(ui_reject))]
    let _ = timer.arm_after(Duration::from_micros(40), _own);
}
