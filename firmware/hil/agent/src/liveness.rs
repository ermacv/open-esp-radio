//! Progress of tasks that have work waiting for them.
//!
//! The hang watchdog sees an executor that stops polling, but not a task that
//! awaits forever while its executor keeps running others. A task whose work
//! can wait for it owns a [`TaskSlot`]: the producer arms the slot when it
//! hands work over, and the task disarms it when nothing is left. The
//! watchdog asks [`TaskLiveness::overdue`] at each check; a slot armed for
//! longer than its deadline is a hang of that task. Arming and disarming are
//! single atomic operations, safe from any context and never awaiting.

use core::sync::atomic::{AtomicU32, Ordering};

use oer_hil_protocol::base::{TaskSlot, TaskStall};

/// A disarmed slot. Arming stores the uptime in milliseconds, so an uptime
/// equal to this value is stored one millisecond early.
const DISARMED: u32 = u32::MAX;

/// How long work may wait for the slot's task.
pub const fn deadline_ms(slot: TaskSlot) -> u32 {
    match slot {
        TaskSlot::Console => 5_000,
        TaskSlot::SessionEvidence => 10_000,
    }
}

/// When each slot's pending work arrived, per slot.
pub struct TaskLiveness {
    armed_at: [AtomicU32; TaskSlot::ALL.len()],
}

impl TaskLiveness {
    pub const fn new() -> Self {
        Self {
            armed_at: [const { AtomicU32::new(DISARMED) }; TaskSlot::ALL.len()],
        }
    }

    fn slot(&self, slot: TaskSlot) -> &AtomicU32 {
        &self.armed_at[slot as usize]
    }

    /// Work arrived at `now_ms`; a slot armed already keeps its older time.
    pub fn arm(&self, slot: TaskSlot, now_ms: u32) {
        let _ = self.slot(slot).compare_exchange(
            DISARMED,
            now_ms.min(DISARMED - 1),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    /// The task took work at `now_ms` and more is waiting.
    pub fn progress(&self, slot: TaskSlot, now_ms: u32) {
        self.slot(slot)
            .store(now_ms.min(DISARMED - 1), Ordering::Release);
    }

    /// No work is waiting for the task.
    pub fn disarm(&self, slot: TaskSlot) {
        self.slot(slot).store(DISARMED, Ordering::Release);
    }

    /// The first slot whose work has waited past its deadline at `now_ms`.
    pub fn overdue(&self, now_ms: u32) -> Option<TaskStall> {
        TaskSlot::ALL.into_iter().find_map(|slot| {
            let armed_at = self.slot(slot).load(Ordering::Acquire);
            let pending_ms = now_ms.checked_sub(armed_at)?;
            (armed_at != DISARMED && pending_ms > deadline_ms(slot))
                .then_some(TaskStall { slot, pending_ms })
        })
    }
}

impl Default for TaskLiveness {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_waiting_past_its_deadline_is_a_stall_of_that_task() {
        let liveness = TaskLiveness::new();
        assert_eq!(liveness.overdue(100_000), None, "nothing is armed");
        liveness.arm(TaskSlot::Console, 1_000);
        // A second arrival keeps the first one's time.
        liveness.arm(TaskSlot::Console, 4_000);
        assert_eq!(liveness.overdue(6_000), None, "at the deadline");
        assert_eq!(
            liveness.overdue(6_001),
            Some(TaskStall {
                slot: TaskSlot::Console,
                pending_ms: 5_001
            })
        );
        liveness.progress(TaskSlot::Console, 6_001);
        assert_eq!(liveness.overdue(10_000), None, "progress restarts the wait");
        liveness.disarm(TaskSlot::Console);
        assert_eq!(liveness.overdue(100_000), None);
    }

    #[test]
    fn each_slot_has_its_own_deadline() {
        let liveness = TaskLiveness::new();
        liveness.arm(TaskSlot::SessionEvidence, 0);
        assert_eq!(liveness.overdue(deadline_ms(TaskSlot::Console) + 1), None);
        assert_eq!(
            liveness
                .overdue(deadline_ms(TaskSlot::SessionEvidence) + 1)
                .map(|stall| stall.slot),
            Some(TaskSlot::SessionEvidence)
        );
    }
}
