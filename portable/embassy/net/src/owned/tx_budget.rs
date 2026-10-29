//! Admission credit follows a software packet through queueing and radio retention.

use core::{
    sync::atomic::{AtomicUsize, Ordering},
    task::Waker,
};

use oer_sync::PriorityWaker;

/// Lock-free admission count. A credit may be released in any context, so
/// releasing never depends on the endpoint's execution context.
pub(super) struct TxBudget {
    capacity: usize,
    outstanding: AtomicUsize,
    sender: PriorityWaker,
}

impl TxBudget {
    pub(super) const fn new(capacity: usize) -> Self {
        Self {
            capacity,
            outstanding: AtomicUsize::new(0),
            sender: PriorityWaker::new(),
        }
    }

    /// Bind the next capacity notification to the sole network device's task.
    pub(super) fn register_sender(&self, waker: &Waker) {
        self.sender.register(waker);
    }

    pub(super) fn is_full(&self) -> bool {
        self.outstanding.load(Ordering::Acquire) == self.capacity
    }

    pub(super) fn try_admit(&self) -> bool {
        self.outstanding
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |outstanding| {
                outstanding
                    .checked_add(1)
                    .filter(|next| *next <= self.capacity)
            })
            .is_ok()
    }

    /// Transfer one already admitted credit, either from a dequeued packet or
    /// back from a failed insertion. Only these two queue paths mint a credit;
    /// each admitted packet therefore releases exactly once.
    pub(super) fn claim(&self) -> TxCredit<'_> {
        TxCredit(self)
    }
}

/// Non-cloneable credit; stored after the packet so packet destruction happens
/// before the producer is told that another admission is possible.
pub(super) struct TxCredit<'resources>(&'resources TxBudget);

impl Drop for TxCredit<'_> {
    fn drop(&mut self) {
        // Each affine credit discharges a successful admission exactly once.
        // No API can construct or clone a credit, so this never wraps.
        let previous = self.0.outstanding.fetch_sub(1, Ordering::AcqRel);
        if previous == self.0.capacity {
            self.0.sender.wake();
        }
    }
}
