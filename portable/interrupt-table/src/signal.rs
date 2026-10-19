//! Static typed signals from an interrupt handler to one task of an
//! interrupt-mode executor, never through that executor's waker.
//!
//! An [`IrqSignal`] is a `static` of the image bound to its executor by a
//! [`SignalExecutor`], which only an interrupt-mode executor provides. Its
//! interrupt side ([`IrqSignal::raise`]) ORs facts into one atomic word with
//! Release and, only when a fact is new, pends the executor: it raises the
//! executor's software interrupt and invokes no waker. Everything else
//! happens in the executor's own context: the executor's handler, before it
//! polls, walks its signals ([`SignalExecutor::service`]), swaps each word to
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
    sync::atomic::{AtomicU32, Ordering},
    task::{Context, Poll, Waker},
};

/// The interrupt-mode executor a signal belongs to: how its interrupt side
/// pends it, whether the caller runs in its context, and the list of its
/// signals it services before it polls.
#[derive(Clone, Copy)]
pub struct SignalExecutor {
    pend: fn(&'static IrqSignal),
    running: fn() -> bool,
    list: &'static SignalList,
}

impl SignalExecutor {
    /// The executor that `pend` makes service `list` and poll, in the
    /// context `running` reports.
    ///
    /// # Safety
    ///
    /// `running` returns `true` exactly in one context that never runs
    /// concurrently with itself: the handler of an interrupt-mode executor,
    /// which services `list` before it polls, or a single-threaded host test.
    /// `pend` makes that context run, and runs no task itself outside it.
    #[allow(
        unsafe_code,
        reason = "the context cells of a signal rely on these functions"
    )]
    pub const unsafe fn new(
        pend: fn(&'static IrqSignal),
        running: fn() -> bool,
        list: &'static SignalList,
    ) -> Self {
        Self {
            pend,
            running,
            list,
        }
    }

    /// In the executor's context, before it polls: move every linked
    /// signal's raised facts to its task and wake that task. Outside that
    /// context nothing changes.
    pub fn service(&self) {
        if (self.running)() {
            self.list.service();
        }
    }
}

/// The task's half of a signal: its waker, the facts it has yet to take and
/// the link of its executor's list. Only the executor's context touches it.
struct Owned {
    waker: Option<Waker>,
    delivered: u32,
    linked: bool,
    next: Option<&'static IrqSignal>,
}

/// One coalesced word of facts from an interrupt handler to one task.
pub struct IrqSignal {
    pending: AtomicU32,
    executor: SignalExecutor,
    owned: UnsafeCell<Owned>,
}

#[allow(unsafe_code, reason = "the owned half is the executor context's")]
// SAFETY: `pending` is atomic. `owned` is reached only through `owned`,
// which checks the executor's `running`: one context that never runs
// concurrently with itself (`SignalExecutor::new`).
unsafe impl Sync for IrqSignal {}

/// The caller runs outside the executor the signal belongs to: its task was
/// spawned on another executor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ForeignContext;

impl IrqSignal {
    /// A signal of `executor` with no fact.
    pub const fn new(executor: SignalExecutor) -> Self {
        Self {
            pending: AtomicU32::new(0),
            executor,
            owned: UnsafeCell::new(Owned {
                waker: None,
                delivered: 0,
                linked: false,
                next: None,
            }),
        }
    }

    /// Raise `facts` from any context; `true` when one of them is new and
    /// the executor was pended. Inlined into the interrupt handler's own
    /// placement.
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

    /// Raise `facts` and return the pend a new fact owes, for a handler
    /// that pends only after it ends its register transaction.
    #[inline(always)]
    pub fn raise_deferred(&'static self, facts: u32) -> Raised {
        let previous = self.pending.fetch_or(facts, Ordering::Release);
        Raised {
            previous,
            pend: (facts & !previous != 0).then_some(SignalPend { signal: self }),
        }
    }

    /// Whether a fact waits for the task: an Acquire check its executor may
    /// make after arming its sources.
    ///
    /// # Errors
    ///
    /// The caller runs outside the signal's executor.
    pub fn has_pending(&'static self) -> Result<bool, ForeignContext> {
        let pending = self.pending.load(Ordering::Acquire) != 0;
        self.owned(|owned| pending || owned.delivered != 0)
    }

