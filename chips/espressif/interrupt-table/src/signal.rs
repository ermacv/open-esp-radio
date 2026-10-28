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
//! tasks strictly below the receiver. Construction checks the same-level
//! class, each raiser's hart and direction at compile time, including runtime constructors
//! through inline const evaluation at monomorphization.

mod binding;
pub use binding::{FromExecutorLevel, FromHandler, FromTask, Raisers};

use core::{
    marker::PhantomData,
    sync::atomic::{AtomicU32, Ordering},
    task::{Context, Poll},
};

use oer_memory::{ContextCell, Entered, ExecutionContext};

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

/// The task's half of a signal. Only its executor's context touches it.
struct Owned<T: 'static, X: ExecutionContext> {
    task: Option<T>,
    delivered: u32,
    linked: bool,
    next: Option<&'static SignalCore<T, X>>,
}

/// Shared implementation, typed by task handle and execution context.
struct SignalCore<T: 'static, X: ExecutionContext> {
    pending: AtomicU32,
    #[cfg(oer_debug_control)]
    held: AtomicU32,
    owned: ContextCell<Owned<T, X>, Entered<X>>,
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
    /// `FromExecutorLevel`, `FromTask`, or nested pairs).
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
            .has_pending(&context::<E>()?)
            .ok_or(ForeignContext)
    }

    /// Take all facts raised so far.
    ///
    /// # Errors
    /// The executor context is unavailable, including a nested state borrow;
    /// facts stay raised.
    pub fn take(&'static self) -> Result<u32, ForeignContext> {
        self.core.take(&context::<E>()?).ok_or(ForeignContext)
    }

    /// Retain facts in the existing atomic instrumentation mask. Releasing
    /// touches no task-owned state; the image arranges another owning poll.
    #[cfg(oer_debug_control)]
    pub fn debug_hold(&'static self, facts: u32, hold: bool) {
        if hold {
            self.core.held.fetch_or(facts, Ordering::Release);
        } else {
            self.core.held.fetch_and(!facts, Ordering::Release);
        }
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
        self.core.poll_take(task, E::list(), &token)
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
        self.core.poll_pending(task, E::list(), &token)
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
            self.core.service_next::<E>(&token);
        }
    }
}

impl<T: Copy + Eq + Send + 'static, X: ExecutionContext> SignalCore<T, X> {
    const fn new() -> Self {
        Self {
            pending: AtomicU32::new(0),
            #[cfg(oer_debug_control)]
            held: AtomicU32::new(0),
            owned: ContextCell::new(Owned {
                task: None,
                delivered: 0,
                linked: false,
                next: None,
            }),
        }
    }

    #[inline(always)]
    fn raise(&self, facts: u32) -> u32 {
        self.pending.fetch_or(facts, Ordering::Release)
    }

    fn has_pending(&self, token: &ContextToken<X>) -> Option<bool> {
        #[cfg(oer_debug_control)]
        {
            let pending = self.pending.load(Ordering::Acquire);
            self.owned(token, |owned| {
                self.deliverable(pending | owned.delivered) != 0
            })
        }
        #[cfg(not(oer_debug_control))]
        {
            let pending = self.pending.load(Ordering::Acquire) != 0;
            self.owned(token, |owned| pending || owned.delivered != 0)
        }
    }

    fn take(&self, token: &ContextToken<X>) -> Option<u32> {
        self.owned(token, |owned| {
            #[cfg(oer_debug_control)]
            {
                let facts = owned.delivered | self.pending.swap(0, Ordering::Acquire);
                let ready = self.deliverable(facts);
                owned.delivered = facts & !ready;
                ready
            }
            #[cfg(not(oer_debug_control))]
            {
                core::mem::take(&mut owned.delivered) | self.pending.swap(0, Ordering::Acquire)
            }
        })
    }

    #[cfg(oer_debug_control)]
    fn deliverable(&self, facts: u32) -> u32 {
        facts & !self.held.load(Ordering::Acquire)
    }

    fn poll_take(
        &'static self,
        task: T,
        list: &'static SignalList<T, X>,
        token: &ContextToken<X>,
    ) -> Poll<Result<u32, ForeignContext>> {
        if self.register(task, list, token).is_none() {
            return Poll::Ready(Err(ForeignContext));
        }
        match self.take(token) {
            Some(0) => Poll::Pending,
            Some(facts) => Poll::Ready(Ok(facts)),
            None => Poll::Ready(Err(ForeignContext)),
        }
    }

    fn poll_pending(
        &'static self,
        task: T,
        list: &'static SignalList<T, X>,
        token: &ContextToken<X>,
    ) -> Poll<Result<(), ForeignContext>> {
        if self.register(task, list, token).is_none() {
            return Poll::Ready(Err(ForeignContext));
        }
        match self.has_pending(token) {
            Some(true) => Poll::Ready(Ok(())),
            Some(false) => Poll::Pending,
            None => Poll::Ready(Err(ForeignContext)),
        }
    }

    /// Link on the first poll and register the polling task.
    fn register(
        &'static self,
        task: T,
        list: &'static SignalList<T, X>,
        token: &ContextToken<X>,
    ) -> Option<()> {
        self.owned(token, |owned| {
            if !owned.linked {
                owned.next = list.push(self, token)?;
                owned.linked = true;
            }
            owned.task = Some(task);
            Some(())
        })?
    }

    fn service_next<E: SignalExecutor<Task = T, Context = X>>(
        &'static self,
        token: &ContextToken<X>,
    ) -> Option<&'static SignalCore<T, X>> {
        let Some((wake, next)) = self.owned(token, |owned| {
            let facts = self.pending.swap(0, Ordering::Acquire);
            owned.delivered |= facts;
            #[cfg(oer_debug_control)]
            let available = self.deliverable(owned.delivered);
            #[cfg(not(oer_debug_control))]
            let available = facts;
            let wake = if available == 0 {
                None
            } else {
                owned.task.take()
            };
            (wake, owned.next)
        }) else {
            // The enclosing access retains the state; no facts were taken.
            return None;
        };
        // The task belongs to this executor: waking it only marks it for
        // this very poll.
        if let Some(task) = wake {
            E::wake(task, token);
        }
        next
    }

    /// Run `f` on the owned half with proof of the executor context.
    fn owned<R>(
        &self,
        token: &ContextToken<X>,
        f: impl FnOnce(&mut Owned<T, X>) -> R,
    ) -> Option<R> {
        self.owned.with(&token.0, f)
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

/// The signals of one executor, linked by their first poll; only that
/// executor's context touches the list.
pub struct SignalList<T: 'static, X: ExecutionContext> {
    head: ContextCell<Option<&'static SignalCore<T, X>>, Entered<X>>,
}

