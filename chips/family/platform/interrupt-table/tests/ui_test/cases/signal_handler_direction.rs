//@revisions: accept reject
//@[accept] check-pass
//@[reject] compile-flags: --cfg ui_reject
//@[reject] error-in-other-file: evaluation panicked: a signal handler must be above its executor
use oer_chips_family_platform_interrupt_table::signal::{FromHandler, IrqSignal, host::Deferred};
include!("support/table.rs");

#[cfg(ui_reject)]
const SIGNAL: IrqSignal<Deferred> = IrqSignal::new::<FromHandler<ExecutorToken>, ExecutorToken>();
#[cfg(not(ui_reject))]
const SIGNAL: IrqSignal<Deferred> = IrqSignal::new::<FromHandler<IrqToken>, ExecutorToken>();
