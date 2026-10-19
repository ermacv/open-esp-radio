//! Exercise the target's real event completion future with a host waker.
use oer_hil_agent::console::progress;
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Wake, Waker},
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
    assert!(first.as_mut().poll(&mut cx).is_ready());
    assert!(second.as_mut().poll(&mut cx).is_ready());
}

#[test]
fn already_written_and_wrapped_sequences_need_no_new_notification() {
    let events = progress::SerializedEvents::new();
    events.publish_next(0);
    let mut done = std::pin::pin!(events.wait_for(u32::MAX));
    assert!(
        done.as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_ready()
    );
}

use oer_hil_agent::console::writer;

#[test]
fn protocol_writer_waits_for_release_without_busy_or_stale_self_wakes() {
    let writer = writer::Writer::new();
    let text = writer.try_acquire().unwrap();
    let wakes = Arc::new(Wakes::default());
    let waker = Waker::from(Arc::clone(&wakes));
    let mut cx = Context::from_waker(&waker);
    let mut waiting = std::pin::pin!(writer.acquire_async());
    assert!(waiting.as_mut().poll(&mut cx).is_pending());
    assert!(
        writer.try_acquire().is_none(),
        "new text cannot bypass admitted protocol work"
    );
    assert_eq!(wakes.0.load(Ordering::Relaxed), 0);
    drop(text);
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    let std::task::Poll::Ready(protocol) = waiting.as_mut().poll(&mut cx) else {
        panic!("release must admit the writer");
    };
    drop(protocol);
    drop(writer.try_acquire().unwrap());
    assert_eq!(
        wakes.0.load(Ordering::Relaxed),
        1,
        "no notifications after the waiter acquires ownership"
    );
}

#[test]
fn cancelled_async_writer_releases_priority_without_releasing_another_owner() {
    let writer = writer::Writer::new();
    let held = writer.try_acquire().unwrap();
    let mut waiting = Box::pin(writer.acquire_async());
    assert!(
        waiting
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    drop(waiting);
    assert!(
        writer.try_acquire().is_none(),
        "the active owner remains exclusive"
    );
    drop(held);
    assert!(
        writer.try_acquire().is_some(),
        "cancelled priority must not poison output"
    );
}
