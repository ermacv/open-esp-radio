extern crate std;

use super::*;
use crate::HostContext;
use std::{boxed::Box, vec::Vec};

type Core = SignalCore<u8, HostContext>;
type List = SignalList<u8, HostContext>;

fn storage() -> (&'static Core, &'static List, Entered<HostContext>) {
    (
        Box::leak(Box::new(Core::new())),
        Box::leak(Box::new(List::new())),
        Entered::enter().expect("constructing thread"),
    )
}

#[test]
fn nested_access_refuses_reads_and_polls_without_consuming_facts() {
    let (signal, list, token) = storage();
    signal.raise(0b11);
    assert_eq!(
        signal.owned(&token, |_| {
            assert_eq!(signal.take(&token), None);
            assert_eq!(signal.has_pending(&token), None);
            assert_eq!(
                signal.poll_take(1, list, &token),
                Poll::Ready(Err(AccessRefused))
            );
            assert_eq!(
                signal.poll_pending(1, list, &token),
                Poll::Ready(Err(AccessRefused))
            );
            signal.service(&token, |_| panic!("borrowed signal must not wake"));
            assert_eq!(signal.pending.load(Ordering::Acquire), 0b11);
        }),
        Some(())
    );
    assert_eq!(signal.poll_take(1, list, &token), Poll::Ready(Ok(0b11)));
    assert_eq!(signal.poll_take(1, list, &token), Poll::Pending);
}

#[test]
fn borrowed_list_refuses_registration_without_publishing_a_partial_link() {
    let (signal, list, token) = storage();
    signal.raise(0b10);
    assert_eq!(
        list.head.with(&token, |_| {
            assert_eq!(
                signal.poll_take(1, list, &token),
                Poll::Ready(Err(AccessRefused))
            );
            assert_eq!(
                signal.poll_pending(1, list, &token),
                Poll::Ready(Err(AccessRefused))
            );
            assert_eq!(list.has_pending(&token), None);
            list.service(&token, |_| panic!("borrowed list must not wake"));
            assert_eq!(signal.pending.load(Ordering::Acquire), 0b10);
            assert_eq!(signal.owned(&token, |owned| owned.linked), Some(false));
        }),
        Some(())
    );
    assert_eq!(signal.poll_take(1, list, &token), Poll::Ready(Ok(0b10)));
    signal.raise(0b100);
    let mut wakes = Vec::new();
    list.service(&token, |task| wakes.push(task));
    assert_eq!(wakes, [1]);
    assert_eq!(signal.poll_take(1, list, &token), Poll::Ready(Ok(0b100)));
}

#[test]
fn nested_list_service_retains_both_borrowed_and_unvisited_signals() {
    let (first, list, token) = storage();
    let head = Box::leak(Box::new(Core::new()));
    assert_eq!(first.poll_take(1, list, &token), Poll::Pending);
    assert_eq!(head.poll_take(2, list, &token), Poll::Pending);
    first.raise(0b01);
    head.raise(0b10);
    assert_eq!(
        head.owned(&token, |_| {
            list.service(&token, |_| panic!("borrowed head stops traversal"));
            assert_eq!(head.pending.load(Ordering::Acquire), 0b10);
            assert_eq!(first.pending.load(Ordering::Acquire), 0b01);
        }),
        Some(())
    );
    let mut wakes = Vec::new();
    list.service(&token, |task| wakes.push(task));
    assert_eq!(wakes, [2, 1]);
    assert_eq!(head.poll_take(2, list, &token), Poll::Ready(Ok(0b10)));
    assert_eq!(first.poll_take(1, list, &token), Poll::Ready(Ok(0b01)));
}

#[test]
fn callback_runs_after_borrow_and_new_facts_survive_service() {
    let (signal, list, token) = storage();
    assert_eq!(signal.poll_take(7, list, &token), Poll::Pending);
    assert_eq!(signal.raise(1), 0);
    assert_eq!(signal.raise(1), 1);
    list.service(&token, |task| {
        assert_eq!(task, 7);
        assert_eq!(signal.has_pending(&token), Some(true));
        assert_eq!(signal.raise(2), 0, "a new service round is owed");
    });
    assert_eq!(signal.take(&token), Some(3));
    assert_eq!(signal.take(&token), Some(0));
}

#[test]
fn cross_thread_raiser_publishes_only_atomic_facts_for_owner_service() {
    let (signal, list, token) = storage();
    assert_eq!(signal.poll_take(3, list, &token), Poll::Pending);
    let raiser = signal.raiser();
    std::thread::spawn(move || {
        assert!(raiser.raise(0b10));
        assert!(!raiser.raise(0b10));
    })
    .join()
    .expect("raiser thread");
    assert_eq!(
        signal.owned(&token, |owned| (owned.delivered, owned.task)),
        Some((0, Some(3)))
    );
    let mut wakes = Vec::new();
    list.service(&token, |task| wakes.push(task));
    assert_eq!(wakes, [3]);
    assert_eq!(signal.take(&token), Some(0b10));
}
