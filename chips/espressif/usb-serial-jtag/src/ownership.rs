//! Who holds the USB Serial/JTAG block, and the writers that hold it
//! without its driver: the early writer of the bootstrap and a runtime's
//! early boot, and the fatal path.
//!
//! The chip's HAL keeps one [`Ownership`] and builds its register owner
//! through the `capture` it passes in, which [`Ownership`] calls only after
//! one compare-and-swap of the block's state made the caller its holder.
//! Nothing waits for a state:
//!
//! - `Free → Early` for one early record ([`Ownership::write`]), back to
//!   `Free` once the record's owner is dropped. A record that finds the
//!   block held by another one, on another hart or the one it interrupted,
//!   is dropped and counted.
//! - `Free → Driver` when the console's driver takes the block
//!   ([`Ownership::retire`]). While a record holds it the driver is refused
//!   ([`RetireError::Busy`]); afterwards every early record is refused and
//!   goes to the driver.
//! - `Free → Fatal`, or `Driver → Fatal` when the caller holds the driver's
//!   hart for good, for one fatal note ([`Ownership::write_fatal`]). The
//!   driver's [`Guarded`] parts make no access in `Fatal`. Every other fatal
//!   note is only counted.
//!
//! The early writer writes while the FIFO has room, drops the rest and
//! counts it ([`oer_diagnostics::FifoWriter`]); before an executor exists
//! there is nothing to yield to, so it waits for room within
//! [`EARLY_PATIENCE`] per record. A fatal note never waits.

use core::marker::PhantomData;
use core::sync::atomic::{AtomicU8, Ordering};

use oer_diagnostics::{FifoWriter, Lost, Note, Patience};

use crate::{EndpointRegisters, Events, InterruptRegisters};

#[cfg(test)]
mod tests;

/// What the early writer may wait for room in the 64-byte FIFO, per record,
/// in polls of the room flag: the host takes a packet in about a millisecond,
/// and a poll is a register read of well under a microsecond, so the budget is
/// some milliseconds up to tens of them at the early boot's slow clock. Before
/// an executor exists there is nothing to yield to.
pub const EARLY_PATIENCE: Patience = Patience::polls(100_000);

/// No one holds the block: an early record may take it.
const FREE: u8 = 0;
/// One early record holds the block.
const EARLY: u8 = 1;
/// The console's driver holds the block.
const DRIVER: u8 = 2;
/// The fatal path holds the block, for good.
const FATAL: u8 = 3;

/// The driver could not take the block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetireError {
    /// An early record holds the block now; the caller may try again once
    /// it ended.
    Busy,
    /// The driver or the fatal path took the block before.
    Taken,
}

/// The block's holder and the bytes the writers without the driver dropped.
pub struct Ownership {
    state: AtomicU8,
    lost: Lost,
}

/// The FIFO of a register owner for the diagnostic writer; without an owner
/// it never has room and a record written to it is only counted.
pub struct Fifo<R>(Option<R>);

impl<R: EndpointRegisters> oer_diagnostics::Fifo for Fifo<R> {
    fn has_room(&self) -> bool {
        self.0.as_ref().is_some_and(EndpointRegisters::has_room)
    }

    fn push(&mut self, byte: u8) {
        if let Some(registers) = &mut self.0 {
            registers.push(byte);
        }
    }

    fn flush(&mut self) {
        if let Some(registers) = &mut self.0 {
            registers.send();
        }
    }
}

