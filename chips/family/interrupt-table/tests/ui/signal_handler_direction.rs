use oer_espressif_interrupt_table::signal::{FromHandler, IrqSignal, host::Deferred};
include!("support/table.rs");

#[cfg(ui_reject)]
const SIGNAL: IrqSignal<Deferred> = IrqSignal::new::<FromHandler<ExecutorToken>, ExecutorToken>(); // ui-error@outside: E0080: evaluation panicked: a signal handler must be above its executor
#[cfg(not(ui_reject))]
const SIGNAL: IrqSignal<Deferred> = IrqSignal::new::<FromHandler<IrqToken>, ExecutorToken>();
