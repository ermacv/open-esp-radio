//@revisions: accept reject
//@[accept] check-pass
//@[reject] compile-flags: --cfg ui_reject
//@[reject] error-in-other-file: evaluation panicked: a same-level signal handler must be on its receiving executor's level
use oer_chips_family_platform_interrupt_table::signal::{FromExecutorLevel, IrqSignal, host::Deferred};
oer_chips_family_platform_interrupt_table::__fake_parts!();

pub fn on_timer(_: &mut TimerIrq) {}
macro_rules! table {
    ($context:ident) => {
        oer_chips_family_platform_interrupt_table::interrupt_table! {
            lines: FAKE_LINES,
            handler_attributes: [];
            contexts { receiver: 2; other: 3; }
            executor: ExecutorToken = RadioIrq => OnExecutor, line 17, context receiver, hart 0, Executor;
            other: OtherToken = AbsentIrq => OnExecutor, line 18, context other, hart 0, Executor;
            same: SameLevelToken = TimerIrq => on_timer, line 19, context $context, hart 0, Exempt(ExecutorLevel, "same-level signal producer");
        }
    };
}
#[cfg(ui_reject)]
table!(other);
#[cfg(not(ui_reject))]
table!(receiver);

pub const fn make() -> IrqSignal<Deferred> {
    IrqSignal::new::<FromExecutorLevel<SameLevelToken>, ExecutorToken>()
}

const SIGNAL: IrqSignal<Deferred> = make();
