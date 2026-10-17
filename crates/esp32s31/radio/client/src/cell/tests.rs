extern crate std;

use core::{
    future::Future,
    pin::pin,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll, Waker},
};
use std::{boxed::Box, sync::Arc, thread, vec::Vec};

use super::*;

fn slot() -> &'static Slot<u32, u64> {
    let doorbell: &'static Doorbell = Box::leak(Box::new(Doorbell::new()));
    Box::leak(Box::new(Slot::new(doorbell, 1 << 3)))
}

fn noop() -> Context<'static> {
    Context::from_waker(Waker::noop())
}

#[test]
fn a_posted_request_rings_once_and_is_answered_once() {
    let slot = slot();
    let mut cx = noop();
    assert!(slot.doorbell.poll_rung(&mut cx).is_pending());
    assert!(slot.take_request().is_none());
    slot.post(7);
    assert_eq!(slot.doorbell.poll_rung(&mut cx), Poll::Ready(1 << 3));
    assert!(slot.doorbell.poll_rung(&mut cx).is_pending());
    assert!(slot.poll_answer(&mut cx).is_pending());
    let (request, responder) = slot.take_request().unwrap();
    assert_eq!(request, 7);
    assert!(slot.take_request().is_none());
    assert!(slot.poll_answer(&mut cx).is_pending());
    responder.answer(70);
    assert_eq!(slot.poll_answer(&mut cx), Poll::Ready(70));
    assert!(slot.poll_answer(&mut cx).is_pending());
    // The slot is idle again and takes the next request.
    slot.post(8);
    assert_eq!(slot.take_request().map(|(request, _)| request), Some(8));
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
    slot.post(1);
    assert!(slot.poll_answer(&mut cx).is_pending());
    let (_, responder) = slot.take_request().unwrap();
    responder.answer(2);
    assert!(flag.0.load(Ordering::SeqCst));
    assert_eq!(slot.poll_answer(&mut cx), Poll::Ready(2));
}

#[test]
fn a_dropped_wait_loses_no_answer() {
    let slot = slot();
    slot.post(5);
    {
        let wait = pin!(core::future::poll_fn(|cx| slot.poll_answer(cx)));
        assert!(wait.poll(&mut noop()).is_pending());
    }
    let (_, responder) = slot.take_request().unwrap();
    responder.answer(50);
    let wait = pin!(core::future::poll_fn(|cx| slot.poll_answer(cx)));
    assert_eq!(wait.poll(&mut noop()), Poll::Ready(50));
}

/// The client posts and awaits on one thread while the actor takes and
/// answers on another, many times over, with the answer published before,
/// during and after the client's polls.
#[test]
fn post_take_and_answer_race_without_loss_or_duplication() {
    const ROUNDS: u32 = 20_000;
    let slot = slot();
    let actor = thread::spawn(move || {
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
    for round in 0..ROUNDS {
        slot.post(round);
        loop {
            if let Poll::Ready(answer) = slot.poll_answer(&mut noop()) {
                answers.push(answer);
                break;
            }
            thread::yield_now();
        }
    }
    actor.join().unwrap();
    assert!(
        answers
            .iter()
            .zip(0..)
            .all(|(answer, round)| *answer == round * 3)
    );
}
