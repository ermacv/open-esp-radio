//! A `u64` that one context publishes and one other context reads, without a
//! lock and without waiting.
//!
//! The writer bumps the generation to odd, stores both halves, then bumps it
//! to even. A reader that sees an odd generation, or a generation that changed
//! while it read, has a torn value and returns `None`: it must not use it and
//! must not wait for the writer either, which may be the context it preempted.
//! The writer therefore always notifies the reader after it finished a write,
//! so a reader that gave up reads again.

use core::sync::atomic::{AtomicU32, Ordering, fence};

/// A single-writer, single-reader `u64` with its write generation.
pub(crate) struct DeadlineCell {
    generation: AtomicU32,
    low: AtomicU32,
    high: AtomicU32,
}

/// A value read whole, with the generation that wrote it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Published {
    pub(crate) value: u64,
    pub(crate) generation: u32,
}

impl DeadlineCell {
    pub(crate) const fn new(value: u64) -> Self {
        Self {
            generation: AtomicU32::new(0),
            low: AtomicU32::new(value as u32),
            high: AtomicU32::new((value >> 32) as u32),
        }
    }

    /// Publish `value`. Only the cell's one writer calls this; it then
    /// notifies the reader.
    pub(crate) fn publish(&self, value: u64) {
        let generation = self.generation.load(Ordering::Relaxed);
        self.generation
            .store(generation.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        self.low.store(value as u32, Ordering::Relaxed);
        self.high.store((value >> 32) as u32, Ordering::Relaxed);
        self.generation
            .store(generation.wrapping_add(2), Ordering::Release);
    }

    /// The last value published whole, or `None` when the read overlapped a
    /// write.
    pub(crate) fn read(&self) -> Option<Published> {
        let before = self.generation.load(Ordering::Acquire);
        if !before.is_multiple_of(2) {
            return None;
        }
        let low = self.low.load(Ordering::Relaxed);
        let high = self.high.load(Ordering::Relaxed);
        fence(Ordering::Acquire);
        let after = self.generation.load(Ordering::Relaxed);
        (before == after).then_some(Published {
            value: (u64::from(high) << 32) | u64::from(low),
            generation: before,
        })
    }
}

#[cfg(test)]
mod tests;
