#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "a test fails loudly on a broken expectation"
)]

extern crate std;

use super::*;
use core::{cell::Cell, pin::pin};
use oer_memory::HostContext;
use oer_network_port::LinkSnapshot;
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

const LEASE: Ipv4Configuration = Ipv4Configuration {
    address: [192, 168, 4, 2],
    prefix_length: 24,
    gateway: Some([192, 168, 4, 1]),
};

fn status(state: LinkState, epoch: u64, ipv4: Ipv4Status, lifecycle: Lifecycle) -> Status {
    Status {
        link: LinkSnapshot { state, epoch },
        ipv4,
        lifecycle,
    }
}

const DOWN: Status = Status {
    link: LinkSnapshot::DOWN,
    ipv4: Ipv4Status::Unconfigured,
    lifecycle: Lifecycle::Running,
};

/// A waker that counts its wakes.
struct Count(AtomicU32);
impl std::task::Wake for Count {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn counting() -> (Waker, Arc<Count>) {
    let count = Arc::new(Count(AtomicU32::new(0)));
    (Waker::from(count.clone()), count)
}

fn poll<F: core::future::Future + ?Sized>(
    future: core::pin::Pin<&mut F>,
    waker: &Waker,
) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(waker))
}

#[test]
fn an_ipv4_wait_ends_with_the_lease_once_the_link_is_up() {
    let watchers = Watchers::<HostContext, 2>::new();
    let current = Cell::new(DOWN);
    let (waker, woken) = counting();
    let mut wait = pin!(wait_ipv4(&watchers, || current.get()));
    assert!(poll(wait.as_mut(), &waker).is_pending());
    // A lease without an up link is not enough.
    current.set(status(
        LinkState::Down,
        1,
        Ipv4Status::Lease(LEASE),
        Lifecycle::Running,
    ));
    watchers.wake_all();
    assert_eq!(woken.0.load(Ordering::Relaxed), 1);
    assert!(poll(wait.as_mut(), &waker).is_pending());
    current.set(status(
        LinkState::Up,
        1,
        Ipv4Status::Lease(LEASE),
        Lifecycle::Running,
    ));
    watchers.wake_all();
    assert_eq!(poll(wait.as_mut(), &waker), Poll::Ready(Ok(LEASE)));
}

#[test]
fn an_ipv4_wait_survives_a_link_loss_and_ends_with_the_next_lease() {
    let watchers = Watchers::<HostContext, 2>::new();
    let current = Cell::new(status(
        LinkState::Up,
        1,
        Ipv4Status::Unconfigured,
        Lifecycle::Running,
    ));
    let (waker, _) = counting();
    let mut wait = pin!(wait_ipv4(&watchers, || current.get()));
    assert!(poll(wait.as_mut(), &waker).is_pending());
    current.set(status(
        LinkState::Down,
        1,
        Ipv4Status::Unconfigured,
        Lifecycle::Running,
    ));
    watchers.wake_all();
    assert!(poll(wait.as_mut(), &waker).is_pending());
    current.set(status(
        LinkState::Up,
        2,
        Ipv4Status::Lease(LEASE),
        Lifecycle::Running,
    ));
    watchers.wake_all();
    assert_eq!(poll(wait.as_mut(), &waker), Poll::Ready(Ok(LEASE)));
}

#[test]
fn a_wait_ends_with_the_services_stop_or_failure() {
    let watchers = Watchers::<HostContext, 2>::new();
    let current = Cell::new(DOWN);
    let (waker, _) = counting();
    let mut stopped = pin!(wait_ipv4(&watchers, || current.get()));
    let mut failed = pin!(wait_link_down(&watchers, || current.get()));
    // The link is down at the start: the link wait ends at once.
    assert_eq!(poll(failed.as_mut(), &waker), Poll::Ready(Ok(())));
    assert!(poll(stopped.as_mut(), &waker).is_pending());
    current.set(status(
        LinkState::Down,
        0,
        Ipv4Status::Unconfigured,
        Lifecycle::Stopped,
    ));
    watchers.wake_all();
    assert_eq!(
        poll(stopped.as_mut(), &waker),
        Poll::Ready(Err(NetError::Stopped))
    );
    current.set(status(
        LinkState::Down,
        0,
        Ipv4Status::Unconfigured,
        Lifecycle::Failed(ServiceError::ContextAccessRefused),
    ));
    let mut after = pin!(wait_ipv4(&watchers, || current.get()));
    assert_eq!(
        poll(after.as_mut(), &waker),
        Poll::Ready(Err(NetError::Failed(ServiceError::ContextAccessRefused)))
    );
}

