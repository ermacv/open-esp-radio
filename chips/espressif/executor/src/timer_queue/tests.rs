#![allow(
    clippy::expect_used,
    reason = "a test fails loudly on a broken expectation"
)]

extern crate std;

use super::*;
use std::vec::Vec;

/// The tasks a queue operation woke, in order.
#[derive(Default)]
struct Woken(Vec<u8>);

impl Woken {
    fn wake(&mut self) -> impl FnMut(u8) + '_ {
        |index| self.0.push(index)
    }

    fn take(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.0)
    }
}

#[test]
fn due_registration_wakes_without_an_alarm_and_preserves_other_tasks() {
    let mut queue = WakeQueue::new();
    let mut woken = Woken::default();
    queue.schedule_wake(200, 0, || 100, woken.wake());
    queue.schedule_wake(110, 1, || 100, woken.wake());
    assert_eq!(queue.next_deadline(), 110);
    assert_eq!(woken.take(), []);

    // A new due timer also wakes an older due entry of the queue.
    queue.schedule_wake(120, 2, || 120, woken.wake());
    assert_eq!(queue.next_deadline(), 200);
    assert_eq!(woken.take(), [1, 2]);

    // A later registration cannot postpone an already retained earlier deadline.
    assert_eq!(queue.schedule_wake(300, 0, || 130, woken.wake()), None);
    assert_eq!(queue.next_deadline(), 200);
    // A task whose expired entry was removed can register a new future timer.
    queue.schedule_wake(150, 1, || 130, woken.wake());
    assert_eq!(queue.next_deadline(), 150);
    queue.dispatch_expired(150, woken.wake());
    assert_eq!(queue.next_deadline(), 200);
    assert_eq!(woken.take(), [1]);

    // Updating the last retained timer to an already due deadline empties the queue.
    queue.schedule_wake(140, 0, || 160, woken.wake());
    assert_eq!(queue.next_deadline(), u64::MAX);
    assert_eq!(woken.take(), [0]);
    queue.dispatch_expired(300, woken.wake());
    assert_eq!(woken.take(), [], "no stale wake at the replaced deadlines");
}

#[test]
fn a_task_beyond_the_queue_registers_nothing() {
    let mut queue = WakeQueue::new();
    let mut woken = Woken::default();
    assert_eq!(
        queue.schedule_wake(100, MAX_TASKS, || 0, woken.wake()),
        None
    );
    assert_eq!(queue.next_deadline(), u64::MAX);
    assert_eq!(woken.take(), []);
}

#[test]
fn self_wakes_keep_future_alarms_armed_and_fired_alarms_reconcile_the_queue() {
    use oer_time::{Clock, Duration};
    let clock = oer_time_virtual::VirtualClock::<1>::new();
    let now = || clock.now().as_micros();
    let start = now();
    let mut queue = WakeQueue::new();
    let mut woken = Woken::default();
    assert_eq!(
        queue.alarm_deadline(false),
        None,
        "empty self-wake needs no write"
    );
    queue.schedule_wake(start + 20, 0, now, woken.wake());
    assert_eq!(queue.alarm_deadline(false), Some(start + 20));
    queue.schedule_wake(start + 50, 1, now, woken.wake());
    queue.dispatch_expired(now(), woken.wake());
    assert_eq!(
        queue.alarm_deadline(false),
        None,
        "later timer and self-wake retain the alarm"
    );
    assert_eq!(
        queue.alarm_deadline(true),
        Some(start + 20),
        "an acknowledged alarm must be rearmed even if the deadline is unchanged"
    );
    clock
        .advance(Duration::from_micros(20))
        .expect("virtual advance");
    queue.dispatch_expired(now(), woken.wake());
    assert_eq!(queue.alarm_deadline(true), Some(start + 50));
    assert_eq!(woken.take(), [0]);
    clock
        .advance(Duration::from_micros(30))
        .expect("virtual advance");
    queue.dispatch_expired(now(), woken.wake());
    assert_eq!(queue.alarm_deadline(true), Some(u64::MAX));
    assert_eq!(woken.take(), [1]);
    assert_eq!(
        queue.alarm_deadline(false),
        None,
        "drained timers do not cause repeated writes"
    );
    queue.schedule_wake(start + 80, 0, now, woken.wake());
    assert_eq!(
        queue.alarm_deadline(false),
        Some(start + 80),
        "a later registration rearms normally"
    );
}
