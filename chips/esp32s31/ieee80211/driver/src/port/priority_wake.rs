//! A single consumer may preempt its producers on the same hart. This is
//! chip-local notification backing, not an executor or a cross-core primitive.
//!
//! Registering ordinary AtomicWaker while a suspended producer holds its
//! WAKING section would continually requeue the higher-priority consumer.
//! Instead it binds the other slot, then checks its input as usual, including
//! notifications before the first binding. Neither operation takes a lock.
use atomic_waker::AtomicWaker;
use core::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    task::Waker,
};

struct Slot {
    waker: AtomicWaker,
    waking: AtomicUsize,
}
impl Slot {
    const fn new() -> Self {
        Self {
            waker: AtomicWaker::new(),
            waking: AtomicUsize::new(0),
        }
    }

    // Keep obsolete-waker destruction separate from registration's wake
    // callbacks so each indirect call retains its own Waker operation scope.
    #[inline(never)]
    fn release_if_idle(&self) {
        if self.waking.load(Ordering::Acquire) == 0 {
            drop(self.waker.take());
        }
    }
}

pub(super) struct DriverWake {
    slots: [Slot; 2],
    active: AtomicBool,
}
impl DriverWake {
    pub(super) const fn new() -> Self {
        Self {
            slots: [const { Slot::new() }; 2],
            active: AtomicBool::new(false),
        }
    }
    fn slot(&self, index: bool) -> &Slot {
        if index {
            &self.slots[1]
        } else {
            &self.slots[0]
        }
    }
    pub(super) fn register(&self, waker: &Waker) {
        let mut selected = self.active.load(Ordering::Acquire);
        if self.slot(selected).waking.load(Ordering::Acquire) != 0 {
            selected = !selected;
        }
        // One serialized consumer preempts the producers; at most one slot
        // can be held by a suspended producer here. Registering in its peer
        // cannot observe AtomicWaker's WAKING state and enqueue the consumer.
        self.slot(selected).waker.register(waker);
        self.active.store(selected, Ordering::Release);
        let inactive = self.slot(!selected);
        // An interrupted producer has already published its input, checked
        // after registration. Keep its old waker only until it finishes.
        inactive.release_if_idle();
    }
    fn begin_wake(&self) -> Waking<'_> {
        let slot = self.slot(self.active.load(Ordering::Acquire));
        // A P2 wake can nest inside a suspended P1 wake; its completion
        // must not release the outer producer's slot.
        slot.waking.fetch_add(1, Ordering::AcqRel);
        Waking(slot)
    }
    pub(super) fn wake(&self) {
        self.begin_wake().fire();
    }
}
struct Waking<'a>(&'a Slot);
impl Waking<'_> {
    fn fire(&self) {
        self.0.waker.wake();
    }
}
impl Drop for Waking<'_> {
    fn drop(&mut self) {
        self.0.waking.fetch_sub(1, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Arc, task::Wake};
    #[derive(Default)]
    struct Count(AtomicUsize);
    impl Wake for Count {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn task() -> (Arc<Count>, Waker) {
        let count = Arc::new(Count::default());
        (count.clone(), Waker::from(count))
    }
    fn count(value: &Count) -> usize {
        value.0.load(Ordering::Relaxed)
    }

    #[test]
    fn registration_preempting_notification_does_not_schedule_itself() {
        let signal = DriverWake::new();
        let (old, old_waker) = task();
        let (new, new_waker) = task();
        signal.register(&old_waker);
        let interrupted = signal.begin_wake();
        for _ in 0..32 {
            signal.register(&new_waker);
            assert_eq!((count(&old), count(&new)), (0, 0));
        }
        interrupted.fire();
        drop(interrupted);
        assert_eq!((count(&old), count(&new)), (1, 0));
        signal.wake();
        assert_eq!((count(&old), count(&new)), (1, 1));
    }

    #[test]
    fn first_registration_during_an_early_notification_can_sleep() {
        let signal = DriverWake::new();
        let (new, new_waker) = task();
        let interrupted = signal.begin_wake();
        signal.register(&new_waker);
        interrupted.fire();
        drop(interrupted);
        assert_eq!(count(&new), 0);
        signal.wake();
        assert_eq!(count(&new), 1);
    }

    #[test]
    fn nested_wake_does_not_release_a_suspended_producers_slot() {
        let signal = DriverWake::new();
        let (_, old_waker) = task();
        let (new, new_waker) = task();
        signal.register(&old_waker);
        let interrupted = signal.begin_wake();
        signal.wake();
        signal.register(&new_waker);
        interrupted.fire();
        drop(interrupted);
        assert_eq!(count(&new), 0);
        signal.wake();
        assert_eq!(count(&new), 1);
    }

    #[test]
    fn previous_registration_is_released_after_notification_finishes() {
        let signal = DriverWake::new();
        let (old, old_waker) = task();
        let (_, new_waker) = task();
        signal.register(&old_waker);
        let interrupted = signal.begin_wake();
        signal.register(&new_waker);
        assert_eq!(Arc::strong_count(&old), 3);
        drop(interrupted);
        signal.register(&new_waker);
        assert_eq!(Arc::strong_count(&old), 2);
    }
}
