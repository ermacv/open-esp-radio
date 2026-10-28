#![allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::panic,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::unreachable
)]
//! The shared conformance rule of every permit pool:
//! [`permits_serve_takers_in_arrival_order`]. Its pool's owner (a backend's
//! held part, a runtime) calls it with the pool, three seats of its line
//! and at most eight permits it holds aside. A pool that breaks it panics
//! with what it observed, so the test fails.

use core::{
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};

/// A [`PermitPool`](crate::PermitPool) of `held.len() + 1` permits whose
/// documented order is first in, first out, with three of the seats of its
/// line: with every permit but one held, three takers wait in arrival
/// order, whatever their seats' indices, and each released permit goes to
/// the earliest of them; a taker that gave up its place before then stalls
/// none of them. A seat outlives each take it lends itself to: once served,
/// and once its take was dropped, it waits again. `held` are the other
/// permits, taken by the caller, at most eight; the seats stay with the
/// caller.
///
/// # Panics
///
/// The pool serves a later taker first, or none at all.
pub fn permits_serve_takers_in_arrival_order<'id, K, P: crate::PermitPool<'id, K>>(
    pool: &P,
    seats: &mut [crate::Seat<'id, K>; 3],
    held: impl IntoIterator<Item = crate::Permit<'id, K>>,
) {
    let mut context = Context::from_waker(Waker::noop());
    let held: alloc_free::Held<'id, K> = alloc_free::Held::new(held);
    // The earliest taker sits on the last seat, so arrival order is not the
    // seats' order.
    let [later, latest, early] = seats;
    let mut last = match pin!(pool.take(early)).as_mut().poll(&mut context) {
        Poll::Ready(permit) => permit,
        Poll::Pending => panic!("a pool with a free permit serves its first taker"),
    };
    // A served seat waits again; a taker that gives up its place before it
    // is served stalls no one.
    {
        let mut gives_up = pin!(pool.take(early));
        assert!(
            gives_up.as_mut().poll(&mut context).is_pending(),
            "a taker waits while every permit is held"
        );
    }
    // The seat of the dropped take waits again.
    let mut first = pin!(pool.take(early));
    let mut second = pin!(pool.take(later));
    let mut third = pin!(pool.take(latest));
    for taker in [
        first.as_mut().poll(&mut context),
        second.as_mut().poll(&mut context),
        third.as_mut().poll(&mut context),
    ] {
        assert!(
            taker.is_pending(),
            "a taker waits while every permit is held"
        );
    }
    pool.release(last);
    assert!(
        second.as_mut().poll(&mut context).is_pending(),
        "a released permit goes to the earliest taker, not a later one"
    );
    assert!(
        third.as_mut().poll(&mut context).is_pending(),
        "a released permit goes to the earliest taker, not a later one"
    );
    last = match first.as_mut().poll(&mut context) {
        Poll::Ready(permit) => permit,
        Poll::Pending => panic!("the earliest taker gets the released permit"),
    };
    pool.release(last);
    assert!(
        third.as_mut().poll(&mut context).is_pending(),
        "a released permit goes to the earliest taker, not a later one"
    );
    let Poll::Ready(permit) = second.as_mut().poll(&mut context) else {
        panic!("the next taker gets the next released permit");
    };
    pool.release(permit);
    let Poll::Ready(permit) = third.as_mut().poll(&mut context) else {
        panic!("every waiting taker is served in turn");
    };
    pool.release(permit);
    held.release_into(pool);
}

/// The permits a rule holds aside, without an allocator.
mod alloc_free {
    /// At most this many permits held aside.
    const HELD: usize = 8;

    pub struct Held<'id, K> {
        permits: [Option<crate::Permit<'id, K>>; HELD],
    }

    impl<'id, K> Held<'id, K> {
        pub fn new(held: impl IntoIterator<Item = crate::Permit<'id, K>>) -> Self {
            let mut permits = [const { None }; HELD];
            let mut held = held.into_iter();
            for place in &mut permits {
                *place = held.next();
            }
            assert!(
                held.next().is_none(),
                "the rule holds at most {HELD} permits aside"
            );
            Self { permits }
        }

        pub fn release_into<P: crate::PermitPool<'id, K>>(self, pool: &P) {
            for permit in self.permits.into_iter().flatten() {
                pool.release(permit);
            }
        }
    }
}
