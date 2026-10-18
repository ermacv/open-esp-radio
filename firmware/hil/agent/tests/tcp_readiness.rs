//! Test the production accept/readiness ordering without a hardware timer.
use oer_hil_agent::traffic::connection;
use std::{
    cell::Cell,
    future::{Future, poll_fn},
    task::{Context, Poll, Waker},
};

#[test]
fn listener_is_started_before_ready_and_the_same_accept_finishes() {
    let listening = Cell::new(false);
    let connected = Cell::new(false);
    let ready = Cell::new(false);
    let accept = poll_fn(|_| {
        listening.set(true);
        if connected.get() {
            Poll::Ready(Ok::<_, ()>(()))
        } else {
            Poll::Pending
        }
    });
    let mut future = std::pin::pin!(connection::before_ready(accept, async {
        assert!(
            listening.get(),
            "the peer must be able to connect when readiness is published"
        );
        ready.set(true);
    }));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(future.as_mut().poll(&mut cx).is_pending());
    assert!(ready.get());
    connected.set(true);
    assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
}

#[test]
fn failed_listen_does_not_advertise_readiness() {
    let ready = Cell::new(false);
    let mut future = std::pin::pin!(connection::before_ready(
        std::future::ready(Err("listen failed")),
        async {
            ready.set(true);
        }
    ));
    assert_eq!(
        future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Err("listen failed"))
    );
    assert!(!ready.get());
}

#[test]
fn early_connection_waits_for_ready_publication() {
    let announced = Cell::new(false);
    let mut future = std::pin::pin!(connection::before_ready(
        std::future::ready(Ok::<_, ()>(())),
        poll_fn(|_| if announced.get() {
            Poll::Ready(())
        } else {
            Poll::Pending
        })
    ));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(future.as_mut().poll(&mut cx).is_pending());
    announced.set(true);
    assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
}
