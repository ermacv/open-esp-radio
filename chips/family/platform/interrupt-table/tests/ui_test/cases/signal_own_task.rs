//@revisions: accept reject
//@[accept] check-pass
//@[reject] compile-flags: --cfg ui_reject
//@[reject] error-in-other-file: evaluation panicked: an own-task raiser must be a task of its receiving executor
use oer_chips_family_platform_interrupt_table::signal::{FromOwnTask, IrqSignal, host::Deferred};
include!("support/table.rs");

#[cfg(ui_reject)]
const SIGNAL: IrqSignal<Deferred> = IrqSignal::new::<FromOwnTask<oer_chips_family_platform_interrupt_table::signal::host::Foreign, WakeToken>, ExecutorToken>();
#[cfg(not(ui_reject))]
const SIGNAL: IrqSignal<Deferred> = IrqSignal::new::<FromOwnTask<Deferred, ExecutorToken>, ExecutorToken>();
