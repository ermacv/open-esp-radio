use super::*;
use core::{
    future::Future,
    task::{Context, Poll},
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use std::{
    sync::{Arc, atomic::AtomicUsize},
    task::{Wake, Waker},
};

#[derive(Default)]
struct WakeCount(AtomicUsize);
impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn cancellation_does_not_lose_the_stop_and_duplicate_stops_coalesce() {
    let control = Control::<NoopRawMutex>::new();
    let wakes = Arc::new(WakeCount::default());
    let waker = Waker::from(wakes.clone());
    let mut cx = Context::from_waker(&waker);
    {
        let mut wait = core::pin::pin!(control.wait_stop());
        assert!(wait.as_mut().poll(&mut cx).is_pending());
    }
    control.request_stop();
    control.request_stop();
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    assert!(control.stop_requested());
    let mut wait = core::pin::pin!(control.wait_stop());
    assert_eq!(wait.as_mut().poll(&mut cx), Poll::Ready(()));
    let mut again = core::pin::pin!(control.wait_stop());
    assert_eq!(
        again.as_mut().poll(&mut cx),
        Poll::Ready(()),
        "the stop stays latched"
    );
}
