//! Each hart's time in its thread-mode executor's wait for an interrupt,
//! and its wakes: an account the wait's two edges keep, read at a window's
//! two ends. It costs two reads of the executor's clock and three stores a
//! wake, and nothing a poll or a frame; only a build of the `cpu-load`
//! instrumentation class keeps it (`oer_cpu_load`).
//!
//! One writer a hart, its thread mode with interrupts masked around the
//! wait, and one reader, on the same hart (`Cpu0`, `Cpu1` of the entry):
//! the reader never runs inside the write, so the words need no sequence.

use core::sync::atomic::{AtomicU32, Ordering};

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
    /// The microseconds' low and high words.
    idle: [AtomicU32; 2],
    wakes: AtomicU32,
}

impl IdleAccount {
    pub const fn new() -> Self {
        Self {
            idle: [AtomicU32::new(0), AtomicU32::new(0)],
            wakes: AtomicU32::new(0),
        }
    }

    /// The hart woke after `span_us` in the wait. The hart's one writer.
    pub fn record(&self, span_us: u32) {
        let total = self.idle_us().wrapping_add(u64::from(span_us));
        let [low, high] = &self.idle;
        low.store(total as u32, Ordering::Relaxed);
        high.store((total >> 32) as u32, Ordering::Relaxed);
        self.wakes.store(
            self.wakes.load(Ordering::Relaxed).wrapping_add(1),
            Ordering::Relaxed,
        );
    }

    /// The account now, read on the writer's hart.
    pub fn read(&self) -> IdleReading {
        IdleReading {
            idle_us: self.idle_us(),
            wakes: self.wakes.load(Ordering::Relaxed),
        }
    }

    fn idle_us(&self) -> u64 {
        let [low, high] = &self.idle;
        u64::from(high.load(Ordering::Relaxed)) << 32 | u64::from(low.load(Ordering::Relaxed))
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

    /// Spans add up past one word and each wake counts.
    #[test]
    fn spans_add_up_past_a_word_and_each_wake_counts() {
        let account = IdleAccount::new();
        assert_eq!(account.read(), IdleReading::default());
        account.record(u32::MAX);
        account.record(2);
        assert_eq!(
            account.read(),
            IdleReading {
                idle_us: u64::from(u32::MAX) + 2,
                wakes: 2
            }
        );
    }
}
