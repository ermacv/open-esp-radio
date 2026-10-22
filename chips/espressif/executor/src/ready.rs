//! Ready run-queue tokens and the thread-mode wake flag share one byte.
//!
//! Embassy invokes ready_begin exactly once per successful enqueue, before
//! publishing its token, and exec_begin once per dequeue, before polling.
//! A batch detached from the queue is still counted until each exec_begin.
//! Wake during poll creates a new token; duplicate wake creates none. Finish
//! does not remove tokens: an already queued finished task is dequeued through
//! poll_exited and receives exec_begin too. No TaskHeader layout is inspected.
use core::sync::atomic::{AtomicU8, Ordering};

/// Bit zero is the thread-mode wake flag; the upper bits count ready tokens.
/// An image may have at most 126 task slots on an executor: dequeue can clear
/// one task's queued bit just before exec_begin, allowing one extra enqueue.
/// Capacity violations fail before modifying state, never wrap to no-ready.
#[repr(transparent)]
pub(crate) struct ReadyState(AtomicU8);

impl ReadyState {
    #[cfg(test)]
    pub(crate) const fn new() -> Self {
        Self(AtomicU8::new(0))
    }

    pub(crate) fn ready_begin(&self) {
        self.0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                state.checked_add(2)
            })
            .expect("executor ready-token capacity exceeded");
    }

    pub(crate) fn exec_begin(&self) {
        self.0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                state.checked_sub(2)
            })
            .expect("executor dequeued an uncounted ready token");
    }

    pub(crate) fn ready(&self) -> bool {
        self.0.load(Ordering::Acquire) >= 2
    }

    pub(crate) fn mark_work(&self) {
        self.0.fetch_or(1, Ordering::Release);
    }

    pub(crate) fn clear_work(&self) {
        self.0.fetch_and(!1, Ordering::AcqRel);
    }

    pub(crate) fn work(&self) -> bool {
        self.0.load(Ordering::Acquire) & 1 != 0
    }
}

#[cfg(test)]
pub(crate) mod tests;

// Two low bits identify the registered core (0/1); 3 remains unassigned.
// The existing core-assignment word also carries a wrapping poll timestamp.
// A live poll is bounded far below the 2^30-microsecond timestamp period.
const STAMP_MASK: u32 = (1 << 30) - 1;
pub(crate) fn assigned_core(state: usize) -> Option<usize> {
    let core = state & 3;
    (core < 2).then_some(core)
}
pub(crate) fn poll_stamp(state: usize, now: u64) -> usize {
    (state & 3) | (((now as u32 & STAMP_MASK) as usize) << 2)
}
pub(crate) fn poll_elapsed(state: usize, now: u64) -> u32 {
    (now as u32).wrapping_sub((state >> 2) as u32) & STAMP_MASK
}
/// Reserve 100 us for admission, one <=50-us atom and the return path.
pub(crate) const POLL_ADMISSION_MICROS: u32 = oer_cooperate::POLL_ADMISSION_MICROS;
