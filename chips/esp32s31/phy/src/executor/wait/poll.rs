//! Direct hardware observation: a named settle bounded in time, and the
//! finite attempt bound of the observations not yet measured.

use oer_esp32s31_hal::settle::{self, Settle, SettleLimit, Unsettled};
use oer_time::{Clock, Duration};

/// The longest a direct observation of a measured site may wait: the worst
/// measured wait was 6 reads, 1698 cycles (about 10.6 µs at 160 MHz).
const OBSERVATION_SETTLE_LIMIT: SettleLimit = match SettleLimit::new(Duration::from_micros(20)) {
    Some(limit) => limit,
    None => panic!("the observation settle limit is within the settle bound"),
};

/// Observe until `attempt` yields a value or [`OBSERVATION_SETTLE_LIMIT`] has
/// passed on `clock`; the first observation always runs.
#[inline]
pub(crate) fn settled<T>(
    clock: &dyn Clock,
    attempt: impl FnMut() -> Option<T>,
) -> Result<T, Unsettled> {
    settle::settle(Settle::new(OBSERVATION_SETTLE_LIMIT, clock), attempt)
}

/// Read immediately and retry directly, as in the rev0 PBus completion loop.
/// The bound counts observations; it does not promise a minimum elapsed time.
/// Returns false on exhaustion, preserving the caller's typed timeout handling.
#[inline]
pub(crate) fn bounded<E>(mut observe: impl FnMut() -> Result<bool, E>) -> Result<bool, E> {
    for _ in 0..crate::HARDWARE_EDGE_LIMIT {
        if observe()? {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    use oer_time_virtual::VirtualClock;

    /// Advance the clock by `step` microseconds at every read.
    fn read_step(clock: &VirtualClock, step: u64) {
        clock.advance(Duration::from_micros(step)).unwrap();
    }

    #[test]
    fn a_settled_observation_stops_at_the_first_ready_read() {
        let clock = VirtualClock::default();
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
        let clock = VirtualClock::default();
        let mut reads = 0;
        let result: Result<(), _> = settled(&clock, || {
            reads += 1;
            read_step(&clock, 6);
            None
        });
        assert_eq!(result, Err(Unsettled));
        // 6 µs per read: the fourth read passes the 20 µs limit.
        assert_eq!(reads, 4);
    }

    #[test]
    fn completion_stops_at_first_or_last_allowed_read() {
        for ready_at in [1, 2, crate::HARDWARE_EDGE_LIMIT] {
            let mut reads = 0;
            let result = bounded::<()>(|| {
                reads += 1;
                Ok(reads == ready_at)
            });
            assert_eq!(result, Ok(true));
            assert_eq!(reads, ready_at);
        }
    }

    #[test]
    fn stuck_hardware_exhausts_the_finite_read_budget() {
        let mut reads = 0;
        assert_eq!(
            bounded::<()>(|| {
                reads += 1;
                Ok(false)
            }),
            Ok(false)
        );
        assert_eq!(reads, crate::HARDWARE_EDGE_LIMIT);
    }

    #[test]
    fn observation_error_stops_without_further_access() {
        let mut reads = 0;
        let result = bounded(|| {
            reads += 1;
            if reads == 3 {
                Err("invalid edge")
            } else {
                Ok(false)
            }
        });
        assert_eq!(result, Err("invalid edge"));
        assert_eq!(reads, 3);
    }
}
