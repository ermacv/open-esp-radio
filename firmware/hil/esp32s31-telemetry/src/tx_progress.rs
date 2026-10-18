//! Bounded progress evidence for the sparse second UDP flow.
//!
//! Each frontier has one writer. The workload is stopped before the radio
//! executor snapshots both frontiers. Timestamps use wrapping microseconds;
//! an observed interval must be shorter than one u32 clock wrap (~71 minutes).
use core::sync::atomic::{AtomicU32, Ordering};

pub struct Counters {
    count: AtomicU32,
    started_at: AtomicU32,
    first_at: AtomicU32,
    errors: AtomicU32,
    last_sequence: AtomicU32,
    last_at: AtomicU32,
    maximum_gap: AtomicU32,
    sequence_after_gap: AtomicU32,
    pending_polls: AtomicU32,
    pending_since: AtomicU32,
    pending: AtomicU32,
    maximum_wait: AtomicU32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Snapshot {
    pub count: u32,
    pub errors: u32,
    pub first_admission_micros: Option<u32>,
    pub last_sequence: u32,
    pub maximum_gap_micros: u32,
    pub sequence_after_maximum_gap: u32,
    pub idle_micros: u32,
    pub pending_polls: u32,
    pub maximum_wait_micros: u32,
    pub pending_micros: u32,
}

impl Counters {
    pub const fn new() -> Self {
        Self {
            count: AtomicU32::new(0),
            started_at: AtomicU32::new(0),
            first_at: AtomicU32::new(0),
            errors: AtomicU32::new(0),
            last_sequence: AtomicU32::new(0),
            last_at: AtomicU32::new(0),
            maximum_gap: AtomicU32::new(0),
            sequence_after_gap: AtomicU32::new(0),
            pending_polls: AtomicU32::new(0),
            pending_since: AtomicU32::new(0),
            pending: AtomicU32::new(0),
            maximum_wait: AtomicU32::new(0),
        }
    }

    /// Called at the workload boundary while the socket producer is suspended.
    pub fn reset(&self, now: u32) {
        self.started_at.store(now, Ordering::Relaxed);
        self.errors.store(0, Ordering::Relaxed);
        self.count.store(0, Ordering::Relaxed);
        self.maximum_gap.store(0, Ordering::Relaxed);
        self.sequence_after_gap.store(0, Ordering::Relaxed);
        self.pending_polls.store(0, Ordering::Relaxed);
        self.pending.store(0, Ordering::Relaxed);
        self.maximum_wait.store(0, Ordering::Relaxed);
        self.last_sequence.store(0, Ordering::Relaxed);
    }

    pub fn blocked(&self, now: u32) {
        self.pending_polls.fetch_add(1, Ordering::Relaxed);
        if self.pending.load(Ordering::Relaxed) == 0 {
            self.pending_since.store(now, Ordering::Relaxed);
            self.pending.store(1, Ordering::Relaxed);
        }
    }

    pub fn admitted(&self, sequence: u32, now: u32) {
        if self.count.load(Ordering::Relaxed) != 0 {
            let gap = now.wrapping_sub(self.last_at.load(Ordering::Relaxed));
            if gap > self.maximum_gap.load(Ordering::Relaxed) {
                self.maximum_gap.store(gap, Ordering::Relaxed);
                self.sequence_after_gap.store(sequence, Ordering::Relaxed);
            }
        } else {
            self.first_at.store(now, Ordering::Relaxed);
        }
        self.finish_wait(now);
        self.last_at.store(now, Ordering::Relaxed);
        self.last_sequence.store(sequence, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// A terminal send failure closes the wait; it is not a successful admission.
    pub fn failed(&self, now: u32) {
        self.finish_wait(now);
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    fn finish_wait(&self, now: u32) {
        if self.pending.swap(0, Ordering::Relaxed) != 0 {
            self.maximum_wait.fetch_max(
                now.wrapping_sub(self.pending_since.load(Ordering::Relaxed)),
                Ordering::Relaxed,
            );
        }
    }

    pub fn snapshot(&self, now: u32) -> Snapshot {
        let count = self.count.load(Ordering::Relaxed);
        Snapshot {
            count,
            errors: self.errors.load(Ordering::Relaxed),
            first_admission_micros: (count != 0).then(|| {
                self.first_at
                    .load(Ordering::Relaxed)
                    .wrapping_sub(self.started_at.load(Ordering::Relaxed))
            }),
            last_sequence: self.last_sequence.load(Ordering::Relaxed),
            maximum_gap_micros: self.maximum_gap.load(Ordering::Relaxed),
            sequence_after_maximum_gap: self.sequence_after_gap.load(Ordering::Relaxed),
            idle_micros: if count == 0 {
                now.wrapping_sub(self.started_at.load(Ordering::Relaxed))
            } else {
                now.wrapping_sub(self.last_at.load(Ordering::Relaxed))
            },
            pending_polls: self.pending_polls.load(Ordering::Relaxed),
            maximum_wait_micros: self.maximum_wait.load(Ordering::Relaxed),
            pending_micros: if self.pending.load(Ordering::Relaxed) == 0 {
                0
            } else {
                now.wrapping_sub(self.pending_since.load(Ordering::Relaxed))
            },
        }
    }
}

impl Default for Counters {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
