use oer_esp32s31_hal::{
    peripherals::{software_interrupt::FromCpu2Irq, timg0_alarm::Timg0AlarmInterrupt},
    platform::interrupt_controller::facts::LINES,
};
use oer_esp32s31_pac::Interrupt;
use oer_espressif_interrupt_table::{Class, Entry};
use oer_mmio::InterruptSource;

pub fn on_timer(_: &mut Timg0AlarmInterrupt) {}
pub fn on_executor(_: &mut FromCpu2Irq) {}

// A test composition only: no production handler, cells, actor or vectors.
// SOURCE: chips/espressif/platform/family.toml [interrupts], CLIC spec
// 3f8ee744 "Specifying Interrupt Priority"; ESP-IDF 4d59230d
// components/soc/esp32s31/include/soc/clic_reg.h (NLBITS/INT_INFO geometry).
// SOURCE for arbitration: ESP32-C5 TRM v1.1,
// documentation/esp32-c5_technical_reference_manual_en.pdf, section 2.8.2.5,
// p.86. This fixture binds lines 20/21 using that family contract.
// SOURCE: HW23_S31_P2_ARBITRATION_20261027 measured lines 16/17 on S31
// hart0 at P2 with TIMG0/FROM_CPU2 assigned both ways; 20/21 was not measured.
oer_espressif_interrupt_table::interrupt_table! {
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
