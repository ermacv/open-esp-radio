use oer_interrupt_table::signal::{FromExecutorLevel, IrqSignal, host::Deferred};
oer_interrupt_table::__fake_parts!();

pub fn on_timer(_: &mut TimerIrq) {}
pub fn on_radio(_: &mut RadioIrq) {}
pub fn on_other(_: &mut AbsentIrq) {}
macro_rules! table {
    ($context:ident) => {
        oer_interrupt_table::interrupt_table! {
            lines: FAKE_LINES,
            handler_attributes: [];
            contexts { receiver: 2; other: 3; }
            executor: ExecutorToken = RadioIrq => on_radio, line 17, context receiver, hart 0, Executor;
            other: OtherToken = AbsentIrq => on_other, line 18, context other, hart 0, Executor;
            same: SameLevelToken = TimerIrq => on_timer, line 19, context $context, hart 0, Exempt(ExecutorLevel, "same-level signal producer");
        }
    };
}
#[cfg(ui_reject)]
table!(other);
#[cfg(not(ui_reject))]
table!(receiver);

pub const fn make() -> IrqSignal<Deferred> {
    IrqSignal::new::<FromExecutorLevel<SameLevelToken>, ExecutorToken>() // ui-error@outside: E0080: evaluation panicked: a same-level signal handler must be on its receiving executor's level
}

static SIGNAL: IrqSignal<Deferred> = make();
