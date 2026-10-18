use super::*;
use core::{
    cell::Cell,
    task::{Context, Poll, Waker},
};

#[test]
fn observation_preserves_result_and_separates_poll_from_suspension() {
    let counters = Counters::new();
    let now = Cell::new(100);
    let mut attempts = 0;
    let work = core::future::poll_fn(|cx| {
        assert!(cx.waker().will_wake(Waker::noop()));
        now.set(now.get() + 3);
        attempts += 1;
        if attempts < 3 {
            Poll::Pending
        } else {
            Poll::Ready(42)
        }
    });
    let mut observed = core::pin::pin!(counters.observe(work, || now.get()));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(observed.as_mut().poll(&mut cx).is_pending());
    now.set(200);
    assert!(observed.as_mut().poll(&mut cx).is_pending());
    now.set(300);
    assert_eq!(observed.as_mut().poll(&mut cx), Poll::Ready(42));
    let result = counters.snapshot();
    assert_eq!(
        (
            result.polls,
            result.pending,
            result.completed,
            result.cancelled
        ),
        (3, 2, 1, 0)
    );
    assert_eq!(result.poll_micros, 9);
    assert_eq!(result.suspended_micros, 194);
}

#[test]
fn session_timeout_retains_the_last_blocked_send() {
    let counters = Counters::new();
    let now = Cell::new(100);
    {
        let mut observed =
            core::pin::pin!(counters.observe(core::future::pending::<()>(), || now.get()));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(observed.as_mut().poll(&mut cx).is_pending());
        now.set(500);
    }
    let result = counters.snapshot();
    assert_eq!(
        (
            result.polls,
            result.pending,
            result.completed,
            result.cancelled
        ),
        (1, 1, 0, 1)
    );
    assert_eq!(result.suspended_micros, 400);
}
