//! Waiting for the arbiter lease without polling it.
//!
//! The arbiter grants its lease only through a non-blocking `try_acquire`.
//! [`LeaseWaiters`] turns that into an event-driven wait: a waiter that finds
//! the lease held registers its waker, and the holder wakes every registered
//! waiter after its lease is released ([`LeaseReleaseNotice`]).

use core::{cell::RefCell, future::poll_fn, task::Poll};

use embassy_sync::{
    blocking_mutex::{Mutex as BlockingMutex, raw::RawMutex},
    waitqueue::MultiWakerRegistration,
};

/// The tasks waiting for the arbiter lease, up to `N` of them registered
/// apart. More waiters stay correct: a full registration wakes every waiter
/// to register again.
pub(crate) struct LeaseWaiters<M: RawMutex, const N: usize> {
    wakers: BlockingMutex<M, RefCell<MultiWakerRegistration<N>>>,
}

impl<M: RawMutex, const N: usize> LeaseWaiters<M, N> {
    pub(crate) const fn new() -> Self {
        Self {
            wakers: BlockingMutex::new(RefCell::new(MultiWakerRegistration::new())),
        }
    }

    /// Wait until `try_acquire` grants the lease.
    ///
    /// A failed attempt registers the task's waker and then tries once more,
    /// so a release that completed between the two attempts is seen by the
    /// second, and one that completes after the registration wakes the task:
    /// no release is lost. Every release wakes every waiter, which race for
    /// the lease as the retry loop did; dropping the future leaves nothing
    /// acquired.
    pub(crate) async fn acquire<L, E>(&self, mut try_acquire: impl FnMut() -> Result<L, E>) -> L {
        poll_fn(|cx| {
            if let Ok(lease) = try_acquire() {
                return Poll::Ready(lease);
            }
            self.wakers
                .lock(|wakers| wakers.borrow_mut().register(cx.waker()));
            match try_acquire() {
                Ok(lease) => Poll::Ready(lease),
                Err(_) => Poll::Pending,
            }
        })
        .await
    }

    /// Wake every waiter; call it after the lease was released.
    pub(crate) fn released(&self) {
        self.wakers.lock(|wakers| wakers.borrow_mut().wake());
    }
}

/// Wakes the lease waiters when dropped.
///
/// A guard declares it after its lease, so the fields drop in that order and
/// the waiters run only once the lease is free.
pub(crate) struct LeaseReleaseNotice<'waiters, M: RawMutex, const N: usize> {
    waiters: &'waiters LeaseWaiters<M, N>,
}

impl<'waiters, M: RawMutex, const N: usize> LeaseReleaseNotice<'waiters, M, N> {
    pub(crate) const fn new(waiters: &'waiters LeaseWaiters<M, N>) -> Self {
        Self { waiters }
    }
}

impl<M: RawMutex, const N: usize> Drop for LeaseReleaseNotice<'_, M, N> {
    fn drop(&mut self) {
        self.waiters.released();
    }
}

#[cfg(test)]
mod tests;
