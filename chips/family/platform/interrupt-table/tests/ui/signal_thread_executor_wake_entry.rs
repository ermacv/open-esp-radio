use oer_chips_family_platform_interrupt_table::signal::{FromHandler, IrqSignal, host::Foreign};
oer_chips_family_platform_interrupt_table::__fake_parts!();

pub fn on_wake(_: &mut AbsentIrq) {}
pub fn on_timer(_: &mut TimerIrq) {}

// `Foreign` runs its tasks in thread mode on source 6: its entry, the
// absent part's, must be its wake entry (class `Hardware`).
#[cfg(ui_reject)]
oer_chips_family_platform_interrupt_table::interrupt_table! {
    lines: FAKE_LINES,
    handler_attributes: [];
    contexts { app_wake: 1; radio: 3; }
    wake: WakeToken = AbsentIrq => OnExecutor, line 16, context app_wake, hart 0, Executor;
    irq: IrqToken = TimerIrq => on_timer, line 18, context radio, hart 0, Hardware;
}
#[cfg(not(ui_reject))]
oer_chips_family_platform_interrupt_table::interrupt_table! {
    lines: FAKE_LINES,
    handler_attributes: [];
    contexts { app_wake: 1; radio: 3; }
    wake: WakeToken = AbsentIrq => on_wake, line 16, context app_wake, hart 0, Hardware;
    irq: IrqToken = TimerIrq => on_timer, line 18, context radio, hart 0, Hardware;
}

const SIGNAL: IrqSignal<Foreign> = IrqSignal::new::<FromHandler<IrqToken>, WakeToken>(); // ui-error@outside: E0080: evaluation panicked: a thread executor needs its wake entry
