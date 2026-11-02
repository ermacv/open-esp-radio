//! An executor's CPU since boot, from its activity boundaries
//! (`oer_espressif_executor::observation::Activity`).
//!
//! Work spans include the executor's preemption, so the spans of two
//! executors overlap and do not add up. A thread-mode executor's idle spans
//! (its masked WFI) are its hart's idle time. Activity times wrap at
//! 2^32 µs; a counter keeps its totals in 64 bits.

use core::sync::atomic::{AtomicU32, Ordering, fence};

use super::Concurrent;

const OPEN_NONE: u32 = 0;
const OPEN_WORK: u32 = 1;
const OPEN_IDLE: u32 = 2;
const SEEN_WORK: u8 = 1;
const SEEN_IDLE: u8 = 2;

/// An executor's activity boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    WorkStart,
    WorkEnd,
    IdleStart,
    IdleEnd,
}

/// One boundary at `at_us`, wrapping activity microseconds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Boundary {
    pub at_us: u32,
    pub executor: u8,
    pub hart: u8,
    pub level: u8,
    pub phase: Phase,
}

/// What one executor accumulated: `None` for a kind of span it never
/// reported.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExecutorSample {
    /// The executor's number, its hart and its interrupt level (0 for
    /// thread mode), as its boundaries name them.
    pub executor: u8,
    pub hart: u8,
    pub level: u8,
    /// Its work and idle microseconds since boot, the open span included.
    pub work_us: Option<u64>,
    pub idle_us: Option<u64>,
}

/// The cumulative work and idle microseconds of one executor.
pub struct CpuCounter {
    sequence: AtomicU32,
    /// Low and high words.
    work: [AtomicU32; 2],
    idle: [AtomicU32; 2],
    /// The start of the open span.
    start: AtomicU32,
    open: AtomicU32,
    /// Executor, hart, level and the kinds of span seen.
    context: AtomicU32,
}

impl CpuCounter {
    pub const fn new() -> Self {
        Self {
            sequence: AtomicU32::new(0),
            work: [AtomicU32::new(0), AtomicU32::new(0)],
            idle: [AtomicU32::new(0), AtomicU32::new(0)],
            start: AtomicU32::new(0),
            open: AtomicU32::new(OPEN_NONE),
            context: AtomicU32::new(0),
        }
    }

    /// Account `boundary`; only the counter's executor calls it. An end
    /// without its start adds nothing.
    pub fn observe(&self, boundary: Boundary) {
        let sequence = self.sequence.load(Ordering::Relaxed);
        self.sequence
            .store(sequence.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        let [_, _, _, seen] = self.context.load(Ordering::Relaxed).to_le_bytes();
        let open = self.open.load(Ordering::Relaxed);
        let span = || {
            boundary
                .at_us
                .wrapping_sub(self.start.load(Ordering::Relaxed))
        };
        let seen = match boundary.phase {
            Phase::WorkStart | Phase::IdleStart => {
                self.start.store(boundary.at_us, Ordering::Relaxed);
                let (kind, open) = match boundary.phase {
                    Phase::WorkStart => (SEEN_WORK, OPEN_WORK),
                    _ => (SEEN_IDLE, OPEN_IDLE),
                };
                self.open.store(open, Ordering::Relaxed);
                seen | kind
            }
            Phase::WorkEnd | Phase::IdleEnd => {
                match (boundary.phase, open) {
                    (Phase::WorkEnd, OPEN_WORK) => add(&self.work, span()),
                    (Phase::IdleEnd, OPEN_IDLE) => add(&self.idle, span()),
                    _ => {}
                }
                self.open.store(OPEN_NONE, Ordering::Relaxed);
                seen
            }
        };
        self.context.store(
            u32::from_le_bytes([boundary.executor, boundary.hart, boundary.level, seen]),
            Ordering::Relaxed,
        );
        self.sequence
            .store(sequence.wrapping_add(2), Ordering::Release);
    }

    /// The totals at `now_us` (activity microseconds), the open span
    /// included.
    ///
    /// # Errors
    /// [`Concurrent`] while the executor writes.
    pub fn sample(&self, now_us: u32) -> Result<ExecutorSample, Concurrent> {
        let sequence = self.sequence.load(Ordering::Acquire);
        if sequence & 1 != 0 {
            return Err(Concurrent);
        }
        let work = total(&self.work);
        let idle = total(&self.idle);
        let start = self.start.load(Ordering::Relaxed);
        let open = self.open.load(Ordering::Relaxed);
        let [executor, hart, level, seen] = self.context.load(Ordering::Relaxed).to_le_bytes();
        fence(Ordering::Acquire);
        if self.sequence.load(Ordering::Relaxed) != sequence {
            return Err(Concurrent);
        }
        let span = u64::from(now_us.wrapping_sub(start));
        let with_open = |total: u64, kind| {
            if open == kind {
                total.wrapping_add(span)
            } else {
                total
            }
        };
        Ok(ExecutorSample {
            executor,
            hart,
            level,
            work_us: (seen & SEEN_WORK != 0).then(|| with_open(work, OPEN_WORK)),
            idle_us: (seen & SEEN_IDLE != 0).then(|| with_open(idle, OPEN_IDLE)),
        })
    }
}

impl Default for CpuCounter {
    fn default() -> Self {
        Self::new()
    }
}

fn add(total: &[AtomicU32; 2], span: u32) {
    let [low, high] = total;
    let [l0, l1, l2, l3, h0, h1, h2, h3] = (u64::from(high.load(Ordering::Relaxed)) << 32
        | u64::from(low.load(Ordering::Relaxed)))
    .wrapping_add(u64::from(span))
    .to_le_bytes();
    low.store(u32::from_le_bytes([l0, l1, l2, l3]), Ordering::Relaxed);
    high.store(u32::from_le_bytes([h0, h1, h2, h3]), Ordering::Relaxed);
}

fn total(total: &[AtomicU32; 2]) -> u64 {
    let [low, high] = total;
    u64::from(high.load(Ordering::Relaxed)) << 32 | u64::from(low.load(Ordering::Relaxed))
}

#[cfg(test)]
mod tests;
