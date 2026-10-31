extern crate std;

use core::{
    future::Future,
    pin::pin,
    sync::atomic::{AtomicU32, Ordering},
    task::{Context, Poll, Waker},
};
use std::{boxed::Box, sync::Arc, task::Wake};

use oer_espressif_interrupt_table::signal::host;
use oer_memory::{InPlace, RequestSlot};
use oer_time::{Clock as _, Duration};
use oer_time_virtual::VirtualClock;

use super::{Occupied, PeerError, pair};

/// One request and its answer, exchanged in place.
#[derive(Debug, Eq, PartialEq)]
struct Square {
    value: u32,
    answer: Option<u32>,
}

fn slot() -> &'static RequestSlot<Square, (), InPlace> {
    Box::leak(Box::new(RequestSlot::new_in_place()))
}

#[derive(Default)]
struct Counting(AtomicU32);

impl Wake for Counting {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn task() -> (Arc<Counting>, Waker) {
    let counting = Arc::new(Counting::default());
    (counting.clone(), Waker::from(counting))
}

fn square(request: &mut Square) {
    request.answer = request.value.checked_mul(request.value);
}

#[test]
fn a_posted_request_is_answered_in_place_and_wakes_the_client_through_its_signal() {
    let (request, _) = host::deferred_signal();
    let (reply, _) = host::signal();
    let (mut client, mut server) = pair(slot(), request, reply).expect("a fresh slot");
    let clock = VirtualClock::<4>::new();
    let deadline = clock
        .deadline_after(Duration::from_millis(10))
        .expect("in range");

    let (client_wakes, client_waker) = task();
    let (_, server_waker) = task();
    let mut client_cx = Context::from_waker(&client_waker);
    let mut server_cx = Context::from_waker(&server_waker);

    let mut serving = pin!(server.serve(square));
    assert_eq!(serving.as_mut().poll(&mut server_cx), Poll::Pending);

    assert_eq!(
        client.post(Square {
            value: 7,
            answer: None
        }),
        Ok(())
    );
    {
        let mut answering = pin!(client.answer(&clock, deadline));
        assert_eq!(answering.as_mut().poll(&mut client_cx), Poll::Pending);
        // The server's executor runs its task: the request is answered and
        // the client's executor is pended, which wakes the client's task.
        assert_eq!(serving.as_mut().poll(&mut server_cx), Poll::Pending);
        assert_eq!(client_wakes.0.load(Ordering::Relaxed), 1);
        assert_eq!(
            answering.as_mut().poll(&mut client_cx),
            Poll::Ready(Ok(Square {
                value: 7,
                answer: Some(49)
            }))
        );
    }
    // The slot is idle again: the next request posts.
    assert_eq!(
        client.post(Square {
            value: 3,
            answer: None
        }),
        Ok(())
    );
}

#[test]
fn a_second_post_before_the_answer_returns_its_request() {
    let (request, _) = host::deferred_signal();
    let (reply, _) = host::deferred_signal();
    let (mut client, _server) = pair(slot(), request, reply).expect("a fresh slot");
    assert_eq!(
        client.post(Square {
            value: 1,
            answer: None
        }),
        Ok(())
    );
    assert_eq!(
        client.post(Square {
            value: 2,
            answer: None
        }),
        Err(Occupied(Square {
            value: 2,
            answer: None
        }))
    );
}

#[test]
fn an_unanswered_request_ends_in_a_terminal_expiry_at_its_deadline() {
    let (request, _) = host::deferred_signal();
    let (reply, _) = host::deferred_signal();
    let (mut client, _server) = pair(slot(), request, reply).expect("a fresh slot");
    let clock = VirtualClock::<4>::new();
    let deadline = clock
        .deadline_after(Duration::from_millis(10))
        .expect("in range");
    let (wakes, waker) = task();
    let mut cx = Context::from_waker(&waker);
    assert_eq!(
        client.post(Square {
            value: 5,
            answer: None
        }),
        Ok(())
    );
    let mut answering = pin!(client.answer(&clock, deadline));
    assert_eq!(answering.as_mut().poll(&mut cx), Poll::Pending);
    let _ = clock.advance(Duration::from_millis(9));
    assert_eq!(answering.as_mut().poll(&mut cx), Poll::Pending);
    let _ = clock.advance(Duration::from_millis(1));
    assert!(
        wakes.0.load(Ordering::Relaxed) >= 1,
        "the deadline woke the client"
    );
    assert_eq!(
        answering.as_mut().poll(&mut cx),
        Poll::Ready(Err(PeerError::Expired))
    );
}

#[test]
fn a_slot_pairs_once() {
    let slot = slot();
    let (request, _) = host::deferred_signal();
    let (reply, _) = host::deferred_signal();
    let first = pair(slot, request, reply);
    assert!(first.is_some());
    assert!(pair(slot, request, reply).is_none());
}

#[test]
fn a_client_polled_outside_its_executor_is_refused() {
    let (request, _) = host::deferred_signal();
    let reply = host::foreign_signal();
    let (mut client, _server) = pair(slot(), request, reply).expect("a fresh slot");
    let clock = VirtualClock::<4>::new();
    let deadline = clock
        .deadline_after(Duration::from_millis(1))
        .expect("in range");
    let (_, waker) = task();
    let mut cx = Context::from_waker(&waker);
    let mut answering = pin!(client.answer(&clock, deadline));
    assert!(matches!(
        answering.as_mut().poll(&mut cx),
        Poll::Ready(Err(PeerError::Foreign(_)))
    ));
}

#[test]
fn serving_one_request_keeps_its_callback_across_a_raise_without_a_post() {
    let (request, raiser) = host::signal();
    let (reply, _) = host::deferred_signal();
    let (mut client, mut server) = pair(slot(), request, reply).expect("a fresh slot");
    let (_, waker) = task();
    let mut cx = Context::from_waker(&waker);
    let mut calls = 0_u32;
    {
        let mut serving = pin!(server.serve_one(|exchange| {
            calls += 1;
            square(exchange);
        }));
        assert_eq!(serving.as_mut().poll(&mut cx), Poll::Pending);
        // A raise that announces no post answers nothing.
        assert!(raiser.raise(super::POSTED));
        assert_eq!(serving.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(
            client.post(Square {
                value: 6,
                answer: None
            }),
            Ok(())
        );
        assert_eq!(serving.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    }
    assert_eq!(calls, 1);
    assert_eq!(
        client.client.try_take_if(|_| Ok::<(), ()>(())),
        Some(Ok(Square {
            value: 6,
            answer: Some(36)
        }))
    );
}
