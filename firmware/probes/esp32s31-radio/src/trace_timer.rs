//! Requested-time executor for the sequential compiled PHY probes.
use core::{
    cell::{Cell, RefCell},
    future::{Future, poll_fn},
    pin::pin,
    task::{Poll, Waker},
};
use oer_time::{Clock, Duration, Instant, Timer};

pub(crate) struct TraceTimer {
    now: Cell<Instant>,
    deadline: Cell<Option<Instant>>,
    waiter: RefCell<Option<Waker>>,
    deliver: fn(u32),
    observation_tick: bool,
}

impl TraceTimer {
    pub(crate) fn new(deliver: fn(u32), observation_tick: bool) -> Self {
        Self {
            now: Cell::new(Instant::EPOCH),
            deadline: Cell::new(None),
            waiter: RefCell::new(None),
            deliver,
            observation_tick,
        }
    }

    /// Drive one sequential production operation. A future registers its wait
    /// and returns Pending before this executor delivers time and wakes it.
    pub(crate) fn run<F: Future>(&self, future: F) -> F::Output {
        let mut future = pin!(future);
        embassy_futures::block_on(poll_fn(|cx| {
            self.deadline.set(None);
            self.waiter.borrow_mut().take();
            let result = future.as_mut().poll(cx);
            if result.is_pending() {
                self.deliver_deadline();
            }
            result
        }))
    }

    fn deliver_deadline(&self) {
        if let Some(deadline) = self.deadline.take() {
            if let Some(interval) = deadline.checked_duration_since(self.now.get())
                && interval != Duration::ZERO
            {
                (self.deliver)(u32::try_from(interval.as_micros()).expect("bounded trace wait"));
                self.now.set(deadline);
            }
            if let Some(waiter) = self.waiter.borrow_mut().take() {
                waiter.wake();
            }
        }
    }
}

impl Clock for TraceTimer {
    fn now(&self) -> Instant {
        let now = self.now.get();
        // Direct, timer-free hardware observations need an elapsed bound too.
        if self.observation_tick {
            self.now.set(
                now.checked_add(Duration::from_micros(1))
                    .expect("bounded trace time"),
            );
        }
        now
    }
}

impl Timer for TraceTimer {
    fn wait_until(&self, deadline: Instant) -> impl Future<Output = ()> {
        poll_fn(move |cx| {
            if self.now.get() >= deadline {
                Poll::Ready(())
            } else {
                self.deadline.set(Some(deadline));
                self.waiter.replace(Some(cx.waker().clone()));
                Poll::Pending
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::task::Context;
    use oer_time_virtual::VirtualClock;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::task::Wake;

    struct Wakes(AtomicUsize);
    impl Wake for Wakes {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn wait_is_pending_until_deadline_delivery_then_wakes_and_resumes() {
        let reference = VirtualClock::<1>::starting_at(Instant::EPOCH);
        let deadline = reference
            .now()
            .checked_add(Duration::from_micros(7))
            .expect("test deadline");
        let timer = TraceTimer::new(|micros| assert_eq!(micros, 7), false);
        let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
        let waker = Waker::from(wakes.clone());
        let mut cx = Context::from_waker(&waker);
        let mut wait = pin!(timer.wait_until(deadline));
        assert_eq!(timer.now(), reference.now());
        assert!(wait.as_mut().poll(&mut cx).is_pending());
        assert!(wait.as_mut().poll(&mut cx).is_pending());
        assert_eq!(timer.now(), reference.now());
        assert_eq!(wakes.0.load(Ordering::Relaxed), 0);
        reference.advance_to(deadline);
        timer.deliver_deadline();
        assert_eq!(timer.now(), reference.now());
        assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
        assert!(wait.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn executor_delivers_only_after_the_child_returns_pending() {
        let timer = TraceTimer::new(|micros| assert_eq!(micros, 3), false);
        let deadline = Instant::EPOCH
            .checked_add(Duration::from_micros(3))
            .expect("test deadline");
        let polls = Cell::new(0);
        let mut wait = pin!(timer.wait_until(deadline));
        timer.run(poll_fn(|cx| {
            polls.set(polls.get() + 1);
            let result = wait.as_mut().poll(cx);
            if polls.get() == 1 {
                assert!(result.is_pending());
                assert_eq!(timer.now(), Instant::EPOCH);
            } else {
                assert!(result.is_ready());
                assert_eq!(timer.now(), deadline);
            }
            result
        }));
        assert_eq!(polls.get(), 2);
    }

    #[test]
    fn unpolled_cancelled_and_elapsed_waits_do_not_deliver_time() {
        let timer = TraceTimer::new(|_| panic!("unexpected delivery"), false);
        let deadline = Instant::EPOCH
            .checked_add(Duration::from_micros(3))
            .expect("test deadline");
        drop(timer.wait_until(deadline));
        let mut cx = Context::from_waker(Waker::noop());
        {
            let mut cancelled = pin!(timer.wait_until(deadline));
            assert!(cancelled.as_mut().poll(&mut cx).is_pending());
        }
        timer.run(core::future::ready(()));
        assert_eq!(timer.now(), Instant::EPOCH);
        timer.run(timer.wait_until(Instant::EPOCH));
        assert_eq!(timer.now(), Instant::EPOCH);
    }
}
