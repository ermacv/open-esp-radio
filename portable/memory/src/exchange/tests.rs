use core::pin::pin;
use core::task::{Context, Poll, Waker};

use super::*;

fn poll<F: core::future::Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

#[test]
fn a_request_is_answered_through_its_reply() {
    let mut exchange = Exchange::<u8, u16>::new();
    let (mut asking, mut answering) = exchange.split();
    let mut next = pin!(answering.next());
    assert!(poll(next.as_mut()).is_pending());
    assert_eq!(asking.hand(4), Ok(()));
    let Poll::Ready((request, reply)) = poll(next.as_mut()) else {
        panic!("the request is handed");
    };
    assert_eq!(request, 4);
    let mut answered = pin!(asking.answered());
    assert!(poll(answered.as_mut()).is_pending());
    reply.answer(40);
    assert_eq!(poll(answered.as_mut()), Poll::Ready(Answered::Given(40)));
}

#[test]
fn a_second_request_while_the_first_lies_in_its_place_comes_back() {
    let mut exchange = Exchange::<u8, u16>::new();
    let (asking, _answering) = exchange.split();
    assert_eq!(asking.hand(1), Ok(()));
    assert_eq!(asking.hand(2), Err(2));
}

#[test]
fn a_dropped_reply_is_paid_as_unanswered_by_the_next_request_once() {
    let mut exchange = Exchange::<u8, u16>::new();
    let (mut asking, mut answering) = exchange.split();
    assert_eq!(asking.hand(5), Ok(()));
    let Poll::Ready((_, reply)) = poll(pin!(answering.next())) else {
        panic!("the request is handed");
    };
    drop(reply);
    assert!(
        poll(pin!(asking.answered())).is_pending(),
        "nothing is sent on the drop"
    );
    // The next call pays the debt, then waits for a request and is dropped.
    assert!(poll(pin!(answering.next())).is_pending());
    assert_eq!(
        poll(pin!(asking.answered())),
        Poll::Ready(Answered::Unanswered)
    );
    // A later call does not pay it again.
    let mut next = pin!(answering.next());
    assert!(poll(next.as_mut()).is_pending());
    assert!(poll(pin!(asking.answered())).is_pending(), "paid once");
    assert_eq!(asking.hand(6), Ok(()));
    let Poll::Ready((request, reply)) = poll(next.as_mut()) else {
        panic!("the next request is handed");
    };
    assert_eq!(request, 6);
    reply.answer(60);
    assert_eq!(
        poll(pin!(asking.answered())),
        Poll::Ready(Answered::Given(60))
    );
}

#[test]
fn an_exchange_with_priority_wakers_answers_as_the_default_one() {
    use oer_portable_sync::PriorityWaker;
    let mut exchange = Exchange::<u8, u8, PriorityWaker, AtomicWaker>::with_notifies(
        PriorityWaker::new(),
        PriorityWaker::new(),
        AtomicWaker::new(),
        AtomicWaker::new(),
    );
    let (mut asking, mut answering) = exchange.split();
    assert_eq!(asking.hand(7), Ok(()));
    let Poll::Ready((request, reply)) = poll(pin!(answering.next())) else {
        panic!("the request is handed");
    };
    reply.answer(request + 1);
    assert_eq!(
        poll(pin!(asking.answered())),
        Poll::Ready(Answered::Given(8))
    );
}

#[test]
fn the_answer_s_arrival_is_awaited_apart_from_its_take() {
    let mut exchange = Exchange::<u8, u16>::new();
    let (mut asking, mut answering) = exchange.split();
    assert!(asking.take().is_none(), "nothing before an answer");
    assert_eq!(asking.hand(3), Ok(()));
    assert!(poll(pin!(asking.arrived())).is_pending());
    let Poll::Ready((request, reply)) = poll(pin!(answering.next())) else {
        panic!("the request is handed");
    };
    reply.answer(u16::from(request) * 10);
    assert_eq!(poll(pin!(asking.arrived())), Poll::Ready(()));
    assert_eq!(
        poll(pin!(asking.arrived())),
        Poll::Ready(()),
        "arrival takes nothing"
    );
    assert_eq!(asking.take(), Some(Answered::Given(30)));
    assert!(asking.take().is_none(), "taken once");
}

#[test]
fn an_answered_request_owes_nothing_and_the_next_one_is_answered_too() {
    let mut exchange = Exchange::<u8, u16>::new();
    let (mut asking, mut answering) = exchange.split();
    for request in [1, 2] {
        assert_eq!(asking.hand(request), Ok(()));
        let Poll::Ready((taken, reply)) = poll(pin!(answering.next())) else {
            panic!("the request is handed");
        };
        assert_eq!(taken, request);
        reply.answer(u16::from(request) * 100);
        assert_eq!(
            poll(pin!(asking.answered())),
            Poll::Ready(Answered::Given(u16::from(request) * 100)),
            "a given answer, never an unanswered one"
        );
    }
    // Nothing more comes: no debt was left to pay.
    assert!(poll(pin!(answering.next())).is_pending());
    assert!(asking.take().is_none());
}
