//! Direct hardware observation: a named settle bounded in time, and the
//! the time bounds of measured direct observations.

use oer_portable_time::Duration;
use oer_portable_time::settle::{self, Settle, SettleCounter, SettleLimit, Unsettled};

/// The longest a direct observation of a measured site may wait: the worst
/// measured direct wait was 87 reads, 2976 cycles (18.6 µs at 160 MHz)
/// in the W2 calibration/wake measurements on s31-a.
pub(crate) const OBSERVATION_MICROS: u64 = 20;
const OBSERVATION_SETTLE_LIMIT: SettleLimit =
    match SettleLimit::new(Duration::from_micros(OBSERVATION_MICROS)) {
        Some(limit) => limit,
        None => panic!("the observation settle limit is within the settle bound"),
    };

/// Observe until `attempt` yields a value or [`OBSERVATION_SETTLE_LIMIT`] has
/// passed on `counter`; the first observation always runs, and so does one
/// after the limit was seen.
#[inline]
pub(crate) fn settled<T, K: SettleCounter + ?Sized>(
    counter: &K,
    attempt: impl FnMut() -> Option<T>,
) -> Result<T, Unsettled> {
    settle::settle(Settle::new(OBSERVATION_SETTLE_LIMIT, counter), attempt)
}

/// Complete one measured direct edge within the named 20 µs settle on the
/// caller's counter. W2 on s31-a measured a worst direct edge of 2,976 cycles
/// at 160 MHz (18.6 µs); status sampling has no minimum delay.
#[inline]
pub(crate) fn bounded<E, K: SettleCounter + ?Sized>(
    counter: &K,
    mut observe: impl FnMut() -> Result<bool, E>,
) -> Result<bool, E> {
    match settled(counter, || match observe() {
        Ok(true) => Some(Ok(true)),
        Ok(false) => None,
        Err(error) => Some(Err(error)),
    }) {
        Ok(result) => result,
        Err(_) => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use oer_portable_time_virtual::VirtualCounter;

    /// Advance the counter by `step` cycles (microseconds) at every read.
    fn read_step(clock: &VirtualCounter, step: u32) {
        clock.advance(step);
    }

    #[test]
    fn a_settled_observation_stops_at_the_first_ready_read() {
        let clock = VirtualCounter::new(0, 0);
        let mut reads = 0;
        let result = settled(&clock, || {
            reads += 1;
            read_step(&clock, 5);
            (reads == 3).then_some(reads)
        });
        assert_eq!(result, Ok(3));
    }

    #[test]
    fn a_settled_observation_gives_up_when_the_limit_has_passed() {
        let clock = VirtualCounter::new(0, 0);
        let mut reads = 0;
        let result: Result<(), _> = settled(&clock, || {
            reads += 1;
            read_step(&clock, 6);
            None
        });
        assert_eq!(result, Err(Unsettled));
        // 6 µs per read: the fourth read passes the 20 µs limit, and the
        // fifth, which follows the passed limit, is the last.
        assert_eq!(reads, 5);
    }

    #[test]
    fn direct_completion_uses_elapsed_time_and_keeps_the_first_ready_or_error() {
        let clock = VirtualCounter::new(0, 0);
        let mut reads = 0;
        assert_eq!(
            bounded::<(), _>(&clock, || {
                reads += 1;
                read_step(&clock, 6);
                Ok(false)
            }),
            Ok(false)
        );
        assert_eq!(reads, 5);
        assert_eq!(bounded::<(), _>(&clock, || Ok(true)), Ok(true));
        assert_eq!(bounded(&clock, || Err("hardware")), Err("hardware"));
    }
}
