use oer_espressif_interrupt_table::signal::{FromExecutorLevel, IrqSignal, host::Deferred};
oer_espressif_interrupt_table::__fake_parts!();

pub fn on_timer(_: &mut TimerIrq) {}
pub fn on_radio(_: &mut RadioIrq) {}
pub fn on_other(_: &mut AbsentIrq) {}
macro_rules! table {
    ($hart:literal) => {
        oer_espressif_interrupt_table::interrupt_table! {
            lines: FAKE_LINES,
            handler_attributes: [];
            contexts { receiver: 2; }
            executor: ExecutorToken = RadioIrq => on_radio, line 17, context receiver, hart 0, Executor;
            other: OtherToken = AbsentIrq => on_other, line 18, context receiver, hart 1, Executor;
            same: SameLevelToken = TimerIrq => on_timer, line 19, context receiver, hart $hart, Exempt(ExecutorLevel, "same-level signal producer");
        }
    };
}
#[cfg(ui_reject)]
table!(1);
#[cfg(not(ui_reject))]
table!(0);

pub const fn make() -> IrqSignal<Deferred> {
    IrqSignal::new::<FromExecutorLevel<SameLevelToken>, ExecutorToken>() // ui-error@outside: E0080: evaluation panicked: a signal and its raiser must share a hart
}

static SIGNAL: IrqSignal<Deferred> = make();
