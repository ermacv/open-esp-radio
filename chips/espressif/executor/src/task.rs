//! Declared tasks: each executor's tasks are declared once, by [`tasks!`],
//! into static slots of their own future types, and polled by index.
//!
//! - A task set ([`Tasks`]) belongs to one executor, named by its software
//!   interrupt and its mode ([`Thread`] or [`Interrupt`]); its tasks start
//!   only through that executor's [`Spawner`] and are polled only by it.
//! - Each task has one [`Task`] slot, which a start fills once: a second
//!   start of the same task is refused with [`AlreadyStarted`]. A finished
//!   task's future is dropped in place and its slot is never reused.
//! - The executor keeps one bit per task in its [`Ready`] mask. A poll round
//!   takes the mask once and polls each ready task once, by a direct call
//!   through [`Tasks::poll`]; a task woken during the round waits for the
//!   next round, which its executor is pended for.
//! - Every waker is one vtable's: its data is the executor and the task's
//!   index, never a pointer, and waking it sets the task's bit and pends its
//!   executor.
//!
//! A start may run in a lower context than the executor that polls the task
//! (the radio executor's tasks start from the application). The starter
//! claims the slot, writes the task's start (its arguments and how to make
//! its future, [`MakeFuture`]) and only then publishes it and sets the
//! task's bit, so a poll that preempts the start finds no task to poll. The
//! executor makes the future in place at the task's first poll, in its own
//! context: only the start crosses contexts, so an interrupt-mode
//! executor's starts are `Send` ([`Admits`]) and its futures need not be.
//!
//! Each declared task exports its poll boundary ([`PollBoundary`]) as
//! `__OER_TASK_POLL.<module path>` for the image checks, which find every
//! task poll as a direct call of a recorded boundary and fail on an image
//! whose executor polls without one.
#![allow(
    unsafe_code,
    reason = "task slots hold their futures in static storage polled in place, and the \
              executor's one waker vtable"
)]

use core::{
    cell::UnsafeCell,
    future::Future,
    marker::PhantomData,
    mem::{ManuallyDrop, MaybeUninit},
    pin::Pin,
    sync::atomic::{AtomicU8, AtomicU32, Ordering},
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};

/// The most tasks one executor declares. The timer queue keeps one deadline
/// per possible task of each executor.
pub const MAX_TASKS: u8 = 8;

/// The software interrupts an executor can run on.
pub(crate) const EXECUTORS: u8 = 4;

/// The mode of an executor polling in thread mode ([`crate::Executor`]): its
/// tasks start and run in the one context that runs the executor.
pub struct Thread(PhantomData<*mut ()>);

/// The mode of an executor polling in its software interrupt's handler
/// ([`crate::InterruptExecutor`]): its tasks may start from another context,
/// so their starts are `Send`.
pub struct Interrupt(());

/// Whether an executor of this mode takes a task's start `M` from its
/// starter. A thread-mode executor's starts run in its own context; an
/// interrupt-mode executor's run in any, so the start moves to the polling
/// context and is `Send`. The future is made there and never moves.
pub trait Admits<M> {}
impl<M> Admits<M> for Thread {}
impl<M: Send> Admits<M> for Interrupt {}

/// A task's start: its arguments, and how its executor makes the task's
/// future from them, in place, at the task's first poll. Every
/// `FnOnce() -> F` is one.
pub trait MakeFuture {
    /// The task's future.
    type Future: Future<Output = ()>;
    /// Make the task's future, in its executor's context.
    fn make(self) -> Self::Future;
}

impl<M: FnOnce() -> F, F: Future<Output = ()>> MakeFuture for M {
    type Future = F;
    fn make(self) -> F {
        self()
    }
}

/// `start` as a task's start: a closure checked as `FnOnce`, so it may move
/// what it captures (a `&'static mut` argument) into the future it makes.
pub const fn once<M: FnOnce() -> F, F: Future<Output = ()>>(start: M) -> M {
    start
}

/// The tasks of one executor, declared by [`tasks!`](crate::tasks), which
/// implements this trait.
pub trait Tasks: Sized + 'static {
    /// [`Thread`] or [`Interrupt`].
    type Mode;
    /// The executor's software interrupt.
    const EXECUTOR: u8;
    /// How many tasks the set declares.
    const COUNT: u8;
    /// Poll the task at `index` once, if it has started and not finished.
    fn poll(index: u8, token: &PollToken<Self>, cx: &mut Context<'_>);
}

/// The right to poll the tasks of `T`, which only `T`'s executor holds, for
/// one poll at a time.
pub struct PollToken<T> {
    _executor: PhantomData<(*mut (), T)>,
}

