//! Static typed signals from an interrupt handler to one task of an
//! interrupt-mode executor, never through that executor's waker.
//!
//! An [`IrqSignal`] is a `static` of the image bound to its executor by a
//! [`SignalExecutor`], which only an interrupt-mode executor provides. Its
//! interrupt side ([`IrqSignal::raise`]) ORs facts into one atomic word with
//! Release and, only when a fact is new, pends the executor: it raises the
//! executor's software interrupt and invokes no waker. Everything else
//! happens in the executor's own context: the executor's handler, before it
//! polls, walks its signals ([`service`]), swaps each word to
//! zero with Acquire into the facts its task has yet to take and wakes the
//! task there by its executor's task handle ([`SignalExecutor::Task`]), a
//! direct call with no `Waker` vtable; the task registers that handle and
//! takes the facts ([`IrqSignal::poll_take`]). The handle never leaves the
//! executor's context, so no lock is shared between the interrupt and the
//! task.
//!
//! A fact raised while the executor walks its signals lands in the swapped
//! word, is new again, and pends the executor once more: it is never lost.
//! Facts raised before the executor services them coalesce into one wake.
//!
//! [`FromHandler`] requires a handler strictly above the receiving executor.
//! [`FromExecutorLevel`] admits only an `Exempt(ExecutorLevel)` handler on
//! that executor's hart and level. Equal-level handlers cannot preempt the
//! executor's round: the handler publishes facts and pends the next round,
//! which services them in the executor's own context. [`FromTask`] admits
//! tasks strictly below the receiver on its hart; [`FromPeerHart`] admits a
//! task of an executor on another hart, which pends the receiver through its
//! software line (a cross-hart wake is a raise of the peer executor's
//! signal, never a waker call). Construction checks the same-level class,
//! each raiser's hart and direction at compile time, including runtime
//! constructors through inline const evaluation at monomorphization.
//!
//! A raise publishes its facts (fetch-or, Release), then the executor's
//! pend fences `w, o` before its software interrupt's raise; the
//! receiving handler lowers its line and fences `o, rw` before it swaps
//! the facts (`oer-espressif-executor`'s software line). Across harts this
//! order is conditional on the line's hardware preconditions stated
//! there.

mod binding;
pub use binding::{FromExecutorLevel, FromHandler, FromPeerHart, FromTask, Raisers};

use core::{
    marker::PhantomData,
    task::{Context, Poll},
};

use oer_memory::{
    Entered, ExecutionContext,
    signal::{SignalCore, SignalList},
};

/// The interrupt-mode executor a signal belongs to.
///
/// Its execution context owns the signal/list memory. Scheduling operations
/// run only after checked entry to that context; they do not grant memory
/// authority themselves. A pend schedules work; wake marks a task ready.
pub trait SignalExecutor: Sized + 'static {
    /// One task of this executor, valid for ever: its storage is static,
    /// and waking a task that has finished is a no-op.
    type Task: Copy + Eq + Send + 'static;
    type Context: ExecutionContext;
    /// Software interrupt source of this executor.
    const SOURCE: u16;
    /// Whether tasks poll in that interrupt, rather than thread mode.
    const INTERRUPT_MODE: bool;
    fn pend(signal: &'static IrqSignal<Self>);
    fn list() -> &'static SignalList<Self::Task, Self::Context>;
    /// The task whose poll runs now, named by `cx`'s waker.
    ///
    /// # Errors
    /// The waker names no task this executor polls now.
    fn current(
        cx: &Context<'_>,
        context: &ContextToken<Self::Context>,
    ) -> Result<Self::Task, ForeignTask>;
    /// Mark `task` ready to be polled by this executor.
    fn wake(task: Self::Task, context: &ContextToken<Self::Context>);
}

/// Proof that the caller passed its executor's context check. Only this
/// crate constructs it; an executor's `current` and `wake` take it.
pub struct ContextToken<X: ExecutionContext>(Entered<X>);

fn context<E: SignalExecutor>() -> Result<ContextToken<E::Context>, ForeignContext> {
    Entered::enter().map(ContextToken).ok_or(ForeignContext)
}

/// Enter executor `E`'s context: the proof that the caller runs in it now,
/// for state that only `E`'s tasks may change.
///
/// # Errors
/// The caller runs outside `E`'s context, including another executor on
/// the same hart.
pub fn enter<E: SignalExecutor>() -> Result<ContextToken<E::Context>, ForeignContext> {
    context::<E>()
}

