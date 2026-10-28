extern crate std;

use core::{
    future::Future,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};
use std::{
    boxed::Box,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::Wake,
};

use oer_radio_port::{Permit, PermitPool, Seat, conformance};

use super::FifoPermitPool;

/// The permits' kind.
enum Kind {}

/// Poll `future` once with a waker that does nothing.
fn poll_once<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

/// Run `check` on a pool of `N` permits of one install and the `S` seats
/// of its line.
fn with_pool<const N: usize, const S: usize, C>(check: C)
where
    C: for<'id> FnOnce(&FifoPermitPool<'id, Kind, N, S>, [Seat<'id, Kind>; S]),
{
    let future = oer_radio_port::branded::<oer_radio_port::Permits<Kind, N, S>, _>(async |mint| {
        let oer_radio_port::PermitTokens {
            supply,
            line,
            seats,
        } = mint.into_pools();
        check(&FifoPermitPool::new(supply, line), seats);
    });
    assert!(
        poll_once(pin!(future)).is_ready(),
        "the pool check finishes without an executor"
    );
}

/// Take a free permit at once.
fn take_now<'id, const N: usize, const S: usize>(
    pool: &FifoPermitPool<'id, Kind, N, S>,
    seat: &mut Seat<'id, Kind>,
) -> Permit<'id, Kind> {
    match poll_once(pin!(pool.take(seat))) {
        Poll::Ready(permit) => permit,
        Poll::Pending => panic!("a free permit serves at once"),
    }
}

#[test]
fn a_pool_of_one_serves_takers_in_arrival_order() {
    with_pool::<1, 3, _>(|pool, mut seats| {
        conformance::permits_serve_takers_in_arrival_order(pool, &mut seats, []);
    });
}

#[test]
fn a_pool_with_a_permit_held_aside_serves_takers_in_arrival_order() {
    with_pool::<2, 4, _>(|pool, [mut aside, first, second, third]| {
        let held = take_now(pool, &mut aside);
        conformance::permits_serve_takers_in_arrival_order(
            pool,
            &mut [first, second, third],
            [held],
        );
    });
}

#[test]
fn out_of_order_returns_preserve_every_permits_capacity() {
    with_pool::<4, 1, _>(|pool, [mut seat]| {
        let first = take_now(pool, &mut seat);
        let second = take_now(pool, &mut seat);
        let third = take_now(pool, &mut seat);
        let fourth = take_now(pool, &mut seat);
        assert_eq!(pool.available(), 0);
        pool.release(third);
        pool.release(first);
        let left = take_now(pool, &mut seat);
        let right = take_now(pool, &mut seat);
        assert_ne!(left.index(), right.index());
        assert!(poll_once(pin!(pool.take(&mut seat))).is_pending());
        for permit in [fourth, right, second, left] {
            pool.release(permit);
        }
        let returned: [_; 4] = core::array::from_fn(|_| take_now(pool, &mut seat));
        let mut identities = [false; 4];
        for permit in &returned {
            assert!(!identities[usize::from(permit.index())]);
            identities[usize::from(permit.index())] = true;
        }
        assert_eq!(identities, [true; 4]);
        assert_eq!(pool.available(), 0);
        for permit in returned {
            pool.release(permit);
        }
    });
}

#[test]
fn every_seat_waits_in_arrival_order_whatever_its_index() {
    with_pool::<1, 4, _>(|pool, [mut zero, mut one, mut two, mut three]| {
        let mut held = take_now(pool, &mut zero);
        // Every seat of the line waits at once: the line has a place for
        // each, so none is ever turned away.
        let mut first = pin!(pool.take(&mut three));
        let mut second = pin!(pool.take(&mut one));
        let mut third = pin!(pool.take(&mut zero));
        let mut fourth = pin!(pool.take(&mut two));
        for taker in [
            poll_once(first.as_mut()),
            poll_once(second.as_mut()),
            poll_once(third.as_mut()),
            poll_once(fourth.as_mut()),
        ] {
            assert!(taker.is_pending());
        }
        assert_eq!(pool.waiting(), 4);
        for taker in [first, second, third, fourth] {
            let mut taker = taker;
            pool.release(held);
            held = match poll_once(taker.as_mut()) {
                Poll::Ready(permit) => permit,
                Poll::Pending => panic!("the earliest taker gets the released permit"),
            };
        }
        assert_eq!(pool.waiting(), 0);
        pool.release(held);
    });
}

