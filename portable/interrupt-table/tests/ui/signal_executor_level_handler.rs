use oer_interrupt_table::signal::{
    FromExecutorLevel, FromHandler, FromTask, IrqSignal,
    host::{Deferred, Foreign},
};
include!("support/executor_level.rs");

#[cfg(ui_reject)]
pub const fn make() -> IrqSignal<Deferred> {
    IrqSignal::new::<FromHandler<SameLevelToken>, ExecutorToken>() // ui-error@outside: E0080: evaluation panicked: a signal handler must be above its executor
}
#[cfg(not(ui_reject))]
pub const fn make() -> IrqSignal<Deferred> {
    IrqSignal::new::<FromExecutorLevel<SameLevelToken>, ExecutorToken>()
}

static SIGNAL: IrqSignal<Deferred> = make();

// Same-level, higher handler and lower task sources also compose in nested pairs.
static MIXED: IrqSignal<Deferred> = IrqSignal::new::<
    (
        (FromExecutorLevel<SameLevelToken>, FromHandler<IrqToken>),
        FromTask<Foreign, WakeToken>,
    ),
    ExecutorToken,
>();
