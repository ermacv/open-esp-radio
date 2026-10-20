use oer_time::{Duration, Instant};

/// Advance the beacon schedule `send_tick + k * interval` (`k >= 1`) to the
/// first target beacon transmission time not before `now`.
///
/// This is the closed-form equivalent of the vendor loop that adds
/// `interval` at least once and then until the next tick is at most one
/// interval ahead of `now`. It performs no catch-up loop. The vendor counts
/// in a wrapping `u32` microsecond tick; on the `u64` axis a cursor ahead of
/// `now` advances by exactly one interval instead of wrapping around.
///
/// Returns `None` for a zero interval or a tick past the end of time.
pub const fn next_tbtt(send_tick: Instant, interval: Duration, now: Instant) -> Option<Instant> {
    let interval = interval.as_micros();
    if interval == 0 {
        return None;
    }

    // The vendor body always adds once, but an exact interval multiple may
    // stop at zero remaining distance.
    let elapsed = now.saturating_duration_since(send_tick).as_micros();
    let steps = if elapsed == 0 {
        1
    } else {
        elapsed.div_ceil(interval)
    };
    match steps.checked_mul(interval) {
        Some(advance) => send_tick.checked_add(Duration::from_micros(advance)),
        None => None,
    }
}

#[cfg(test)]
mod tests;