/// Enter executor `E`'s context as the task `cx`'s waker names: the proof
/// that one of `E`'s tasks polls now, before it registers that waker.
///
/// # Errors
/// The caller runs outside `E`'s context, or `cx`'s waker names no task
/// `E` polls now.
pub fn enter_task<E: SignalExecutor>(
    cx: &Context<'_>,
) -> Result<ContextToken<E::Context>, ForeignContext> {
    context_task::<E>(cx).map(|(token, _)| token)
}

/// The context proof and the polling task's handle.
fn context_task<E: SignalExecutor>(
    cx: &Context<'_>,
) -> Result<(ContextToken<E::Context>, E::Task), ForeignContext> {
    let token = context::<E>()?;
    match E::current(cx, &token) {
        Ok(task) => Ok((token, task)),
        Err(ForeignTask) => Err(ForeignContext),
    }
}

/// One coalesced word of facts from an interrupt handler to one task,
/// bound to its executor at the type level.
#[repr(transparent)]
pub struct IrqSignal<E: SignalExecutor> {
    core: SignalCore<E::Task, E::Context>,
    executor: PhantomData<fn() -> E>,
}

/// The caller cannot access the signal's executor-owned state: it runs in
/// another context, has a foreign task waker, or the state is already borrowed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ForeignContext;

/// The waker of a poll names no task the signal's executor polls now: the
/// future runs under another executor's waker or none (`Waker::noop`).
/// [`IrqSignal`]'s polls report it as [`ForeignContext`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ForeignTask;

impl<E: SignalExecutor> IrqSignal<E> {
    /// A signal of `E` with no fact. `Target` is its executor's table entry;
    /// `R` names every raising handler or task (`FromHandler`,
    /// `FromExecutorLevel`, `FromTask`, `FromPeerHart`, or nested pairs).
    /// The software source, class, hart and directions are checked
    /// at compile time. No scheduling metadata is stored in the signal.
    pub const fn new<R: Raisers, Target: crate::Entry>() -> Self
    where
        Target::Part: crate::InterruptSource,
    {
        const { R::CONTEXTS.check(binding::executor_context::<E, Target>()) };
        Self::empty()
    }

    const fn empty() -> Self {
        Self {
            core: SignalCore::new(),
            executor: PhantomData,
        }
    }

    /// A synthetic host signal without a chip table. Test support only.
    #[cfg(any(test, feature = "host-signal"))]
    pub const fn for_host_test() -> Self {
        Self::empty()
    }

    /// Raise `facts`; `true` when one is new and the executor was pended.
    /// The composition must call this only from the raisers declared at
    /// construction; this method does not check the caller's context.
    /// Inlined into the interrupt handler's placement.
    #[inline(always)]
    pub fn raise(&'static self, facts: u32) -> bool {
        match self.raise_deferred(facts).pend {
            Some(pend) => {
                pend.pend();
                true
            }
            None => false,
        }
    }

    /// Raise facts, deferring a new fact's pend until the register
    /// transaction ends.
    #[inline(always)]
    pub fn raise_deferred(&'static self, facts: u32) -> Raised<E> {
        let previous = self.core.raise(facts);
        Raised {
            previous,
            pend: (facts & !previous != 0).then_some(SignalPend { signal: self }),
        }
    }

    /// Whether facts wait, without consuming them.
    ///
    /// # Errors
    /// The executor context is unavailable, including a nested state borrow.
    pub fn has_pending(&'static self) -> Result<bool, ForeignContext> {
        self.core
            .has_pending(&context::<E>()?.0)
            .ok_or(ForeignContext)
    }

    /// Take all facts raised so far.
    ///
    /// # Errors
    /// The executor context is unavailable, including a nested state borrow;
    /// facts stay raised.
    pub fn take(&'static self) -> Result<u32, ForeignContext> {
        self.core.take(&context::<E>()?.0).ok_or(ForeignContext)
    }

    /// Retain facts in the existing atomic instrumentation mask. Releasing
    /// touches no task-owned state; the image arranges another owning poll.
    #[cfg(oer_debug_control)]
    pub fn debug_hold(&'static self, facts: u32, hold: bool) {
        self.core.debug_hold(facts, hold);
    }

    /// Register the polling task, then take facts; pending while none
    /// exist. The first poll links the signal into its executor's list.
    ///
    /// # Errors
    /// The caller runs outside the signal's executor, or `cx`'s waker names
    /// no task that executor polls now ([`ForeignTask`]), or executor-owned
    /// state is already borrowed; facts stay raised.
    pub fn poll_take(&'static self, cx: &mut Context<'_>) -> Poll<Result<u32, ForeignContext>> {
        let (token, task) = match context_task::<E>(cx) {
            Ok(found) => found,
            Err(e) => return Poll::Ready(Err(e)),
        };
        self.core
            .poll_take(task, E::list(), &token.0)
            .map(|result| result.map_err(|_| ForeignContext))
    }

