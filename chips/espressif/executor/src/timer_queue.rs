//! Deadline bookkeeping around the single Embassy queue owned by the time driver.
use core::task::Waker;
use embassy_time_queue_utils::Queue;

pub(crate) struct WakeQueue {
    queue: Queue,
    next_deadline: u64,
    armed: u64,
}

impl WakeQueue {
    pub(crate) const fn new() -> Self {
        Self {
            queue: Queue::new(),
            next_deadline: u64::MAX,
            armed: u64::MAX,
        }
    }

    #[cfg(test)]
    pub(crate) fn next_deadline(&self) -> u64 {
        self.next_deadline
    }

    /// Reconcile a changed queue or an alarm that actually fired. An unrelated
    /// executor wake leaves a retained future alarm programmed as it was.
    pub(crate) fn alarm_deadline(&mut self, fired: bool) -> Option<u64> {
        if !fired && self.next_deadline == self.armed {
            return None;
        }
        self.armed = self.next_deadline;
        Some(self.next_deadline)
    }

    /// Return the sampled time when the hardware alarm needs reconciliation.
    pub(crate) fn schedule_wake(
        &mut self,
        at: u64,
        waker: &Waker,
        now: impl FnOnce() -> u64,
    ) -> Option<u64> {
        if !self.queue.schedule_wake(at, waker) {
            return None;
        }
        self.next_deadline = self.next_deadline.min(at);
        let now = now();
        if self.next_deadline <= now {
            // Consume through the existing queue: this retires the old entry,
            // wakes every due task and preserves the earliest future deadline.
            self.dispatch_expired(now);
        }
        Some(now)
    }

    pub(crate) fn dispatch_expired(&mut self, now: u64) {
        self.next_deadline = self.queue.next_expiration(now);
    }
}

#[cfg(all(test, not(feature = "chip")))]
mod tests;
