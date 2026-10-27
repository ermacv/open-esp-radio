oer_interrupt_table::__fake_parts!();

pub fn on_timer(_: &mut TimerIrq) {}
pub fn on_radio(_: &mut RadioIrq) {}
pub fn on_wake(_: &mut AbsentIrq) {}
pub fn on_irq(_: &mut PairIrq) {}

oer_interrupt_table::interrupt_table! {
    lines: FAKE_LINES,
    handler_attributes: [];
    contexts { app_wake: 1; radio_executor: 2; radio: 3; }
    wake: WakeToken = AbsentIrq => on_wake, line 16, context app_wake, hart 0, Hardware;
    executor: ExecutorToken = RadioIrq => on_radio, line 17, context radio_executor, hart 0, Executor;
    same: SameLevelToken = TimerIrq => on_timer, line 18, context radio_executor, hart 0, Exempt(ExecutorLevel, "same-level signal producer");
    irq: IrqToken = PairIrq => on_irq, line 19, context radio, hart 0, Exempt(WakerCall, "class rejection fixture");
}