    /// Register the polling task, then report pending facts without taking
    /// them. The first poll links the signal into its executor's list.
    ///
    /// # Errors
    /// As [`poll_take`](Self::poll_take).
    pub fn poll_pending(&'static self, cx: &mut Context<'_>) -> Poll<Result<(), ForeignContext>> {
        let (token, task) = match context_task::<E>(cx) {
            Ok(found) => found,
            Err(e) => return Poll::Ready(Err(e)),
        };
        self.core
            .poll_pending(task, E::list(), &token.0)
            .map(|result| result.map_err(|_| ForeignContext))
    }

    /// Wait for facts.
    ///
    /// # Errors
    /// The executor context is unavailable, including a nested state borrow.
    pub async fn wait(&'static self) -> Result<u32, ForeignContext> {
        core::future::poll_fn(|cx| self.poll_take(cx)).await
    }

    /// Move raised facts to the task and wake it in the executor context.
    /// Outside that context, or during a nested state borrow, facts stay pending.
    pub fn service(&'static self) {
        if let Ok(token) = context::<E>() {
            self.core.service(&token.0, |task| E::wake(task, &token));
        }
    }
}

/// A queue end's notify across harts: the waiting end's task polls the
/// signal of its executor, and the other hart's end raises it. No waker
/// crosses harts. One signal wakes one task: every queue end that notifies
/// through it must be polled by that task. It is a copyable handle: every
/// queue end of that task holds one, and its calls are direct.
pub struct PeerNotify<E: SignalExecutor>(&'static IrqSignal<E>);

impl<E: SignalExecutor> Clone for PeerNotify<E> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<E: SignalExecutor> Copy for PeerNotify<E> {}

impl<E: SignalExecutor> PeerNotify<E> {
    /// The notify of `signal`, which the composition constructed with the
    /// other hart's raising task (`FromPeerHart`).
    pub const fn new(signal: &'static IrqSignal<E>) -> Self {
        Self(signal)
    }
}

impl<E: SignalExecutor> oer_memory::QueueNotify for PeerNotify<E>
where
    IrqSignal<E>: Sync,
{
    fn register(&self, cx: &mut Context<'_>) {
        // Registering takes the facts raised so far: the caller checks its
        // queue next, so a fact taken here is not lost. A poll outside the
        // signal's executor registers nothing; the composition places every
        // end that notifies through a signal on that signal's executor.
        let _facts = self.0.poll_take(cx);
    }
    fn notify(&self) {
        let _pended = self.0.raise(1);
    }
}

/// One raise: the previous facts and the pend a new fact owes.
#[must_use = "pend the executor after ending the register transaction"]
pub struct Raised<E: SignalExecutor> {
    pub previous: u32,
    pub pend: Option<SignalPend<E>>,
}

/// The pend a newly raised fact owes its executor.
#[must_use = "pend the executor after ending the register transaction"]
pub struct SignalPend<E: SignalExecutor> {
    signal: &'static IrqSignal<E>,
}

impl<E: SignalExecutor> SignalPend<E> {
    /// Pend the signal's executor.
    #[inline(always)]
    pub fn pend(self) {
        E::pend(self.signal);
    }
}

/// Whether a linked signal has pending or delivered facts, without consuming
/// them. This is supplementary to ready-task accounting; a signal not polled
/// yet is not linked, and its interrupt pend remains the admission boundary.
pub fn has_pending<E: SignalExecutor>() -> Result<bool, ForeignContext> {
    E::list()
        .has_pending(&context::<E>()?.0)
        .ok_or(ForeignContext)
}

/// Service every signal of `E` before polling its tasks. Outside `E`'s
/// context, or during a nested state borrow, unvisited facts stay pending.
/// Checked entry admits traversal; every cell access rechecks its context.
pub fn service<E: SignalExecutor>() {
    if let Ok(token) = context::<E>() {
        E::list().service(&token.0, |task| E::wake(task, &token));
    }
}

/// Host executors for tests of signal users. Constructors return a local
/// owner (!Send/!Sync) and a Send + Sync atomic raiser. Only the owner may
/// take, poll or service payload. Cross-thread raises wait for owner service.
#[cfg(any(test, feature = "host-signal"))]
pub mod host {
    extern crate std;
    use super::{ContextToken, ForeignTask, IrqSignal, SignalExecutor};
    use core::{cell::RefCell, task::Context};
    use oer_memory::signal::{Raiser, SignalList};
    use std::{boxed::Box, task::Waker, vec::Vec};

    std::thread_local! {
        static HOST_LIST: &'static SignalList<HostTask, oer_memory::HostContext> = Box::leak(Box::new(SignalList::new()));
        static DEFERRED_LIST: &'static SignalList<HostTask, oer_memory::HostContext> = Box::leak(Box::new(SignalList::new()));
        static FOREIGN_LIST: &'static SignalList<HostTask, oer_memory::Never> = Box::leak(Box::new(SignalList::new()));
        static TASKS: RefCell<Vec<&'static Waker>> = const { RefCell::new(Vec::new()) };
    }

    /// One test task: a waker leaked on its thread's first poll with it.
    #[derive(Clone, Copy, Debug)]
    pub struct HostTask(&'static Waker);

    impl PartialEq for HostTask {
        fn eq(&self, other: &Self) -> bool {
            core::ptr::eq(self.0, other.0)
        }
    }

    impl Eq for HostTask {}

    impl HostTask {
        /// The host task of `cx` on this thread. A test task is a distinct
        /// waker: its data and the contents of its vtable (a clone of
        /// `Waker::noop` may carry another copy of the same vtable, which
        /// `will_wake` tells apart).
        pub fn of(cx: &Context<'_>) -> Self {
            let waker = cx.waker();
            TASKS.with_borrow_mut(|tasks| {
                let known = tasks
                    .iter()
                    .find(|task| task.data() == waker.data() && task.vtable() == waker.vtable());
                Self(match known {
                    Some(task) => task,
                    None => {
                        let task: &'static Waker = Box::leak(Box::new(waker.clone()));
                        tasks.push(task);
                        task
                    }
                })
            })
        }

        /// Wake the test's waker, from any thread.
        pub fn wake(self) {
            self.0.wake_by_ref();
        }
    }

    /// A pend services the signal immediately on the owning test thread.
    pub enum Host {}
    /// Facts wait until the owning test thread takes or services them.
    pub enum Deferred {}
    /// The executor context never runs.
    pub enum Foreign {}

    impl SignalExecutor for Host {
        type Context = oer_memory::HostContext;
        type Task = HostTask;
        const SOURCE: u16 = 5;
        const INTERRUPT_MODE: bool = true;
        fn pend(signal: &'static IrqSignal<Self>) {
            signal.service();
        }
        fn list() -> &'static SignalList<HostTask, Self::Context> {
            HOST_LIST.with(|list| *list)
        }
        fn current(
            cx: &Context<'_>,
            _: &ContextToken<Self::Context>,
        ) -> Result<HostTask, ForeignTask> {
            Ok(HostTask::of(cx))
        }
        fn wake(task: HostTask, _: &ContextToken<Self::Context>) {
            task.wake();
        }
    }

    impl SignalExecutor for Deferred {
        type Context = oer_memory::HostContext;
        type Task = HostTask;
        const SOURCE: u16 = 5;
        const INTERRUPT_MODE: bool = true;
        fn pend(_: &'static IrqSignal<Self>) {}
        fn list() -> &'static SignalList<HostTask, Self::Context> {
            DEFERRED_LIST.with(|list| *list)
        }
        fn current(
            cx: &Context<'_>,
            _: &ContextToken<Self::Context>,
        ) -> Result<HostTask, ForeignTask> {
            Ok(HostTask::of(cx))
        }
        fn wake(task: HostTask, _: &ContextToken<Self::Context>) {
            task.wake();
        }
    }

    impl SignalExecutor for Foreign {
        type Context = oer_memory::Never;
        type Task = HostTask;
        const SOURCE: u16 = 6;
        const INTERRUPT_MODE: bool = false;
        fn pend(_: &'static IrqSignal<Self>) {}
        fn list() -> &'static SignalList<HostTask, Self::Context> {
            FOREIGN_LIST.with(|list| *list)
        }
        fn current(
            _: &Context<'_>,
            _: &ContextToken<Self::Context>,
        ) -> Result<HostTask, ForeignTask> {
            Err(ForeignTask)
        }
        fn wake(_: HostTask, _: &ContextToken<Self::Context>) {}
    }

    pub fn signal() -> (&'static IrqSignal<Host>, Raiser<'static>) {
        let owner = Box::leak(Box::new(IrqSignal::for_host_test()));
        (owner, owner.core.raiser())
    }
    pub fn deferred_signal() -> (&'static IrqSignal<Deferred>, Raiser<'static>) {
        let owner = Box::leak(Box::new(IrqSignal::for_host_test()));
        (owner, owner.core.raiser())
    }
    pub fn foreign_signal() -> &'static IrqSignal<Foreign> {
        Box::leak(Box::new(IrqSignal::for_host_test()))
    }
}

#[cfg(test)]
mod tests;