impl<T: Tasks> PollToken<T> {
    const fn new() -> Self {
        Self {
            _executor: PhantomData,
        }
    }
}

/// The start right of the tasks of `T`, which only `T`'s executor gives out.
/// It is `Send` for an interrupt-mode executor, whose tasks may start from
/// another context, and stays in the executor's context otherwise.
pub struct Spawner<T: Tasks> {
    _tasks: PhantomData<Starts<T>>,
}

/// What a spawner's marker holds: the set, and its mode's thread bounds.
type Starts<T> = (fn() -> T, <T as Tasks>::Mode);

impl<T: Tasks> Spawner<T> {
    pub(crate) const fn new() -> Self {
        Self {
            _tasks: PhantomData,
        }
    }

    /// Start `task` with `start`, written in place: the executor calls it at
    /// the task's first poll, in its own context, and polls the future it
    /// returns in place. An interrupt-mode executor's start may run in
    /// another context than its polls, so `start`, which crosses to them, is
    /// `Send` ([`Admits`]); the future never leaves the executor's context.
    ///
    /// # Errors
    /// The task has started already.
    pub fn start<M>(&self, task: &'static Task<M, T>, start: M) -> Result<(), AlreadyStarted>
    where
        M: MakeFuture,
        T::Mode: Admits<M>,
    {
        task.claim()?.publish(start);
        crate::task::wake(TaskId {
            executor: T::EXECUTOR,
            index: task.index,
        });
        Ok(())
    }
}

/// The task has started already: each task starts once.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AlreadyStarted;

const EMPTY: u8 = 0;
const CLAIMED: u8 = 1;
/// The start is published; the future is not made yet.
const STARTED: u8 = 2;
const LIVE: u8 = 3;
const POLLING: u8 = 4;
const DONE: u8 = 5;

/// What a task's slot holds: its start until its first poll, its future
/// from then on, in the same place.
union Stage<M: MakeFuture> {
    start: ManuallyDrop<M>,
    future: ManuallyDrop<M::Future>,
}

/// One task's static slot: its start, written once by the starter, then its
/// future, made in place at the first poll and polled in place by its
/// executor.
pub struct Task<M: MakeFuture, T> {
    state: AtomicU8,
    index: u8,
    stage: UnsafeCell<MaybeUninit<Stage<M>>>,
    _tasks: PhantomData<fn() -> T>,
}

// SAFETY: the slot is reached only by the one start that claims it (`EMPTY`
// to `CLAIMED`), until it publishes the start (`STARTED`, release), and then
// only by the poll that takes it (`STARTED` or `LIVE` to `POLLING`,
// acquire) until it returns it. A poll needs `T`'s token, which only `T`'s
// executor holds, in its one context, which makes the future from the start
// and polls it there. A start comes from another context than that only
// through a `Send` spawner, of an interrupt-mode executor, which takes only a
// `Send` start (`Admits`); the future never leaves the executor's context.
unsafe impl<M: MakeFuture, T> Sync for Task<M, T> {}

impl<M: MakeFuture, T: Tasks> Task<M, T> {
    /// The empty slot of the task at `index` of `T`.
    pub const fn new(index: u8) -> Self {
        assert!(index < T::COUNT, "the task's index lies in its set");
        Self {
            state: AtomicU8::new(EMPTY),
            index,
            stage: UnsafeCell::new(MaybeUninit::uninit()),
            _tasks: PhantomData,
        }
    }

    /// The task's poll boundary, which its declaration exports for the
    /// image checks.
    pub const BOUNDARY: PollBoundary = PollBoundary(poll_task::<M>);

    /// Take the empty slot for one start.
    fn claim(&'static self) -> Result<Claim<M, T>, AlreadyStarted> {
        self.state
            .compare_exchange(EMPTY, CLAIMED, Ordering::Acquire, Ordering::Relaxed)
            .map(|_| Claim { task: self })
            .map_err(|_| AlreadyStarted)
    }

