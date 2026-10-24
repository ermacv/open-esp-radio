#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "a test fails loudly on a broken expectation"
)]

extern crate std;

use super::*;
use core::cell::Cell;
use std::{boxed::Box, rc::Rc};

std::thread_local! {
    /// This test thread's executors: each test runs on its own thread, so
    /// tests share no ready mask.
    static READY: &'static [Ready; EXECUTORS as usize] =
        Box::leak(Box::new([const { Ready::new() }; EXECUTORS as usize]));
    static PENDS: Cell<[u32; EXECUTORS as usize]> = const { Cell::new([0; EXECUTORS as usize]) };
}

pub(crate) fn ready(executor: u8) -> Option<&'static Ready> {
    READY.with(|ready| ready.get(usize::from(executor)))
}

pub(crate) fn pend(executor: u8) {
    PENDS.with(|pends| {
        let mut counts = pends.get();
        counts[usize::from(executor)] += 1;
        pends.set(counts);
    });
}

fn pends(executor: u8) -> u32 {
    PENDS.with(|pends| pends.get()[usize::from(executor)])
}

/// The executor the tests' sets run on.
const EXECUTOR: u8 = 2;

fn round<T: Tasks>(current: &Current) -> bool {
    poll_round::<T>(ready(T::EXECUTOR).expect("a test executor"), current, || {})
}

/// What a probe task saw: its polls, the argument it started with, and
/// whether its future was dropped.
#[derive(Default)]
struct Seen {
    polls: Cell<u32>,
    argument: Cell<Option<u32>>,
    dropped: Cell<bool>,
    /// Finish at this poll.
    finish_at: Cell<Option<u32>>,
    /// Wake itself during each poll.
    wake_self: Cell<bool>,
    /// The task the executor named as the current one during the last poll.
    current: Cell<Option<TaskId>>,
}

/// A task's future: it records its polls in `Seen`.
struct Probe {
    seen: Rc<Seen>,
    argument: u32,
    current: &'static Current,
}

impl Future for Probe {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let seen = &self.seen;
        seen.polls.set(seen.polls.get() + 1);
        seen.argument.set(Some(self.argument));
        seen.current
            .set(self.current.matching(EXECUTOR, cx.waker()));
        if seen.wake_self.get() {
            cx.waker().wake_by_ref();
        }
        if seen.finish_at.get() == Some(seen.polls.get()) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.seen.dropped.set(true);
    }
}

/// A set of three probe tasks, its slots leaked for the test that declares
/// it: `$set::slot(index)`.
macro_rules! probe_set {
    ($set:ident, $mode:ty) => {
        probe_set!($set, $mode, Probe);
    };
    ($set:ident, $mode:ty, $future:ty) => {
        struct $set;
        std::thread_local! {
            static SLOTS: [&'static Task<$future, $set>; 3] = [
                Box::leak(Box::new(Task::new(0))),
                Box::leak(Box::new(Task::new(1))),
                Box::leak(Box::new(Task::new(2))),
            ];
        }
        impl $set {
            fn slot(index: u8) -> &'static Task<$future, $set> {
                SLOTS.with(|slots| slots[usize::from(index)])
            }
        }
        impl Tasks for $set {
            type Mode = $mode;
            const EXECUTOR: u8 = EXECUTOR;
            const COUNT: u8 = 3;
            fn poll(index: u8, token: &PollToken<Self>, cx: &mut Context<'_>) {
                Self::slot(index).poll(token, cx);
            }
        }
    };
}

fn probe(seen: &Rc<Seen>, argument: u32, current: &'static Current) -> Probe {
    Probe {
        seen: seen.clone(),
        argument,
        current,
    }
}

fn current() -> &'static Current {
    Box::leak(Box::new(Current::new()))
}

