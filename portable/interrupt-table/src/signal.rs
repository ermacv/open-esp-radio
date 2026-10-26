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

mod binding;
pub use binding::{FromHandler, FromTask, Raisers};

use core::{
    marker::PhantomData,
    sync::atomic::{AtomicU32, Ordering},
    task::{Context, Poll},
};

use oer_memory::{ContextCell, ContextProof};

/// The interrupt-mode executor a signal belongs to.
///
/// # Safety
/// `running()` is true exactly in one context that never runs concurrently
/// with itself: an executor's own polling context (an interrupt-mode
/// executor's handler or a thread-mode executor's run loop), which services
/// `list()` before it polls, or a single-threaded host test. It is false in
/// every context that preempts that one, such as a hardware interrupt
/// handler, though the executor's context stays entered beneath it. `pend` makes
/// that context run and runs no task itself outside it. `list()` returns
/// the same list on every call; only that context touches it. `current`
/// returns the handle of the task whose poll runs now and refuses any other
/// waker; `wake` marks a task ready in that context and never polls it in
/// place.
#[allow(
    unsafe_code,
    reason = "the context cells of a signal rely on this contract"
)]
pub unsafe trait SignalExecutor: Sized + 'static {
    /// One task of this executor, valid for ever: its storage is static,
    /// and waking a task that has finished is a no-op.
    type Task: Copy + Eq + 'static;
    /// Software interrupt source of this executor.
    const SOURCE: u16;
    /// Whether tasks poll in that interrupt, rather than thread mode.
    const INTERRUPT_MODE: bool;
    fn pend(signal: &'static IrqSignal<Self>);
    fn running() -> bool;
    fn list() -> &'static SignalList<Self::Task>;
    /// The task whose poll runs now, named by `cx`'s waker.
    ///
    /// # Errors
    /// The waker names no task this executor polls now.
    fn current(cx: &Context<'_>, context: &ContextToken) -> Result<Self::Task, ForeignTask>;
    /// Mark `task` ready to be polled by this executor.
    fn wake(task: Self::Task, context: &ContextToken);
}

/// Proof that the caller passed its executor's context check. Only this
/// crate constructs it; an executor's `current` and `wake` take it.
pub struct ContextToken(PhantomData<*mut ()>);

// Kept private so an executor cannot reuse its public ContextToken to access
// cells belonging to another executor. Only the checked signal/list paths
// below construct this proof, and neither their cells nor the proof escape.
struct CellContext(PhantomData<*mut ()>);

#[allow(
    unsafe_code,
    reason = "the signal/list access proof replaces their Sync promises"
)]
// SAFETY: every construction below follows the private ContextToken check for
// the signal/list's owning executor. SignalExecutor confines that executor to
// one non-concurrent context, and list() always returns its own list. Empty
// state is const-initialized; task handles are created and used only there.
// No public API exposes CellContext or either ContextCell.
unsafe impl ContextProof for CellContext {}

fn context<E: SignalExecutor>() -> Result<ContextToken, ForeignContext> {
    if E::running() {
        Ok(ContextToken(PhantomData))
    } else {
        Err(ForeignContext)
    }
}

/// The context proof and the polling task's handle.
fn context_task<E: SignalExecutor>(
    cx: &Context<'_>,
) -> Result<(ContextToken, E::Task), ForeignContext> {
    let token = context::<E>()?;
    match E::current(cx, &token) {
        Ok(task) => Ok((token, task)),
        Err(ForeignTask) => Err(ForeignContext),
    }
}

/// The task's half of a signal. Only its executor's context touches it.
struct Owned<T: 'static> {
    task: Option<T>,
    delivered: u32,
    linked: bool,
    next: Option<&'static SignalCore<T>>,
}

/// Shared implementation, independent of the executor type: generic only
/// over its task handle.
struct SignalCore<T: 'static> {
    pending: AtomicU32,
    #[cfg(oer_debug_control)]
    held: AtomicU32,
    owned: ContextCell<Owned<T>, CellContext>,
}

/// One coalesced word of facts from an interrupt handler to one task,
/// bound to its executor at the type level.
#[repr(transparent)]
pub struct IrqSignal<E: SignalExecutor> {
    core: SignalCore<E::Task>,
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
    /// `R` names every raising handler or task (`FromHandler`, `FromTask`,
    /// or nested pairs). The software source, hart and directions are checked
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

impl<T: Copy + Eq + 'static> SignalCore<T> {
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

