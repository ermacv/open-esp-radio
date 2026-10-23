//! A synchronous byte writer for a small hardware FIFO: it writes while the
//! FIFO has room, drops the rest and counts what it dropped. A writer with a
//! [`Patience`] waits for room first, up to a budget of polls per record, for
//! the time before an executor exists and there is nothing to yield to; a
//! writer without it never waits. The chip supplies the [`Fifo`]; the logic is
//! host-testable.
//!
//! A loss is never silent: the next note is preceded by a
//! [`Note::diag_lost`] carrying the dropped bytes, and the count is cleared
//! only after that note went out whole.

use core::convert::Infallible;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::{Note, emit_note};

/// The transmit side of a hardware FIFO.
pub trait Fifo {
    /// Whether [`Fifo::push`] would be accepted now.
    fn has_room(&self) -> bool;
    /// Appends one byte; called only after `has_room` said yes.
    fn push(&mut self, byte: u8);
    /// Hands the bytes pushed so far to the receiver; never waits.
    fn flush(&mut self);
}

/// Bytes dropped by the writers of one chip, shared by every writer that
/// owns the FIFO in turn.
pub struct Lost(AtomicU32);

impl Lost {
    pub const fn new() -> Self {
        Self(AtomicU32::new(0))
    }

    /// Bytes dropped and not yet reported.
    pub fn pending(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }

    /// Takes the count for a reporter that has a way out of its own.
    pub fn take(&self) -> u32 {
        self.0.swap(0, Ordering::Relaxed)
    }

    fn add(&self, bytes: usize) {
        let bytes = u32::try_from(bytes).unwrap_or(u32::MAX);
        let _ = self
            .0
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                Some(count.saturating_add(bytes))
            });
    }

    fn settle(&self, reported: u32) {
        let _ = self
            .0
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                Some(count.saturating_sub(reported))
            });
    }
}

impl Default for Lost {
    fn default() -> Self {
        Self::new()
    }
}

/// How long a writer may wait for room in a full FIFO: polls of
/// [`Fifo::has_room`] per record, shared by all the bytes of the record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Patience(u32);

impl Patience {
    /// Never waits: a byte without room is dropped at once.
    pub const NONE: Self = Self(0);

    pub const fn polls(polls: u32) -> Self {
        Self(polls)
    }
}

/// The one writer of a FIFO at a time.
pub struct FifoWriter<'a, F> {
    fifo: F,
    lost: &'a Lost,
    patience: Patience,
    /// Polls this record may still spend.
    budget: u32,
}

impl<'a, F: Fifo> FifoWriter<'a, F> {
    pub const fn new(fifo: F, lost: &'a Lost, patience: Patience) -> Self {
        Self {
            fifo,
            lost,
            patience,
            budget: patience.0,
        }
    }

    /// Waits for room within the record's budget; `false` when none came.
    fn wait_for_room(&mut self) -> bool {
        if self.budget == 0 {
            return false;
        }
        // What is in the FIFO must be on its way to the host before room can
        // appear.
        self.fifo.flush();
        while self.budget > 0 {
            self.budget -= 1;
            if self.fifo.has_room() {
                return true;
            }
        }
        false
    }

    /// Writes `bytes` as far as the FIFO has room and returns how many it
    /// dropped. Does not flush and does not count.
    fn push_all(&mut self, bytes: &[u8]) -> usize {
        let mut dropped = 0;
        for &byte in bytes {
            if self.fifo.has_room() || self.wait_for_room() {
                self.fifo.push(byte);
            } else {
                dropped += 1;
            }
        }
        dropped
    }

    /// Writes `bytes` and flushes, counting what did not fit: for text that
    /// is not a note.
    pub fn text(&mut self, bytes: &[u8]) {
        self.budget = self.patience.0;
        let dropped = self.push_all(bytes);
        self.lost.add(dropped);
        self.fifo.flush();
    }

    /// Writes `note` as one `OER-DIAG:01` line, after the report of earlier
    /// losses if there are any, and flushes. A line the FIFO has no room for
    /// is dropped whole or in its tail, never waited for.
    pub fn note(&mut self, note: &Note<'_>) {
        let pending = self.lost.pending();
        if pending != 0 && self.emit(&Note::diag_lost(pending)) == 0 {
            self.lost.settle(pending);
        }
        let dropped = self.emit(note);
        self.lost.add(dropped);
        self.fifo.flush();
    }

    /// Emits `note` and returns the bytes dropped.
    fn emit(&mut self, note: &Note<'_>) -> usize {
        self.budget = self.patience.0;
        let mut dropped = 0;
        let Ok(()) = emit_note(note, |bytes: &[u8]| {
            dropped += self.push_all(bytes);
            Ok::<(), Infallible>(())
        });
        dropped
    }
}

#[cfg(test)]
mod tests;
