//! The schedule of paced transmission: datagram `n` is due `n` payload
//! times after the start, the payload time being its UDP payload bits over
//! the rate. The sender wakes on the Embassy timer, sends what is due in
//! batches and sleeps at least [`GRANULARITY_US`] between them. A datagram
//! the socket has no room for waits for room: the schedule moves on only
//! with a datagram the socket took, and once room came, a schedule further
//! behind than one batch's sleep or [`CATCH_UP`] payload times starts
//! again from the present. A burst's datagrams all fall due at its first
//! one's time, which keeps the rate on average.
//! Nothing is sent once the window ended: what the window held and was not
//! sent by then is missed.

/// The shortest sleep between two batches: at 65 Mbit/s of 1400-byte
/// datagrams a batch holds some six of them.
pub(crate) const GRANULARITY_US: u64 = 1000;
/// The most datagrams one batch sends before it yields to the executor:
/// the IP stack's transmit credits.
pub(crate) const MAX_BATCH: u32 = 16;
/// The datagrams an unpaced sender sends between two looks at the clock
/// and two yields: one blocked send already yields to the executor.
pub(crate) const UNPACED_BATCH: u32 = 128;
/// The most payload times a schedule runs behind before it starts again
/// from the present: a sender held back by the socket catches up with at
/// most this many datagrams at once.
pub(crate) const CATCH_UP: u64 = 4;

/// What the sender does after a batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Next {
    /// More datagrams are due: yield to the executor and send again.
    Yield,
    /// Sleep until this image microsecond.
    Wait(u64),
    /// The socket had no room for the due datagram: wait for room and
    /// send it ([`Schedule::taken`]).
    Blocked,
    /// The transmission's window ended.
    Done,
}

/// A paced transmission between two image microseconds.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Schedule {
    /// Where the window began, for [`Self::missed`].
    start_us: u64,
    /// Where the schedule's datagram 0 lies: the window's start, moved on
    /// when the schedule started again from the present.
    begin_us: u64,
    end_us: u64,
    payload: u16,
    rate_bps: u32,
    /// Datagrams the socket took.
    attempts: u64,
    /// One payload time, `payload` bits over the rate: whole microseconds
    /// and the rest, in units of 1 / `rate_bps` µs.
    step_us: u64,
    step_rest: u64,
    /// The next datagram's time after `begin_us`, whole microseconds and
    /// the rest, below `rate_bps`; `None` past `u64`.
    after_us: Option<u64>,
    after_rest: u64,
    /// The datagrams due together, and how many of the current burst are
    /// still to go; the current burst's first time after `begin_us`.
    burst: u16,
    burst_left: u16,
    burst_after_us: Option<u64>,
}

impl Schedule {
    /// The schedule of `payload`-byte datagrams at `rate_bps` from `begin_us`
    /// until `end_us`, `burst` of them due together (a `burst` of 0 counts
    /// as 1); `None` at the rate 0, which sends unpaced.
    pub(crate) fn new(
        begin_us: u64,
        end_us: u64,
        payload: u16,
        rate_bps: u32,
        burst: u16,
    ) -> Option<Self> {
        // At most 65 535 × 8 000 000 bit·µs/s: within a u64.
        let bits = u64::from(payload).saturating_mul(8_000_000);
        let rate = u64::from(rate_bps);
        Some(Self {
            start_us: begin_us,
            begin_us,
            end_us,
            payload,
            rate_bps,
            attempts: 0,
            step_us: bits.checked_div(rate)?,
            step_rest: bits.checked_rem(rate)?,
            after_us: Some(0),
            after_rest: 0,
            burst: burst.max(1),
            burst_left: burst.max(1),
            burst_after_us: Some(0),
        })
    }

    /// When the next datagram is due, before the window ends: datagram `n`
    /// at `begin_us` plus `n` payload times, rounded down, or its burst's
    /// first datagram's time.
    fn due(&self) -> Option<u64> {
        self.begin_us
            .checked_add(self.burst_after_us?)
            .filter(|due| *due < self.end_us)
    }

    /// The socket took the due datagram.
    fn sent_one(&mut self) {
        self.attempts = self.attempts.saturating_add(1);
        self.advance();
    }