#[test]
fn a_dropped_take_gives_up_its_place_and_its_seat_waits_again() {
    with_pool::<1, 2, _>(|pool, [mut mine, mut other]| {
        let held = take_now(pool, &mut other);
        {
            let mut gives_up = pin!(pool.take(&mut mine));
            assert!(poll_once(gives_up.as_mut()).is_pending());
            assert_eq!(pool.waiting(), 1);
        }
        assert_eq!(pool.waiting(), 0);
        // The seat came back with the dropped take, and waits again.
        let mut again = pin!(pool.take(&mut mine));
        assert!(poll_once(again.as_mut()).is_pending());
        pool.release(held);
        let Poll::Ready(permit) = poll_once(again.as_mut()) else {
            panic!("the seat's new take is served");
        };
        pool.release(permit);
    });
}

#[test]
fn a_seat_whose_take_leaked_takes_its_place_again() {
    with_pool::<1, 2, _>(|pool, [mut mine, mut other]| {
        let held = take_now(pool, &mut other);
        let mut leaked = Box::pin(pool.take(&mut mine));
        assert!(poll_once(leaked.as_mut()).is_pending());
        // A leaked take never drops, and no one polls it again.
        core::mem::forget(leaked);
        let mut again = pin!(pool.take(&mut mine));
        assert!(poll_once(again.as_mut()).is_pending());
        assert_eq!(pool.waiting(), 1);
        pool.release(held);
        let Poll::Ready(permit) = poll_once(again.as_mut()) else {
            panic!("the seat's new take leads the line");
        };
        pool.release(permit);
    });
}

#[test]
fn two_releases_before_any_taker_runs_serve_two_takers() {
    with_pool::<2, 4, _>(|pool, [mut a, mut b, mut c, mut d]| {
        let one = take_now(pool, &mut a);
        let two = take_now(pool, &mut b);
        let mut third = pin!(pool.take(&mut c));
        let mut fourth = pin!(pool.take(&mut d));
        assert!(poll_once(third.as_mut()).is_pending());
        assert!(poll_once(fourth.as_mut()).is_pending());
        pool.release(one);
        pool.release(two);
        assert!(poll_once(third.as_mut()).is_ready());
        assert!(poll_once(fourth.as_mut()).is_ready());
        assert_eq!(pool.available(), 0);
    });
}

/// A waker that counts its wakes.
#[derive(Default)]
struct Count(AtomicUsize);

