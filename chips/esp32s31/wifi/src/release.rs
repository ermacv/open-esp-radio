//! The release answer's wait, bounded by the stop's deadline.

use core::{
    future::Future,
    task::{Context, Poll},
};
use oer_time::{Instant, Timer};

/// Wait for `answer` until `deadline`, which is checked before and after
/// each poll of it: an answer that becomes ready only after the deadline is
/// not taken, and the wait ends with `None`. The answer's owner keeps what
/// it was handed; nothing here takes it back.
pub(crate) async fn within<A>(
    timer: &impl Timer,
    deadline: Option<Instant>,
    mut answer: impl FnMut(&mut Context<'_>) -> Poll<A>,
) -> Option<A> {
    let mut alarm = core::pin::pin!(async {
        match deadline {
            Some(deadline) => timer.wait_until(deadline).await,
            None => core::future::pending().await,
        }
    });
    core::future::poll_fn(|cx| {
        let expired = alarm.as_mut().poll(cx).is_ready();
        let ready = answer(cx);
        if expired || alarm.as_mut().poll(cx).is_ready() {
            return Poll::Ready(None);
        }
        ready.map(Some)
    })
    .await
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, reason = "test assertions")]
mod tests {
    use super::*;
    use core::cell::Cell;
    use oer_time::{Clock as _, Duration};
    use oer_time_virtual::VirtualClock;

    fn poll<F: Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
        future.poll(&mut Context::from_waker(core::task::Waker::noop()))
    }

    #[test]
    fn an_answer_in_time_is_taken_once() {
        let clock = VirtualClock::<2>::new();
        let ready = Cell::new(false);
        let taken = Cell::new(0);
        let deadline = Some(clock.now().saturating_add(Duration::from_millis(10)));
        let mut wait = core::pin::pin!(within(&clock, deadline, |_| {
            if ready.get() {
                taken.set(taken.get() + 1);
                Poll::Ready(7)
            } else {
                Poll::Pending
            }
        }));
        assert!(poll(wait.as_mut()).is_pending());
        ready.set(true);
        assert_eq!(poll(wait.as_mut()), Poll::Ready(Some(7)));
        assert_eq!(taken.get(), 1);
    }

    #[test]
    fn an_answer_ready_only_after_the_deadline_is_not_taken() {
        let clock = VirtualClock::<2>::new();
        let ready = Cell::new(false);
        let deadline = Some(clock.now().saturating_add(Duration::from_millis(10)));
        let mut wait = core::pin::pin!(within(&clock, deadline, |_| {
            if ready.get() {
                Poll::Ready(7)
            } else {
                Poll::Pending
            }
        }));
        assert!(poll(wait.as_mut()).is_pending());
        // Both happen before the next poll: the deadline wins.
        clock.advance(Duration::from_millis(10)).unwrap();
        ready.set(true);
        assert_eq!(poll(wait.as_mut()), Poll::Ready(None));
    }

    #[test]
    fn a_deadline_that_passed_before_the_wait_takes_no_answer() {
        let clock = VirtualClock::<2>::new();
        clock.advance(Duration::from_millis(5)).unwrap();
        let deadline = Some(clock.now());
        let mut wait = core::pin::pin!(within(&clock, deadline, |_| Poll::Ready(7)));
        assert_eq!(poll(wait.as_mut()), Poll::Ready(None));
    }
}
