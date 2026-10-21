//! Requested-time executor for the sequential compiled PHY probes.
use core::{
    future::{Future, poll_fn},
    pin::pin,
};
use oer_time::{Clock, Duration, Instant, Timer};
use oer_time_virtual::VirtualClock;

pub(crate) struct TraceTimer {
    clock: VirtualClock<1>,
    deliver: fn(u32),
    observation_tick: bool,
}

impl TraceTimer {
    pub(crate) fn new(deliver: fn(u32), observation_tick: bool) -> Self {
        Self {
            clock: VirtualClock::starting_at(Instant::EPOCH),
            deliver,
            observation_tick,
        }
    }

    /// Drive one sequential production operation. A future registers its wait
    /// and returns Pending before this executor delivers time and wakes it.
    pub(crate) fn run<F: Future>(&self, future: F) -> F::Output {
        let mut future = pin!(future);
        embassy_futures::block_on(poll_fn(|cx| {
            let result = future.as_mut().poll(cx);
            if result.is_pending()
                && let Some(deadline) = self.clock.next_deadline()
            {
                let interval = deadline.saturating_duration_since(self.clock.now());
                if interval != Duration::ZERO {
                    (self.deliver)(
                        u32::try_from(interval.as_micros()).expect("bounded trace wait"),
                    );
                }
                self.clock.advance_to_next_deadline();
            }
            result
        }))
    }
}

impl Clock for TraceTimer {
    fn now(&self) -> Instant {
        let now = self.clock.now();
        // Direct, timer-free hardware observations need an elapsed bound too.
        if self.observation_tick {
            self.clock
                .advance(Duration::from_micros(1))
                .expect("bounded trace time");
        }
        now
    }
}

impl Timer for TraceTimer {
    fn wait_until(&self, deadline: Instant) -> impl Future<Output = ()> {
        self.clock.wait_until(deadline)
    }
}
