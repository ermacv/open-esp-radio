//! The writer of the USB Serial/JTAG FIFO before its driver owns the block:
//! the bootstrap, a runtime's early boot and a fatal path.
//!
//! The chip's HAL keeps one [`Early`] and builds its register owner for
//! each record through the `capture` it passes to [`Early::write`], which
//! calls it only until [`Early::retire`]: the console's driver retires the
//! early writer before it takes the block, so the two never own it at the
//! same time. The writer writes while the FIFO has room, drops the rest and
//! counts it ([`oer_diagnostics::FifoWriter`]); before an executor exists
//! there is nothing to yield to, so it waits for room within
//! [`EARLY_PATIENCE`] per record. [`Early::write_fatal`] never waits.

use core::sync::atomic::{AtomicBool, Ordering};

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

/// The early writer's state: whether the driver took the block, and the
/// bytes dropped so far.
pub struct Early {
    retired: AtomicBool,
    lost: Lost,
}

/// The FIFO of a register owner, for the diagnostic writer.
pub struct Fifo<R>(R);

impl<R: EndpointRegisters> oer_diagnostics::Fifo for Fifo<R> {
    fn has_room(&self) -> bool {
        self.0.has_room()
    }

    fn push(&mut self, byte: u8) {
        self.0.push(byte);
    }

    fn flush(&mut self) {
        self.0.send();
    }
}

/// A FIFO that never has room: a record written to it is only counted.
struct Closed;

impl oer_diagnostics::Fifo for Closed {
    fn has_room(&self) -> bool {
        false
    }

    fn push(&mut self, _: u8) {}

    fn flush(&mut self) {}
}

impl Early {
    pub const fn new() -> Self {
        Self {
            retired: AtomicBool::new(false),
            lost: Lost::new(),
        }
    }

    /// Runs `write` with a writer over the owner `capture` builds, unless the
    /// writer is retired: then `capture` is not called. Returns whether it
    /// wrote.
    pub fn write<R: EndpointRegisters>(
        &self,
        capture: impl FnOnce() -> R,
        write: impl FnOnce(&mut FifoWriter<'_, Fifo<R>>),
    ) -> bool {
        if self.retired.load(Ordering::Acquire) {
            return false;
        }
        write(&mut FifoWriter::new(
            Fifo(capture()),
            &self.lost,
            EARLY_PATIENCE,
        ));
        true
    }

    /// Writes `note` on a path that never returns, without waiting, retired
    /// or not: over `owner` when the caller could take the block, otherwise
    /// only counting its bytes.
    pub fn write_fatal<R: EndpointRegisters>(&self, owner: Option<R>, note: &Note<'_>) {
        match owner {
            Some(owner) => FifoWriter::new(Fifo(owner), &self.lost, Patience::NONE).note(note),
            None => FifoWriter::new(Closed, &self.lost, Patience::NONE).note(note),
        }
    }

    /// Stops the early writer for good: the console's driver takes the
    /// block. Every later [`write`](Self::write) is refused.
    pub fn retire(&self) {
        self.retired.store(true, Ordering::Release);
    }

    /// Bytes dropped since the last call, for the driver's owner to report
    /// once it can: a loss is never silent.
    pub fn take_lost(&self) -> u32 {
        self.lost.take()
    }
}

impl Default for Early {
    fn default() -> Self {
        Self::new()
    }
}