    /// The next datagram's time: one payload time later, its rest carried
    /// into a whole microsecond once it reaches the rate.
    fn advance(&mut self) {
        let rate = u64::from(self.rate_bps);
        let rest = self.after_rest.saturating_add(self.step_rest);
        let (carry, rest) = if rest >= rate {
            (1, rest.saturating_sub(rate))
        } else {
            (0, rest)
        };
        self.after_rest = rest;
        self.after_us = self
            .after_us
            .and_then(|after| after.checked_add(self.step_us))
            .and_then(|after| after.checked_add(carry));
        self.burst_left = self.burst_left.saturating_sub(1);
        if self.burst_left == 0 {
            self.burst_left = self.burst;
            self.burst_after_us = self.after_us;
        }
    }

    /// Datagrams the window holds that the socket never took: the sender
    /// reached their time only after the window ended, or waited for room
    /// past them.
    pub(crate) fn missed(&self) -> u64 {
        let span = u128::from(self.end_us.saturating_sub(self.start_us));
        let per_datagram = u128::from(self.payload).saturating_mul(8_000_000);
        let held = span
            .saturating_mul(u128::from(self.rate_bps))
            .div_ceil(per_datagram.max(1));
        u64::try_from(held)
            .unwrap_or(u64::MAX)
            .saturating_sub(self.attempts)
    }

    /// The socket took the due datagram after the sender waited for room,
    /// at `now_us`: the next one is due a payload time later, or now when
    /// that lies further behind than [`GRANULARITY_US`] or [`CATCH_UP`]
    /// payload times.
    pub(crate) fn taken(&mut self, now_us: u64) {
        self.sent_one();
        let Some(due) = self
            .burst_after_us
            .and_then(|after| self.begin_us.checked_add(after))
        else {
            return;
        };
        let behind = self
            .step_us
            .saturating_add(1)
            .saturating_mul(CATCH_UP)
            .max(GRANULARITY_US);
        if now_us.saturating_sub(due) > behind {
            self.begin_us = self.begin_us.saturating_add(now_us.saturating_sub(due));
        }
    }

