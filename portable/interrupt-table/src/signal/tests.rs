extern crate std;

use core::{
    cell::Cell,
    pin::pin,
    task::{Context, Poll, Waker},
};
use std::{boxed::Box, sync::Arc, task::Wake};

use super::*;

#[cfg(oer_debug_control)]
#[test]
fn held_facts_survive_service_and_unrelated_wakes_until_release() {
    let signal = host::deferred_signal();
    let (counting, waker) = counting(None);
    let mut cx = Context::from_waker(&waker);
    assert_eq!(signal.poll_take(&mut cx), Poll::Pending);
    signal.debug_hold(0b10, true);
    signal.raise(0b10);
    signal.service();
    assert_eq!(counting.wakes.get(), 0);
    assert_eq!(signal.has_pending(), Ok(false));
    assert_eq!(signal.poll_take(&mut cx), Poll::Pending);
    signal.raise(0b01);
    signal.service();
    assert_eq!(counting.wakes.get(), 1);
    assert_eq!(signal.poll_take(&mut cx), Poll::Ready(Ok(0b01)));
    assert_eq!(signal.poll_take(&mut cx), Poll::Pending);
    signal.debug_hold(0b10, false);
    signal.service();
    assert_eq!(counting.wakes.get(), 2);
    assert_eq!(signal.poll_take(&mut cx), Poll::Ready(Ok(0b10)));
    assert_eq!(signal.poll_take(&mut cx), Poll::Pending);
}

#[cfg(oer_debug_control)]
#[test]
fn a_control_from_another_context_never_consumes_task_facts() {
    let signal = host::foreign_signal();
    signal.debug_hold(1, true);
    signal.raise(1);
    signal.debug_hold(1, false);
    assert_eq!(signal.take(), Err(ForeignContext));
    assert_eq!(signal.core.pending.load(Ordering::Acquire), 1);
}

std::thread_local! {
    static LIST_READS: Cell<u32> = const { Cell::new(0) };
    static PENDS: Cell<u32> = const { Cell::new(0) };
    static RUNNING: Cell<bool> = const { Cell::new(true) };
}

enum TestExecutor {}
std::thread_local! {
    static LIST: &'static SignalList<host::HostTask> = Box::leak(Box::new(SignalList::new()));
}
#[allow(unsafe_code, reason = "the test executor's contract")]
// SAFETY: the test thread alone touches its leaked signals and list;
// the pend counts, and the test plays its executor's handler.
unsafe impl SignalExecutor for TestExecutor {
    type Task = host::HostTask;
    fn pend(_: &'static IrqSignal<Self>) {
        PENDS.with(|pends| pends.set(pends.get() + 1));
    }
    fn running() -> bool {
        RUNNING.with(Cell::get)
    }
    fn list() -> &'static SignalList<host::HostTask> {
        LIST_READS.with(|reads| reads.set(reads.get() + 1));
        LIST.with(|list| *list)
    }
    fn current(cx: &Context<'_>, _: &ContextToken) -> Result<host::HostTask, ForeignTask> {
        Ok(host::HostTask::of(cx))
    }
    fn wake(task: host::HostTask, _: &ContextToken) {
        task.wake();
    }
}
fn pends() -> u32 {
    PENDS.with(Cell::get)
}
fn signal() -> &'static IrqSignal<TestExecutor> {
    Box::leak(Box::new(IrqSignal::new()))
}

/// A waker that counts its wakes and runs `on_wake` in them.
struct Counting {
    wakes: Cell<u32>,
    on_wake: Option<&'static IrqSignal<TestExecutor>>,
}

// The tests wake on their one thread only.
#[allow(unsafe_code, reason = "a single-threaded test waker")]
// SAFETY: every test touches its waker from its one thread.
unsafe impl Send for Counting {}
#[allow(unsafe_code, reason = "a single-threaded test waker")]
// SAFETY: as `Send`.
unsafe impl Sync for Counting {}

impl Wake for Counting {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.wakes.set(self.wakes.get() + 1);
        // A fact raised while the executor walks its signals.
        if let Some(signal) = self.on_wake {
            signal.raise(0b100);
        }
    }
}

fn counting(on_wake: Option<&'static IrqSignal<TestExecutor>>) -> (Arc<Counting>, Waker) {
    let counting = Arc::new(Counting {
        wakes: Cell::new(0),
        on_wake,
    });
    (counting.clone(), Waker::from(counting))
}

#[test]
fn raised_facts_coalesce_into_one_pend_and_one_wake() {
    let signal = signal();
    let (counting, waker) = counting(None);
    let mut cx = Context::from_waker(&waker);
    assert_eq!(signal.poll_take(&mut cx), Poll::Pending);
    let before = pends();
    // Two raises before the executor runs: one new-fact edge.
    assert!(signal.raise(0b01));
    assert!(signal.raise(0b10));
    assert!(!signal.raise(0b01));
    assert_eq!(pends() - before, 2);
    assert_eq!(signal.has_pending(), Ok(true));
    service::<TestExecutor>();
    assert_eq!(counting.wakes.get(), 1);
    assert_eq!(signal.poll_take(&mut cx), Poll::Ready(Ok(0b11)));
    assert_eq!(signal.has_pending(), Ok(false));
    // Without facts the executor wakes nobody.
    service::<TestExecutor>();
    assert_eq!(counting.wakes.get(), 1);
}

