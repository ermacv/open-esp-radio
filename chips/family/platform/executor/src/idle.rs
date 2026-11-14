//! Each hart's time in its thread-mode executor's wait for an interrupt,
//! and its wakes: an account the wait's two edges keep, read at a window's
//! two ends. It costs two reads of the executor's clock and a few stores a
//! wake, and nothing a poll or a frame; every build keeps it.
//!
//! One writer a hart (its thread mode, masked around the wait), readers on
//! any hart: the total is two words behind a sequence, and a read that
//! overlaps a write answers [`Concurrent`] for its caller to read again.

use core::sync::atomic::{AtomicU32, Ordering, fence};

/// A read overlapped the hart's write of its account: read it again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Concurrent;

/// A hart's idle account at one read.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IdleReading {
    /// Microseconds in the wait since boot.
    pub idle_us: u64,
    /// Wakes from the wait since boot, wrapping.
    pub wakes: u32,
}

/// One hart's idle account.
pub struct IdleAccount {
    /// Odd while the writer updates the words.
    sequence: AtomicU32,
    idle: [AtomicU32; 2],
    wakes: AtomicU32,
}

impl IdleAccount {
    pub const fn new() -> Self {
        Self {
            sequence: AtomicU32::new(0),
            idle: [AtomicU32::new(0), AtomicU32::new(0)],
            wakes: AtomicU32::new(0),
        }
    }

    /// The hart woke after `span_us` in the wait. The hart's one writer.
    pub fn record(&self, span_us: u32) {
        let sequence = self.sequence.load(Ordering::Relaxed);
        self.sequence
            .store(sequence.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        let [low, high] = &self.idle;
        let total = (u64::from(high.load(Ordering::Relaxed)) << 32
            | u64::from(low.load(Ordering::Relaxed)))
        .wrapping_add(u64::from(span_us));
        low.store(total as u32, Ordering::Relaxed);
        high.store((total >> 32) as u32, Ordering::Relaxed);
        self.wakes.store(
            self.wakes.load(Ordering::Relaxed).wrapping_add(1),
            Ordering::Relaxed,
        );
        self.sequence
            .store(sequence.wrapping_add(2), Ordering::Release);
    }

    /// The account now.
    ///
    /// # Errors
    /// [`Concurrent`]: the hart wrote it meanwhile.
    pub fn read(&self) -> Result<IdleReading, Concurrent> {
        let before = self.sequence.load(Ordering::Acquire);
        if before % 2 != 0 {
            return Err(Concurrent);
        }
        let [low, high] = &self.idle;
        let idle_us =
            u64::from(high.load(Ordering::Relaxed)) << 32 | u64::from(low.load(Ordering::Relaxed));
        let wakes = self.wakes.load(Ordering::Relaxed);
        fence(Ordering::Acquire);
        if self.sequence.load(Ordering::Relaxed) == before {
            Ok(IdleReading { idle_us, wakes })
        } else {
            Err(Concurrent)
        }
    }
}

impl Default for IdleAccount {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spans add up past one word, wakes count, and a read in the middle
    /// of a write is refused.
    #[test]
    fn spans_add_up_past_a_word_and_a_read_inside_a_write_is_refused() {
        let account = IdleAccount::new();
        assert_eq!(account.read(), Ok(IdleReading::default()));
        account.record(u32::MAX);
        account.record(2);
        assert_eq!(
            account.read(),
            Ok(IdleReading {
                idle_us: u64::from(u32::MAX) + 2,
                wakes: 2
            })
        );
        account.sequence.store(5, Ordering::Relaxed);
        assert_eq!(account.read(), Err(Concurrent));
    }
}
