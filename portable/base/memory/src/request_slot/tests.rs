extern crate std;

use core::{
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll, Waker},
};
use std::{boxed::Box, sync::Arc, thread, vec::Vec};

use super::*;

fn slot() -> &'static RequestSlot<u32, u64> {
    let doorbell: &'static Doorbell = Box::leak(Box::new(Doorbell::new()));
    Box::leak(Box::new(RequestSlot::new(doorbell, 1 << 3)))
}

fn noop() -> Context<'static> {
    Context::from_waker(Waker::noop())
}

#[test]
fn a_slot_has_one_poster() {
    let slot = slot();
    assert!(slot.claim().is_some());
    assert!(slot.claim().is_none());
}

#[test]
fn a_posted_request_rings_once_and_is_answered_once() {
    let slot = slot();
    let mut cx = noop();
    assert!(slot.doorbell.poll_rung(&mut cx).is_pending());
    assert!(slot.take_request().is_none());
    let mut awaiting = slot.claim().unwrap().post(7);
    assert_eq!(slot.doorbell.poll_rung(&mut cx), Poll::Ready(1 << 3));
    assert!(slot.doorbell.poll_rung(&mut cx).is_pending());
    assert!(awaiting.poll_answer(&mut cx).is_pending());
    let (request, responder) = slot.take_request().unwrap();
    assert_eq!(request, 7);
    assert!(slot.take_request().is_none());
    assert!(awaiting.poll_answer(&mut cx).is_pending());
    responder.answer(70);
    let Poll::Ready((poster, 70)) = awaiting.poll_answer(&mut cx) else {
        panic!("the answer is ready");
    };
    assert!(awaiting.poll_answer(&mut cx).is_pending());
    // The slot is idle again and takes the next request.
    let _next = poster.post(8);
    let (request, responder) = slot.take_request().unwrap();
    assert_eq!(request, 8);
    responder.answer(80);
}

#[test]
fn a_spent_wait_never_takes_a_later_answer() {
    let slot = slot();
    let mut first = slot.claim().unwrap().post(1);
    let (_, responder) = slot.take_request().unwrap();
    responder.answer(10);
    let Poll::Ready((poster, 10)) = first.poll_answer(&mut noop()) else {
        panic!("the first answer is ready");
    };
    let mut second = poster.post(2);
    let (_, responder) = slot.take_request().unwrap();
    responder.answer(20);
    // The first wait is spent: the second answer is the second wait's.
    assert!(first.poll_answer(&mut noop()).is_pending());
    assert!(matches!(
        second.poll_answer(&mut noop()),
        Poll::Ready((_, 20))
    ));
}

struct Flag(AtomicBool);

impl std::task::Wake for Flag {
    fn wake(self: Arc<Self>) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[test]
fn the_client_is_woken_when_the_answer_arrives() {
    let slot = slot();
    let flag = Arc::new(Flag(AtomicBool::new(false)));
    let waker = Waker::from(flag.clone());
    let mut cx = Context::from_waker(&waker);
    let mut awaiting = slot.claim().unwrap().post(1);
    assert!(awaiting.poll_answer(&mut cx).is_pending());
    let (_, responder) = slot.take_request().unwrap();
    responder.answer(2);
    assert!(flag.0.load(Ordering::SeqCst));
    assert!(matches!(awaiting.poll_answer(&mut cx), Poll::Ready((_, 2))));
}

#[test]
fn a_dropped_wait_loses_no_answer() {
    let slot = slot();
    let mut awaiting = slot.claim().unwrap().post(5);
    assert!(awaiting.poll_answer(&mut noop()).is_pending());
    let (_, responder) = slot.take_request().unwrap();
    responder.answer(50);
    assert!(matches!(
        awaiting.poll_answer(&mut noop()),
        Poll::Ready((_, 50))
    ));
}

/// The client posts and awaits on one thread while the server takes and
/// answers on another, many times over, with the answer published before,
/// during and after the client's polls.
#[test]
fn post_take_and_answer_race_without_loss_or_duplication() {
    const ROUNDS: u32 = 20_000;
    let slot = slot();
    let server = thread::spawn(move || {
        let mut answered = 0;
        while answered < ROUNDS {
            if let Some((request, responder)) = slot.take_request() {
                responder.answer(u64::from(request) * 3);
                answered += 1;
            } else {
                thread::yield_now();
            }
        }
    });
    let mut answers = Vec::new();
    let mut poster = slot.claim().unwrap();
    for round in 0..ROUNDS {
        let mut awaiting = poster.post(round);
        poster = loop {
            if let Poll::Ready((poster, answer)) = awaiting.poll_answer(&mut noop()) {
                answers.push(answer);
                break poster;
            }
            thread::yield_now();
        };
    }
    server.join().unwrap();
    assert!(
        answers
            .iter()
            .zip(0..)
            .all(|(answer, round)| *answer == round * 3)
    );
}

#[test]
#[should_panic(expected = "a taken request must be answered")]
fn a_responder_dropped_unanswered_panics() {
    let slot = slot();
    let _awaiting = slot.claim().unwrap().post(7);
    let (_request, responder) = slot.take_request().unwrap();
    drop(responder);
}
