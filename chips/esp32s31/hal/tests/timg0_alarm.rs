use oer_esp32s31_hal::platform::{
    FromCpu2Irq, interrupt_controller::facts::LINES, timg0_alarm::Timg0AlarmInterrupt,
};
use oer_esp32s31_pac::Interrupt;
use oer_interrupt_table::{Class, Entry};
use oer_mmio::InterruptSource;

pub fn on_timer(_: &mut Timg0AlarmInterrupt) {}
pub fn on_executor(_: &mut FromCpu2Irq) {}

// A test composition only: no production handler, cells, actor or vectors.
// SOURCE: chips/espressif/platform/family.toml [interrupts], CLIC spec
// 3f8ee744 "Specifying Interrupt Priority"; ESP-IDF 4d59230d
// components/soc/esp32s31/include/soc/clic_reg.h (NLBITS/INT_INFO geometry).
// SOURCE for arbitration: ESP32-C5 TRM v1.1,
// documentation/esp32-c5_technical_reference_manual_en.pdf, section 2.8.2.5,
// p.86. Lower-ID ordering assumes that family contract; unverified on S31 (probe P3).
oer_interrupt_table::interrupt_table! {
    lines: LINES,
    handler_attributes: [];
    contexts { radio: 2; }
    timer: TimerEntry = Timg0AlarmInterrupt => on_timer, line 20, context radio, hart 0,
        Exempt(ExecutorLevel, "PHY steps between executor rounds");
    executor: ExecutorEntry = FromCpu2Irq => on_executor, line 21, context radio, hart 0, Executor;
}

#[test]
fn the_alarm_part_binds_below_from_cpu2_at_the_exempt_executor_level() {
    assert_eq!(
        <Timg0AlarmInterrupt as InterruptSource>::SOURCE,
        Interrupt::TIMERGRP0_T0.source()
    );
    assert_eq!(TimerEntry::LEVEL, ExecutorEntry::LEVEL);
    const { assert!(TimerEntry::LINE < ExecutorEntry::LINE) };
    let timer = INTERRUPT_TABLE
        .iter()
        .find(|entry| entry.source == <Timg0AlarmInterrupt as InterruptSource>::SOURCE)
        .expect("table includes the timer");
    assert_eq!(timer.class, Class::ExemptExecutorLevel);
}
