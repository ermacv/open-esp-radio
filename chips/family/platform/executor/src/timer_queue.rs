//! One executor's timers: one deadline per task, by the task's index, so an
//! expired timer wakes its task by its handle and never through a waker.
use crate::task::MAX_TASKS;

pub(crate) struct WakeQueue {
    deadlines: [u64; MAX_TASKS as usize],
    next_deadline: u64,
    armed: u64,
}

impl WakeQueue {
    pub(crate) const fn new() -> Self {
        Self {
            deadlines: [u64::MAX; MAX_TASKS as usize],
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

    /// Wake task `index` at `at`, or at its earlier retained deadline. Return
    /// the sampled time when the hardware alarm needs reconciliation. A task
    /// whose deadline is already due is woken at once through `wake`, with
    /// every other due task.
    pub(crate) fn schedule_wake(
        &mut self,
        at: u64,
        index: u8,
        now: impl FnOnce() -> u64,
        wake: impl FnMut(u8),
    ) -> Option<u64> {
        let deadline = self.deadlines.get_mut(usize::from(index))?;
        if at >= *deadline {
            return None;
        }
        *deadline = at;
        self.next_deadline = self.next_deadline.min(at);
        let now = now();
        if self.next_deadline <= now {
            self.dispatch_expired(now, wake);
        }
        Some(now)
    }

    /// Wake every task whose deadline is due by `now` and keep the earliest
    /// future deadline.
    pub(crate) fn dispatch_expired(&mut self, now: u64, mut wake: impl FnMut(u8)) {
        let mut next = u64::MAX;
        for (index, deadline) in (0..MAX_TASKS).zip(&mut self.deadlines) {
            if *deadline <= now {
                *deadline = u64::MAX;
                wake(index);
            } else {
                next = next.min(*deadline);
            }
        }
        self.next_deadline = next;
    }
}

#[cfg(test)]
mod tests;
