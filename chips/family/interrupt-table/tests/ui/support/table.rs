oer_espressif_interrupt_table::__fake_parts!();

pub fn on_timer(_: &mut TimerIrq) {}
pub fn on_wake(_: &mut AbsentIrq) {}

oer_espressif_interrupt_table::interrupt_table! {
    lines: FAKE_LINES,
    handler_attributes: [];
    contexts { app_wake: 1; radio_executor: 2; radio: 3; }
    wake: WakeToken = AbsentIrq => on_wake, line 16, context app_wake, hart 0, Hardware;
    executor: ExecutorToken = RadioIrq => OnExecutor, line 17, context radio_executor, hart 0, Executor;
    irq: IrqToken = TimerIrq => on_timer, line 18, context radio, hart 0, Hardware;
}
