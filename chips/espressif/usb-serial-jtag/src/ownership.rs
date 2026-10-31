//! Who holds the USB Serial/JTAG block, and the writers that hold it
//! without its driver: the early writer of the bootstrap and a runtime's
//! early boot, and the fatal path.
//!
//! The chip's root installs the console's parts once ([`Ownership::install`])
//! in one [`ClaimCell`], whose compare-and-swap of the block's state makes
//! each caller its only holder. Nothing waits for a state:
//!
//! - Free → lent for one early record ([`Ownership::write`]), free again
//!   once the record ends. A record that finds the block lent to another
//!   one, on another hart or the one it interrupted, or not installed yet,
//!   is dropped and counted. The record's [`Written`] says which: only a
//!   whole one acknowledges what it reports.
//! - Free → the driver's, for good: the console's driver takes the parts
//!   out ([`Ownership::retire`]). While a record holds them the driver is
//!   refused ([`RetireError::Busy`]); afterwards every early record is
//!   refused and goes to the driver.
//! - Free → the fatal path's, for good ([`Ownership::write_fatal`]). The
//!   fatal path never takes the block from anyone: while an early record or
//!   the driver holds it, or a fatal note took it before, the note is
//!   refused and its caller keeps it elsewhere. So every holder of the
//!   block is alone, and the driver's parts hold it for as long as they
//!   live.
//!
//! The early writer writes while the FIFO has room, drops the rest and
//! counts it ([`oer_diagnostics::FifoWriter`]); before an executor exists
//! there is nothing to yield to, so it waits for room within
//! [`EARLY_PATIENCE`] per record. A fatal note never waits.

use oer_diagnostics::{FifoWriter, Lost, Note, Patience, Written};
use oer_memory::{Claim, ClaimCell};

use crate::EndpointRegisters;

#[cfg(test)]
mod tests;

/// What the early writer may wait for room in the 64-byte FIFO, per record,
/// in polls of the room flag: the host takes a packet in about a millisecond,
/// and a poll is a register read of well under a microsecond, so the budget is
/// some milliseconds up to tens of them at the early boot's slow clock. Before
/// an executor exists there is nothing to yield to.
pub const EARLY_PATIENCE: Patience = Patience::polls(100_000);

/// The console's parts as the chip's root installs them: the early and
/// fatal writers reach only their endpoint.
pub trait Console {
    /// The task's registers of the block.
    type Endpoint: EndpointRegisters;
    /// The endpoint, for one record.
    fn endpoint(&mut self) -> &mut Self::Endpoint;
}

/// The driver could not take the block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetireError {
    /// The chip's root installed no parts yet.
    Empty,
    /// An early record holds the block now; the caller may try again once
    /// it ended.
    Busy,
    /// The driver or the fatal path took the block before.
    Taken,
}

/// The block's holder and the bytes the writers without the driver dropped.
pub struct Ownership<T> {
    parts: ClaimCell<T>,
    lost: Lost,
}

/// The FIFO of a lent endpoint for the diagnostic writer; without one it
/// never has room and a record written to it is only counted.
pub struct Fifo<'a, R>(Option<&'a mut R>);

impl<R: EndpointRegisters> oer_diagnostics::Fifo for Fifo<'_, R> {
    fn has_room(&self) -> bool {
        self.0.as_deref().is_some_and(EndpointRegisters::has_room)
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

impl<T: Console> Ownership<T> {
    pub const fn new() -> Self {
        Self {
            parts: ClaimCell::new(),
            lost: Lost::new(),
        }
    }

    /// Install the console's parts, once: the block is free from now on.
    ///
    /// # Errors
    ///
    /// Parts were installed before: `parts` comes back unchanged.
    pub fn install(&self, parts: T) -> Result<(), T> {
        self.parts.install(parts)
    }

    /// Runs `write` with a writer over the endpoint, if the block is free;
    /// the block is free again once `write` returns, which reports the
    /// bytes it dropped. While another early record holds the block, or
    /// before the parts are installed, `write` runs over no endpoint and its
    /// bytes are counted. Once the driver or the fatal path took the block,
    /// nothing is called and nothing counted. Only [`Written::Whole`]
    /// acknowledges the record; [`Written::Retired`] hands it to the driver.
    pub fn write(
        &self,
        write: impl FnOnce(&mut FifoWriter<'_, Fifo<'_, T::Endpoint>>) -> usize,
    ) -> Written {
        let mut write = Some(write);
        let mut dropped = 0;
        let lent = self.parts.lend(|parts| {
            if let Some(write) = write.take() {
                dropped = write(&mut FifoWriter::new(
                    Fifo(Some(parts.endpoint())),
                    &self.lost,
                    EARLY_PATIENCE,
                ));
            }
        });
        let refused = match lent {
            Ok(()) => return Written::of_dropped(dropped),
            Err(Claim::Kept) => return Written::Retired,
            Err(Claim::Empty) => Written::NoConsole,
            Err(Claim::Lent) => Written::Busy,
        };
        if let Some(write) = write.take() {
            let _dropped = write(&mut FifoWriter::new(Fifo(None), &self.lost, Patience::NONE));
        }
        refused
    }

    /// Writes `note` without waiting over the endpoint, if the block is
    /// free; the block then stays the fatal path's for good. While anyone
    /// holds the block, or before the parts are installed, nothing is
    /// written or counted: returns `false`, and the caller keeps the note
    /// elsewhere.
    pub fn write_fatal(&self, note: &Note<'_>) -> bool {
        self.parts
            .seal(|parts| {
                FifoWriter::new(Fifo(Some(parts.endpoint())), &self.lost, Patience::NONE)
                    .note(note);
            })
            .is_ok()
    }

    /// Gives the parts to the console's driver for good, if the block is
    /// free: every later early record is refused.
    ///
    /// # Errors
    /// No parts were installed, an early record holds the block, or the
    /// block was taken before.
    pub fn retire(&self) -> Result<T, RetireError> {
        self.parts.take().map_err(|claim| match claim {
            Claim::Empty => RetireError::Empty,
            Claim::Lent => RetireError::Busy,
            Claim::Kept => RetireError::Taken,
        })
    }

    /// Bytes dropped since the last call, for the driver's owner to report
    /// once it can: a loss is never silent.
    pub fn take_lost(&self) -> u32 {
        self.lost.take()
    }
}

impl<T: Console> Default for Ownership<T> {
    fn default() -> Self {
        Self::new()
    }
}