    /// Poll the task once, when it has started and is not polled already:
    /// make its future in place at its first poll, and drop it in place
    /// when it finishes.
    pub fn poll(&self, _: &PollToken<T>, cx: &mut Context<'_>) {
        let first =
            match self
                .state
                .compare_exchange(LIVE, POLLING, Ordering::Acquire, Ordering::Relaxed)
            {
                Ok(_) => false,
                Err(STARTED) => {
                    if self
                        .state
                        .compare_exchange(STARTED, POLLING, Ordering::Acquire, Ordering::Relaxed)
                        .is_err()
                    {
                        return;
                    }
                    true
                }
                Err(_) => return,
            };
        // SAFETY: `POLLING` gives this call the only access to the slot,
        // which holds the start the starter published (`STARTED`) on the
        // first poll and the future made from it (`LIVE`) after; the slot is
        // static, so the future never moves.
        let finished = unsafe { poll_task::<M>(self.stage.get().cast(), first, cx) };
        self.state
            .store(if finished { DONE } else { LIVE }, Ordering::Release);
    }
}

/// A claimed slot, before its start is published.
struct Claim<M: MakeFuture + 'static, T: 'static> {
    task: &'static Task<M, T>,
}

impl<M: MakeFuture, T> Claim<M, T> {
    /// Write the start in place and publish it.
    fn publish(self, start: M) {
        // SAFETY: the claim is the only access to the uninitialized slot;
        // no reference to its contents is formed before it is written.
        unsafe {
            (*self.task.stage.get()).as_mut_ptr().write(Stage {
                start: ManuallyDrop::new(start),
            });
        }
        self.task.state.store(STARTED, Ordering::Release);
    }
}

/// A task's poll boundary: the instance of the executor's one poll of a
/// task's future, [`poll_task`], for that task's future type.
///
/// Each task declared by [`tasks!`](crate::tasks) exports its boundary,
/// `#[used]`, as `__OER_TASK_POLL.<module path of the task's slot>`. The
/// image checks read these records, not a function name: every direct call
/// of a recorded boundary is a task poll and the boundary is the task's
/// root; a recorded boundary without a direct call (inlined or never
/// polled), or an image with the executor's waker and no record, fails the
/// analysis.
#[derive(Clone, Copy)]
pub struct PollBoundary(
    #[allow(dead_code, reason = "the image checks read the record from the image")]
    unsafe fn(*mut (), bool, &mut Context<'_>) -> bool,
);

/// The boundary between an executor and its tasks: one poll of a task's
/// future, made from its start at the `first` poll, and, when it finishes,
/// its drop. Everything this function reaches, the making of the future
/// included, is the task's code. It is never inlined, so each executor's
/// poll of a task is one direct call of the task's instance, which the
/// task's [`PollBoundary`] record names.
///
/// # Safety
/// `stage` is a `Stage<M>` at a fixed address that holds the start when
/// `first` and the future made from it otherwise, and the caller has the
/// only access to it until this returns. After `true` the future is
/// dropped.
#[inline(never)]
unsafe fn poll_task<M: MakeFuture>(stage: *mut (), first: bool, cx: &mut Context<'_>) -> bool {
    let stage = stage.cast::<Stage<M>>();
    if first {
        // SAFETY: the caller's contract: the stage holds the start.
        unsafe { make_in_place(stage) };
    }
    let future = stage.cast::<M::Future>();
    // SAFETY: the caller's contract: the stage holds the future, which never
    // moves and is accessed by this call alone (`ManuallyDrop` is
    // transparent).
    let pinned = unsafe { Pin::new_unchecked(&mut *future) };
    match pinned.poll(cx) {
        Poll::Ready(()) => {
            // SAFETY: the future finished; nothing polls or reads it again.
            unsafe { future.drop_in_place() };
            true
        }
        Poll::Pending => false,
    }
}

/// Replace the start `stage` holds by the future it makes, in place. Its
/// own frame holds any copy the making needs, apart from the poll that
/// follows.
///
/// # Safety
/// `stage` holds the start, and the caller has the only access to it.
#[inline(never)]
unsafe fn make_in_place<M: MakeFuture>(stage: *mut Stage<M>) {
    // SAFETY: the caller's contract: the start is read once here and
    // replaced by the future it makes.
    unsafe {
        let start = ManuallyDrop::take(&mut (*stage).start);
        (&raw mut (*stage).future).write(ManuallyDrop::new(start.make()));
    }
}

/// One task of one executor: its software interrupt and its index.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskId {
    executor: u8,
    index: u8,
}

impl TaskId {
    pub(crate) const fn executor(self) -> u8 {
        self.executor
    }

    pub(crate) const fn index(self) -> u8 {
        self.index
    }

    /// The waker data of the task: never a pointer.
    const fn data(self) -> *const () {
        ((self.executor as usize) << 8 | self.index as usize) as *const ()
    }

    /// The task a waker's data names, as [`Self::data`] wrote it.
    fn from_data(data: *const ()) -> Self {
        let data = data as usize;
        Self {
            executor: (data >> 8) as u8,
            index: data as u8,
        }
    }

    /// The task `waker` wakes, when it is one of the executor's wakers.
    pub(crate) fn of(waker: &Waker) -> Option<Self> {
        core::ptr::eq(waker.vtable(), &VTABLE).then(|| Self::from_data(waker.data()))
    }

