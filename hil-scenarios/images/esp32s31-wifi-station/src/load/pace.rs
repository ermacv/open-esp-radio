//! The schedule of paced transmission: datagram `n` is due `n` payload
//! times after the start, the payload time being its UDP payload bits over
//! the rate. The sender wakes on the Embassy timer, sends what is due in
//! batches and sleeps at least [`GRANULARITY_US`] between them.

/// The shortest sleep between two batches: at 65 Mbit/s of 1400-byte
/// datagrams a batch holds some six of them.
pub(crate) const GRANULARITY_US: u64 = 1000;
/// The most datagrams one batch sends before it yields to the executor:
/// the IP stack's transmit credits.
pub(crate) const MAX_BATCH: u32 = 16;

/// What the sender does after a batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Next {
    /// More datagrams are due: yield to the executor and send again.
    Yield,
    /// Sleep until this image microsecond.
    Wait(u64),
    /// The transmission's window ended.
    Done,
}

/// A paced transmission between two image microseconds.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Schedule {
    begin_us: u64,
    end_us: u64,
    payload: u16,
    rate_bps: u32,
    /// Datagrams the schedule handed to the sender, taken or refused.
    attempts: u64,
}

impl Schedule {
    /// The schedule of `payload`-byte datagrams at `rate_bps` from `begin_us`
    /// until `end_us`; `None` at the rate 0, which sends unpaced.
    pub(crate) fn new(begin_us: u64, end_us: u64, payload: u16, rate_bps: u32) -> Option<Self> {
        (rate_bps != 0).then_some(Self {
            begin_us,
            end_us,
            payload,
            rate_bps,
            attempts: 0,
        })
    }

    /// When the next datagram is due, before the window ends.
    fn due(&self) -> Option<u64> {
        let bits = u128::from(self.attempts)
            .checked_mul(u128::from(self.payload))?
            .checked_mul(8_000_000)?;
        let after = u64::try_from(bits.checked_div(u128::from(self.rate_bps))?).ok()?;
        self.begin_us
            .checked_add(after)
            .filter(|due| *due < self.end_us)
    }

    /// Send, through `send`, every datagram due at `now_us`, at most
    /// [`MAX_BATCH`] of them; a datagram the socket refuses is dropped, its
    /// time passes all the same.
    pub(crate) fn batch(&mut self, now_us: u64, mut send: impl FnMut()) -> Next {
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
            send();
            sent += 1;
            self.attempts = self.attempts.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(schedule: &mut Schedule, now_us: u64) -> (u32, Next) {
        let mut sent = 0;
        let next = schedule.batch(now_us, || sent += 1);
        (sent, next)
    }

    #[test]
    fn a_rate_of_zero_has_no_schedule() {
        assert!(Schedule::new(0, 1000, 1400, 0).is_none());
    }

    #[test]
    fn a_batch_sends_what_is_due_and_sleeps_at_least_the_granularity() {
        // 1400 bytes at 11.2 Mbit/s: one datagram each 1000 µs.
        let mut schedule = Schedule::new(10_000, 20_000, 1400, 11_200_000).unwrap();
        assert_eq!(batch(&mut schedule, 9_999), (0, Next::Wait(10_999)));
        assert_eq!(batch(&mut schedule, 10_000), (1, Next::Wait(11_000)));
        assert_eq!(batch(&mut schedule, 13_500), (3, Next::Wait(14_500)));
        assert_eq!(batch(&mut schedule, 14_000), (1, Next::Wait(15_000)));
        assert_eq!(
            batch(&mut schedule, 30_000),
            (5, Next::Done),
            "until the end"
        );
    }

    #[test]
    fn a_fast_rate_sends_in_batches_of_at_most_the_bound() {
        // 1400 bytes at 65 Mbit/s: one datagram each 172.3 µs.
        let mut schedule = Schedule::new(0, 1_000_000, 1400, 65_000_000).unwrap();
        assert_eq!(batch(&mut schedule, 1_000), (6, Next::Wait(2_000)));
        // 53 more are due at 10 ms (0..=58): batches of 16, 16, 16 and 5.
        for _ in 0..3 {
            assert_eq!(batch(&mut schedule, 10_000), (MAX_BATCH, Next::Yield));
        }
        assert_eq!(batch(&mut schedule, 10_000), (5, Next::Wait(11_000)));
        assert_eq!(schedule.attempts, 59);
    }

    #[test]
    fn a_slow_rate_sleeps_until_the_next_datagram() {
        // 100 bytes at 8 kbit/s: one datagram each 100 ms.
        let mut schedule = Schedule::new(0, 1_000_000, 100, 8_000).unwrap();
        assert_eq!(batch(&mut schedule, 0), (1, Next::Wait(100_000)));
        assert_eq!(batch(&mut schedule, 950_000), (9, Next::Done));
    }

    #[test]
    fn an_empty_or_overflowing_window_sends_nothing() {
        let mut empty = Schedule::new(5, 5, 1400, 1).unwrap();
        assert_eq!(batch(&mut empty, 5), (0, Next::Done));
        let mut late = Schedule::new(u64::MAX - 1, u64::MAX, 1400, 1).unwrap();
        assert_eq!(batch(&mut late, u64::MAX), (1, Next::Done));
    }
}