impl Ownership {
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(FREE),
            lost: Lost::new(),
        }
    }

    /// Runs `write` with a writer over the owner `capture` builds, if the
    /// block is free; the block is free again once that owner is dropped.
    /// While another early record holds the block, `write` runs over no
    /// owner and its bytes are counted. Once the driver or the fatal path
    /// took the block, nothing is called and nothing counted. Returns
    /// whether the early writer took the record: `false` hands it to the
    /// driver.
    pub fn write<R: EndpointRegisters>(
        &self,
        capture: impl FnOnce() -> R,
        write: impl FnOnce(&mut FifoWriter<'_, Fifo<R>>),
    ) -> bool {
        match self
            .state
            .compare_exchange(FREE, EARLY, Ordering::Acquire, Ordering::Acquire)
        {
            Ok(_) => {
                write(&mut FifoWriter::new(
                    Fifo(Some(capture())),
                    &self.lost,
                    EARLY_PATIENCE,
                ));
                // The writer and its owner were dropped above.
                self.state.store(FREE, Ordering::Release);
                true
            }
            Err(EARLY) => {
                write(&mut FifoWriter::new(Fifo(None), &self.lost, Patience::NONE));
                true
            }
            Err(_) => false,
        }
    }

    /// Writes `note` without waiting, over the owner `capture` builds if
    /// the block is free, or held by the driver while `driver_suspended`:
    /// the caller holds the driver's hart, so that neither the driver's
    /// task nor its interrupt handler runs again. The block then stays the
    /// fatal path's for good. Otherwise only the note's bytes are counted.
    /// Returns whether the note was written.
    pub fn write_fatal<R: EndpointRegisters>(
        &self,
        driver_suspended: bool,
        capture: impl FnOnce() -> R,
        note: &Note<'_>,
    ) -> bool {
        let taken =
            match self
                .state
                .compare_exchange(FREE, FATAL, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => true,
                Err(DRIVER) if driver_suspended => self
                    .state
                    .compare_exchange(DRIVER, FATAL, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok(),
                Err(_) => false,
            };
        let fifo = Fifo(taken.then(capture));
        FifoWriter::new(fifo, &self.lost, Patience::NONE).note(note);
        taken
    }

    /// Gives the block to the console's driver for good, if it is free:
    /// every later early record is refused.
    ///
    /// # Errors
    /// An early record holds the block, or the block was taken before.
    pub fn retire(&self) -> Result<(), RetireError> {
        match self
            .state
            .compare_exchange(FREE, DRIVER, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => Ok(()),
            Err(EARLY) => Err(RetireError::Busy),
            Err(_) => Err(RetireError::Taken),
        }
    }

    /// Whether the console's driver holds the block.
    pub fn driver_holds(&self) -> bool {
        self.state.load(Ordering::Acquire) == DRIVER
    }

    /// Bytes dropped since the last call, for the driver's owner to report
    /// once it can: a loss is never silent.
    pub fn take_lost(&self) -> u32 {
        self.lost.take()
    }
}

impl Default for Ownership {
    fn default() -> Self {
        Self::new()
    }
}

/// The chip's one [`Ownership`] of the block, which its driver's
/// [`Guarded`] parts check.
pub trait Owner: 'static {
    fn ownership() -> &'static Ownership;
}

/// A part of the console's driver that touches its registers `R` only
/// while the driver holds the block of `O`: once the fatal path took it,
/// the part reads no room, no data and no event and writes nothing.
pub struct Guarded<O, R> {
    registers: R,
    owner: PhantomData<fn() -> O>,
}

impl<O: Owner, R> Guarded<O, R> {
    pub const fn new(registers: R) -> Self {
        Self {
            registers,
            owner: PhantomData,
        }
    }

    fn held() -> bool {
        O::ownership().driver_holds()
    }
}

impl<O: Owner, R: EndpointRegisters> EndpointRegisters for Guarded<O, R> {
    fn has_room(&self) -> bool {
        Self::held() && self.registers.has_room()
    }

    fn push(&mut self, byte: u8) {
        if Self::held() {
            self.registers.push(byte);
        }
    }

    fn send(&mut self) {
        if Self::held() {
            self.registers.send();
        }
    }

    fn has_data(&self) -> bool {
        Self::held() && self.registers.has_data()
    }

    fn pop(&mut self) -> u8 {
        if Self::held() {
            self.registers.pop()
        } else {
            0
        }
    }

    fn enable_events(&mut self) {
        if Self::held() {
            self.registers.enable_events();
        }
    }
}

impl<O: Owner, R: InterruptRegisters> InterruptRegisters for Guarded<O, R> {
    fn take_events(&mut self) -> Events {
        if Self::held() {
            self.registers.take_events()
        } else {
            Events::NONE
        }
    }
}