    /// The task's waker.
    pub(crate) fn waker(self) -> Waker {
        // SAFETY: the vtable's functions keep the `RawWaker` contract: the
        // data is a plain value, so cloning copies it and dropping does
        // nothing, and waking only sets an atomic bit and pends the
        // executor, from any context.
        unsafe { Waker::new(self.data(), &VTABLE) }
    }
}

/// The executor's one waker vtable.
static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake_data, wake_data, drop_data);

fn clone(data: *const ()) -> RawWaker {
    RawWaker::new(data, &VTABLE)
}

fn wake_data(data: *const ()) {
    wake(TaskId::from_data(data));
}

fn drop_data(_: *const ()) {}

/// Mark `task` ready and pend its executor.
pub(crate) fn wake(task: TaskId) {
    if let Some(ready) = ready(task.executor) {
        ready.mark(task.index);
        pend(task.executor);
    }
}

/// One executor's ready tasks, one bit each.
pub(crate) struct Ready(AtomicU32);

impl Ready {
    pub(crate) const fn new() -> Self {
        Self(AtomicU32::new(0))
    }

    /// Mark the task at `index` ready.
    pub(crate) fn mark(&self, index: u8) {
        if let Some(bit) = 1u32.checked_shl(u32::from(index)) {
            self.0.fetch_or(bit, Ordering::Release);
        }
    }

    /// Whether any task is ready.
    pub(crate) fn any(&self) -> bool {
        self.0.load(Ordering::Acquire) != 0
    }

    /// Take the mark of the task at `index`: whether it was ready.
    fn take(&self, index: u8) -> bool {
        let bit = 1u32 << index;
        self.0.fetch_and(!bit, Ordering::AcqRel) & bit != 0
    }
}

/// The task one executor polls now, or none.
pub(crate) struct Current(AtomicU8);

const NO_TASK: u8 = u8::MAX;

impl Current {
    pub(crate) const fn new() -> Self {
        Self(AtomicU8::new(NO_TASK))
    }

    /// The task polled now, when `waker` is its waker.
    pub(crate) fn matching(&self, executor: u8, waker: &Waker) -> Option<TaskId> {
        let current = self.0.load(Ordering::Relaxed);
        TaskId::of(waker).filter(|task| task.executor == executor && task.index == current)
    }
}

/// The signal side of an executor (`oer_espressif_interrupt_table::signal`): the task
/// its `current` names, when `cx`'s waker is the waker of the task
/// executor `executor` polls now.
pub(crate) fn signal_current(
    current: &Current,
    executor: u8,
    cx: &Context<'_>,
) -> Result<TaskId, oer_espressif_interrupt_table::signal::ForeignTask> {
    current
        .matching(executor, cx.waker())
        .ok_or(oer_espressif_interrupt_table::signal::ForeignTask)
}

/// The signal side's `wake`, in the executor's context before its poll
/// round: mark the task ready, which the round takes; no pend is needed.
pub(crate) fn signal_wake(task: TaskId) {
    if let Some(ready) = ready(task.executor) {
        ready.mark(task.index);
    }
}

/// One poll round of executor `T::EXECUTOR`: each task ready when the round
/// starts is polled once, in index order, after `before_poll`. Returns
/// whether a task is ready again.
pub(crate) fn poll_round<T: Tasks>(
    ready: &Ready,
    current: &Current,
    mut before_poll: impl FnMut(),
) -> bool {
    let token = PollToken::<T>::new();
    let round = ready.0.load(Ordering::Acquire);
    for index in 0..T::COUNT {
        if round & (1 << index) == 0 || !ready.take(index) {
            continue;
        }
        current.0.store(index, Ordering::Relaxed);
        before_poll();
        let waker = TaskId {
            executor: T::EXECUTOR,
            index,
        }
        .waker();
        T::poll(index, &token, &mut Context::from_waker(&waker));
        current.0.store(NO_TASK, Ordering::Relaxed);
    }
    ready.any()
}

/// Executor `executor`'s ready mask, when it is one.
#[cfg(feature = "chip")]
fn ready(executor: u8) -> Option<&'static Ready> {
    crate::executor::ready(executor)
}

/// Pend executor `executor` for a ready task.
#[cfg(feature = "chip")]
fn pend(executor: u8) {
    crate::executor::pend_task(executor);
}

#[cfg(all(test, not(feature = "chip")))]
use tests::{pend, ready};

