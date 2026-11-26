//@revisions: accept reject
//@[accept] check-pass
//@[reject] compile-flags: --cfg ui_reject
//@[reject] error-in-other-file: evaluation panicked: an executor's entry must name its software interrupt
use oer_chips_family_platform_interrupt_table::signal::{FromHandler, IrqSignal, host::Deferred};
include!("support/table.rs");

#[cfg(ui_reject)]
const SIGNAL: IrqSignal<Deferred> = IrqSignal::new::<FromHandler<IrqToken>, WakeToken>();
#[cfg(not(ui_reject))]
const SIGNAL: IrqSignal<Deferred> = IrqSignal::new::<FromHandler<IrqToken>, ExecutorToken>();
