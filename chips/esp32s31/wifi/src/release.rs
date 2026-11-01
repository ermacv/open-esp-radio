//! The release answer's wait, bounded by the stop's deadline.

use core::{
    future::Future,
    task::{Context, Poll},
};
use oer_time::{Instant, Timer};

/// Wait for `answer` until `deadline`, which is checked before each poll of
/// it: once the deadline has passed `answer` is not polled again, so an
/// answer ready only after it stays where it is, and the wait ends with
/// `None`. An answer taken by a poll that began before the deadline is
/// returned. The answer's owner keeps what it was handed; nothing here
/// takes it back.
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
        if alarm.as_mut().poll(cx).is_ready() {
            return Poll::Ready(None);
        }
        answer(cx).map(Some)
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
    fn an_answer_ready_only_after_the_deadline_is_not_read() {
        let clock = VirtualClock::<2>::new();
        let ready = Cell::new(false);
        let polls = Cell::new(0);
        let taken = Cell::new(0);
        let deadline = Some(clock.now().saturating_add(Duration::from_millis(10)));
        let mut wait = core::pin::pin!(within(&clock, deadline, |_| {
            polls.set(polls.get() + 1);
            if ready.get() {
                taken.set(taken.get() + 1);
                Poll::Ready(7)
            } else {
                Poll::Pending
            }
        }));
        assert!(poll(wait.as_mut()).is_pending());
        assert_eq!(polls.get(), 1);
        // Both happen before the next poll: the deadline wins.
        clock.advance(Duration::from_millis(10)).unwrap();
        ready.set(true);
        assert_eq!(poll(wait.as_mut()), Poll::Ready(None));
        assert_eq!(
            polls.get(),
            1,
            "the answer is not polled after the deadline"
        );
        assert_eq!(taken.get(), 0, "the late answer stays in its slot");
    }

    #[test]
    fn a_deadline_that_passed_before_the_wait_reads_no_answer() {
        let clock = VirtualClock::<2>::new();
        clock.advance(Duration::from_millis(5)).unwrap();
        let deadline = Some(clock.now());
        let taken = Cell::new(0);
        let mut wait = core::pin::pin!(within(&clock, deadline, |_| {
            taken.set(taken.get() + 1);
            Poll::Ready(7)
        }));
        assert_eq!(poll(wait.as_mut()), Poll::Ready(None));
        assert_eq!(taken.get(), 0);
    }
}
