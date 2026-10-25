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
//! - `Free → Fatal` for one fatal note ([`Ownership::write_fatal`]), for
//!   good. The fatal path never takes the block from anyone: while an early
//!   record or the driver holds it, or a fatal note took it before, the note
//!   is refused and its caller keeps it elsewhere. So every owner of the
//!   block lives alone, and the driver's parts hold it for as long as they
//!   live.
//! - `any → TakenOver` for a fatal path that stopped every other context
//!   ([`Ownership::take_over`]), for good: the holder's owner, an early
//!   record or the driver, never runs again, so the owner the fatal path
//!   builds lives alone too. A second takeover is refused, as the first
//!   one's owner may still live.
//!
//! The early writer writes while the FIFO has room, drops the rest and
//! counts it ([`oer_diagnostics::FifoWriter`]); before an executor exists
//! there is nothing to yield to, so it waits for room within
//! [`EARLY_PATIENCE`] per record. A fatal note never waits; a note over a
//! block taken over waits within [`EARLY_PATIENCE`], as nothing else runs
//! to yield to.

use core::sync::atomic::{AtomicU8, Ordering};

use oer_diagnostics::{FifoWriter, Lost, Note, Patience};

use crate::EndpointRegisters;

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
/// A fatal path that stopped every other context took the block over, for
/// good.
const TAKEN_OVER: u8 = 4;

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
                // The writer and its owner were dropped above. A takeover
                // during the record keeps the block.
                let _ =
                    self.state
                        .compare_exchange(EARLY, FREE, Ordering::Release, Ordering::Relaxed);
                true
            }
            Err(EARLY) => {
                write(&mut FifoWriter::new(Fifo(None), &self.lost, Patience::NONE));
                true
            }
            Err(_) => false,
        }
    }

    /// Writes `note` without waiting over the owner `capture` builds, if
    /// the block is free; the block then stays the fatal path's for good.
    /// While anyone holds the block, nothing is built, written or counted:
    /// returns `false`, and the caller keeps the note elsewhere.
    pub fn write_fatal<R: EndpointRegisters>(
        &self,
        capture: impl FnOnce() -> R,
        note: &Note<'_>,
    ) -> bool {
        if self
            .state
            .compare_exchange(FREE, FATAL, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        FifoWriter::new(Fifo(Some(capture())), &self.lost, Patience::NONE).note(note);
        true
    }

    /// Takes the block from whoever holds it, for good, for a fatal path
    /// that stopped every other context: the holder's owner never runs
    /// again. Returns `false` when a takeover took it before, whose owner
    /// may still live; then the caller builds no owner.
    pub fn take_over(&self) -> bool {
        self.state.swap(TAKEN_OVER, Ordering::AcqRel) != TAKEN_OVER
    }

    /// Writes `note` over `registers`, the owner a takeover built, waiting
    /// for room within [`EARLY_PATIENCE`]: nothing else runs to yield to.
    /// What does not fit is dropped and counted.
    pub fn write_taken_over<R: EndpointRegisters>(&self, registers: R, note: &Note<'_>) {
        FifoWriter::new(Fifo(Some(registers)), &self.lost, EARLY_PATIENCE).note(note);
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
