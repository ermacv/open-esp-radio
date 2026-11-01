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

fn pend(executor: u8) {
    PENDS.with(|pends| {
        let mut counts = pends.get();
        counts[usize::from(executor)] += 1;
        pends.set(counts);
    });
}

/// The tests' binding: its one waker vtable and a pend each test thread
/// counts.
pub(crate) struct TestBinding;

/// The tests' one waker vtable.
static TEST_VTABLE: RawWakerVTable = waker_vtable::<TestBinding>();

impl Wakes for TestBinding {
    fn vtable() -> &'static RawWakerVTable {
        &TEST_VTABLE
    }
    fn pend_task(executor: SoftwareInterrupt) {
        pend(executor as u8);
    }
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
            .set(self.current.matching::<TestBinding>(EXECUTOR, cx.waker()));
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
/// it: `$set::slot(index)`. Each start is a boxed closure, `Send` when the
/// set names `Send`.
macro_rules! probe_set {
    ($set:ident, $mode:ty) => {
        probe_set!($set, $mode, Probe);
    };
    ($set:ident, $mode:ty, $future:ty $(, $send:ident)?) => {
        struct $set;
        std::thread_local! {
            static SLOTS: [&'static Task<Box<dyn FnOnce() -> $future $(+ $send)?>, $set>; 3] = [
                Box::leak(Box::new(Task::new(0))),
                Box::leak(Box::new(Task::new(1))),
                Box::leak(Box::new(Task::new(2))),
            ];
        }
        impl $set {
            fn slot(index: u8) -> &'static Task<Box<dyn FnOnce() -> $future $(+ $send)?>, $set> {
                SLOTS.with(|slots| slots[usize::from(index)])
            }
        }
        impl Tasks for $set {
            type Binding = TestBinding;
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

/// A probe task's start: its future is made at its first poll.
fn start_probe(
    seen: &Rc<Seen>,
    argument: u32,
    current: &'static Current,
) -> Box<dyn FnOnce() -> Probe> {
    let seen = seen.clone();
    Box::new(move || probe(&seen, argument, current))
}

/// A poll record for a test's direct start of a task whose start is `M`.
fn record<M: MakeFuture>() -> &'static PollBoundary<M> {
    Box::leak(Box::new(PollBoundary(
        poll_task::<M>,
        core::marker::PhantomData,
    )))
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
        .start(Set::slot(0), record(), start_probe(&seen[0], 10, current))
        .expect("first start");
    spawner
        .start(Set::slot(2), record(), start_probe(&seen[2], 12, current))
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
        .start(Set::slot(1), record(), start_probe(&seen[1], 0, current))
        .expect("first start");
    round::<Set>(current);
    assert_eq!(
        seen[1].current.get(),
        Some(TaskId {
            executor: SoftwareInterrupt::constant::<EXECUTOR>(),
            index: 1
        })
    );
    // Between polls no task is current, and a foreign waker names none.
    let waker = TaskId {
        executor: SoftwareInterrupt::constant::<EXECUTOR>(),
        index: 1,
    }
    .waker::<TestBinding>();
    assert_eq!(current.matching::<TestBinding>(EXECUTOR, &waker), None);
    assert_eq!(TaskId::of::<TestBinding>(Waker::noop()), None);
    let named = TaskId::of::<TestBinding>(&waker).expect("the executor's waker");
    assert_eq!(
        (named.executor(), named.index()),
        (SoftwareInterrupt::constant::<EXECUTOR>(), 1)
    );
    assert_eq!(
        TaskId::of::<TestBinding>(&waker.clone()),
        Some(TaskId {
            executor: SoftwareInterrupt::constant::<EXECUTOR>(),
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
        .start(Set::slot(2), record(), start_probe(&seen[2], 0, current))
        .expect("first start");
    round::<Set>(current);
    let waker = TaskId {
        executor: SoftwareInterrupt::constant::<EXECUTOR>(),
        index: 2,
    }
    .waker::<TestBinding>();
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
        .start(Set::slot(0), record(), start_probe(&first, 1, current))
        .expect("first start");
    let made = Rc::new(Cell::new(false));
    let refused = {
        let (made, second) = (made.clone(), second.clone());
        Box::new(move || {
            made.set(true);
            probe(&second, 2, current)
        })
    };
    assert_eq!(
        spawner.start(Set::slot(0), record(), refused),
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
        spawner.start(Set::slot(0), record(), start_probe(&second, 3, current)),
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
    probe_set!(Set, Interrupt, Recorder, Send);
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
        spawner.start(
            Set::slot(1),
            record(),
            Box::new(move || Recorder { argument: 8, seen })
        ),
        Err(AlreadyStarted)
    );

    // P1 resumes: it publishes the future with its arguments, then marks it.
    claim.publish(Box::new(move || Recorder { argument: 7, seen }));
    assert_eq!(seen.1.load(Ordering::Relaxed), 0);
    wake::<TestBinding>(TaskId {
        executor: SoftwareInterrupt::constant::<EXECUTOR>(),
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
        .start(Set::slot(0), record(), start_probe(&seen, 0, current))
        .expect("first start");
    // The slot is taken for the poll, before its future is made and after:
    // a nested poll of it does nothing.
    let slot = Set::slot(0);
    let poll = || slot.poll(&PollToken::new(), &mut Context::from_waker(Waker::noop()));
    for (state, polls) in [(STARTED, 0), (LIVE, 1)] {
        assert!(
            slot.state
                .compare_exchange(state, POLLING, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        );
        poll();
        assert_eq!(seen.polls.get(), polls);
        slot.state.store(state, Ordering::Release);
        poll();
        assert_eq!(seen.polls.get(), polls + 1);
    }
}

#[test]
fn a_task_makes_its_future_at_its_first_poll() {
    probe_set!(Set, Thread);
    let current = current();
    let seen = Rc::new(Seen::default());
    let made = Rc::new(Cell::new(false));
    let start = {
        let (seen, made) = (seen.clone(), made.clone());
        Box::new(move || {
            made.set(true);
            probe(&seen, 4, current)
        })
    };
    Spawner::<Set>::new()
        .start(Set::slot(1), record(), start)
        .expect("first start");
    assert!(!made.get(), "the start makes no future");
    assert!(!round::<Set>(current));
    assert!(made.get());
    assert_eq!((seen.polls.get(), seen.argument.get()), (1, Some(4)));
}

/// What [`Local`] saw when its executor polled it: whether it was made in
/// the polling thread, and the thread-local value it cloned.
static LOCAL_SEEN: std::sync::Mutex<Option<(bool, u32)>> = std::sync::Mutex::new(None);

std::thread_local! {
    /// A value of the thread that holds it: the starter's is 1, the
    /// executor's 2.
    static LOCAL: Rc<Cell<u32>> = Rc::new(Cell::new(0));
}

/// The reproduction of the review: a future that clones a thread-local
/// `Rc` when it is made, so it must be made, and stay, in the context that
/// polls it.
struct Local {
    local: Rc<Cell<u32>>,
    made_in: std::thread::ThreadId,
}

impl Future for Local {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        *LOCAL_SEEN.lock().expect("one poller") = Some((
            self.made_in == std::thread::current().id(),
            self.local.get(),
        ));
        Poll::Ready(())
    }
}

struct LocalSet;
static LOCAL_TASK: Task<fn() -> Local, LocalSet> = Task::new(0);
impl Tasks for LocalSet {
    type Binding = TestBinding;
    type Mode = Interrupt;
    const EXECUTOR: u8 = EXECUTOR;
    const COUNT: u8 = 1;
    fn poll(_: u8, token: &PollToken<Self>, cx: &mut Context<'_>) {
        LOCAL_TASK.poll(token, cx);
    }
}

#[test]
fn an_interrupt_mode_start_makes_its_future_in_its_executors_context() {
    // The start captures nothing, so it is `Send`; the future it makes is
    // not. It is made by the executor, on its own thread, never moved.
    fn make() -> Local {
        Local {
            local: LOCAL.with(Rc::clone),
            made_in: std::thread::current().id(),
        }
    }
    LOCAL.with(|local| local.set(1));
    Spawner::<LocalSet>::new()
        .start(&LOCAL_TASK, record(), make as fn() -> Local)
        .expect("first start");
    std::thread::spawn(|| {
        LOCAL.with(|local| local.set(2));
        <LocalSet as Tasks>::poll(
            0,
            &PollToken::new(),
            &mut Context::from_waker(Waker::noop()),
        );
    })
    .join()
    .expect("the executor's thread polls the task");
    assert_eq!(*LOCAL_SEEN.lock().expect("one poller"), Some((true, 2)));
    assert_eq!(LOCAL_TASK.state.load(Ordering::Acquire), DONE);
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
        pub(super) Declared: InterruptExecutor<super::TestBinding, 2> {
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

/// A library's set and the image's set extending it, on one executor.
mod extended {
    use super::*;

    pub(super) async fn library(seen: &'static AtomicU32) {
        seen.fetch_add(1, Ordering::Relaxed);
        core::future::pending::<()>().await;
    }

    pub(super) async fn application(seen: &'static AtomicU32) {
        seen.fetch_add(10, Ordering::Relaxed);
    }

    crate::tasks! {
        /// The library's tasks.
        pub(super) Library: InterruptExecutor<super::TestBinding, 2> {
            service(seen: &'static AtomicU32) = library;
            other(seen: &'static AtomicU32) = library;
        }
    }

    crate::tasks! {
        /// The image's tasks, after the library's.
        pub(super) Application: InterruptExecutor<super::TestBinding, 2> extends Library {
            main(seen: &'static AtomicU32) = application;
        }
    }

    pub(super) fn main_index() -> u8 {
        main::TASK_INDEX
    }
}

#[test]
fn an_extending_set_numbers_its_tasks_after_its_base_and_polls_both() {
    use extended::{Application, Library};
    assert_eq!(<Library as Tasks>::COUNT, 2);
    assert_eq!(<Application as Tasks>::COUNT, 3);
    assert_eq!(extended::main_index(), 2);
    let seen: &'static AtomicU32 = Box::leak(Box::new(AtomicU32::new(0)));
    let spawner = Spawner::<Application>::new();
    // The base's tasks start through the extending set's spawner.
    Library::service(&spawner.base(), seen).expect("library start");
    Application::main(&spawner, seen).expect("application start");
    let current = current();
    // The executor runs the extending set: one round polls both sets' tasks.
    assert!(!round::<Application>(current));
    assert_eq!(seen.load(Ordering::Relaxed), 11);
    // Each slot still starts once, through either spawner.
    assert_eq!(Library::service(&spawner.base(), seen), Err(AlreadyStarted));
    // A base task's wake marks its own index on the shared executor.
    ready(EXECUTOR).expect("a test executor").mark(0);
    assert!(!round::<Application>(current));
    assert_eq!(seen.load(Ordering::Relaxed), 11);
}

/// A thread-mode executor's signal side on the test thread, made of the same
/// parts as the chip's `Executor<B, SWI>`: the core's running context
/// ([`crate::context::Running`]) at the level the test thread runs at, the
/// task the round polls now naming the polling task, and a wake marking it
/// ready for the next round.
mod thread_signals {
    use super::*;
    use crate::context::Running;
    use oer_espressif_interrupt_table::signal::{
        ContextToken, ForeignTask, IrqSignal, SignalExecutor,
    };
    use oer_memory::signal::SignalList;

    pub(super) struct TestThread;

    /// The level of the executor's run loop: thread mode.
    const THREAD: u8 = 0;

    std::thread_local! {
        static RUNNING: &'static Running = Box::leak(Box::new(Running::new()));
        /// The interrupt level the test thread runs at now.
        static LEVEL: Cell<u8> = const { Cell::new(THREAD) };
        static LIST: &'static SignalList<TaskId, TestThread> = Box::leak(Box::new(SignalList::new()));
        pub(super) static CURRENT: &'static Current = Box::leak(Box::new(Current::new()));
    }

    /// Run `handler` as a hardware interrupt at `level` that preempts the
    /// code running now, on the same core.
    pub(super) fn interrupt<R>(level: u8, handler: impl FnOnce() -> R) -> R {
        let preempted = LEVEL.with(|now| now.replace(level));
        let result = handler();
        LEVEL.with(|now| now.set(preempted));
        result
    }

    #[allow(unsafe_code, reason = "a test executor's signal context contract")]
    // SAFETY: the test thread is the executor's one context; `running` is
    // true only inside `run_round`, at the level it entered at, which
    // services the list before polling; a preempting `interrupt` runs at
    // another level.
    unsafe impl oer_memory::ExecutionContext for TestThread {
        type Sharing = *mut ();
        fn running() -> bool {
            RUNNING.with(|running| running.at(LEVEL.with(Cell::get))) == EXECUTOR
        }
    }
    impl SignalExecutor for TestThread {
        type Context = Self;
        type Task = TaskId;
        const SOURCE: u16 = 5;
        const INTERRUPT_MODE: bool = true;
        fn pend(_: &'static IrqSignal<Self>) {
            pend(EXECUTOR);
        }
        fn list() -> &'static SignalList<TaskId, TestThread> {
            LIST.with(|list| *list)
        }
        fn current(
            cx: &Context<'_>,
            _: &ContextToken<Self::Context>,
        ) -> Result<TaskId, ForeignTask> {
            CURRENT.with(|current| signal_current::<TestBinding>(current, EXECUTOR, cx))
        }
        fn wake(task: TaskId, _: &ContextToken<Self::Context>) {
            signal_wake(task);
        }
    }

    std::thread_local! {
        pub(super) static SIGNAL: &'static IrqSignal<TestThread> = Box::leak(Box::new(IrqSignal::for_host_test()));
        /// Preempted's signal: each test owns its thread-local signal.
        pub(super) static PREEMPTED_SIGNAL: &'static IrqSignal<TestThread> = Box::leak(Box::new(IrqSignal::for_host_test()));
    }

    /// One run of the executor's loop: its signals, then a poll round.
    pub(super) fn run_round<T: Tasks>() -> bool {
        let running = RUNNING.with(|running| *running);
        let mut more = false;
        running.enter(EXECUTOR, LEVEL.with(Cell::get), || {
            oer_espressif_interrupt_table::signal::service::<TestThread>();
            more = CURRENT.with(|current| round::<T>(current));
        });
        more
    }

    /// A task that takes the signal's facts into `taken`.
    pub(super) struct Waits(pub(super) &'static AtomicU32);
    impl Future for Waits {
        type Output = ();
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if let Poll::Ready(Ok(facts)) = SIGNAL.with(|signal| signal.poll_take(cx)) {
                self.0.fetch_or(facts, Ordering::Relaxed);
            }
            Poll::Pending
        }
    }

    /// What [`Preempted`]'s last poll got from the signal: its own take,
    /// and the context-only calls of the hardware interrupt that preempted
    /// it.
    #[derive(Default)]
    pub(super) struct Seized {
        pub(super) own: Option<Poll<Result<u32, ForeignContext>>>,
        pub(super) take: Option<Result<u32, ForeignContext>>,
        pub(super) poll_take: Option<Poll<Result<u32, ForeignContext>>>,
    }

    /// A task whose poll a hardware interrupt at `level` preempts, which
    /// tries the signal's calls only its executor's context may make, with
    /// the task's own waker as well.
    pub(super) struct Preempted {
        pub(super) level: u8,
        pub(super) seized: &'static core::cell::RefCell<Seized>,
    }
    impl Future for Preempted {
        type Output = ();
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            let own = PREEMPTED_SIGNAL.with(|signal| signal.poll_take(cx));
            let (take, poll_take) = interrupt(self.level, || {
                (
                    PREEMPTED_SIGNAL.with(|signal| signal.take()),
                    PREEMPTED_SIGNAL.with(|signal| signal.poll_take(cx)),
                )
            });
            *self.seized.borrow_mut() = Seized {
                own: Some(own),
                take: Some(take),
                poll_take: Some(poll_take),
            };
            Poll::Pending
        }
    }
    use oer_espressif_interrupt_table::signal::ForeignContext;
}

#[test]
fn a_signal_raised_for_a_thread_executor_task_wakes_it() {
    use thread_signals::{SIGNAL, Waits, run_round};
    probe_set!(Set, Thread, Waits);
    let taken: &'static AtomicU32 = Box::leak(Box::new(AtomicU32::new(0)));
    Spawner::<Set>::new()
        .start(Set::slot(0), record(), Box::new(move || Waits(taken)))
        .expect("first start");
    // The first poll registers the task with the signal.
    assert!(!run_round::<Set>());
    assert_eq!(taken.load(Ordering::Relaxed), 0);
    // An interrupt raises a fact: the executor is pended, not the task run.
    let before = pends(EXECUTOR);
    assert!(SIGNAL.with(|signal| signal.raise(0b10)));
    assert_eq!(pends(EXECUTOR), before + 1);
    assert_eq!(taken.load(Ordering::Relaxed), 0);
    // The executor's next run services the signal, which wakes the task.
    assert!(!run_round::<Set>());
    assert_eq!(taken.load(Ordering::Relaxed), 0b10);
    // Without a new fact the task stays asleep.
    assert!(!run_round::<Set>());
    assert!(!ready(EXECUTOR).expect("a test executor").any());
}

#[test]
fn a_hardware_interrupt_that_preempts_a_thread_executor_task_is_refused_its_signals() {
    use oer_espressif_interrupt_table::signal::ForeignContext;
    use thread_signals::{PREEMPTED_SIGNAL, Preempted, Seized, run_round};
    probe_set!(Set, Thread, Preempted);
    let seized: &'static core::cell::RefCell<Seized> = Box::leak(Box::default());
    Spawner::<Set>::new()
        .start(
            Set::slot(0),
            record(),
            Box::new(move || Preempted { level: 3, seized }),
        )
        .expect("first start");
    // The task registers with the signal; the interrupt that preempts its
    // poll runs while the executor's context stays entered beneath it, and
    // gets no context: the task's half of the signal is never aliased.
    assert!(!run_round::<Set>());
    {
        let seized = seized.borrow();
        assert_eq!(seized.own, Some(Poll::Pending));
        assert_eq!(seized.take, Some(Err(ForeignContext)));
        assert_eq!(seized.poll_take, Some(Poll::Ready(Err(ForeignContext))));
    }
    // A fact raised now reaches the task in its own context, untouched by
    // the interrupt that preempts it again.
    assert!(PREEMPTED_SIGNAL.with(|signal| signal.raise(0b1)));
    assert!(!run_round::<Set>());
    let seized = seized.borrow();
    assert_eq!(seized.own, Some(Poll::Ready(Ok(0b1))));
    assert_eq!(seized.take, Some(Err(ForeignContext)));
}
