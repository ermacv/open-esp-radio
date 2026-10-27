use oer_espressif_interrupt_table::signal::{FromExecutorLevel, IrqSignal, host::Deferred};
include!("support/executor_level.rs");

#[cfg(ui_reject)]
pub const fn make() -> IrqSignal<Deferred> {
    IrqSignal::new::<FromExecutorLevel<WakeToken>, ExecutorToken>() // ui-error@outside: E0080: evaluation panicked: a same-level signal handler must have class Exempt(ExecutorLevel)
}
#[cfg(not(ui_reject))]
pub const fn make() -> IrqSignal<Deferred> {
    IrqSignal::new::<FromExecutorLevel<SameLevelToken>, ExecutorToken>()
}

static SIGNAL: IrqSignal<Deferred> = make();
