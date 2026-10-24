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
//! claims the slot, writes the future and only then publishes it and sets
//! the task's bit, so a poll that preempts the start finds no task to poll.
#![allow(
    unsafe_code,
    reason = "task slots hold their futures in static storage polled in place, and the \
              executor's one waker vtable"
)]

use core::{
    cell::UnsafeCell,
    future::Future,
    marker::PhantomData,
    mem::MaybeUninit,
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
/// so their futures are `Send`.
pub struct Interrupt(());

/// Whether an executor of this mode takes a start `M`, which holds a task's
/// arguments.
pub trait Admits<M> {}
impl<M> Admits<M> for Thread {}
impl<M: Send> Admits<M> for Interrupt {}

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

    /// Start `task` with the future `make` returns, written in place.
    /// `make` holds the task's arguments: an interrupt-mode executor's start
    /// may run in another context than its polls, so its arguments are
    /// `Send`, as the future holds only them until its first poll.
    ///
    /// # Errors
    /// The task has started already.
    pub fn start<F, M>(&self, task: &'static Task<F, T>, make: M) -> Result<(), AlreadyStarted>
    where
        F: Future<Output = ()>,
        M: FnOnce() -> F,
        T::Mode: Admits<M>,
    {
        task.claim()?.publish(make);
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
const LIVE: u8 = 2;
const POLLING: u8 = 3;
const DONE: u8 = 4;

/// One task's static slot: its future, written once by its start and polled
/// in place by its executor.
pub struct Task<F, T> {
    state: AtomicU8,
    index: u8,
    future: UnsafeCell<MaybeUninit<F>>,
    _tasks: PhantomData<fn() -> T>,
}

// SAFETY: the future is reached only by the one start that claims the slot
// (`EMPTY` to `CLAIMED`), until it publishes it (`LIVE`, release), and then
// only by the poll that takes it (`LIVE` to `POLLING`, acquire) until it
// returns it. A poll needs `T`'s token, which only `T`'s executor holds, in
// its one context. A future starts from another context than that only
// through a `Send` spawner, of an interrupt-mode executor, which takes only a
// `Send` start (`Admits`): the arguments the future holds alone until its
// first poll, in its executor's context.
unsafe impl<F, T> Sync for Task<F, T> {}

impl<F: Future<Output = ()>, T: Tasks> Task<F, T> {
    /// The empty slot of the task at `index` of `T`.
    pub const fn new(index: u8) -> Self {
        assert!(index < T::COUNT, "the task's index lies in its set");
        Self {
            state: AtomicU8::new(EMPTY),
            index,
            future: UnsafeCell::new(MaybeUninit::uninit()),
            _tasks: PhantomData,
        }
    }

    /// Take the empty slot for one start.
    fn claim(&'static self) -> Result<Claim<F, T>, AlreadyStarted> {
        self.state
            .compare_exchange(EMPTY, CLAIMED, Ordering::Acquire, Ordering::Relaxed)
            .map(|_| Claim { task: self })
            .map_err(|_| AlreadyStarted)
    }

    /// Poll the task once, when it has started and is not polled already;
    /// drop its future in place when it finishes.
    pub fn poll(&self, _: &PollToken<T>, cx: &mut Context<'_>) {
        if self
            .state
            .compare_exchange(LIVE, POLLING, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        // SAFETY: `POLLING` gives this call the only access to the future,
        // which the start initialized before it published `LIVE`; the slot is
        // static, so the future never moves.
        let finished = unsafe { poll_task(self.future.get().cast::<F>(), cx) };
        self.state
            .store(if finished { DONE } else { LIVE }, Ordering::Release);
    }
}

/// A claimed slot, before its future is published.
struct Claim<F: 'static, T: 'static> {
    task: &'static Task<F, T>,
}

impl<F, T> Claim<F, T> {
    /// Write the future in place and publish it.
    fn publish(self, make: impl FnOnce() -> F) {
        // SAFETY: the claim is the only access to the uninitialized slot;
        // no reference to its future is formed before it is written.
        unsafe { (*self.task.future.get()).as_mut_ptr().write(make()) };
        self.task.state.store(LIVE, Ordering::Release);
    }
}

/// The boundary between an executor and its tasks: one poll of a task's
/// future and, when it finishes, its drop. Everything this function reaches
/// is the task's code; the image checks find a task's root by this
/// function's name (`oer_espressif_executor::task::poll_task`).
///
/// # Safety
/// `future` is initialized, pinned at its address, and the caller has the
/// only access to it until this returns. After `true` it is dropped.
#[inline(never)]
unsafe fn poll_task<F: Future<Output = ()>>(future: *mut F, cx: &mut Context<'_>) -> bool {
    // SAFETY: the caller's contract: the future is initialized, never moves
    // and is accessed by this call alone.
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

/// The signal side of an executor (`oer_interrupt_table::signal`): the task
/// its `current` names, when `cx`'s waker is the waker of the task
/// executor `executor` polls now.
pub(crate) fn signal_current(
    current: &Current,
    executor: u8,
    cx: &Context<'_>,
) -> Result<TaskId, oer_interrupt_table::signal::ForeignTask> {
    current
        .matching(executor, cx.waker())
        .ok_or(oer_interrupt_table::signal::ForeignTask)
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
/// function per task: `Set::task(&spawner, arguments…)`. Each task's future
/// lives in a static slot in a module named after the task, so the crate
/// enables `type_alias_impl_trait`.
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
                #[define_opaque($name::Future)]
                $vis fn $name(
                    spawner: &$crate::task::Spawner<Self>,
                    $($argument: $type),*
                ) -> ::core::result::Result<(), $crate::task::AlreadyStarted> {
                    spawner.start(&$name::TASK, move || -> $name::Future {
                        $body($($argument),*)
                    })
                }
            )*
        }

        $crate::tasks!(@slots $set [0] $($(#[$attr])* $name;)*);
    };
    (@slots $set:ident [$index:expr]) => {};
    (@slots $set:ident [$index:expr] $(#[$attr:meta])* $name:ident; $($rest:tt)*) => {
        $(#[$attr])*
        mod $name {
            pub(super) type Future = impl ::core::future::Future<Output = ()>;
            pub(super) const TASK_INDEX: u8 = $index;
            pub(super) static TASK: $crate::task::Task<Future, super::$set> =
                $crate::task::Task::new(TASK_INDEX);
        }
        $crate::tasks!(@slots $set [$index + 1] $($rest)*);
    };
}

#[cfg(test)]
pub(crate) mod tests;
