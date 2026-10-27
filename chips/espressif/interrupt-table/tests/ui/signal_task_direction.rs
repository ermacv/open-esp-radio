use oer_espressif_interrupt_table::signal::{FromTask, IrqSignal, host::Deferred};
include!("support/table.rs");

#[cfg(ui_reject)]
static SIGNAL: IrqSignal<Deferred> = IrqSignal::new::<FromTask<Deferred, ExecutorToken>, ExecutorToken>(); // ui-error@outside: E0080: evaluation panicked: a signal task must be below its executor
#[cfg(not(ui_reject))]
static SIGNAL: IrqSignal<Deferred> = IrqSignal::new::<FromTask<oer_espressif_interrupt_table::signal::host::Foreign, WakeToken>, ExecutorToken>();