#[test]
fn a_fact_raised_while_the_executor_walks_its_signals_is_not_lost() {
    let signal = signal();
    let (counting, waker) = counting(Some(signal));
    let mut cx = Context::from_waker(&waker);
    assert_eq!(signal.poll_take(&mut cx), Poll::Pending);
    signal.raise(0b01);
    let before = pends();
    // The wake raises another fact: it lands in the swapped word, is new
    // and pends the executor again.
    service::<TestExecutor>();
    assert_eq!(counting.wakes.get(), 1);
    assert_eq!(pends() - before, 1);
    assert_eq!(signal.poll_take(&mut cx), Poll::Ready(Ok(0b101)));
}

#[test]
fn the_executor_wakes_every_signal_its_tasks_polled() {
    let first = signal();
    let second = signal();
    let (one, waker_one) = counting(None);
    let (two, waker_two) = counting(None);
    assert!(
        first
            .poll_take(&mut Context::from_waker(&waker_one))
            .is_pending()
    );
    assert!(
        second
            .poll_take(&mut Context::from_waker(&waker_two))
            .is_pending()
    );
    first.raise(1);
    second.raise(1);
    service::<TestExecutor>();
    assert_eq!((one.wakes.get(), two.wakes.get()), (1, 1));
}

#[test]
fn outside_its_executor_a_signal_only_raises() {
    let signal = signal();
    let (counting, waker) = counting(None);
    RUNNING.with(|running| running.set(false));
    assert_eq!(
        signal.poll_take(&mut Context::from_waker(&waker)),
        Poll::Ready(Err(ForeignContext))
    );
    assert!(signal.raise(1));
    assert_eq!(signal.take(), Err(ForeignContext));
    service::<TestExecutor>();
    RUNNING.with(|running| running.set(true));
    assert_eq!(counting.wakes.get(), 0);
    // The fact waited for the executor's context.
    assert_eq!(signal.take(), Ok(1));
}

#[test]
fn the_host_executor_wakes_at_the_raise() {
    let signal = host::signal();
    let (counting, waker) = counting(None);
    let mut wait = pin!(signal.wait());
    let mut cx = Context::from_waker(&waker);
    assert!(wait.as_mut().poll(&mut cx).is_pending());
    signal.raise(0b1);
    assert_eq!(counting.wakes.get(), 1);
    assert_eq!(wait.as_mut().poll(&mut cx), Poll::Ready(Ok(0b1)));
}

#[test]
fn a_pending_poll_leaves_the_facts_for_the_take() {
    let signal = signal();
    let (counting, waker) = counting(None);
    let mut cx = Context::from_waker(&waker);
    assert_eq!(signal.poll_pending(&mut cx), Poll::Pending);
    signal.raise(0b10);
    service::<TestExecutor>();
    assert_eq!(counting.wakes.get(), 1);
    // Ready until the task takes the fact, however often it polls.
    assert_eq!(signal.poll_pending(&mut cx), Poll::Ready(Ok(())));
    assert_eq!(signal.poll_pending(&mut cx), Poll::Ready(Ok(())));
    assert_eq!(signal.take(), Ok(0b10));
    assert_eq!(signal.poll_pending(&mut cx), Poll::Pending);
    RUNNING.with(|running| running.set(false));
    assert_eq!(
        signal.poll_pending(&mut cx),
        Poll::Ready(Err(ForeignContext))
    );
    RUNNING.with(|running| running.set(true));
}

#[test]
fn service_outside_the_context_neither_reads_the_list_nor_consumes_facts() {
    let signal = signal();
    let (counting, waker) = counting(None);
    assert!(
        signal
            .poll_take(&mut Context::from_waker(&waker))
            .is_pending()
    );
    signal.raise(0b101);
    let before = LIST_READS.with(Cell::get);
    RUNNING.with(|running| running.set(false));
    service::<TestExecutor>();
    assert_eq!(LIST_READS.with(Cell::get), before);
    assert_eq!(counting.wakes.get(), 0);
    RUNNING.with(|running| running.set(true));
    service::<TestExecutor>();
    assert_eq!(counting.wakes.get(), 1);
    assert_eq!(signal.take(), Ok(0b101));
}

#[test]
fn two_signals_share_the_executor_list_and_one_service_traversal() {
    let first = signal();
    let second = signal();
    let (one, first_waker) = counting(None);
    let (two, second_waker) = counting(None);
    assert!(
        first
            .poll_take(&mut Context::from_waker(&first_waker))
            .is_pending()
    );
    assert!(
        second
            .poll_take(&mut Context::from_waker(&second_waker))
            .is_pending()
    );
    first.raise(1);
    second.raise(2);
    let before = LIST_READS.with(Cell::get);
    service::<TestExecutor>();
    assert_eq!(LIST_READS.with(Cell::get) - before, 1);
    assert_eq!((one.wakes.get(), two.wakes.get()), (1, 1));
    assert_eq!((first.take(), second.take()), (Ok(1), Ok(2)));
}

