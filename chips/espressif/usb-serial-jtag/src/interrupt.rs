//! The console's interrupt: the endpoint's signal, in one static of the
//! image. The handler's registers are the interrupt table entry's part,
//! which the router moves where only the handler reaches it.

use core::marker::PhantomData;

use oer_interrupt_table::signal::{IrqSignal, SignalExecutor};

use crate::{Events, InterruptRegisters};

#[cfg(test)]
mod tests;

/// The console's interrupt side, a `static` of the image placed where its
/// handler runs: the signal of the task's executor `E`, which the handler
/// raises from the registers `R` its table entry holds.
pub struct Interrupt<E: SignalExecutor, R> {
    signal: IrqSignal<E>,
    registers: PhantomData<fn(&mut R)>,
}

impl<E: SignalExecutor, R: InterruptRegisters> Interrupt<E, R> {
    pub const fn new() -> Self {
        Self {
            signal: IrqSignal::new(),
            registers: PhantomData,
        }
    }

    /// The signal the endpoint waits on.
    pub fn signal(&'static self) -> &'static IrqSignal<E> {
        &self.signal
    }

    /// The handler's work over the part its entry holds: take the pending
    /// events, acknowledging exactly those, and raise them to the endpoint.
    #[inline(always)]
    pub fn service(&'static self, registers: &mut R) {
        let events = registers.take_events();
        if events != Events::NONE {
            self.signal.raise(events.facts());
        }
    }
}

impl<E: SignalExecutor, R: InterruptRegisters> Default for Interrupt<E, R> {
    fn default() -> Self {
        Self::new()
    }
}
