use oer_interrupt_table::signal::{FromHandler, IrqSignal, host::Deferred};
include!("support/table.rs");

#[cfg(ui_reject)]
static SIGNAL: IrqSignal<Deferred> = IrqSignal::new::<FromHandler<IrqToken>, WakeToken>(); // ui-error@outside: E0080: evaluation panicked: the signal entry must name its executor's software interrupt
#[cfg(not(ui_reject))]
static SIGNAL: IrqSignal<Deferred> = IrqSignal::new::<FromHandler<IrqToken>, ExecutorToken>();
