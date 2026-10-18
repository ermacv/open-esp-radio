extern crate std;

use core::{
    cell::Cell,
    future::Future,
    pin::pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll, Waker},
};
use std::{sync::Arc, task::Wake};

use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use super::{LeaseReleaseNotice, LeaseWaiters};

/// A waker that counts its wakes.
#[derive(Default)]
struct Wakes(AtomicUsize);

impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn waker() -> (Arc<Wakes>, Waker) {
    let wakes = Arc::new(Wakes::default());
    (wakes.clone(), Waker::from(wakes))
}

fn count(wakes: &Wakes) -> usize {
    wakes.0.load(Ordering::Relaxed)
}

/// The arbiter's held flag.
struct Arbiter(Cell<bool>);

struct Busy;

impl Arbiter {
    fn try_acquire(&self) -> Result<(), Busy> {
        if self.0.replace(true) {
            Err(Busy)
        } else {
            Ok(())
        }
    }
}

type Waiters = LeaseWaiters<NoopRawMutex, 2>;

/// A holder's guard: the lease field before the notice, so the lease is
/// free when the waiters wake.
struct Guard<'a> {
    _lease: Lease<'a>,
    _notice: LeaseReleaseNotice<'a, NoopRawMutex, 2>,
}

struct Lease<'a>(&'a Arbiter);

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        self.0.0.set(false);
    }
}

fn guard<'a>(arbiter: &'a Arbiter, waiters: &'a Waiters) -> Guard<'a> {
    assert!(arbiter.try_acquire().is_ok(), "the arbiter is free");
    Guard {
        _lease: Lease(arbiter),
        _notice: LeaseReleaseNotice::new(waiters),
    }
}

#[test]
fn a_free_lease_is_granted_without_registering() {
    let arbiter = Arbiter(Cell::new(false));
    let waiters = Waiters::new();
    let (wakes, waker) = waker();
    let mut future = pin!(waiters.acquire(|| arbiter.try_acquire()));
    assert!(matches!(
        future.as_mut().poll(&mut Context::from_waker(&waker)),
        Poll::Ready(())
    ));
    waiters.released();
    assert_eq!(count(&wakes), 0);
}

#[test]
fn a_release_wakes_the_waiter_which_then_takes_the_lease() {
    let arbiter = Arbiter(Cell::new(false));
    let waiters = Waiters::new();
    let holder = guard(&arbiter, &waiters);
    let (wakes, waker) = waker();
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(waiters.acquire(|| arbiter.try_acquire()));
    assert!(future.as_mut().poll(&mut context).is_pending());
    assert_eq!(count(&wakes), 0, "a waiter sleeps until the release");
    drop(holder);
    assert_eq!(count(&wakes), 1);
    assert!(matches!(
        future.as_mut().poll(&mut context),
        Poll::Ready(())
    ));
}

/// A release between the failed attempt and the registration is seen by the
/// attempt after the registration, so it is not lost.
#[test]
fn a_release_before_the_registration_is_not_lost() {
    let arbiter = Arbiter(Cell::new(true));
    let waiters = Waiters::new();
    let attempts = Cell::new(0);
    let (_, waker) = waker();
    let mut future = pin!(waiters.acquire(|| {
        attempts.set(attempts.get() + 1);
        let result = arbiter.try_acquire();
        if attempts.get() == 1 {
            // The holder releases right after the first attempt failed.
            arbiter.0.set(false);
            waiters.released();
        }
        result
    }));
    assert!(matches!(
        future.as_mut().poll(&mut Context::from_waker(&waker)),
        Poll::Ready(())
    ));
    assert_eq!(attempts.get(), 2);
}

/// Two waiters wait without waking each other; a release wakes both, one
/// takes the lease and the other waits for the next release.
#[test]
fn concurrent_waiters_sleep_until_each_release() {
    let arbiter = Arbiter(Cell::new(false));
    let waiters = Waiters::new();
    let holder = guard(&arbiter, &waiters);
    let (first_wakes, first_waker) = waker();
    let (second_wakes, second_waker) = waker();
    let mut first_context = Context::from_waker(&first_waker);
    let mut second_context = Context::from_waker(&second_waker);
    let mut first = pin!(waiters.acquire(|| arbiter.try_acquire()));
    let mut second = pin!(waiters.acquire(|| arbiter.try_acquire()));
    assert!(first.as_mut().poll(&mut first_context).is_pending());
    assert!(second.as_mut().poll(&mut second_context).is_pending());
    assert!(first.as_mut().poll(&mut first_context).is_pending());
    assert_eq!((count(&first_wakes), count(&second_wakes)), (0, 0));

    drop(holder);
    assert_eq!((count(&first_wakes), count(&second_wakes)), (1, 1));
    assert!(matches!(
        second.as_mut().poll(&mut second_context),
        Poll::Ready(())
    ));
    assert!(first.as_mut().poll(&mut first_context).is_pending());
    assert_eq!(count(&first_wakes), 1);

    // The second waiter now holds the lease; its release wakes the first.
    let second_holder = Guard {
        _lease: Lease(&arbiter),
        _notice: LeaseReleaseNotice::new(&waiters),
    };
    drop(second_holder);
    assert_eq!(count(&first_wakes), 2);
    assert!(matches!(
        first.as_mut().poll(&mut first_context),
        Poll::Ready(())
    ));
}

#[test]
fn a_dropped_wait_acquires_nothing() {
    let arbiter = Arbiter(Cell::new(false));
    let waiters = Waiters::new();
    let holder = guard(&arbiter, &waiters);
    let (_, waker) = waker();
    {
        let mut future = pin!(waiters.acquire(|| arbiter.try_acquire()));
        assert!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(&waker))
                .is_pending()
        );
    }
    drop(holder);
    assert!(arbiter.try_acquire().is_ok(), "the lease is free");
}
