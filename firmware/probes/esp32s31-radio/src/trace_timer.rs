//! Requested-time executor for the sequential compiled PHY probes.
use core::{
    cell::Cell,
    future::{Future, poll_fn},
    pin::pin,
};
use oer_time::{Clock, Duration, Instant, Timer};
use oer_time_virtual::VirtualClock;

pub(crate) struct TraceTimer {
    clock: VirtualClock<1>,
    deliver: fn(u32),
    observation_tick: bool,
    observation_reads: Cell<u8>,
    zero_time_queries: u8,
}

impl TraceTimer {
    pub(crate) fn new(deliver: fn(u32), observation_tick: bool) -> Self {
        Self {
            clock: VirtualClock::starting_at(Instant::EPOCH),
            deliver,
            observation_tick,
            observation_reads: Cell::new(0),
            zero_time_queries: 0,
        }
    }

    /// RFPLL compares the full requested settle in a zero-cost ready poll.
    /// A finite query prefix keeps direct stuck observations time-bounded.
    pub(crate) fn with_zero_time_polls(mut self) -> Self {
        self.zero_time_queries = 64;
        self
    }

    /// Drive one sequential production operation. A future registers its wait
    /// and returns Pending before this executor delivers time and wakes it.
    pub(crate) fn run<F: Future>(&self, future: F) -> F::Output {
        let mut future = pin!(future);
        embassy_futures::block_on(poll_fn(|cx| {
            // Ready work has zero cost in this requested-time environment.
            // Restart the stuck-observation guard for each executor poll.
            self.observation_reads.set(0);
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
        // Normal ready polls consume no modeled time. A stuck synchronous
        // observation still needs a progressing clock to reach its timeout.
        // This guard is not an execution-cost estimate: after the finite
        // zero-time prefix, every query advances one microsecond. Explicit
        // delays remain delivered verbatim; none is filtered or synthesized.
        if self.observation_tick {
            let reads = self.observation_reads.get();
            if reads < self.zero_time_queries {
                self.observation_reads.set(reads + 1);
            } else {
                self.clock
                    .advance(Duration::from_micros(1))
                    .expect("bounded trace time");
            }
        }
        now
    }
}

impl Timer for TraceTimer {
    fn wait_until(&self, deadline: Instant) -> impl Future<Output = ()> {
        self.clock.wait_until(deadline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::task::Poll;

    #[test]
    fn rfpll_ready_polls_do_not_spend_the_requested_settle() {
        let timer = TraceTimer::new(|_| {}, true).with_zero_time_polls();
        let mut polls = 0;
        timer.run(poll_fn(|cx| {
            for _ in 0..64 {
                assert_eq!(timer.now(), Instant::EPOCH);
            }
            polls += 1;
            if polls == 2 {
                Poll::Ready(())
            } else {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }));
        assert_eq!(polls, 2);
    }

    #[test]
    fn a_stuck_rfpll_poll_still_observes_the_full_timeout() {
        let timer = TraceTimer::new(|_| {}, true).with_zero_time_polls();
        let deadline = Instant::EPOCH
            .checked_add(Duration::from_micros(20))
            .expect("time fits");
        let mut queries = 0;
        timer.run(poll_fn(|_| {
            loop {
                queries += 1;
                if timer.now() >= deadline {
                    break Poll::Ready(());
                }
            }
        }));
        assert_eq!(queries, 85);
    }

    #[test]
    fn ordinary_observation_probes_keep_their_existing_tick() {
        let timer = TraceTimer::new(|_| {}, true);
        assert_eq!(timer.now(), Instant::EPOCH);
        assert_eq!(timer.now(), Instant::from_micros(1));
    }
}

// The sequential probe executor has no peer tasks and deliberately preserves
// one checkpoint per atom; requested-time traces never model CPU cost.
impl oer_cooperate::CooperativeTimer for TraceTimer {
    fn should_yield(&self) -> bool {
        true
    }
}