#[test]
fn a_round_polls_each_ready_task_once_and_a_wake_during_it_waits_for_the_next() {
    probe_set!(Set, Thread);
    let current = current();
    let spawner = Spawner::<Set>::new();
    let seen: [Rc<Seen>; 3] = Default::default();
    seen[0].wake_self.set(true);
    spawner
        .start(Set::slot(0), || probe(&seen[0], 10, current))
        .expect("first start");
    spawner
        .start(Set::slot(2), || probe(&seen[2], 12, current))
        .expect("first start");
    assert_eq!(pends(EXECUTOR), 2, "each start pends the executor");

    // Task 0 wakes itself during the round: it is ready again, not polled twice.
    assert!(round::<Set>(current));
    assert_eq!(seen.each_ref().map(|seen| seen.polls.get()), [1, 0, 1]);
    assert_eq!(pends(EXECUTOR), 3, "the self-wake pends the executor");

    assert!(round::<Set>(current));
    assert_eq!(seen.each_ref().map(|seen| seen.polls.get()), [2, 0, 1]);

    seen[0].wake_self.set(false);
    assert!(!round::<Set>(current), "no task is ready after the round");
    assert_eq!(seen.each_ref().map(|seen| seen.polls.get()), [3, 0, 1]);
    assert!(!round::<Set>(current));
    assert_eq!(seen.each_ref().map(|seen| seen.polls.get()), [3, 0, 1]);
}

#[test]
fn the_polled_task_is_current_and_its_waker_names_it() {
    probe_set!(Set, Thread);
    let current = current();
    let seen: [Rc<Seen>; 3] = Default::default();
    Spawner::<Set>::new()
        .start(Set::slot(1), || probe(&seen[1], 0, current))
        .expect("first start");
    round::<Set>(current);
    assert_eq!(
        seen[1].current.get(),
        Some(TaskId {
            executor: EXECUTOR,
            index: 1
        })
    );
    // Between polls no task is current, and a foreign waker names none.
    let waker = TaskId {
        executor: EXECUTOR,
        index: 1,
    }
    .waker();
    assert_eq!(current.matching(EXECUTOR, &waker), None);
    assert_eq!(TaskId::of(Waker::noop()), None);
    let named = TaskId::of(&waker).expect("the executor's waker");
    assert_eq!((named.executor(), named.index()), (EXECUTOR, 1));
    assert_eq!(
        TaskId::of(&waker.clone()),
        Some(TaskId {
            executor: EXECUTOR,
            index: 1
        })
    );
}

#[test]
fn a_waker_marks_its_task_ready_and_pends_its_executor() {
    probe_set!(Set, Thread);
    let current = current();
    let seen: [Rc<Seen>; 3] = Default::default();
    Spawner::<Set>::new()
        .start(Set::slot(2), || probe(&seen[2], 0, current))
        .expect("first start");
    round::<Set>(current);
    let waker = TaskId {
        executor: EXECUTOR,
        index: 2,
    }
    .waker();
    let before = pends(EXECUTOR);
    // A clone wakes as the waker does; dropping either does nothing.
    let clone = waker.clone();
    clone.wake();
    assert_eq!(pends(EXECUTOR), before + 1);
    assert!(ready(EXECUTOR).expect("a test executor").any());
    assert!(!round::<Set>(current));
    assert_eq!(seen[2].polls.get(), 2);
}

#[test]
fn a_second_start_is_refused_and_keeps_the_first_arguments() {
    probe_set!(Set, Thread);
    let current = current();
    let spawner = Spawner::<Set>::new();
    let first = Rc::new(Seen::default());
    let second = Rc::new(Seen::default());
    first.finish_at.set(Some(2));
    spawner
        .start(Set::slot(0), || probe(&first, 1, current))
        .expect("first start");
    let made = Cell::new(false);
    assert_eq!(
        spawner.start(Set::slot(0), || {
            made.set(true);
            probe(&second, 2, current)
        }),
        Err(AlreadyStarted)
    );
    assert!(!made.get(), "a refused start makes no future");
    round::<Set>(current);
    assert_eq!(first.argument.get(), Some(1));

    // A finished task is dropped in place and stays started.
    ready(EXECUTOR).expect("a test executor").mark(0);
    round::<Set>(current);
    assert_eq!(first.polls.get(), 2);
    assert!(first.dropped.get());
    assert_eq!(
        spawner.start(Set::slot(0), || probe(&second, 3, current)),
        Err(AlreadyStarted)
    );
    ready(EXECUTOR).expect("a test executor").mark(0);
    round::<Set>(current);
    assert_eq!(
        first.polls.get(),
        2,
        "a finished task is never polled again"
    );
    assert_eq!(second.polls.get(), 0);
}