#[test]
fn each_new_fact_pends_once_and_duplicates_do_not_pend() {
    let signal = signal();
    let before = pends();
    assert!(!signal.raise(0));
    assert!(signal.raise(1));
    assert!(!signal.raise(1));
    assert!(signal.raise(3));
    assert!(!signal.raise(3));
    assert_eq!(pends() - before, 2);
    assert_eq!(signal.take(), Ok(3));
    let raised = signal.raise_deferred(1);
    assert_eq!(pends() - before, 2);
    assert!(signal.raise_deferred(1).pend.is_none());
    raised.pend.expect("one new fact").pend();
    assert_eq!(pends() - before, 3);
}

#[test]
fn list_pending_observation_preserves_undelivered_and_delivered_facts() {
    enum ObserveExecutor {}
    std::thread_local! {static OBSERVE_LIST: &'static SignalList<host::HostTask>=Box::leak(Box::new(SignalList::new()));}
    #[allow(
        unsafe_code,
        reason = "fixture owns its signal list on this test thread"
    )]
    // SAFETY: only this test thread accesses its fixture list and signals;
    // pend never polls, and running is true only in this test's context.
    unsafe impl SignalExecutor for ObserveExecutor {
        type Task = host::HostTask;
        fn pend(_: &'static IrqSignal<Self>) {}
        fn running() -> bool {
            true
        }
        fn list() -> &'static SignalList<host::HostTask> {
            OBSERVE_LIST.with(|list| *list)
        }
        fn current(cx: &Context<'_>, _: &ContextToken) -> Result<host::HostTask, ForeignTask> {
            Ok(host::HostTask::of(cx))
        }
        fn wake(task: host::HostTask, _: &ContextToken) {
            task.wake();
        }
    }
    let signal = Box::leak(Box::new(IrqSignal::<ObserveExecutor>::new()));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(signal.poll_take(&mut cx).is_pending());
    assert_eq!(has_pending::<ObserveExecutor>(), Ok(false));
    signal.raise(1);
    assert_eq!(has_pending::<ObserveExecutor>(), Ok(true));
    service::<ObserveExecutor>();
    assert_eq!(has_pending::<ObserveExecutor>(), Ok(true));
    assert_eq!(signal.take(), Ok(1));
    assert_eq!(has_pending::<ObserveExecutor>(), Ok(false));
}

#[test]
fn a_waker_of_no_task_is_refused_without_registering() {
    enum Refusing {}
    std::thread_local! {
        static REFUSING_LIST: &'static SignalList<host::HostTask> = Box::leak(Box::new(SignalList::new()));
        static WAKES: Cell<u32> = const { Cell::new(0) };
    }
    #[allow(
        unsafe_code,
        reason = "fixture owns its signal list on this test thread"
    )]
    // SAFETY: only this test thread accesses its fixture list and signals;
    // `current` refuses every waker, so no task is ever registered.
    unsafe impl SignalExecutor for Refusing {
        type Task = host::HostTask;
        fn pend(_: &'static IrqSignal<Self>) {}
        fn running() -> bool {
            true
        }
        fn list() -> &'static SignalList<host::HostTask> {
            REFUSING_LIST.with(|list| *list)
        }
        fn current(_: &Context<'_>, _: &ContextToken) -> Result<host::HostTask, ForeignTask> {
            Err(ForeignTask)
        }
        fn wake(_: host::HostTask, _: &ContextToken) {
            WAKES.with(|wakes| wakes.set(wakes.get() + 1));
        }
    }
    let signal = Box::leak(Box::new(IrqSignal::<Refusing>::new()));
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(signal.poll_take(&mut cx), Poll::Ready(Err(ForeignContext)));
    assert_eq!(
        signal.poll_pending(&mut cx),
        Poll::Ready(Err(ForeignContext))
    );
    signal.raise(1);
    // Never linked: the executor's walk neither finds nor wakes it, and the
    // fact stays for a take.
    service::<Refusing>();
    assert_eq!(WAKES.with(Cell::get), 0);
    assert_eq!(has_pending::<Refusing>(), Ok(false));
    assert_eq!(signal.take(), Ok(1));
}

#[test]
fn a_new_task_replaces_the_registered_one() {
    let signal = signal();
    let (first, first_waker) = counting(None);
    let (second, second_waker) = counting(None);
    assert!(
        signal
            .poll_take(&mut Context::from_waker(&first_waker))
            .is_pending()
    );
    assert!(
        signal
            .poll_take(&mut Context::from_waker(&second_waker))
            .is_pending()
    );
    signal.raise(1);
    service::<TestExecutor>();
    assert_eq!((first.wakes.get(), second.wakes.get()), (0, 1));
}
