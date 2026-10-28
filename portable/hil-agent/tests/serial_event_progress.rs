#![cfg_attr(test, allow(clippy::disallowed_macros))]
//! Exercise the target's real event completion future with a host waker.
use oer_hil_agent::console::progress;
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};

#[derive(Default)]
struct Wakes(AtomicUsize);
impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn serialization_wait_sleeps_and_accepts_progress_past_its_own_event() {
    let events = progress::SerializedEvents::new();
    let wakes = Arc::new(Wakes::default());
    let waker = Waker::from(Arc::clone(&wakes));
    let mut cx = Context::from_waker(&waker);
    let mut first = std::pin::pin!(events.wait_for(3));
    let mut second = std::pin::pin!(events.wait_for(4));
    assert!(first.as_mut().poll(&mut cx).is_pending());
    assert!(second.as_mut().poll(&mut cx).is_pending());
    assert_eq!(
        wakes.0.load(Ordering::Relaxed),
        0,
        "pending work must not wake itself"
    );
    events.publish_next(6);
    assert!(wakes.0.load(Ordering::Relaxed) > 0);
    assert_eq!(first.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    assert_eq!(second.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
}

#[test]
fn already_written_and_wrapped_sequences_need_no_new_notification() {
    let events = progress::SerializedEvents::new();
    events.publish_next(0);
    let mut done = std::pin::pin!(events.wait_for(u32::MAX));
    let waker = Waker::from(Arc::new(Wakes::default()));
    assert_eq!(
        done.as_mut().poll(&mut Context::from_waker(&waker)),
        Poll::Ready(Ok(()))
    );
}

#[test]
fn seventeenth_wait_reports_its_source_and_does_not_steal_an_existing_receiver() {
    let events = progress::SerializedEvents::new();
    events.publish_next(41);
    let waker = Waker::from(Arc::new(Wakes::default()));
    let mut cx = Context::from_waker(&waker);
    let mut waiting: Vec<_> = (0..16).map(|_| Box::pin(events.wait_for(99))).collect();
    for future in &mut waiting {
        assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
    }
    let mut refused = Box::pin(events.wait_for(123));
    assert_eq!(
        refused.as_mut().poll(&mut cx),
        Poll::Ready(Err(progress::ReceiverLimit {
            sequence: 123,
            next: Some(41),
            capacity: 16,
        }))
    );
    // A refused registration consumes no slot, and cancellation releases one.
    waiting.pop();
    let mut admitted = Box::pin(events.wait_for(123));
    assert_eq!(admitted.as_mut().poll(&mut cx), Poll::Pending);
    events.publish_next(124);
    assert_eq!(admitted.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    for future in &mut waiting {
        assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    }
}