    /// Send, through `send`, every datagram due at `now_us`, at most
    /// [`MAX_BATCH`] of them, and none once the window ended at `now_us`;
    /// `send` says whether the socket took it, and the batch ends
    /// [`Next::Blocked`] at the first it did not.
    pub(crate) fn batch(&mut self, now_us: u64, mut send: impl FnMut() -> bool) -> Next {
        if now_us >= self.end_us {
            return Next::Done;
        }
        let mut sent = 0;
        loop {
            let Some(due) = self.due() else {
                return Next::Done;
            };
            if due > now_us {
                return Next::Wait(due.max(now_us.saturating_add(GRANULARITY_US)));
            }
            if sent == MAX_BATCH {
                return Next::Yield;
            }
            if !send() {
                return Next::Blocked;
            }
            sent += 1;
            self.sent_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(schedule: &mut Schedule, now_us: u64) -> (u32, Next) {
        let mut sent = 0;
        let next = schedule.batch(now_us, || {
            sent += 1;
            true
        });
        (sent, next)
    }

    #[test]
    fn a_rate_of_zero_has_no_schedule() {
        assert!(Schedule::new(0, 1000, 1400, 0, 1).is_none());
    }

    #[test]
    fn a_batch_sends_what_is_due_and_sleeps_at_least_the_granularity() {
        // 1400 bytes at 11.2 Mbit/s: one datagram each 1000 µs.
        let mut schedule = Schedule::new(10_000, 20_000, 1400, 11_200_000, 1).unwrap();
        assert_eq!(batch(&mut schedule, 9_999), (0, Next::Wait(10_999)));
        assert_eq!(batch(&mut schedule, 10_000), (1, Next::Wait(11_000)));
        assert_eq!(batch(&mut schedule, 13_500), (3, Next::Wait(14_500)));
        assert_eq!(batch(&mut schedule, 14_000), (1, Next::Wait(15_000)));
        assert_eq!(
            batch(&mut schedule, 30_000),
            (0, Next::Done),
            "nothing after the window"
        );
        assert_eq!(schedule.missed(), 5, "15 000 to 19 000 were due");
    }

    #[test]
    fn a_fast_rate_sends_in_batches_of_at_most_the_bound() {
        // 1400 bytes at 65 Mbit/s: one datagram each 172.3 µs.
        let mut schedule = Schedule::new(0, 1_000_000, 1400, 65_000_000, 1).unwrap();
        assert_eq!(batch(&mut schedule, 1_000), (6, Next::Wait(2_000)));
        // 53 more are due at 10 ms (0..=58): batches of 16, 16, 16 and 5.
        for _ in 0..3 {
            assert_eq!(batch(&mut schedule, 10_000), (MAX_BATCH, Next::Yield));
        }
        assert_eq!(batch(&mut schedule, 10_000), (5, Next::Wait(11_000)));
        assert_eq!(schedule.attempts, 59);
    }

    #[test]
    fn each_datagram_is_due_at_its_exact_payload_time_rounded_down() {
        // 1472 bytes at 100 Mbit/s: 117.76 µs each, whose rests carry.
        let mut schedule = Schedule::new(7, u64::MAX, 1472, 100_000_000, 1).unwrap();
        for attempt in 0..100_000_u64 {
            let exact = attempt * 1472 * 8_000_000 / 100_000_000;
            assert_eq!(schedule.due(), Some(7 + exact));
            schedule.advance();
        }
    }

    #[test]
    fn a_refused_datagram_waits_and_a_late_schedule_starts_again_from_the_present() {
        // 1400 bytes at 11.2 Mbit/s: one datagram each 1000 µs.
        let mut schedule = Schedule::new(0, 100_000, 1400, 11_200_000, 1).unwrap();
        assert_eq!(schedule.batch(0, || false), Next::Blocked);
        assert_eq!(schedule.attempts, 0, "a refused datagram is still due");
        // Room came 2.5 payload times later: the schedule catches up.
        schedule.taken(2_500);
        assert_eq!(schedule.due(), Some(1_000));
        // Room came far later: the next one is due a payload time on.
        schedule.taken(50_000);
        assert_eq!(schedule.due(), Some(50_000));
        assert_eq!(schedule.attempts, 2);
        assert_eq!(schedule.missed(), 98, "the window held 100");
    }

    #[test]
    fn a_slow_rate_sleeps_until_the_next_datagram() {
        // 100 bytes at 8 kbit/s: one datagram each 100 ms.
        let mut schedule = Schedule::new(0, 1_000_000, 100, 8_000, 1).unwrap();
        assert_eq!(batch(&mut schedule, 0), (1, Next::Wait(100_000)));
        assert_eq!(batch(&mut schedule, 950_000), (9, Next::Done));
        assert_eq!(schedule.missed(), 0);
    }

    #[test]
    fn a_burst_falls_due_together_and_keeps_the_mean_rate() {
        // 1400 bytes at 11.2 Mbit/s, four together: a burst each 4000 µs.
        let mut schedule = Schedule::new(0, 12_000, 1400, 11_200_000, 4).unwrap();
        assert_eq!(batch(&mut schedule, 0), (4, Next::Wait(4_000)));
        assert_eq!(batch(&mut schedule, 3_999), (0, Next::Wait(4_999)));
        assert_eq!(batch(&mut schedule, 4_000), (4, Next::Wait(8_000)));
        assert_eq!(batch(&mut schedule, 8_000), (4, Next::Done));
        assert_eq!(schedule.missed(), 0);
    }

    #[test]
    fn a_burst_longer_than_a_batch_yields_and_stays_due() {
        // 1400 bytes at 11.2 Mbit/s, 20 together: a burst each 20 ms.
        let mut schedule = Schedule::new(0, 40_000, 1400, 11_200_000, 20).unwrap();
        assert_eq!(batch(&mut schedule, 0), (MAX_BATCH, Next::Yield));
        assert_eq!(batch(&mut schedule, 10), (4, Next::Wait(20_000)));
    }

    #[test]
    fn a_refused_datagram_of_a_burst_stays_at_the_bursts_time() {
        let mut schedule = Schedule::new(0, 40_000, 1400, 11_200_000, 4).unwrap();
        let mut left = 2;
        let next = schedule.batch(0, || {
            left -= 1;
            left >= 0
        });
        assert_eq!(next, Next::Blocked);
        assert_eq!(schedule.due(), Some(0));
        schedule.taken(100);
        assert_eq!(schedule.due(), Some(0), "the burst's last one is still due");
    }

    #[test]
    fn an_empty_or_overflowing_window_sends_nothing() {
        let mut empty = Schedule::new(5, 5, 1400, 1, 1).unwrap();
        assert_eq!(batch(&mut empty, 5), (0, Next::Done));
        let mut late = Schedule::new(u64::MAX - 1, u64::MAX, 1400, 1, 1).unwrap();
        assert_eq!(batch(&mut late, u64::MAX - 1), (1, Next::Done));
        assert_eq!(batch(&mut late, u64::MAX), (0, Next::Done));
    }
}
