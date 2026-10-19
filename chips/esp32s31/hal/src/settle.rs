//! The one named settle: a busy-wait of known length, at most
//! [`SettleLimit::MAX`], measured on the caller's clock.
//!
//! A settle repeats an attempt until it yields a value or the limit has
//! passed; the first attempt always runs. It is the only busy-wait a driver
//! may make, and it counts against the step bound of its context. A wait of
//! unknown length is a timer-woken step with a deadline instead.

use oer_time::{Clock, Duration};

/// The longest a settle may wait: nonzero and at most [`Self::MAX`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SettleLimit(Duration);

impl SettleLimit {
    /// The longest busy-wait a driver may make.
    pub const MAX: Duration = Duration::from_micros(20);

    /// Reject a zero limit, and one beyond [`Self::MAX`].
    pub const fn new(limit: Duration) -> Option<Self> {
        if limit.as_micros() == 0 || limit.as_micros() > Self::MAX.as_micros() {
            None
        } else {
            Some(Self(limit))
        }
    }

    /// The limit.
    pub const fn limit(self) -> Duration {
        self.0
    }
}

/// One settle: its limit and the clock that measures it.
#[derive(Clone, Copy)]
pub struct Settle<'clock> {
    limit: SettleLimit,
    clock: &'clock dyn Clock,
}

impl<'clock> Settle<'clock> {
    /// Settle within `limit` as `clock` measures it.
    pub const fn new(limit: SettleLimit, clock: &'clock dyn Clock) -> Self {
        Self { limit, clock }
    }

    /// The settle's limit.
    pub const fn limit(&self) -> SettleLimit {
        self.limit
    }
}

impl core::fmt::Debug for Settle<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Settle")
            .field("limit", &self.limit)
            .finish_non_exhaustive()
    }
}

/// The attempt yielded no value within the settle's limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Unsettled;

/// Repeat `attempt` until it yields a value or the settle's limit has
/// passed; the first attempt always runs.
pub fn settle<T>(
    settle: Settle<'_>,
    mut attempt: impl FnMut() -> Option<T>,
) -> Result<T, Unsettled> {
    let start = settle.clock.now();
    loop {
        if let Some(value) = attempt() {
            return Ok(value);
        }
        if settle.clock.now().saturating_duration_since(start) >= settle.limit.limit() {
            return Err(Unsettled);
        }
    }
}

#[cfg(test)]
mod tests;
