use oer_esp32s31_hal::platform::{
    FromCpu2Irq, interrupt_controller::facts::LINES, timg0_alarm::Timg0Alarm,
};
use oer_esp32s31_pac::Interrupt;
use oer_interrupt_table::{Class, Entry};
use oer_mmio::InterruptSource;

pub fn on_timer(_: &mut Timg0Alarm) {}
pub fn on_executor(_: &mut FromCpu2Irq) {}

// A test composition only: no production handler, cells, actor or vectors.
// Increasing line IDs follow the sourced highest-ID-wins arbitration.
oer_interrupt_table::interrupt_table! {
    lines: LINES,
    handler_attributes: [];
    contexts { radio: 2; }
    timer: TimerEntry = Timg0Alarm => on_timer, line 20, context radio, hart 0,
        Exempt(ExecutorLevel, "PHY steps between executor rounds");
    executor: ExecutorEntry = FromCpu2Irq => on_executor, line 21, context radio, hart 0, Executor;
}

#[test]
fn the_alarm_part_binds_below_from_cpu2_at_the_exempt_executor_level() {
    assert_eq!(
        <Timg0Alarm as InterruptSource>::SOURCE,
        Interrupt::TIMERGRP0_T0.source()
    );
    assert_eq!(TimerEntry::LEVEL, ExecutorEntry::LEVEL);
    const { assert!(TimerEntry::LINE < ExecutorEntry::LINE) };
    let timer = INTERRUPT_TABLE
        .iter()
        .find(|entry| entry.source == <Timg0Alarm as InterruptSource>::SOURCE)
        .expect("table includes the timer");
    assert_eq!(timer.class, Class::ExemptExecutorLevel);
}
