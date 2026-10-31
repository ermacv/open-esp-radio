use oer_esp32s31_hal::peripherals::{
    timg0_alarm::Timg0AlarmInterrupt, timg1_alarm::Timg1AlarmInterrupt,
};
use oer_esp32s31_pac::Interrupt;
use oer_mmio::InterruptSource;

#[test]
fn the_group_one_alarm_part_serves_its_own_timer0_source() {
    assert_eq!(
        <Timg1AlarmInterrupt as InterruptSource>::SOURCE,
        Interrupt::TIMERGRP1_T0.source()
    );
    assert_ne!(
        <Timg1AlarmInterrupt as InterruptSource>::SOURCE,
        <Timg0AlarmInterrupt as InterruptSource>::SOURCE
    );
}
