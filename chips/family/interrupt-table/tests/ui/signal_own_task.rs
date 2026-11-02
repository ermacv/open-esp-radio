use oer_espressif_interrupt_table::signal::{FromOwnTask, IrqSignal, host::Deferred};
include!("support/table.rs");

#[cfg(ui_reject)]
const SIGNAL: IrqSignal<Deferred> = IrqSignal::new::<FromOwnTask<oer_espressif_interrupt_table::signal::host::Foreign, WakeToken>, ExecutorToken>(); // ui-error@outside: E0080: evaluation panicked: an own-task raiser must be a task of its receiving executor
#[cfg(not(ui_reject))]
const SIGNAL: IrqSignal<Deferred> = IrqSignal::new::<FromOwnTask<Deferred, ExecutorToken>, ExecutorToken>();
