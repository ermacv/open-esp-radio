//! The console's interrupt: the handler's registers and the endpoint's
//! signal, in one static of the image.

use oer_interrupt_table::signal::{IrqSignal, SignalExecutor};
use oer_memory::HandlerCell;
use static_cell::StaticCell;

use crate::{Events, InterruptRegisters};

#[cfg(test)]
mod tests;

/// The handler's registers were installed before.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AlreadyInstalled;

/// The console's interrupt side, a `static` of the image placed where its
/// handler runs: the handler's registers `R` and the signal of the task's
/// executor `E`.
pub struct Interrupt<E: SignalExecutor, R> {
    storage: StaticCell<R>,
    registers: HandlerCell<R>,
    signal: IrqSignal<E>,
}

impl<E: SignalExecutor, R: InterruptRegisters + Send> Interrupt<E, R> {
    pub const fn new() -> Self {
        Self {
            storage: StaticCell::new(),
            registers: HandlerCell::new(),
            signal: IrqSignal::new(),
        }
    }

    /// Gives `registers` to the handler, once; returns the signal the
    /// endpoint waits on.
    ///
    /// # Errors
    /// The handler already has its registers.
    pub fn install(&'static self, registers: R) -> Result<&'static IrqSignal<E>, AlreadyInstalled> {
        let registers = self.storage.try_init(registers).ok_or(AlreadyInstalled)?;
        let _installed = self
            .registers
            .install(registers)
            .map_err(|_| AlreadyInstalled)?;
        Ok(&self.signal)
    }

    /// The handler's work: take the pending events, acknowledging exactly
    /// those, and raise them to the endpoint. Returns `false` before the
    /// registers are installed, when nothing is read.
    #[inline(always)]
    pub fn service(&'static self) -> bool {
        self.registers
            .with(|registers| {
                let events = registers.take_events();
                if events != Events::NONE {
                    self.signal.raise(events.facts());
                }
            })
            .is_some()
    }
}

impl<E: SignalExecutor, R: InterruptRegisters + Send> Default for Interrupt<E, R> {
    fn default() -> Self {
        Self::new()
    }
}
