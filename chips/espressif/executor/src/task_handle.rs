//! The task handle by which an interrupt signal wakes an Embassy task: the
//! task's storage, found only after the polling waker names the task the
//! executor polls now, and woken by a direct call with no `Waker` vtable.
use core::{
    sync::atomic::{AtomicU32, Ordering},
    task::Waker,
};

use embassy_executor::raw::{self, TaskRef};

/// One Embassy task, named by its static storage. Waking it after it has
/// finished is a no-op: the executor dequeues it without polling a future.
#[derive(Clone, Copy, Debug)]
pub struct EmbassyTask(TaskRef);

impl PartialEq for EmbassyTask {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for EmbassyTask {}

impl EmbassyTask {
    /// Mark the task ready on its executor and pend that executor.
    pub(crate) fn wake(self) {
        raw::wake_task(self.0);
    }
}

/// The task one executor polls now, as the trace hooks report it: Embassy's
/// task id, the address of the task's header (32 bits on the chip), or zero
/// between polls.
pub(crate) struct CurrentTask(AtomicU32);

impl CurrentTask {
    pub(crate) const fn new() -> Self {
        Self(AtomicU32::new(0))
    }

    /// The executor starts polling `task`.
    pub(crate) fn begin(&self, task: u32) {
        self.0.store(task, Ordering::Relaxed);
    }

    /// The executor's poll of its task returned.
    pub(crate) fn end(&self) {
        self.0.store(0, Ordering::Relaxed);
    }

    /// The task polled now, when `waker` carries its header: Embassy's own
    /// waker of that task. Any other waker is refused before Embassy
    /// inspects it, so `Waker::noop` or another executor's waker never
    /// reaches `task_from_waker`'s panic.
    pub(crate) fn matching(&self, waker: &Waker) -> Option<EmbassyTask> {
        let current = self.0.load(Ordering::Relaxed);
        // The trace ABI truncates the header address to `u32`, as here.
        let named = waker.data() as usize as u32;
        (current != 0 && named == current).then(|| EmbassyTask(raw::task_from_waker(waker)))
    }
}

#[cfg(all(test, feature = "trace-tests"))]
pub(crate) mod tests;
