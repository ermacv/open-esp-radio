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
//! task's waker there; the task registers that waker and takes the facts
//! ([`IrqSignal::poll_take`]). The waker never leaves the executor's
//! context, so no lock is shared between the interrupt and the task.
//!
//! A fact raised while the executor walks its signals lands in the swapped
//! word, is new again, and pends the executor once more: it is never lost.
//! Facts raised before the executor services them coalesce into one wake.

use core::{
    cell::UnsafeCell,
    marker::PhantomData,
    sync::atomic::{AtomicU32, Ordering},
    task::{Context, Poll, Waker},
};

/// The interrupt-mode executor a signal belongs to.
///
/// # Safety
/// `running()` is true exactly in one context that never runs concurrently
/// with itself: the handler of an interrupt-mode executor, which services
/// `list()` before it polls, or a single-threaded host test. `pend` makes
/// that context run and runs no task itself outside it. `list()` returns
/// the same list on every call; only that context touches it.
#[allow(
    unsafe_code,
    reason = "the context cells of a signal rely on this contract"
)]
pub unsafe trait SignalExecutor: Sized + 'static {
    fn pend(signal: &'static IrqSignal<Self>);
    fn running() -> bool;
    fn list() -> &'static SignalList;
}

/// Proof that the caller passed its executor's context check. Only
/// `context` constructs it; no public API accepts or returns it.
struct ContextToken(PhantomData<*mut ()>);

fn context<E: SignalExecutor>() -> Result<ContextToken, ForeignContext> {
    if E::running() {
        Ok(ContextToken(PhantomData))
    } else {
        Err(ForeignContext)
    }
}

/// The task's half of a signal. Only its executor's context touches it.
struct Owned {
    waker: Option<Waker>,
    delivered: u32,
    linked: bool,
    next: Option<&'static SignalCore>,
}

/// Shared implementation, independent of the executor type.
struct SignalCore {
    pending: AtomicU32,
    #[cfg(oer_debug_control)]
    held: AtomicU32,
    owned: UnsafeCell<Owned>,
}

#[allow(unsafe_code, reason = "the owned half is the executor context's")]
// SAFETY: `pending` is atomic. `owned` is reached only through `owned`,
// with a private token obtained after the executor's `running` check:
// one context that never runs concurrently with itself (`SignalExecutor`).
unsafe impl Sync for SignalCore {}

/// One coalesced word of facts from an interrupt handler to one task,
/// bound to its executor at the type level.
#[repr(transparent)]
pub struct IrqSignal<E: SignalExecutor> {
    core: SignalCore,
    executor: PhantomData<fn() -> E>,
}

/// The caller runs outside the executor the signal belongs to: its task was
/// spawned on another executor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ForeignContext;

impl<E: SignalExecutor> IrqSignal<E> {
    /// A signal of `E` with no fact.
    pub const fn new() -> Self {
        Self {
            core: SignalCore::new(),
            executor: PhantomData,
        }
    }

    /// Raise `facts` from any context; `true` when one is new and the
    /// executor was pended. Inlined into the interrupt handler's placement.
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
    /// The caller runs outside the signal's executor.
    pub fn has_pending(&'static self) -> Result<bool, ForeignContext> {
        Ok(self.core.has_pending(&context::<E>()?))
    }

