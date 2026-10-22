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