    fn has_pending(&self, token: &ContextToken) -> Option<bool> {
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

    fn take(&self, token: &ContextToken) -> Option<u32> {
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
        list: &'static SignalList<T>,
        token: &ContextToken,
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
        list: &'static SignalList<T>,
        token: &ContextToken,
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
        list: &'static SignalList<T>,
        token: &ContextToken,
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

    fn service_next<E: SignalExecutor<Task = T>>(
        &'static self,
        token: &ContextToken,
    ) -> Option<&'static SignalCore<T>> {
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
    fn owned<R>(&self, _token: &ContextToken, f: impl FnOnce(&mut Owned<T>) -> R) -> Option<R> {
        self.owned.with(&CellContext(PhantomData), f)
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
pub struct SignalList<T: 'static> {
    head: ContextCell<Option<&'static SignalCore<T>>, CellContext>,
}

impl<T: Copy + Eq + 'static> SignalList<T> {
    /// An empty list.
    pub const fn new() -> Self {
        Self {
            head: ContextCell::new(None),
        }
    }

    fn push(
        &self,
        signal: &'static SignalCore<T>,
        _token: &ContextToken,
    ) -> Option<Option<&'static SignalCore<T>>> {
        self.head
            .with(&CellContext(PhantomData), |head| head.replace(signal))
    }

    fn has_pending(&self, token: &ContextToken) -> Option<bool> {
        let mut next = self.head.with(&CellContext(PhantomData), |head| *head)?;
        while let Some(signal) = next {
            if signal.has_pending(token)? {
                return Some(true);
            }
            next = signal.owned(token, |owned| owned.next)?;
        }
        Some(false)
    }

    fn service<E: SignalExecutor<Task = T>>(&self, token: &ContextToken) {
        let Some(mut next) = self.head.with(&CellContext(PhantomData), |head| *head) else {
            // The enclosing access retains the list; no signal was visited.
            return;
        };
        while let Some(signal) = next {
            next = signal.service_next::<E>(token);
        }
    }
}

impl<T: Copy + Eq + 'static> Default for SignalList<T> {
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
/// One context check covers the complete traversal.
pub fn service<E: SignalExecutor>() {
    if let Ok(token) = context::<E>() {
        E::list().service::<E>(&token);
    }
}

/// Host executors for tests of signal users. Each thread owns its list and
/// leaked signals; a host signal must not be shared between threads, except
/// that a raise from another thread wakes the registered task.
#[cfg(any(test, feature = "host-signal"))]
pub mod host {
    extern crate std;
    use super::{ContextToken, ForeignTask, IrqSignal, SignalExecutor, SignalList};
    use core::{cell::RefCell, task::Context};
    use std::{boxed::Box, task::Waker, vec::Vec};

    std::thread_local! {
        static HOST_LIST: &'static SignalList<HostTask> = Box::leak(Box::new(SignalList::new()));
        static DEFERRED_LIST: &'static SignalList<HostTask> = Box::leak(Box::new(SignalList::new()));
        static FOREIGN_LIST: &'static SignalList<HostTask> = Box::leak(Box::new(SignalList::new()));
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

    #[allow(unsafe_code, reason = "the host executor's contract")]
    // SAFETY: the context is the one test thread that owns its leaked
    // signals and thread-local list; a pend services the signal there, and
    // a wake only wakes the test's waker.
    unsafe impl SignalExecutor for Host {
        type Task = HostTask;
        const SOURCE: u16 = 5;
        const INTERRUPT_MODE: bool = true;
        fn pend(signal: &'static IrqSignal<Self>) {
            signal.service();
        }
        fn running() -> bool {
            true
        }
        fn list() -> &'static SignalList<HostTask> {
            HOST_LIST.with(|list| *list)
        }
        fn current(cx: &Context<'_>, _: &ContextToken) -> Result<HostTask, ForeignTask> {
            Ok(HostTask::of(cx))
        }
        fn wake(task: HostTask, _: &ContextToken) {
            task.wake();
        }
    }

    #[allow(unsafe_code, reason = "the host executor's contract")]
    // SAFETY: as `Host`; a pend changes nothing, and the test services its
    // own thread-local list explicitly.
    unsafe impl SignalExecutor for Deferred {
        type Task = HostTask;
        const SOURCE: u16 = 5;
        const INTERRUPT_MODE: bool = true;
        fn pend(_: &'static IrqSignal<Self>) {}
        fn running() -> bool {
            true
        }
        fn list() -> &'static SignalList<HostTask> {
            DEFERRED_LIST.with(|list| *list)
        }
        fn current(cx: &Context<'_>, _: &ContextToken) -> Result<HostTask, ForeignTask> {
            Ok(HostTask::of(cx))
        }
        fn wake(task: HostTask, _: &ContextToken) {
            task.wake();
        }
    }

    #[allow(unsafe_code, reason = "the host executor's contract")]
    // SAFETY: the context never runs, so no owned half, list or task is
    // reached.
    unsafe impl SignalExecutor for Foreign {
        type Task = HostTask;
        const SOURCE: u16 = 6;
        const INTERRUPT_MODE: bool = false;
        fn pend(_: &'static IrqSignal<Self>) {}
        fn running() -> bool {
            false
        }
        fn list() -> &'static SignalList<HostTask> {
            FOREIGN_LIST.with(|list| *list)
        }
        fn current(_: &Context<'_>, _: &ContextToken) -> Result<HostTask, ForeignTask> {
            Err(ForeignTask)
        }
        fn wake(_: HostTask, _: &ContextToken) {}
    }

    pub fn signal() -> &'static IrqSignal<Host> {
        Box::leak(Box::new(IrqSignal::for_host_test()))
    }
    pub fn deferred_signal() -> &'static IrqSignal<Deferred> {
        Box::leak(Box::new(IrqSignal::for_host_test()))
    }
    pub fn foreign_signal() -> &'static IrqSignal<Foreign> {
        Box::leak(Box::new(IrqSignal::for_host_test()))
    }
}

#[cfg(test)]
mod tests;
