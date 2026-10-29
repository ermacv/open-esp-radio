#![no_std]
use oer_espressif_interrupt_table::signal::{IrqSignal, SignalExecutor};

fn accept<E: SignalExecutor>(_: &'static IrqSignal<E>) {}

pub fn bind<A: SignalExecutor, B: SignalExecutor>(signal: &'static IrqSignal<A>) {
    #[cfg(not(ui_reject))]
    accept::<A>(signal);
    #[cfg(ui_reject)]
    accept::<B>(signal); // ui-error: E0308
}
