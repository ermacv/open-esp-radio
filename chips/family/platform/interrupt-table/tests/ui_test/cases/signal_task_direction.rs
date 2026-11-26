//@revisions: accept reject
//@[accept] check-pass
//@[reject] compile-flags: --cfg ui_reject
//@[reject] error-in-other-file: evaluation panicked: a signal task must be below its executor
use oer_chips_family_platform_interrupt_table::signal::{FromTask, IrqSignal, host::Deferred};
include!("support/table.rs");

#[cfg(ui_reject)]
const SIGNAL: IrqSignal<Deferred> = IrqSignal::new::<FromTask<Deferred, ExecutorToken>, ExecutorToken>();
#[cfg(not(ui_reject))]
const SIGNAL: IrqSignal<Deferred> = IrqSignal::new::<FromTask<oer_chips_family_platform_interrupt_table::signal::host::Foreign, WakeToken>, ExecutorToken>();
