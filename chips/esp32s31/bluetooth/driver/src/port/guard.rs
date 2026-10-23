//! Cancel and deadline checks precede every poll that can publish hardware.
use super::Fault;
use core::{
    future::{Future, poll_fn},
    pin::pin,
    sync::atomic::{AtomicBool, Ordering},
    task::Poll,
};
use embassy_futures::select::{Either, select};
use oer_time::{Instant, Timer};

pub(super) async fn guard<T: Timer, F: Future>(
    cancel: &AtomicBool,
    timer: &T,
    deadline: Instant,
    future: F,
) -> Result<F::Output, Fault> {
    let mut future = pin!(future);
    let poll = poll_fn(|cx| {
        if cancel.load(Ordering::Acquire) {
            Poll::Ready(Err(Fault::Abandoned))
        } else if timer.now() >= deadline {
            Poll::Ready(Err(Fault::Deadline))
        } else {
            future.as_mut().poll(cx).map(Ok)
        }
    });
    match select(poll, timer.wait_until(deadline)).await {
        Either::First(result) => result,
        Either::Second(()) => Err(Fault::Deadline),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::{
        cell::Cell,
        task::{Context, Waker},
    };
    use oer_time::{Clock, Duration};
    use oer_time_virtual::VirtualClock;

    #[test]
    fn cancellation_never_polls_the_hardware_future_again() {
        let timer = VirtualClock::<8>::new();
        let cancel = AtomicBool::new(false);
        let polls = Cell::new(0);
        let deadline = timer
            .now()
            .checked_add(Duration::from_micros(10))
            .expect("test deadline");
        let hardware = poll_fn(|_| {
            polls.set(polls.get() + 1);
            Poll::<()>::Pending
        });
        let mut wait = pin!(guard(&cancel, &timer, deadline, hardware));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(wait.as_mut().poll(&mut cx).is_pending());
        assert_eq!(polls.get(), 1);
        cancel.store(true, Ordering::Release);
        assert_eq!(
            wait.as_mut().poll(&mut cx),
            Poll::Ready(Err(Fault::Abandoned))
        );
        assert_eq!(polls.get(), 1);
    }
    #[test]
    fn expired_deadline_precedes_even_an_immediately_ready_publication() {
        let timer = VirtualClock::<8>::new();
        let cancel = AtomicBool::new(false);
        let polls = Cell::new(0);
        let hardware = poll_fn(|_| {
            polls.set(polls.get() + 1);
            Poll::Ready(())
        });
        let mut wait = pin!(guard(&cancel, &timer, timer.now(), hardware));
        assert_eq!(
            wait.as_mut().poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Err(Fault::Deadline))
        );
        assert_eq!(polls.get(), 0);
    }
}
