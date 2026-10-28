//! Coalesced signal facts and context-local task/list storage.
//!
//! The execution context admits every cell access. Atomic raisers publish
//! facts from any context; only an admitted owner takes, registers or services
//! payload. A callback marks a copied task handle ready after releasing its
//! cell borrow. This module knows no interrupt source, hart, CLIC or scheduler.
#![forbid(unsafe_code)]

use crate::{ContextCell, Entered, ExecutionContext};
use core::{
    sync::atomic::{AtomicU32, Ordering},
    task::Poll,
};

/// Context entry or a nested state borrow refused access; facts stay raised.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessRefused;

/// The task's half of a signal. Only its admitted context touches it.
struct Owned<T: 'static, X: ExecutionContext> {
    task: Option<T>,
    delivered: u32,
    linked: bool,
    next: Option<&'static SignalCore<T, X>>,
}

/// Storage typed by a task handle and its execution context.
pub struct SignalCore<T: 'static, X: ExecutionContext> {
    pending: AtomicU32,
    #[cfg(oer_debug_control)]
    held: AtomicU32,
    owned: ContextCell<Owned<T, X>, Entered<X>>,
}

impl<T: Copy + Eq + Send + 'static, X: ExecutionContext> SignalCore<T, X> {
    /// Empty storage with no registered task or pending facts.
    pub const fn new() -> Self {
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
    /// Publish facts with Release; return the previous pending word.
    pub fn raise(&self, facts: u32) -> u32 {
        self.pending.fetch_or(facts, Ordering::Release)
    }

    /// Observe facts without consuming them; `None` refuses context/nesting.
    pub fn has_pending(&self, token: &Entered<X>) -> Option<bool> {
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

    /// Take pending and delivered facts, retaining them on refused access.
    pub fn take(&self, token: &Entered<X>) -> Option<u32> {
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

    /// Register the task in `list`, then take facts or wait.
    pub fn poll_take(
        &'static self,
        task: T,
        list: &'static SignalList<T, X>,
        token: &Entered<X>,
    ) -> Poll<Result<u32, AccessRefused>> {
        if self.register(task, list, token).is_none() {
            return Poll::Ready(Err(AccessRefused));
        }
        match self.take(token) {
            Some(0) => Poll::Pending,
            Some(facts) => Poll::Ready(Ok(facts)),
            None => Poll::Ready(Err(AccessRefused)),
        }
    }

    /// Register the task in `list`, then observe facts without consuming them.
    pub fn poll_pending(
        &'static self,
        task: T,
        list: &'static SignalList<T, X>,
        token: &Entered<X>,
    ) -> Poll<Result<(), AccessRefused>> {
        if self.register(task, list, token).is_none() {
            return Poll::Ready(Err(AccessRefused));
        }
        match self.has_pending(token) {
            Some(true) => Poll::Ready(Ok(())),
            Some(false) => Poll::Pending,
            None => Poll::Ready(Err(AccessRefused)),
        }
    }

    /// Link on the first poll and register the polling task.
    fn register(
        &'static self,
        task: T,
        list: &'static SignalList<T, X>,
        token: &Entered<X>,
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

    fn service_next(
        &'static self,
        token: &Entered<X>,
        wake_task: &mut impl FnMut(T),
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
        // Release the cell borrow before invoking the owner's wake callback.
        if let Some(task) = wake {
            wake_task(task);
        }
        next
    }

    /// Service this signal; refusal retains its pending facts and task handle.
    pub fn service(&'static self, token: &Entered<X>, mut wake: impl FnMut(T)) {
        self.service_next(token, &mut wake);
    }

    /// Only the atomic half may cross a local owner's thread boundary.
    pub const fn raiser(&self) -> Raiser<'_> {
        Raiser {
            pending: &self.pending,
        }
    }

    /// Retain facts without touching task-owned state.
    #[cfg(oer_debug_control)]
    pub fn debug_hold(&self, facts: u32, hold: bool) {
        if hold {
            self.held.fetch_or(facts, Ordering::Release);
        } else {
            self.held.fetch_and(!facts, Ordering::Release);
        }
    }

    /// Run `f` on the owned half with proof of the executor context.
    fn owned<R>(&self, token: &Entered<X>, f: impl FnOnce(&mut Owned<T, X>) -> R) -> Option<R> {
        self.owned.with(token, f)
    }
}

/// Signals linked by their first poll; only their admitted context touches
/// the list.
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
        token: &Entered<X>,
    ) -> Option<Option<&'static SignalCore<T, X>>> {
        self.head.with(token, |head| head.replace(signal))
    }

    /// Observe facts without consuming them; `None` refuses context/nesting.
    pub fn has_pending(&self, token: &Entered<X>) -> Option<bool> {
        let mut next = self.head.with(token, |head| *head)?;
        while let Some(signal) = next {
            if signal.has_pending(token)? {
                return Some(true);
            }
            next = signal.owned(token, |owned| owned.next)?;
        }
        Some(false)
    }

    /// Service linked signals, invoking `wake` only after releasing each borrow.
    /// A refused borrow leaves that signal and unvisited signals pending.
    pub fn service(&self, token: &Entered<X>, mut wake: impl FnMut(T)) {
        let Some(mut next) = self.head.with(token, |head| *head) else {
            // The enclosing access retains the list; no signal was visited.
            return;
        };
        while let Some(signal) = next {
            next = signal.service_next(token, &mut wake);
        }
    }
}

impl<T: Copy + Eq + Send + 'static, X: ExecutionContext> Default for SignalList<T, X> {
    fn default() -> Self {
        Self::new()
    }
}

/// Transferable atomic publication, without a signal, list, task or proof.
#[derive(Clone, Copy)]
pub struct Raiser<'a> {
    pending: &'a AtomicU32,
}
impl Raiser<'_> {
    /// Publish facts for owner service, returning whether any fact is new.
    #[inline(always)]
    pub fn raise(self, facts: u32) -> bool {
        facts & !self.pending.fetch_or(facts, Ordering::Release) != 0
    }
}

impl<T: Copy + Eq + Send + 'static, X: ExecutionContext> Default for SignalCore<T, X> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
