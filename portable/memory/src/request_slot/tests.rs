extern crate std;

use core::{
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll, Waker},
};
use std::{boxed::Box, sync::Arc, thread, vec::Vec};

use super::*;

fn slot() -> &'static RequestSlot<u32, u64> {
    let doorbell: &'static Doorbell = Box::leak(Box::new(Doorbell::new()));
    Box::leak(Box::new(RequestSlot::new(doorbell, 1 << 3, 17)))
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
    assert!(slot.mode.doorbell.poll_rung(&mut cx).is_pending());
    assert!(slot.take_request().is_none());
    let mut awaiting = slot.claim().unwrap().post(7);
    assert_eq!(slot.mode.doorbell.poll_rung(&mut cx), Poll::Ready(1 << 3));
    assert!(slot.mode.doorbell.poll_rung(&mut cx).is_pending());
    assert!(awaiting.poll_answer(&mut cx).is_pending());
    let (request, responder) = slot.take_request().unwrap();
    assert_eq!(request, 7);
    assert!(slot.take_request().is_none());
    assert!(awaiting.poll_answer(&mut cx).is_pending());
    responder.answer(70);
    let Poll::Ready(Ok((poster, 70))) = awaiting.poll_answer(&mut cx) else {
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
    let Poll::Ready(Ok((poster, 10))) = first.poll_answer(&mut noop()) else {
        panic!("the first answer is ready");
    };
    let mut second = poster.post(2);
    let (_, responder) = slot.take_request().unwrap();
    responder.answer(20);
    // The first wait is spent: the second answer is the second wait's.
    assert!(first.poll_answer(&mut noop()).is_pending());
    assert!(matches!(
        second.poll_answer(&mut noop()),
        Poll::Ready(Ok((_, 20)))
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
    assert!(matches!(
        awaiting.poll_answer(&mut cx),
        Poll::Ready(Ok((_, 2)))
    ));
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
        Poll::Ready(Ok((_, 50)))
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
            if let Poll::Ready(Ok((poster, answer))) = awaiting.poll_answer(&mut noop()) {
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
fn an_unanswered_responder_wakes_the_client_and_permanently_quarantines_the_seat() {
    let slot = slot();
    let flag = Arc::new(Flag(AtomicBool::new(false)));
    let waker = Waker::from(flag.clone());
    let mut cx = Context::from_waker(&waker);
    let mut awaiting = slot.claim().unwrap().post(7);
    assert!(awaiting.poll_answer(&mut cx).is_pending());
    let (request, responder) = slot.take_request().unwrap();
    drop(responder);
    assert_eq!(request, 7);
    assert!(flag.0.load(Ordering::SeqCst));
    assert!(matches!(
        awaiting.poll_answer(&mut cx),
        Poll::Ready(Err(RequestError::Unanswered {
            kind: 17,
            slot: 3,
            epoch: 0,
            sequence: 1
        }))
    ));
    assert!(awaiting.poll_answer(&mut cx).is_pending());
    assert_eq!(slot.state.load(Ordering::Acquire), QUARANTINED);
    assert!(slot.claim().is_none());
    assert!(slot.take_request().is_none());
}

fn in_place<T>() -> &'static RequestSlot<T, (), InPlace> {
    Box::leak(Box::new(RequestSlot::new_in_place()))
}

#[derive(Debug, PartialEq)]
struct Exchange {
    request: u32,
    answer: Option<u32>,
}

fn exchange(request: u32) -> Exchange {
    Exchange {
        request,
        answer: None,
    }
}

#[test]
fn in_place_service_borrows_the_same_place_and_reuses_it_after_take() {
    let slot = in_place();
    let (mut client, mut server) = slot.claim_in_place().unwrap();
    assert!(slot.claim_in_place().is_none());
    assert!(client.is_idle());
    assert!(!server.service(|_| panic!("idle slot has no request")));
    assert!(client.try_take_if(|_| Ok::<_, ()>(())).is_none());
    let mut address = None;
    for request in [3, 7] {
        client.post(exchange(request)).unwrap();
        assert!(!client.is_idle());
        assert!(client.try_take_if(|_| Ok::<_, ()>(())).is_none());
        assert!(server.service(|value| {
            let place = core::ptr::from_ref(value).addr();
            if let Some(previous) = address {
                assert_eq!(place, previous);
            }
            address = Some(place);
            assert_eq!(value.request, request);
            value.answer = Some(request * 2);
            assert_eq!(client.post(exchange(99)), Err(exchange(99)));
            assert!(client.try_take_if(|_| Ok::<_, ()>(())).is_none());
        }));
        assert!(!server.service(|_| panic!("answered request cannot execute twice")));
        assert_eq!(client.post(exchange(99)), Err(exchange(99)));
        let answer = client.try_take_if(|value| {
            assert_eq!(Some(core::ptr::from_ref(value).addr()), address);
            Ok::<_, ()>(())
        });
        assert_eq!(
            answer,
            Some(Ok(Exchange {
                request,
                answer: Some(request * 2)
            }))
        );
        assert!(client.is_idle());
        assert!(client.try_take_if(|_| Ok::<_, ()>(())).is_none());
    }
    assert!(client.reunite(server).is_ok());
    assert!(slot.claim_in_place().is_some());
}

#[test]
fn in_place_validation_refusal_retains_the_exchange_until_a_valid_take() {
    let slot = in_place();
    let (mut client, mut server) = slot.claim_in_place().unwrap();
    client.post(exchange(4)).unwrap();
    assert!(server.service(|value| value.answer = Some(8)));
    for _ in 0..2 {
        assert_eq!(
            client.try_take_if(|value| {
                assert_eq!(value.answer, Some(8));
                Err("foreign reply")
            }),
            Some(Err("foreign reply"))
        );
        assert_eq!(client.post(exchange(5)), Err(exchange(5)));
        assert!(!client.is_idle());
        assert!(!server.service(|_| panic!("a rejected reply stays answered")));
    }
    assert_eq!(
        client.try_take_if(|_| Ok::<_, ()>(())),
        Some(Ok(Exchange {
            request: 4,
            answer: Some(8)
        }))
    );
    assert!(client.reunite(server).is_ok());
}

#[derive(Debug)]
struct Retained(Arc<core::sync::atomic::AtomicUsize>);
impl Drop for Retained {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn in_place_abandonment_retains_the_payload_in_each_phase() {
    for phase in 0..3 {
        let drops = Arc::new(core::sync::atomic::AtomicUsize::new(0));
        let slot = in_place();
        let (mut client, mut server) = slot.claim_in_place().unwrap();
        client.post(Retained(drops.clone())).unwrap();
        match phase {
            0 => {
                drop(client);
                assert!(server.service(|_| {}));
            }
            1 => assert!(server.service(|_| drop(client))),
            _ => {
                assert!(server.service(|_| {}));
                drop(client);
            }
        }
        assert!(slot.claim_in_place().is_none());
        assert!(!server.service(|_| panic!("abandonment retains the answered seat")));
        drop(server);
        assert!(slot.claim_in_place().is_none());
        assert_eq!(drops.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn in_place_take_returns_the_owned_payload_exactly_once() {
    let drops = Arc::new(core::sync::atomic::AtomicUsize::new(0));
    let slot = in_place();
    let (mut client, mut server) = slot.claim_in_place().unwrap();
    client.post(Retained(drops.clone())).unwrap();
    assert!(server.service(|_| {}));
    let retained = client.try_take_if(|_| Ok::<_, ()>(())).unwrap().unwrap();
    assert!(client.try_take_if(|_| Ok::<_, ()>(())).is_none());
    assert!(client.reunite(server).is_ok());
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(retained);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn in_place_reunion_refuses_occupied_or_foreign_endpoints_and_returns_both() {
    let slot = in_place();
    let other = in_place();
    let (mut client, server) = slot.claim_in_place().unwrap();
    let (other_client, other_server) = other.claim_in_place().unwrap();
    let (returned, other_server) = client.reunite(other_server).err().unwrap();
    client = returned;
    assert!(slot.claim_in_place().is_none());
    assert!(other.claim_in_place().is_none());
    assert!(other_client.reunite(other_server).is_ok());
    client.post(exchange(1)).unwrap();
    let (client, mut server) = client.reunite(server).err().unwrap();
    assert!(slot.claim_in_place().is_none());
    assert!(server.service(|value| value.answer = Some(2)));
    let (mut client, server) = client.reunite(server).err().unwrap();
    assert!(slot.claim_in_place().is_none());
    assert!(matches!(
        client.try_take_if(|_| Ok::<_, ()>(())),
        Some(Ok(_))
    ));
    assert!(client.reunite(server).is_ok());
    assert!(slot.claim_in_place().is_some());
}

#[test]
fn in_place_post_service_and_take_race_without_loss_or_duplication() {
    const ROUNDS: u32 = 20_000;
    let slot = in_place::<Exchange>();
    let (mut client, mut server) = slot.claim_in_place().unwrap();
    let worker = thread::spawn(move || {
        for expected in 0..ROUNDS {
            while !server.service(|value| {
                assert_eq!(value.request, expected);
                value.answer = Some(expected * 3);
            }) {
                thread::yield_now();
            }
        }
        server
    });
    for request in 0..ROUNDS {
        client.post(exchange(request)).unwrap();
        loop {
            if let Some(answer) = client.try_take_if(|_| Ok::<_, ()>(())) {
                assert_eq!(
                    answer,
                    Ok(Exchange {
                        request,
                        answer: Some(request * 3)
                    })
                );
                break;
            }
            thread::yield_now();
        }
    }
    assert!(client.reunite(worker.join().unwrap()).is_ok());
}

#[test]
fn dropping_a_quarantine_does_not_release_its_original_owners() {
    struct Owner<'a>(&'a core::cell::Cell<usize>);
    impl Drop for Owner<'_> {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let drops = core::cell::Cell::new(0);
    let quarantine = RequestQuarantine::new(
        RequestError::Unanswered {
            kind: 17,
            slot: 3,
            epoch: 9,
            sequence: 12,
        },
        Owner(&drops),
    );
    assert_eq!(quarantine.reason.observations(), [17, 3, 9, 12]);
    drop(quarantine);
    assert_eq!(drops.get(), 0);
}

#[test]
fn abandonment_preserves_the_slot_kind_index_and_wrapping_post_sequence() {
    for (previous, expected) in [(0, 2), (u32::MAX - 1, 0)] {
        let slot = slot();
        let mut poster = slot.claim().unwrap();
        poster.sequence = previous;
        let mut first = poster.post(1);
        slot.take_request().unwrap().1.answer(10);
        let Poll::Ready(Ok((poster, 10))) = first.poll_answer(&mut noop()) else {
            panic!("first request answered");
        };
        let mut second = poster.post(2);
        drop(slot.take_request().unwrap().1);
        let Poll::Ready(Err(error)) = second.poll_answer(&mut noop()) else {
            panic!("second request abandoned");
        };
        assert_eq!(error.observations(), [17, 3, 0, expected]);
        assert_eq!(
            error.with_epoch(u32::MAX).observations(),
            [17, 3, u32::MAX, expected]
        );
        assert!(second.poll_answer(&mut noop()).is_pending());
        assert!(slot.claim().is_none());
    }
}

#[test]
fn extracting_a_quarantined_error_preserves_its_observations_and_retains_owners() {
    struct Owner<'a>(&'a core::cell::Cell<u32>);
    impl Drop for Owner<'_> {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let drops = core::cell::Cell::new(0);
    let source = RequestError::Unanswered {
        kind: 4,
        slot: 1,
        epoch: 0xfedc_ba98,
        sequence: 27,
    };
    let error = RequestError::from(RequestQuarantine::new(source, Owner(&drops)));
    assert_eq!(error.observations(), [4, 1, 0xfedc_ba98, 27]);
    assert_eq!(drops.get(), 0);
}