    /// Take every fact raised so far, without waiting.
    ///
    /// # Errors
    ///
    /// The caller runs outside the signal's executor.
    pub fn take(&'static self) -> Result<u32, ForeignContext> {
        // Outside the executor's context the facts stay raised.
        self.owned(|owned| {
            core::mem::take(&mut owned.delivered) | self.pending.swap(0, Ordering::Acquire)
        })
    }

    /// Register the task's waker, then take the facts raised so far;
    /// pending while there are none. The first poll links the signal into
    /// its executor's list.
    ///
    /// # Errors
    ///
    /// The caller runs outside the signal's executor; its task can never be
    /// woken by this signal.
    pub fn poll_take(&'static self, cx: &mut Context<'_>) -> Poll<Result<u32, ForeignContext>> {
        if let Err(foreign) = self.register(cx) {
            return Poll::Ready(Err(foreign));
        }
        match self.take() {
            Ok(0) => Poll::Pending,
            facts => Poll::Ready(facts),
        }
    }

    /// Register the task's waker, then report whether a fact waits, without
    /// taking it: for a task that takes its facts only once it acquired the
    /// work they announce. Pending while there are none; the first poll
    /// links the signal into its executor's list.
    ///
    /// # Errors
    ///
    /// As [`Self::poll_take`].
    pub fn poll_pending(&'static self, cx: &mut Context<'_>) -> Poll<Result<(), ForeignContext>> {
        if let Err(foreign) = self.register(cx) {
            return Poll::Ready(Err(foreign));
        }
        match self.has_pending() {
            Ok(false) => Poll::Pending,
            Ok(true) => Poll::Ready(Ok(())),
            Err(foreign) => Poll::Ready(Err(foreign)),
        }
    }

    /// Link the signal on its first poll and register the task's waker.
    fn register(&'static self, cx: &mut Context<'_>) -> Result<(), ForeignContext> {
        let list = self.executor.list;
        let replaced = self.owned(|owned| {
            if !owned.linked {
                owned.linked = true;
                owned.next = list.push(self);
            }
            match &mut owned.waker {
                Some(waker) if waker.will_wake(cx.waker()) => None,
                waker => waker.replace(cx.waker().clone()),
            }
        })?;
        // The replaced waker drops outside the owned half: its drop may
        // raise this very signal.
        drop(replaced);
        Ok(())
    }

    /// Wait for facts.
    ///
    /// # Errors
    ///
    /// As [`Self::poll_take`].
    pub async fn wait(&'static self) -> Result<u32, ForeignContext> {
        core::future::poll_fn(|cx| self.poll_take(cx)).await
    }

    /// In the executor's context, before it polls: move the raised facts
    /// to the task and wake it. A context without the facts' executor
    /// leaves them pending.
    pub fn service(&'static self) {
        let _ = self.service_next();
    }

    /// [`Self::service`], returning the next signal of the list.
    fn service_next(&'static self) -> Result<Option<&'static IrqSignal>, ForeignContext> {
        let wake = self.owned(|owned| {
            let facts = self.pending.swap(0, Ordering::Acquire);
            owned.delivered |= facts;
            let wake = if facts == 0 { None } else { owned.waker.take() };
            (wake, owned.next)
        })?;
        let (wake, next) = wake;
        // The waker belongs to this executor's task: waking it only marks
        // the task for this very poll.
        if let Some(waker) = wake {
            waker.wake();
        }
        Ok(next)
    }

    /// Run `f` on the owned half, in the executor's context only.
    fn owned<R>(&self, f: impl FnOnce(&mut Owned) -> R) -> Result<R, ForeignContext> {
        if !(self.executor.running)() {
            return Err(ForeignContext);
        }
        #[allow(unsafe_code, reason = "the owned half is the executor context's")]
        // SAFETY: the executor's context runs now, and it never runs
        // concurrently with itself (`SignalExecutor::new`); no caller of
        // `owned` calls it again inside `f`.
        let owned = unsafe { &mut *self.owned.get() };
        Ok(f(owned))
    }
}

/// One raise: the facts the executor had yet to service, and the pend a new
/// fact owes.
#[must_use = "pend the executor after ending the register transaction"]
pub struct Raised {
    /// The facts raised before, which the executor had not serviced.
    pub previous: u32,
    /// The pend, when a fact was new.
    pub pend: Option<SignalPend>,
}

/// The pend a newly raised fact owes its executor.
#[must_use = "pend the executor after ending the register transaction"]
pub struct SignalPend {
    signal: &'static IrqSignal,
}

impl SignalPend {
    /// Pend the signal's executor.
    #[inline(always)]
    pub fn pend(self) {
        (self.signal.executor.pend)(self.signal);
    }
}

/// The signals of one executor, linked by their first poll; only that
/// executor's context touches the list.
pub struct SignalList {
    head: UnsafeCell<Option<&'static IrqSignal>>,
}

#[allow(unsafe_code, reason = "the list is its executor context's")]
// SAFETY: `head` is touched only by `push`, from a signal's `owned`, and by
// `service`, from `SignalExecutor::service` after its context check: both
// run in the list's executor context, which never runs concurrently with
// itself.
unsafe impl Sync for SignalList {}

impl SignalList {
    /// An empty list.
    pub const fn new() -> Self {
        Self {
            head: UnsafeCell::new(None),
        }
    }

