//@revisions: accept reject
//@[accept] check-pass
//@[reject] compile-flags: --cfg ui_reject
//@[reject] error-in-other-file: evaluation panicked: an interrupt-mode executor's entry must poll its tasks
use oer_chips_family_platform_interrupt_table::signal::{FromHandler, IrqSignal, host::Deferred};
oer_chips_family_platform_interrupt_table::__fake_parts!();

pub fn on_radio(_: &mut RadioIrq) {}
pub fn on_timer(_: &mut TimerIrq) {}

// `Deferred` runs its tasks in interrupt mode on source 5: its entry, the
// radio's, must poll them (class `Executor`).
#[cfg(ui_reject)]
oer_chips_family_platform_interrupt_table::interrupt_table! {
    lines: FAKE_LINES,
    handler_attributes: [];
    contexts { radio_executor: 2; radio: 3; }
    executor: ExecutorToken = RadioIrq => on_radio, line 17, context radio_executor, hart 0, Hardware;
    irq: IrqToken = TimerIrq => on_timer, line 18, context radio, hart 0, Hardware;
}
#[cfg(not(ui_reject))]
oer_chips_family_platform_interrupt_table::interrupt_table! {
    lines: FAKE_LINES,
    handler_attributes: [];
    contexts { radio_executor: 2; radio: 3; }
    executor: ExecutorToken = RadioIrq => OnExecutor, line 17, context radio_executor, hart 0, Executor;
    irq: IrqToken = TimerIrq => on_timer, line 18, context radio, hart 0, Hardware;
}

const SIGNAL: IrqSignal<Deferred> = IrqSignal::new::<FromHandler<IrqToken>, ExecutorToken>();