/// A `Send` task's future for an interrupt-mode executor: it records its
/// argument and its polls in atomics the test leaks.
struct Recorder {
    argument: u32,
    seen: &'static (AtomicU32, AtomicU32),
}

impl Future for Recorder {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        self.seen.0.store(self.argument, Ordering::Relaxed);
        self.seen.1.fetch_add(1, Ordering::Relaxed);
        Poll::Pending
    }
}

#[test]
fn a_start_preempted_by_its_executor_is_polled_only_once_published() {
    // The radio executor (P2) preempts a start from the application (P1)
    // between its claim and its publication.
    probe_set!(Set, Interrupt, Recorder);
    fn send<S: Send>(_: &S) {}
    let spawner = Spawner::<Set>::new();
    send(&spawner);
    let current = current();
    let seen: &'static (AtomicU32, AtomicU32) =
        Box::leak(Box::new((AtomicU32::new(0), AtomicU32::new(0))));
    let claim = Set::slot(1).claim().expect("the empty slot");

    // P2 runs: even a ready mark finds no task to poll, and a second start
    // is refused.
    ready(EXECUTOR).expect("a test executor").mark(1);
    assert!(!round::<Set>(current));
    assert_eq!(
        spawner.start(Set::slot(1), || Recorder { argument: 8, seen }),
        Err(AlreadyStarted)
    );

    // P1 resumes: it publishes the future with its arguments, then marks it.
    claim.publish(|| Recorder { argument: 7, seen });
    assert_eq!(seen.1.load(Ordering::Relaxed), 0);
    wake(TaskId {
        executor: EXECUTOR,
        index: 1,
    });
    assert!(!round::<Set>(current));
    assert_eq!(seen.1.load(Ordering::Relaxed), 1);
    assert_eq!(seen.0.load(Ordering::Relaxed), 7);
}

