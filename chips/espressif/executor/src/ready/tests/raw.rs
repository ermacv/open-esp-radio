use super::*;
use core::{
    future::Future,
    task::{Poll, Waker},
};
use embassy_executor::raw::{Executor, TaskStorage};
use std::{
    boxed::Box,
    cell::RefCell,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

struct Fixture {
    ids: [u32; 2],
    states: [ReadyState; 2],
}
std::thread_local! {
    // Owned by this test; a cross-thread wake explicitly installs the same Arc.
    static ACTIVE: RefCell<Option<Arc<Fixture>>> = const { RefCell::new(None) };
}
pub(crate) fn with_state(id: u32, f: impl FnOnce(&ReadyState)) {
    ACTIVE.with_borrow(|active| {
        if let Some(fixture) = active {
            let index = fixture
                .ids
                .iter()
                .position(|entry| *entry == id)
                .expect("fixture executor registered before spawn");
            f(&fixture.states[index]);
        }
        // Other existing raw-executor unit tests do not install a fixture.
    });
}
struct Installed;
impl Drop for Installed {
    fn drop(&mut self) {
        ACTIVE.with_borrow_mut(|active| *active = None);
    }
}
fn installed<R>(fixture: Arc<Fixture>, f: impl FnOnce() -> R) -> R {
    ACTIVE.with_borrow_mut(|active| {
        assert!(active.is_none());
        *active = Some(fixture);
    });
    let _guard = Installed;
    f()
}
fn fixture() -> ([&'static Executor; 2], Arc<Fixture>) {
    let executors: [&'static Executor; 2] = core::array::from_fn(|_| {
        let executor = Box::leak(Box::new(Executor::new(core::ptr::null_mut())));
        &*executor
    });
    let ids = executors.map(|executor| executor.id() as u32);
    assert_ne!(ids[0], ids[1]);
    (
        executors,
        Arc::new(Fixture {
            ids,
            states: core::array::from_fn(|_| ReadyState::new()),
        }),
    )
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
    // SAFETY: only this fixture thread polls. Wakers and pender never poll.
    unsafe { executor.poll() };
}

#[test]
fn spawn_and_detached_batch_tokens_remain_visible_until_each_poll() {
    let (executors, fixture) = fixture();
    installed(fixture.clone(), || {
        let observations = Arc::new(Mutex::new(std::vec::Vec::new()));
        for label in [0, 1] {
            let fixture = fixture.clone();
            let observations = observations.clone();
            spawn(
                executors[0],
                core::future::poll_fn(move |_| {
                    observations
                        .lock()
                        .expect("observations lock")
                        .push((label, fixture.states[0].ready()));
                    Poll::Ready(())
                }),
            );
        }
        assert!(fixture.states[0].ready());
        assert!(!fixture.states[1].ready());
        poll(executors[0]);
        assert_eq!(
            *observations.lock().expect("observations lock"),
            [(1, true), (0, false)]
        );
        assert!(!fixture.states[0].ready());
    });
}

fn saved_task(
    executor: &'static Executor,
    fixture: Arc<Fixture>,
) -> (Arc<Mutex<Option<Waker>>>, Arc<AtomicUsize>) {
    let waker = Arc::new(Mutex::new(None::<Waker>));
    let polls = Arc::new(AtomicUsize::new(0));
    let saved = waker.clone();
    let count = polls.clone();
    spawn(
        executor,
        core::future::poll_fn(move |cx| {
            assert!(!fixture.states[0].ready());
            *saved.lock().expect("waker lock") = Some(cx.waker().clone());
            count.fetch_add(1, Ordering::Relaxed);
            Poll::Pending
        }),
    );
    (waker, polls)
}

#[test]
fn duplicate_wakes_enqueue_one_token_and_isolate_executors() {
    let (executors, fixture) = fixture();
    installed(fixture.clone(), || {
        let (saved, polls) = saved_task(executors[0], fixture.clone());
        poll(executors[0]);
        let waker = saved
            .lock()
            .expect("waker lock")
            .clone()
            .expect("first poll waker");
        for _ in 0..20 {
            waker.wake_by_ref();
        }
        assert!(fixture.states[0].ready());
        assert!(!fixture.states[1].ready());
        poll(executors[0]);
        assert_eq!(polls.load(Ordering::Relaxed), 2);
        assert!(!fixture.states[0].ready());
    });
}

#[test]
fn wake_during_poll_and_finished_queued_token_are_balanced() {
    let (executors, fixture) = fixture();
    installed(fixture.clone(), || {
        let polls = Arc::new(AtomicUsize::new(0));
        let count = polls.clone();
        let observed = fixture.clone();
        spawn(
            executors[0],
            core::future::poll_fn(move |cx| {
                assert!(!observed.states[0].ready());
                count.fetch_add(1, Ordering::Relaxed);
                cx.waker().wake_by_ref();
                cx.waker().wake_by_ref();
                assert!(observed.states[0].ready());
                Poll::Ready(())
            }),
        );
        poll(executors[0]);
        assert!(
            fixture.states[0].ready(),
            "finished task still has a queued token"
        );
        poll(executors[0]);
        assert!(!fixture.states[0].ready(), "poll_exited consumes its token");
        assert_eq!(polls.load(Ordering::Relaxed), 1);
    });
}

fn wake_inside_poll(cross_thread: bool) {
    let (executors, fixture) = fixture();
    installed(fixture.clone(), || {
        let observed = fixture.clone();
        let mut first = true;
        spawn(
            executors[0],
            core::future::poll_fn(move |cx| {
                assert!(!observed.states[0].ready());
                if first {
                    first = false;
                    let waker = cx.waker().clone();
                    if cross_thread {
                        let worker_fixture = observed.clone();
                        std::thread::scope(|scope| {
                            scope
                                .spawn(move || {
                                    installed(worker_fixture, || {
                                        for _ in 0..20 {
                                            waker.wake_by_ref();
                                        }
                                    })
                                })
                                .join()
                                .expect("cross-thread waker");
                        });
                    } else {
                        waker.wake_by_ref();
                    }
                    assert!(observed.states[0].ready());
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            }),
        );
        poll(executors[0]);
        assert!(fixture.states[0].ready());
        poll(executors[0]);
        assert!(!fixture.states[0].ready());
    });
}
#[test]
fn pending_self_wake_survives_to_the_next_poll() {
    wake_inside_poll(false);
}
#[test]
fn cross_thread_wake_while_polling_is_visible_before_return() {
    wake_inside_poll(true);
}

#[test]
fn finished_storage_respawns_on_another_executor() {
    let (executors, fixture) = fixture();
    installed(fixture.clone(), || {
        let storage = Box::leak(Box::new(TaskStorage::<core::future::Ready<()>>::new()));
        for executor in executors {
            executor.spawner().spawn(
                storage
                    .spawn(|| core::future::ready(()))
                    .expect("completed slot"),
            );
            poll(executor);
            assert!(!fixture.states[0].ready());
            assert!(!fixture.states[1].ready());
        }
    });
}

#[test]
fn due_timer_wake_is_counted_through_the_real_integrated_queue() {
    use oer_time::{Clock, Duration};
    let (executors, fixture) = fixture();
    installed(fixture.clone(), || {
        let clock = oer_time_virtual::VirtualClock::<1>::new();
        let (saved, _) = saved_task(executors[0], fixture.clone());
        poll(executors[0]);
        let waker = saved
            .lock()
            .expect("waker lock")
            .clone()
            .expect("first poll waker");
        let mut queue = crate::timer_queue::WakeQueue::new();
        queue.schedule_wake(clock.now().as_micros() + 10, &waker, || {
            clock.now().as_micros()
        });
        assert!(!fixture.states[0].ready());
        clock
            .advance(Duration::from_micros(10))
            .expect("virtual advance");
        queue.dispatch_expired(clock.now().as_micros());
        assert!(fixture.states[0].ready());
        poll(executors[0]);
        assert!(!fixture.states[0].ready());
    });
}
