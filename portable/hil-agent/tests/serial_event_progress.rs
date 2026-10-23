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
