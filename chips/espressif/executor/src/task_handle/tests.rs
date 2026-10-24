extern crate std;
use super::*;
use core::{future::Future, task::Poll};
use embassy_executor::raw::{Executor, TaskStorage};
use std::{
    boxed::Box,
    cell::RefCell,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    task::Wake,
};

std::thread_local! {
    // The executor this test polls and its current task; other raw-executor
    // tests install none.
    static ACTIVE: RefCell<Option<(u32, &'static CurrentTask)>> = const { RefCell::new(None) };
}

/// The trace hooks' view of this thread's fixture.
pub(crate) fn with_current(executor: u32, f: impl FnOnce(&CurrentTask)) {
    let active = ACTIVE.with_borrow(|active| *active);
    if let Some((id, current)) = active
        && id == executor
    {
        f(current);
    }
}

struct Installed;
impl Drop for Installed {
    fn drop(&mut self) {
        ACTIVE.with_borrow_mut(|active| *active = None);
    }
}

fn fixture() -> (&'static Executor, &'static CurrentTask, Installed) {
    let executor: &'static Executor = Box::leak(Box::new(Executor::new(core::ptr::null_mut())));
    let current: &'static CurrentTask = Box::leak(Box::new(CurrentTask::new()));
    ACTIVE.with_borrow_mut(|active| {
        assert!(active.is_none());
        *active = Some((executor.id() as u32, current));
    });
    (executor, current, Installed)
}

fn spawn<F: Future<Output = ()> + 'static>(executor: &'static Executor, future: F) {
    let storage = Box::leak(Box::new(TaskStorage::<F>::new()));
    executor
        .spawner()
        .spawn(storage.spawn(|| future).expect("fresh task slot"));
}

#[allow(
    unsafe_code,
    reason = "fixture exclusively polls its raw executor without reentry"
)]
fn poll(executor: &'static Executor) {
    // SAFETY: only this fixture thread polls. Wakes and the pender never poll.
    unsafe { executor.poll() };
}

/// Another executor's waker, which counts its wakes.
#[derive(Default)]
struct Foreign(AtomicUsize);
impl Wake for Foreign {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn only_the_polled_tasks_own_waker_names_a_task() {
    let (executor, current, _installed) = fixture();
    let seen = Arc::new(Mutex::new(None));
    let saved = seen.clone();
    spawn(
        executor,
        core::future::poll_fn(move |cx| {
            let foreign = Waker::from(Arc::new(Foreign::default()));
            *saved.lock().expect("seen lock") = Some((
                current.matching(Waker::noop()).is_none(),
                current.matching(&foreign).is_none(),
                current.matching(cx.waker()),
                cx.waker().clone(),
            ));
            Poll::<()>::Pending
        }),
    );
    poll(executor);
    let (noop_refused, foreign_refused, own, waker) =
        seen.lock().expect("seen lock").take().expect("polled");
    assert!(noop_refused && foreign_refused);
    assert!(own.is_some());
    // Between polls no task runs, so even the task's own waker names none.
    assert!(current.matching(&waker).is_none());
}

#[test]
fn a_wake_polls_a_pending_task_and_is_a_no_op_after_it_finished() {
    let (executor, current, _installed) = fixture();
    let polls = Arc::new(AtomicUsize::new(0));
    let handle = Arc::new(Mutex::new(None::<EmbassyTask>));
    let (count, saved) = (polls.clone(), handle.clone());
    spawn(
        executor,
        core::future::poll_fn(move |cx| {
            let task = current.matching(cx.waker()).expect("own waker");
            *saved.lock().expect("handle lock") = Some(task);
            match count.fetch_add(1, Ordering::Relaxed) {
                0 => Poll::Pending,
                _ => Poll::Ready(()),
            }
        }),
    );
    poll(executor);
    let task = handle.lock().expect("handle lock").expect("first poll");
    task.wake();
    poll(executor);
    assert_eq!(polls.load(Ordering::Relaxed), 2, "the wake polled the task");
    assert_eq!(handle.lock().expect("handle lock").as_ref(), Some(&task));
    // The task finished on its second poll; its handle stays valid.
    task.wake();
    poll(executor);
    task.wake();
    task.wake();
    poll(executor);
    assert_eq!(
        polls.load(Ordering::Relaxed),
        2,
        "a finished task never polls"
    );
}
