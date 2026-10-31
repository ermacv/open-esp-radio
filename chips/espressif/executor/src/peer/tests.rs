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

use super::{PeerError, PostError, pair};

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
        client.post(
            Square {
                value: 7,
                answer: None
            },
            deadline
        ),
        Ok(())
    );
    {
        let mut answering = pin!(client.answer(&clock));
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
        client.post(
            Square {
                value: 3,
                answer: None
            },
            deadline
        ),
        Ok(())
    );
}

#[test]
fn a_second_post_before_the_answer_returns_its_request() {
    let (request, _) = host::deferred_signal();
    let (reply, _) = host::deferred_signal();
    let (mut client, _server) = pair(slot(), request, reply).expect("a fresh slot");
    let clock = VirtualClock::<4>::new();
    let deadline = clock
        .deadline_after(Duration::from_millis(10))
        .expect("in range");
    assert_eq!(
        client.post(
            Square {
                value: 1,
                answer: None
            },
            deadline
        ),
        Ok(())
    );
    assert_eq!(
        client.post(
            Square {
                value: 2,
                answer: None
            },
            deadline
        ),
        Err(PostError::Occupied(Square {
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
        client.post(
            Square {
                value: 5,
                answer: None
            },
            deadline
        ),
        Ok(())
    );
    let mut answering = pin!(client.answer(&clock));
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
        Poll::Ready(Err(PeerError::Expired { deadline }))
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
    assert_eq!(
        client.post(
            Square {
                value: 1,
                answer: None
            },
            deadline
        ),
        Ok(())
    );
    let mut answering = pin!(client.answer(&clock));
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
        let clock = VirtualClock::<4>::new();
        let deadline = clock
            .deadline_after(Duration::from_millis(10))
            .expect("in range");
        assert_eq!(
            client.post(
                Square {
                    value: 6,
                    answer: None
                },
                deadline
            ),
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

/// A client with one posted request, its server and the request's deadline
/// 10 ms from the clock's start.
fn posted(
    clock: &VirtualClock<4>,
) -> (
    super::PeerClient<Square, host::Deferred, host::Host>,
    super::PeerServer<Square, host::Deferred, host::Host>,
    oer_time::Instant,
) {
    let (request, _) = host::deferred_signal();
    let (reply, _) = host::signal();
    let (mut client, server) = pair(slot(), request, reply).expect("a fresh slot");
    let deadline = clock
        .deadline_after(Duration::from_millis(10))
        .expect("in range");
    assert_eq!(
        client.post(
            Square {
                value: 4,
                answer: None
            },
            deadline
        ),
        Ok(())
    );
    (client, server, deadline)
}

#[test]
fn an_answer_published_after_the_deadline_is_refused() {
    let clock = VirtualClock::<4>::new();
    let (mut client, mut server, deadline) = posted(&clock);
    let (_, waker) = task();
    let mut cx = Context::from_waker(&waker);
    // The server answers at 11 ms, before the client's task runs again.
    let _ = clock.advance(Duration::from_millis(11));
    {
        let mut serving = pin!(server.serve_one(square));
        assert_eq!(serving.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    }
    let mut answering = pin!(client.answer(&clock));
    assert_eq!(
        answering.as_mut().poll(&mut cx),
        Poll::Ready(Err(PeerError::Expired { deadline }))
    );
}

#[test]
fn an_answer_taken_at_the_deadline_is_refused() {
    let clock = VirtualClock::<4>::new();
    let (mut client, mut server, deadline) = posted(&clock);
    let (_, waker) = task();
    let mut cx = Context::from_waker(&waker);
    let _ = clock.advance(Duration::from_millis(10));
    {
        let mut serving = pin!(server.serve_one(square));
        assert_eq!(serving.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    }
    let mut answering = pin!(client.answer(&clock));
    assert_eq!(
        answering.as_mut().poll(&mut cx),
        Poll::Ready(Err(PeerError::Expired { deadline }))
    );
}

#[test]
fn an_expired_client_never_returns_the_late_answer_and_posts_nothing() {
    let clock = VirtualClock::<4>::new();
    let (mut client, mut server, deadline) = posted(&clock);
    let (_, waker) = task();
    let mut cx = Context::from_waker(&waker);
    let _ = clock.advance(Duration::from_millis(10));
    {
        let mut answering = pin!(client.answer(&clock));
        assert_eq!(
            answering.as_mut().poll(&mut cx),
            Poll::Ready(Err(PeerError::Expired { deadline }))
        );
    }
    // The answer arrives late; the quarantined client never returns it.
    {
        let mut serving = pin!(server.serve_one(square));
        assert_eq!(serving.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    }
    for _ in 0..2 {
        let mut answering = pin!(client.answer(&clock));
        assert_eq!(
            answering.as_mut().poll(&mut cx),
            Poll::Ready(Err(PeerError::Expired { deadline }))
        );
    }
    let later = clock
        .deadline_after(Duration::from_millis(10))
        .expect("in range");
    assert_eq!(
        client.post(
            Square {
                value: 2,
                answer: None
            },
            later
        ),
        Err(PostError::Expired {
            request: Square {
                value: 2,
                answer: None
            },
            deadline
        })
    );
}

#[test]
fn a_cancelled_wait_keeps_the_request_and_its_deadline() {
    let clock = VirtualClock::<4>::new();
    let (mut client, _server, deadline) = posted(&clock);
    let (_, waker) = task();
    let mut cx = Context::from_waker(&waker);
    {
        let mut answering = pin!(client.answer(&clock));
        assert_eq!(answering.as_mut().poll(&mut cx), Poll::Pending);
    }
    // The wait was dropped: the request still occupies the client, and the
    // next wait resumes its deadline.
    assert!(matches!(
        client.post(
            Square {
                value: 3,
                answer: None
            },
            deadline
        ),
        Err(PostError::Occupied(_))
    ));
    let _ = clock.advance(Duration::from_millis(10));
    let mut answering = pin!(client.answer(&clock));
    assert_eq!(
        answering.as_mut().poll(&mut cx),
        Poll::Ready(Err(PeerError::Expired { deadline }))
    );
}

#[test]
fn a_wait_without_a_posted_request_is_refused() {
    let (request, _) = host::deferred_signal();
    let (reply, _) = host::signal();
    let (mut client, _server) = pair(slot(), request, reply).expect("a fresh slot");
    let clock = VirtualClock::<4>::new();
    let (_, waker) = task();
    let mut cx = Context::from_waker(&waker);
    let mut answering = pin!(client.answer(&clock));
    assert_eq!(
        answering.as_mut().poll(&mut cx),
        Poll::Ready(Err(PeerError::NotPosted))
    );
}