#[test]
fn a_link_wait_ends_on_loss_or_on_a_new_epoch() {
    let watchers = Watchers::<HostContext, 2>::new();
    let current = Cell::new(status(
        LinkState::Up,
        3,
        Ipv4Status::Lease(LEASE),
        Lifecycle::Running,
    ));
    let (waker, _) = counting();
    let mut lost = pin!(wait_link_down(&watchers, || current.get()));
    assert!(poll(lost.as_mut(), &waker).is_pending());
    current.set(status(
        LinkState::Down,
        3,
        Ipv4Status::Unconfigured,
        Lifecycle::Running,
    ));
    watchers.wake_all();
    assert_eq!(poll(lost.as_mut(), &waker), Poll::Ready(Ok(())));
    // Down and up again between polls: the new epoch shows the loss.
    current.set(status(
        LinkState::Up,
        3,
        Ipv4Status::Lease(LEASE),
        Lifecycle::Running,
    ));
    let mut coalesced = pin!(wait_link_down(&watchers, || current.get()));
    assert!(poll(coalesced.as_mut(), &waker).is_pending());
    current.set(status(
        LinkState::Up,
        4,
        Ipv4Status::Unconfigured,
        Lifecycle::Running,
    ));
    watchers.wake_all();
    assert_eq!(poll(coalesced.as_mut(), &waker), Poll::Ready(Ok(())));
}

#[test]
fn waiters_beyond_the_slots_are_refused_and_a_wait_keeps_its_one_slot() {
    let watchers = Watchers::<HostContext, 1>::new();
    let current = Cell::new(DOWN);
    let (first, _) = counting();
    let (other, _) = counting();
    let (second, _) = counting();
    let mut a = pin!(wait_ipv4(&watchers, || current.get()));
    let mut b = pin!(wait_ipv4(&watchers, || current.get()));
    assert!(poll(a.as_mut(), &first).is_pending());
    // The same wait again, with the same or another waker, keeps its slot.
    assert!(poll(a.as_mut(), &first).is_pending());
    assert!(poll(a.as_mut(), &other).is_pending());
    assert_eq!(
        poll(b.as_mut(), &second),
        Poll::Ready(Err(NetError::Waiters))
    );
}

#[test]
fn a_dropped_or_ended_wait_frees_its_slot_for_the_next_one() {
    let watchers = Watchers::<HostContext, 2>::new();
    let current = Cell::new(DOWN);
    // More cancelled waits than slots, each with its own waker.
    for _ in 0..4 {
        let (waker, _) = counting();
        let mut cancelled = pin!(wait_ipv4(&watchers, || current.get()));
        assert!(poll(cancelled.as_mut(), &waker).is_pending());
    }
    // Waits that ended free theirs too.
    for epoch in 1..=4 {
        let (waker, _) = counting();
        current.set(status(
            LinkState::Up,
            epoch,
            Ipv4Status::Unconfigured,
            Lifecycle::Running,
        ));
        let mut lost = pin!(wait_link_down(&watchers, || current.get()));
        assert!(poll(lost.as_mut(), &waker).is_pending());
        current.set(DOWN);
        assert_eq!(poll(lost.as_mut(), &waker), Poll::Ready(Ok(())));
    }
    // A live wait finds a slot, and its waker is the one woken.
    let (live, count) = counting();
    let mut wait = pin!(wait_ipv4(&watchers, || current.get()));
    assert!(poll(wait.as_mut(), &live).is_pending());
    current.set(status(
        LinkState::Up,
        5,
        Ipv4Status::Lease(LEASE),
        Lifecycle::Running,
    ));
    watchers.wake_all();
    assert_eq!(count.0.load(Ordering::Relaxed), 1);
    assert_eq!(poll(wait.as_mut(), &live), Poll::Ready(Ok(LEASE)));
}