#[test]
fn a_task_is_not_polled_again_from_inside_its_own_poll() {
    probe_set!(Set, Thread);
    let current = current();
    let seen = Rc::new(Seen::default());
    Spawner::<Set>::new()
        .start(Set::slot(0), || probe(&seen, 0, current))
        .expect("first start");
    // The slot is taken for the poll: a nested poll of it does nothing.
    let slot = Set::slot(0);
    assert!(
        slot.state
            .compare_exchange(LIVE, POLLING, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    );
    slot.poll(&PollToken::new(), &mut Context::from_waker(Waker::noop()));
    assert_eq!(seen.polls.get(), 0);
    slot.state.store(LIVE, Ordering::Release);
    slot.poll(&PollToken::new(), &mut Context::from_waker(Waker::noop()));
    assert_eq!(seen.polls.get(), 1);
}

mod declared {
    use core::sync::atomic::{AtomicU32, Ordering};

    pub(super) async fn first(seen: &'static AtomicU32, value: u32) {
        seen.store(value, Ordering::Relaxed);
        core::future::pending::<()>().await;
    }

    pub(super) async fn second(seen: &'static AtomicU32) {
        seen.fetch_add(1, Ordering::Relaxed);
    }

    crate::tasks! {
        /// A set with a task its build leaves out.
        pub(super) Declared: InterruptExecutor<2> {
            #[cfg(any())]
            absent(value: u32) = no_such_body;
            /// The first declared task.
            first(seen: &'static AtomicU32, value: u32) = first;
            second(seen: &'static AtomicU32) = second;
        }
    }

    pub(super) fn indices() -> (u8, u8) {
        (first::TASK_INDEX, second::TASK_INDEX)
    }
}

#[test]
fn declared_tasks_start_once_with_their_arguments_and_poll_by_index() {
    use declared::Declared;
    assert_eq!(<Declared as Tasks>::COUNT, 3);
    assert_eq!(<Declared as Tasks>::EXECUTOR, 2);
    // A left-out task keeps its index: the others follow declaration order.
    assert_eq!(declared::indices(), (1, 2));
    let seen: &'static (AtomicU32, AtomicU32) =
        Box::leak(Box::new((AtomicU32::new(0), AtomicU32::new(0))));
    let spawner = Spawner::<Declared>::new();
    Declared::first(&spawner, &seen.0, 7).expect("first start");
    assert_eq!(Declared::first(&spawner, &seen.0, 8), Err(AlreadyStarted));
    let current = current();
    assert!(!round::<Declared>(current));
    assert_eq!(seen.0.load(Ordering::Relaxed), 7);
    Declared::second(&spawner, &seen.1).expect("first start");
    assert!(!round::<Declared>(current));
    assert_eq!(seen.1.load(Ordering::Relaxed), 1);
    // The finished task is not polled again; the pending one stays put.
    ready(EXECUTOR).expect("a test executor").mark(2);
    assert!(!round::<Declared>(current));
    assert_eq!(seen.1.load(Ordering::Relaxed), 1);
    assert_eq!(Declared::second(&spawner, &seen.1), Err(AlreadyStarted));
}

/// A thread-mode executor's signal side on the test thread, made of the same
/// parts as the chip's `Executor<SWI>`: the task the round polls now names
/// the polling task, and a wake marks it ready for the next round.
mod thread_signals {
    use super::*;
    use oer_interrupt_table::signal::{
        ContextToken, ForeignTask, IrqSignal, SignalExecutor, SignalList,
    };

    pub(super) struct TestThread;

    std::thread_local! {
        static RUNNING: Cell<bool> = const { Cell::new(false) };
        static LIST: &'static SignalList<TaskId> = Box::leak(Box::new(SignalList::new()));
        pub(super) static CURRENT: &'static Current = Box::leak(Box::new(Current::new()));
    }

    #[allow(unsafe_code, reason = "a test executor's signal context contract")]
    // SAFETY: the test thread is the executor's one context; `running` is
    // true only inside `run_round`, which services the list before polling.
    unsafe impl SignalExecutor for TestThread {
        type Task = TaskId;
        fn pend(_: &'static IrqSignal<Self>) {
            pend(EXECUTOR);
        }
        fn running() -> bool {
            RUNNING.with(Cell::get)
        }
        fn list() -> &'static SignalList<TaskId> {
            LIST.with(|list| *list)
        }
        fn current(cx: &Context<'_>, _: &ContextToken) -> Result<TaskId, ForeignTask> {
            CURRENT.with(|current| signal_current(current, EXECUTOR, cx))
        }
        fn wake(task: TaskId, _: &ContextToken) {
            signal_wake(task);
        }
    }

    pub(super) static SIGNAL: IrqSignal<TestThread> = IrqSignal::new();

    /// One run of the executor's loop: its signals, then a poll round.
    pub(super) fn run_round<T: Tasks>() -> bool {
        RUNNING.with(|running| running.set(true));
        oer_interrupt_table::signal::service::<TestThread>();
        let more = CURRENT.with(|current| round::<T>(current));
        RUNNING.with(|running| running.set(false));
        more
    }

    /// A task that takes the signal's facts into `taken`.
    pub(super) struct Waits(pub(super) &'static AtomicU32);
    impl Future for Waits {
        type Output = ();
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if let Poll::Ready(Ok(facts)) = SIGNAL.poll_take(cx) {
                self.0.fetch_or(facts, Ordering::Relaxed);
            }
            Poll::Pending
        }
    }
}

#[test]
fn a_signal_raised_for_a_thread_executor_task_wakes_it() {
    use thread_signals::{SIGNAL, Waits, run_round};
    probe_set!(Set, Thread, Waits);
    let taken: &'static AtomicU32 = Box::leak(Box::new(AtomicU32::new(0)));
    Spawner::<Set>::new()
        .start(Set::slot(0), || Waits(taken))
        .expect("first start");
    // The first poll registers the task with the signal.
    assert!(!run_round::<Set>());
    assert_eq!(taken.load(Ordering::Relaxed), 0);
    // An interrupt raises a fact: the executor is pended, not the task run.
    let before = pends(EXECUTOR);
    assert!(SIGNAL.raise(0b10));
    assert_eq!(pends(EXECUTOR), before + 1);
    assert_eq!(taken.load(Ordering::Relaxed), 0);
    // The executor's next run services the signal, which wakes the task.
    assert!(!run_round::<Set>());
    assert_eq!(taken.load(Ordering::Relaxed), 0b10);
    // Without a new fact the task stays asleep.
    assert!(!run_round::<Set>());
    assert!(!ready(EXECUTOR).expect("a test executor").any());
}
