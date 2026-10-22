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
    let signal = PriorityWaker::new();
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
    let signal = PriorityWaker::new();
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
    let signal = PriorityWaker::new();
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
    let signal = PriorityWaker::new();
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

// Hold the producer at every boundary around its opaque AtomicWaker operation.
// The marked interval includes every instruction of AtomicWaker::take/wake:
// checking that its original waker is retained proves registration never touches
// that slot, even if it is suspended with WAKING set.
#[test]
fn consumer_preemption_at_each_notification_boundary_keeps_the_next_wait_live() {
    for boundary in 0..5 {
        let signal = PriorityWaker::new();
        let (old, old_waker) = task();
        let (new, new_waker) = task();
        signal.register(&old_waker);
        if boundary == 0 {
            signal.register(&new_waker);
        }
        let selected = signal.active.load(Ordering::Acquire);
        if boundary == 1 {
            signal.register(&new_waker);
        }
        let slot = signal.slot(selected);
        slot.waking.fetch_add(1, Ordering::AcqRel);
        let held = Waking(slot);
        if boundary == 2 {
            // Repeat as a preempting executor would if spuriously requeued.
            for _ in 0..32 {
                signal.register(&new_waker);
                assert_eq!((count(&old), count(&new)), (0, 0));
                assert_eq!(Arc::strong_count(&old), 3);
            }
        }
        held.fire();
        if boundary == 3 {
            signal.register(&new_waker);
        }
        drop(held);
        if boundary == 4 {
            signal.register(&new_waker);
        }
        assert_eq!(
            (count(&old), count(&new)),
            if boundary < 2 { (0, 1) } else { (1, 0) }
        );
        // A new polling cycle registers before checking its input.
        signal.register(&new_waker);
        let before = count(&new);
        signal.wake();
        assert_eq!(count(&new), before + 1);
        assert_eq!(Arc::strong_count(&old), 2);
    }
}

#[test]
fn a_wake_callback_can_rebind_and_notify_without_releasing_the_outer_slot() {
    struct Rebind {
        signal: Arc<PriorityWaker>,
        next: Waker,
        calls: AtomicUsize,
    }
    impl Wake for Rebind {
        fn wake(self: Arc<Self>) {
            self.calls.fetch_add(1, Ordering::Relaxed);
            // Two notifications complete on the old slot before switching.
            self.signal.wake();
            self.signal.wake();
            for _ in 0..32 {
                self.signal.register(&self.next);
            }
            self.signal.wake();
            self.signal.register(&self.next);
        }
    }
    let signal = Arc::new(PriorityWaker::new());
    let (next, next_waker) = task();
    let first = Arc::new(Rebind {
        signal: signal.clone(),
        next: next_waker,
        calls: AtomicUsize::new(0),
    });
    signal.register(&Waker::from(first.clone()));
    signal.wake();
    assert_eq!(first.calls.load(Ordering::Relaxed), 1);
    assert_eq!(count(&next), 1);
    signal.wake();
    assert_eq!(count(&next), 2);
}

#[test]
fn repeated_replacement_releases_every_obsolete_registration() {
    let signal = PriorityWaker::new();
    let (first, first_waker) = task();
    let (second, second_waker) = task();
    signal.register(&first_waker);
    let held = signal.begin_wake();
    signal.register(&second_waker);
    let nested = signal.begin_wake();
    nested.fire();
    drop(nested);
    // The second slot is idle again while the first producer is suspended.
    signal.register(&second_waker);
    assert_eq!(Arc::strong_count(&first), 3);
    held.fire();
    drop(held);
    signal.register(&first_waker);
    assert_eq!(Arc::strong_count(&second), 2);
    signal.register(&second_waker);
    assert_eq!(Arc::strong_count(&first), 2);
    drop(signal);
    assert_eq!(Arc::strong_count(&second), 2);
}

#[test]
fn a_higher_priority_notification_interrupting_registration_can_finish() {
    struct OnDrop {
        signal: std::sync::Weak<PriorityWaker>,
        input: Arc<AtomicBool>,
        invoked: Arc<Count>,
    }
    impl Wake for OnDrop {
        fn wake(self: Arc<Self>) {
            self.invoked.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    impl Drop for OnDrop {
        fn drop(&mut self) {
            // AtomicWaker holds REGISTERING while replacing the old waker.
            // Model a higher-priority producer at precisely that callback.
            self.input.store(true, Ordering::Release);
            self.signal
                .upgrade()
                .expect("consumer retains its cell")
                .wake();
        }
    }
    let signal = Arc::new(PriorityWaker::new());
    let input = Arc::new(AtomicBool::new(false));
    let invoked = Arc::new(Count::default());
    let old = Waker::from(Arc::new(OnDrop {
        signal: Arc::downgrade(&signal),
        input: input.clone(),
        invoked: invoked.clone(),
    }));
    signal.register(&old);
    drop(old);
    let (new, waker) = task();
    signal.register(&waker);
    assert!(input.load(Ordering::Acquire));
    assert_eq!(count(&invoked), 0);
    assert_eq!(count(&new), 1);
    for _ in 0..32 {
        signal.register(&waker);
    }
    assert_eq!(count(&new), 1);
    signal.wake();
    assert_eq!(count(&new), 2);
}