/// Declare the tasks of one executor.
///
/// Each task names its arguments and the `async fn` (or any function
/// returning a future with output `()`) that makes its future from them. The
/// macro declares a type for the set, implementing [`Tasks`], with one start
/// function per task: `Set::task(&spawner, arguments…)`. Each task's start
/// (its arguments), then its future, which its executor makes from them at
/// its first poll, lives in a static slot in a module named after the task,
/// so the crate enables `type_alias_impl_trait`.
///
/// ```ignore
/// oer_espressif_executor::tasks! {
///     /// The radio executor's tasks (P2).
///     pub(crate) RadioTasks: InterruptExecutor<2> {
///         /// The radio system's actor.
///         radio_actor(actor: &'static mut Actor) = radio_actor;
///         #[cfg(feature = "timers")]
///         p2_timers(lines: Lines) = measure::p2_timers;
///     }
/// }
/// RadioTasks::radio_actor(&radio, actor)?;
/// ```
///
/// The set's executor is `InterruptExecutor<N>` or `Executor<N>`; its
/// interrupt handler is `interrupt_handler::<Set>`, its run is
/// `Executor::<N>::run::<Set, _>`.
#[macro_export]
macro_rules! tasks {
    (
        $(#[$set_attr:meta])*
        $vis:vis $set:ident : InterruptExecutor < $executor:literal > { $($tasks:tt)* }
    ) => {
        $crate::tasks!(@set [$(#[$set_attr])*] [$vis] $set [$crate::task::Interrupt] $executor { $($tasks)* });
    };
    (
        $(#[$set_attr:meta])*
        $vis:vis $set:ident : Executor < $executor:literal > { $($tasks:tt)* }
    ) => {
        $crate::tasks!(@set [$(#[$set_attr])*] [$vis] $set [$crate::task::Thread] $executor { $($tasks)* });
    };
    (
        @set [$($set_attr:tt)*] [$vis:vis] $set:ident [$mode:ty] $executor:literal {
            $(
                $(#[$attr:meta])*
                $name:ident ( $($argument:ident : $type:ty),* $(,)? ) = $body:path;
            )*
        }
    ) => {
        $($set_attr)*
        $vis struct $set {
            _tasks: (),
        }

        impl $crate::task::Tasks for $set {
            type Mode = $mode;
            const EXECUTOR: u8 = $executor;
            const COUNT: u8 = 0 $(+ { let _ = stringify!($name); 1 })*;
            #[allow(unused_doc_comments, unused_variables)]
            fn poll(
                index: u8,
                token: &$crate::task::PollToken<Self>,
                cx: &mut ::core::task::Context<'_>,
            ) {
                $(
                    $(#[$attr])*
                    {
                        if index == $name::TASK_INDEX {
                            $name::TASK.poll(token, cx);
                            return;
                        }
                    }
                )*
            }
        }

        const _: () = assert!(
            <$set as $crate::task::Tasks>::COUNT <= $crate::task::MAX_TASKS,
            "an executor declares at most MAX_TASKS tasks"
        );

        impl $set {
            $(
                $(#[$attr])*
                $vis fn $name(
                    spawner: &$crate::task::Spawner<Self>,
                    $($argument: $type),*
                ) -> ::core::result::Result<(), $crate::task::AlreadyStarted> {
                    // The start's one defining use: it holds the arguments
                    // and calls the body in the executor's context. The
                    // start below is no defining use, so it sees whether
                    // the start is `Send`.
                    #[define_opaque($name::Start)]
                    fn start($($argument: $type),*) -> $name::Start {
                        $crate::task::once(move || $body($($argument),*))
                    }
                    spawner.start(&$name::TASK, start($($argument),*))
                }
            )*
        }

        $crate::tasks!(@slots $set [0] $($(#[$attr])* $name;)*);
    };
    (@slots $set:ident [$index:expr]) => {};
    (@slots $set:ident [$index:expr] $(#[$attr:meta])* $name:ident; $($rest:tt)*) => {
        $(#[$attr])*
        mod $name {
            pub(super) type Start = impl $crate::task::MakeFuture;
            pub(super) const TASK_INDEX: u8 = $index;
            pub(super) static TASK: $crate::task::Task<Start, super::$set> =
                $crate::task::Task::new(TASK_INDEX);
            /// The task's poll boundary, for the image checks. `#[used]`
            /// keeps the record and the exported name lets the tools find it.
            #[used]
            #[unsafe(export_name = concat!("__OER_TASK_POLL.", module_path!()))]
            static POLL_BOUNDARY: $crate::task::PollBoundary =
                $crate::task::Task::<Start, super::$set>::BOUNDARY;
        }
        $crate::tasks!(@slots $set [$index + 1] $($rest)*);
    };
}

#[cfg(test)]
pub(crate) mod tests;
