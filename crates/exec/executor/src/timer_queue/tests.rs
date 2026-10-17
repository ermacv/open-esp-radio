extern crate std;

use super::*;
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use embassy_executor::raw::{Executor, TaskStorage};
use std::{
    boxed::Box,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

#[allow(
    unsafe_code,
    reason = "test executor needs its unique pender ABI symbol"
)]
#[unsafe(export_name = "__pender")]
fn pender(_: *mut ()) {}

#[derive(Default)]
struct Probe {
    polls: AtomicUsize,
    waker: Mutex<Option<Waker>>,
}
struct Task(Arc<Probe>);
impl Future for Task {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        self.0.polls.fetch_add(1, Ordering::Relaxed);
        *self.0.waker.lock().unwrap() = Some(cx.waker().clone());
        Poll::Pending
    }
}

#[allow(
    unsafe_code,
    reason = "test exclusively polls its leaked raw executor without reentry"
)]
fn poll(executor: &'static Executor) {
    // SAFETY: only this test thread polls this executor; task wakers only pend it.
    unsafe { executor.poll() };
}

#[test]
fn due_registration_wakes_without_an_alarm_and_preserves_other_tasks() {
    let executor = Box::leak(Box::new(Executor::new(core::ptr::null_mut())));
    let probes: [Arc<Probe>; 3] = core::array::from_fn(|_| Arc::default());
    for probe in &probes {
        let storage = Box::leak(Box::new(TaskStorage::<Task>::new()));
        let task = storage.spawn(|| Task(probe.clone())).unwrap();
        executor.spawner().spawn(task);
    }
    poll(executor);
    let wakers = probes
        .each_ref()
        .map(|probe| probe.waker.lock().unwrap().clone().unwrap());
    let counts = || {
        probes
            .each_ref()
            .map(|probe| probe.polls.load(Ordering::Relaxed))
    };
    let mut queue = WakeQueue::new();
    assert_eq!(counts(), [1, 1, 1]);
    queue.schedule_wake(200, &wakers[0], || 100);
    queue.schedule_wake(110, &wakers[1], || 100);
    assert_eq!(queue.next_deadline(), 110);

    // A new due timer also retires an older due entry in the shared queue.
    queue.schedule_wake(120, &wakers[2], || 120);
    assert_eq!(queue.next_deadline(), 200);
    poll(executor);
    assert_eq!(counts(), [1, 2, 2]);

    // A later registration cannot postpone an already retained earlier deadline.
    assert_eq!(queue.schedule_wake(300, &wakers[0], || 130), None);
    assert_eq!(queue.next_deadline(), 200);
    // A task whose expired entry was removed can register a new future timer.
    queue.schedule_wake(150, &wakers[1], || 130);
    assert_eq!(queue.next_deadline(), 150);
    queue.dispatch_expired(150);
    assert_eq!(queue.next_deadline(), 200);
    poll(executor);
    assert_eq!(counts(), [1, 3, 2]);

    // Updating the last retained timer to an already due deadline empties the queue.
    queue.schedule_wake(140, &wakers[0], || 160);
    assert_eq!(queue.next_deadline(), u64::MAX);
    poll(executor);
    assert_eq!(counts(), [2, 3, 2]);
    queue.dispatch_expired(300);
    poll(executor);
    assert_eq!(counts(), [2, 3, 2]); // No stale wake at the replaced deadlines.
}