impl Wake for Count {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

impl Count {
    fn wakes(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
}

#[test]
fn a_release_wakes_the_leader_and_a_served_leader_wakes_the_next() {
    with_pool::<2, 4, _>(|pool, [mut a, mut b, mut c, mut d]| {
        let one = take_now(pool, &mut a);
        let two = take_now(pool, &mut b);
        let (leader, next) = (Arc::new(Count::default()), Arc::new(Count::default()));
        let (leader_waker, next_waker) = (Waker::from(leader.clone()), Waker::from(next.clone()));
        let mut first = pin!(pool.take(&mut c));
        let mut second = pin!(pool.take(&mut d));
        assert!(
            first
                .as_mut()
                .poll(&mut Context::from_waker(&leader_waker))
                .is_pending()
        );
        assert!(
            second
                .as_mut()
                .poll(&mut Context::from_waker(&next_waker))
                .is_pending()
        );
        pool.release(one);
        pool.release(two);
        // Each release wakes the leader; the taker behind it sleeps on.
        assert_eq!((leader.wakes(), next.wakes()), (2, 0));
        assert!(
            first
                .as_mut()
                .poll(&mut Context::from_waker(&leader_waker))
                .is_ready()
        );
        // The served leader left a permit free: it wakes the next leader.
        assert_eq!(next.wakes(), 1);
        assert!(
            second
                .as_mut()
                .poll(&mut Context::from_waker(&next_waker))
                .is_ready()
        );
    });
}

#[test]
fn a_departing_leader_wakes_the_next_one_when_a_permit_is_free() {
    with_pool::<1, 3, _>(|pool, [mut aside, mut leads, mut follows]| {
        let held = take_now(pool, &mut aside);
        let next_wakes = Arc::new(Count::default());
        let next_waker = Waker::from(next_wakes.clone());
        let mut next = pin!(pool.take(&mut follows));
        {
            let mut leader = pin!(pool.take(&mut leads));
            assert!(poll_once(leader.as_mut()).is_pending());
            assert!(
                next.as_mut()
                    .poll(&mut Context::from_waker(&next_waker))
                    .is_pending()
            );
            pool.release(held);
            assert_eq!(next_wakes.wakes(), 0, "the release woke the leader");
        }
        // The woken leader gave up its place: the next one leads and is
        // woken to take the free permit.
        assert_eq!(next_wakes.wakes(), 1);
        assert!(
            next.as_mut()
                .poll(&mut Context::from_waker(&next_waker))
                .is_ready()
        );
    });
}

#[test]
fn a_leaked_leader_replaced_by_its_seat_wakes_the_taker_behind_it() {
    with_pool::<1, 3, _>(|pool, [mut aside, mut mine, mut other]| {
        let held = take_now(pool, &mut aside);
        let mut leaked = Box::pin(pool.take(&mut mine));
        assert!(poll_once(leaked.as_mut()).is_pending());
        let behind_wakes = Arc::new(Count::default());
        let behind_waker = Waker::from(behind_wakes.clone());
        let mut behind = pin!(pool.take(&mut other));
        assert!(
            behind
                .as_mut()
                .poll(&mut Context::from_waker(&behind_waker))
                .is_pending()
        );
        // The leaked leader never drops; the release wakes it alone.
        core::mem::forget(leaked);
        pool.release(held);
        assert_eq!(behind_wakes.wakes(), 0);
        // Its seat's next take replaces it and leads no more: the taker
        // behind it now leads and is woken to take the free permit.
        let mut again = pin!(pool.take(&mut mine));
        assert!(poll_once(again.as_mut()).is_pending());
        assert_eq!(behind_wakes.wakes(), 1);
        let Poll::Ready(permit) = behind
            .as_mut()
            .poll(&mut Context::from_waker(&behind_waker))
        else {
            panic!("the taker behind the leaked leader is served");
        };
        pool.release(permit);
        assert!(poll_once(again.as_mut()).is_ready());
    });
}

#[test]
fn distinct_permit_pools_share_one_waiter_domain_without_sharing_capacity() {
    type Layout = (
        oer_radio_port::Permits<Kind, 1, 1>,
        oer_radio_port::Permits<Kind, 1, 0>,
    );
    let future = oer_radio_port::branded::<Layout, _>(async |mint| {
        let (a, b) = mint.into_pools();
        let [mut seat] = a.seats;
        let line = a.line;
        let a = FifoPermitPool::new(a.supply, line);
        let b = FifoPermitPool::new(b.supply, line);
        let token = a.take(&mut seat).await;
        assert_eq!(a.available(), 0);
        let other = b.take(&mut seat).await;
        assert_eq!(b.available(), 0);
        b.release(other);
        a.release(token);
        assert_eq!(a.available(), 1);
    });
    assert!(poll_once(pin!(future)).is_ready());
}

#[test]
fn empty_capacity_and_zero_waiters_need_no_special_case() {
    with_pool::<0, 0, _>(|pool, seats| {
        assert_eq!(pool.available(), 0);
        assert_eq!(pool.waiting(), 0);
        assert!(seats.is_empty());
    });
}

#[test]
fn fifo_order_survives_the_ticket_counter_wrap() {
    with_pool::<1, 3, _>(|pool, mut seats| {
        pool.next.set(u32::MAX - 2);
        conformance::permits_serve_takers_in_arrival_order(pool, &mut seats, []);
        assert_eq!(pool.available(), 1);
        assert_eq!(pool.waiting(), 0);
    });
}

#[test]
fn repoll_replaces_the_wake_target_without_losing_the_place() {
    with_pool::<1, 2, _>(|pool, [mut a, mut b]| {
        let held = take_now(pool, &mut a);
        let old = Arc::new(Count::default());
        let new = Arc::new(Count::default());
        let old_waker = Waker::from(old.clone());
        let new_waker = Waker::from(new.clone());
        let mut take = pin!(pool.take(&mut b));
        for waker in [&old_waker, &new_waker] {
            assert!(
                take.as_mut()
                    .poll(&mut Context::from_waker(waker))
                    .is_pending()
            );
        }
        assert_eq!(pool.waiting(), 1);
        pool.release(held);
        assert_eq!((old.wakes(), new.wakes()), (0, 1));
        let Poll::Ready(permit) = poll_once(take.as_mut()) else {
            panic!("the repolled leader keeps its place");
        };
        pool.release(permit);
        assert_eq!(pool.available(), 1);
        assert_eq!(pool.waiting(), 0);
    });
}

#[test]
fn every_permit_and_seat_in_the_largest_pool_retains_its_capacity() {
    with_pool::<256, 256, _>(|pool, mut seats| {
        let held: std::vec::Vec<_> = seats.iter_mut().map(|seat| take_now(pool, seat)).collect();
        assert_eq!(pool.available(), 0);
        let mut waiting: std::vec::Vec<_> = seats
            .iter_mut()
            .rev()
            .map(|seat| Box::pin(pool.take(seat)))
            .collect();
        for take in &mut waiting {
            assert!(poll_once(take.as_mut()).is_pending());
        }
        assert_eq!(pool.waiting(), 256);
        for (permit, take) in held.into_iter().rev().zip(waiting.iter_mut()) {
            pool.release(permit);
            let Poll::Ready(returned) = poll_once(take.as_mut()) else {
                panic!("every seat is served in arrival order");
            };
            pool.release(returned);
        }
        assert_eq!(pool.available(), 256);
        assert_eq!(pool.waiting(), 0);
    });
}

#[test]
fn a_wake_callback_can_repoll_and_replace_its_waker() {
    with_pool::<1, 2, _>(|pool, [mut a, mut b]| {
        let held = take_now(pool, &mut a);
        let old = Arc::new(Count::default());
        let new = Arc::new(Count::default());
        let old_waker = Waker::from(old.clone());
        let new_waker = Waker::from(new.clone());
        let mut take = Box::pin(pool.take(&mut b));
        assert!(
            take.as_mut()
                .poll(&mut Context::from_waker(&old_waker))
                .is_pending()
        );
        // Invoke the same callback boundary as wake_leader, with a scoped
        // closure so reentry needs neither an unsafe RawWaker nor global state.
        let slot = pool
            .line
            .iter()
            .find(|slot| slot.ticket.get().is_some())
            .unwrap();
        slot.wake(|waker| {
            assert_eq!(pool.waiting(), 1, "the leader remains observable");
            assert!(
                take.as_mut()
                    .poll(&mut Context::from_waker(&new_waker))
                    .is_pending()
            );
            waker.wake_by_ref();
        });
        pool.release(held);
        assert_eq!((old.wakes(), new.wakes()), (1, 1));
        let Poll::Ready(permit) = poll_once(take.as_mut()) else {
            panic!("the callback's new waker retains the waiter's place");
        };
        pool.release(permit);
    });
}

#[test]
fn a_wake_callback_can_cancel_its_take_without_restoring_a_stale_waker() {
    with_pool::<1, 2, _>(|pool, [mut a, mut b]| {
        let held = take_now(pool, &mut a);
        let count = Arc::new(Count::default());
        let waker = Waker::from(count.clone());
        let mut take = Box::pin(pool.take(&mut b));
        assert!(
            take.as_mut()
                .poll(&mut Context::from_waker(&waker))
                .is_pending()
        );
        let slot = pool
            .line
            .iter()
            .find(|slot| slot.ticket.get().is_some())
            .unwrap();
        slot.wake(|waker| {
            drop(take);
            assert_eq!(pool.waiting(), 0);
            waker.wake_by_ref();
        });
        pool.release(held);
        assert_eq!(count.wakes(), 1, "a departed waiter is not woken again");
        assert_eq!(Arc::strong_count(&count), 2, "the queue released its waker");
        let permit = take_now(pool, &mut b);
        pool.release(permit);
    });
}
