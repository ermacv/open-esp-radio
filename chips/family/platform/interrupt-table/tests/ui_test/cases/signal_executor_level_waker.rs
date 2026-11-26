//@revisions: accept reject
//@[accept] check-pass
//@[reject] compile-flags: --cfg ui_reject
//@[reject] error-in-other-file: evaluation panicked: a same-level signal handler must have class Exempt(ExecutorLevel)
use oer_chips_family_platform_interrupt_table::signal::{FromExecutorLevel, IrqSignal, host::Deferred};
include!("support/executor_level.rs");

#[cfg(ui_reject)]
pub const fn make() -> IrqSignal<Deferred> {
    IrqSignal::new::<FromExecutorLevel<IrqToken>, ExecutorToken>()
}
#[cfg(not(ui_reject))]
pub const fn make() -> IrqSignal<Deferred> {
    IrqSignal::new::<FromExecutorLevel<SameLevelToken>, ExecutorToken>()
}

const SIGNAL: IrqSignal<Deferred> = make();
