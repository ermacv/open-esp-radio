#![no_std]
use oer_esp32s31_hal::platform::{SystimerAlarm0, timg0_alarm::Timg0Alarm};
use oer_interrupt_table::Silent;
use oer_time::Duration;

pub fn arm(
    timer: &mut Timg0Alarm,
    _own: &Silent<Timg0Alarm>,
    _other: &Silent<SystimerAlarm0>,
) {
    #[cfg(ui_reject)]
    let _ = timer.arm_after(Duration::from_micros(40), _other); // ui-error: E0277
    #[cfg(not(ui_reject))]
    let _ = timer.arm_after(Duration::from_micros(40), _own);
}