    /// Link `signal` first; the previous head.
    fn push(&self, signal: &'static IrqSignal) -> Option<&'static IrqSignal> {
        #[allow(unsafe_code, reason = "the list is its executor context's")]
        // SAFETY: called only from `IrqSignal::owned`, in the executor's
        // context.
        let head = unsafe { &mut *self.head.get() };
        head.replace(signal)
    }

    /// Service every linked signal; only `SignalExecutor::service` calls it,
    /// in the list's executor context.
    fn service(&self) {
        #[allow(unsafe_code, reason = "the list is its executor context's")]
        // SAFETY: the caller runs in the list's executor context, which also
        // runs every `push`.
        let mut next = unsafe { *self.head.get() };
        while let Some(signal) = next {
            match signal.service_next() {
                Ok(following) => next = following,
                Err(ForeignContext) => return,
            }
        }
    }
}

impl Default for SignalList {
    fn default() -> Self {
        Self::new()
    }
}

/// A host executor for the tests of a signal's users: its context is the
/// calling thread's, and a pend services the raised signal at once, as an
/// interrupt-mode executor's handler would before its next poll.
///
/// Each test leaks its own signals; a host signal must not be shared
/// between threads.
#[cfg(any(test, feature = "host-signal"))]
pub mod host {
    use super::{IrqSignal, SignalExecutor, SignalList};

    fn pend(signal: &'static IrqSignal) {
        signal.service();
    }

    fn running() -> bool {
        true
    }

    fn defer(_: &'static IrqSignal) {}

    fn elsewhere() -> bool {
        false
    }

    /// A fresh signal of an executor whose context never runs: a task that
    /// polls it runs on another executor.
    pub fn foreign_signal() -> &'static IrqSignal {
        extern crate std;
        let list = std::boxed::Box::leak(std::boxed::Box::new(SignalList::new()));
        // SAFETY: the context never runs, so no owned half is reached.
        #[allow(unsafe_code, reason = "the host executor's contract")]
        let executor = unsafe { SignalExecutor::new(defer, elsewhere, list) };
        std::boxed::Box::leak(std::boxed::Box::new(IrqSignal::new(executor)))
    }

    /// A fresh signal of a host executor that never runs on its own: its
    /// facts wait, as they wait for an executor still preempted by the
    /// interrupt, until the task takes them or the test services it.
    pub fn deferred_signal() -> &'static IrqSignal {
        extern crate std;
        let list = std::boxed::Box::leak(std::boxed::Box::new(SignalList::new()));
        // SAFETY: as `signal`; a pend changes nothing.
        #[allow(unsafe_code, reason = "the host executor's contract")]
        let executor = unsafe { SignalExecutor::new(defer, running, list) };
        std::boxed::Box::leak(std::boxed::Box::new(IrqSignal::new(executor)))
    }

    /// A fresh signal of the host executor, with a list of its own.
    pub fn signal() -> &'static IrqSignal {
        extern crate std;
        let list = std::boxed::Box::leak(std::boxed::Box::new(SignalList::new()));
        // SAFETY: the host context is the one test thread that owns the
        // leaked signal and its list; a pend services it in that thread.
        #[allow(unsafe_code, reason = "the host executor's contract")]
        let executor = unsafe { SignalExecutor::new(pend, running, list) };
        std::boxed::Box::leak(std::boxed::Box::new(IrqSignal::new(executor)))
    }
}

#[cfg(test)]
mod tests;