    /// Take all facts raised so far.
    ///
    /// # Errors
    /// The caller runs outside the signal's executor; facts stay raised.
    pub fn take(&'static self) -> Result<u32, ForeignContext> {
        Ok(self.core.take(&context::<E>()?))
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

    /// Register the task's waker, then take facts; pending while none exist.
    /// The first poll links the signal into its executor's list.
    ///
    /// # Errors
    /// The caller runs outside the signal's executor.
    pub fn poll_take(&'static self, cx: &mut Context<'_>) -> Poll<Result<u32, ForeignContext>> {
        let token = match context::<E>() {
            Ok(token) => token,
            Err(e) => return Poll::Ready(Err(e)),
        };
        self.core.poll_take(cx, E::list(), &token).map(Ok)
    }

    /// Register the task's waker, then report pending facts without taking
    /// them. The first poll links the signal into its executor's list.
    ///
    /// # Errors
    /// The caller runs outside the signal's executor.
    pub fn poll_pending(&'static self, cx: &mut Context<'_>) -> Poll<Result<(), ForeignContext>> {
        let token = match context::<E>() {
            Ok(token) => token,
            Err(e) => return Poll::Ready(Err(e)),
        };
        self.core.poll_pending(cx, E::list(), &token).map(Ok)
    }

    /// Wait for facts.
    ///
    /// # Errors
    /// The caller runs outside the signal's executor.
    pub async fn wait(&'static self) -> Result<u32, ForeignContext> {
        core::future::poll_fn(|cx| self.poll_take(cx)).await
    }

    /// Move raised facts to the task and wake it in the executor context.
    /// Outside that context the facts stay pending.
    pub fn service(&'static self) {
        if let Ok(token) = context::<E>() {
            self.core.service_next(&token);
        }
    }
}

impl<E: SignalExecutor> Default for IrqSignal<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl SignalCore {
    const fn new() -> Self {
        Self {
            pending: AtomicU32::new(0),
            #[cfg(oer_debug_control)]
            held: AtomicU32::new(0),
            owned: UnsafeCell::new(Owned {
                waker: None,
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

    fn has_pending(&self, token: &ContextToken) -> bool {
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

    fn take(&self, token: &ContextToken) -> u32 {
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
        cx: &mut Context<'_>,
        list: &'static SignalList,
        token: &ContextToken,
    ) -> Poll<u32> {
        self.register(cx, list, token);
        match self.take(token) {
            0 => Poll::Pending,
            facts => Poll::Ready(facts),
        }
    }

    fn poll_pending(
        &'static self,
        cx: &mut Context<'_>,
        list: &'static SignalList,
        token: &ContextToken,
    ) -> Poll<()> {
        self.register(cx, list, token);
        if self.has_pending(token) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }

    /// Link on the first poll and register the task's waker.
    fn register(
        &'static self,
        cx: &mut Context<'_>,
        list: &'static SignalList,
        token: &ContextToken,
    ) {
        let replaced = self.owned(token, |owned| {
            if !owned.linked {
                owned.linked = true;
                owned.next = list.push(self, token);
            }
            match &mut owned.waker {
                Some(waker) if waker.will_wake(cx.waker()) => None,
                waker => waker.replace(cx.waker().clone()),
            }
        });
        // The replaced waker drops outside the owned half: its drop may
        // raise this very signal.
        drop(replaced);
    }

    fn service_next(&'static self, token: &ContextToken) -> Option<&'static SignalCore> {
        let (wake, next) = self.owned(token, |owned| {
            let facts = self.pending.swap(0, Ordering::Acquire);
            owned.delivered |= facts;
            #[cfg(oer_debug_control)]
            let available = self.deliverable(owned.delivered);
            #[cfg(not(oer_debug_control))]
            let available = facts;
            let wake = if available == 0 {
                None
            } else {
                owned.waker.take()
            };
            (wake, owned.next)
        });
        // The waker belongs to this executor's task: waking it only marks
        // the task for this very poll.
        if let Some(waker) = wake {
            waker.wake();
        }
        next
    }

    /// Run `f` on the owned half with proof of the executor context.
    fn owned<R>(&self, _token: &ContextToken, f: impl FnOnce(&mut Owned) -> R) -> R {
        #[allow(unsafe_code, reason = "the owned half is the executor context's")]
        // SAFETY: the token proves the executor's context runs now, and it
        // never runs concurrently with itself (`SignalExecutor`); no caller
        // of `owned` calls it again inside `f`.
        let owned = unsafe { &mut *self.owned.get() };
        f(owned)
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
pub struct SignalList {
    head: UnsafeCell<Option<&'static SignalCore>>,
}

#[allow(unsafe_code, reason = "the list is its executor context's")]
// SAFETY: `head` is touched only by `push`, from a signal's `owned`, and by
// `service`, after its context check: both run with a private token in the
// list's executor context, which never runs concurrently with itself.
unsafe impl Sync for SignalList {}

impl SignalList {
    /// An empty list.
    pub const fn new() -> Self {
        Self {
            head: UnsafeCell::new(None),
        }
    }

    fn push(
        &self,
        signal: &'static SignalCore,
        _token: &ContextToken,
    ) -> Option<&'static SignalCore> {
        #[allow(unsafe_code, reason = "the list is its executor context's")]
        // SAFETY: called only from `SignalCore::owned`, with proof of the
        // executor context.
        let head = unsafe { &mut *self.head.get() };
        head.replace(signal)
    }

    fn service(&self, token: &ContextToken) {
        #[allow(unsafe_code, reason = "the list is its executor context's")]
        // SAFETY: the token proves the caller runs in the list's executor
        // context, which also runs every `push`.
        let mut next = unsafe { *self.head.get() };
        while let Some(signal) = next {
            next = signal.service_next(token);
        }
    }
}

impl Default for SignalList {
    fn default() -> Self {
        Self::new()
    }
}

/// Service every signal of `E` before polling its tasks. Outside `E`'s
/// context neither the list nor its facts are touched. One context check
/// covers the complete traversal.
pub fn service<E: SignalExecutor>() {
    if let Ok(token) = context::<E>() {
        E::list().service(&token);
    }
}

/// Host executors for tests of signal users. Each thread owns its list and
/// leaked signals; a host signal must not be shared between threads.
#[cfg(any(test, feature = "host-signal"))]
pub mod host {
    extern crate std;
    use super::{IrqSignal, SignalExecutor, SignalList};
    use std::boxed::Box;

    std::thread_local! {
        static HOST_LIST: &'static SignalList = Box::leak(Box::new(SignalList::new()));
        static DEFERRED_LIST: &'static SignalList = Box::leak(Box::new(SignalList::new()));
        static FOREIGN_LIST: &'static SignalList = Box::leak(Box::new(SignalList::new()));
    }

    /// A pend services the signal immediately on the owning test thread.
    pub enum Host {}
    /// Facts wait until the owning test thread takes or services them.
    pub enum Deferred {}
    /// The executor context never runs.
    pub enum Foreign {}

    #[allow(unsafe_code, reason = "the host executor's contract")]
    // SAFETY: the context is the one test thread that owns its leaked
    // signals and thread-local list; a pend services the signal there.
    unsafe impl SignalExecutor for Host {
        fn pend(signal: &'static IrqSignal<Self>) {
            signal.service();
        }
        fn running() -> bool {
            true
        }
        fn list() -> &'static SignalList {
            HOST_LIST.with(|list| *list)
        }
    }

    #[allow(unsafe_code, reason = "the host executor's contract")]
    // SAFETY: as `Host`; a pend changes nothing, and the test services its
    // own thread-local list explicitly.
    unsafe impl SignalExecutor for Deferred {
        fn pend(_: &'static IrqSignal<Self>) {}
        fn running() -> bool {
            true
        }
        fn list() -> &'static SignalList {
            DEFERRED_LIST.with(|list| *list)
        }
    }

    #[allow(unsafe_code, reason = "the host executor's contract")]
    // SAFETY: the context never runs, so no owned half or list is reached.
    unsafe impl SignalExecutor for Foreign {
        fn pend(_: &'static IrqSignal<Self>) {}
        fn running() -> bool {
            false
        }
        fn list() -> &'static SignalList {
            FOREIGN_LIST.with(|list| *list)
        }
    }

    pub fn signal() -> &'static IrqSignal<Host> {
        Box::leak(Box::new(IrqSignal::new()))
    }
    pub fn deferred_signal() -> &'static IrqSignal<Deferred> {
        Box::leak(Box::new(IrqSignal::new()))
    }
    pub fn foreign_signal() -> &'static IrqSignal<Foreign> {
        Box::leak(Box::new(IrqSignal::new()))
    }
}

#[cfg(test)]
mod tests;