impl<T: Copy + Eq + Send + 'static, X: ExecutionContext> SignalList<T, X> {
    /// An empty list.
    pub const fn new() -> Self {
        Self {
            head: ContextCell::new(None),
        }
    }

    fn push(
        &self,
        signal: &'static SignalCore<T, X>,
        token: &ContextToken<X>,
    ) -> Option<Option<&'static SignalCore<T, X>>> {
        self.head.with(&token.0, |head| head.replace(signal))
    }

    fn has_pending(&self, token: &ContextToken<X>) -> Option<bool> {
        let mut next = self.head.with(&token.0, |head| *head)?;
        while let Some(signal) = next {
            if signal.has_pending(token)? {
                return Some(true);
            }
            next = signal.owned(token, |owned| owned.next)?;
        }
        Some(false)
    }

    fn service<E: SignalExecutor<Task = T, Context = X>>(&self, token: &ContextToken<X>) {
        let Some(mut next) = self.head.with(&token.0, |head| *head) else {
            // The enclosing access retains the list; no signal was visited.
            return;
        };
        while let Some(signal) = next {
            next = signal.service_next::<E>(token);
        }
    }
}

impl<T: Copy + Eq + Send + 'static, X: ExecutionContext> Default for SignalList<T, X> {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether a linked signal has pending or delivered facts, without consuming
/// them. This is supplementary to ready-task accounting; a signal not polled
/// yet is not linked, and its interrupt pend remains the admission boundary.
pub fn has_pending<E: SignalExecutor>() -> Result<bool, ForeignContext> {
    E::list()
        .has_pending(&context::<E>()?)
        .ok_or(ForeignContext)
}

/// Service every signal of `E` before polling its tasks. Outside `E`'s
/// context, or during a nested state borrow, unvisited facts stay pending.
/// Checked entry admits traversal; every cell access rechecks its context.
pub fn service<E: SignalExecutor>() {
    if let Ok(token) = context::<E>() {
        E::list().service::<E>(&token);
    }
}

/// Host executors for tests of signal users. Constructors return a local
/// owner (!Send/!Sync) and a Send + Sync atomic raiser. Only the owner may
/// take, poll or service payload. Cross-thread raises wait for owner service.
#[cfg(any(test, feature = "host-signal"))]
pub mod host {
    extern crate std;
    use super::{ContextToken, ForeignTask, IrqSignal, SignalExecutor, SignalList};
    use core::{cell::RefCell, task::Context};
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

    /// Transferable half: it never holds a signal, task handle, list or proof.
    #[derive(Clone, Copy)]
    pub struct Raiser {
        pending: &'static core::sync::atomic::AtomicU32,
    }
    impl Raiser {
        /// Publish facts for the owner's next take or service, without waking
        /// or servicing payload on the raising thread.
        pub fn raise(self, facts: u32) -> bool {
            facts
                & !self
                    .pending
                    .fetch_or(facts, core::sync::atomic::Ordering::Release)
                != 0
        }
    }

    pub fn signal() -> (&'static IrqSignal<Host>, Raiser) {
        let owner = Box::leak(Box::new(IrqSignal::for_host_test()));
        (
            owner,
            Raiser {
                pending: &owner.core.pending,
            },
        )
    }
    pub fn deferred_signal() -> (&'static IrqSignal<Deferred>, Raiser) {
        let owner = Box::leak(Box::new(IrqSignal::for_host_test()));
        (
            owner,
            Raiser {
                pending: &owner.core.pending,
            },
        )
    }
    pub fn foreign_signal() -> &'static IrqSignal<Foreign> {
        Box::leak(Box::new(IrqSignal::for_host_test()))
    }
}

#[cfg(test)]
mod tests;
