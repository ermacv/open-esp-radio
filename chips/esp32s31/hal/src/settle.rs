//! The one named settle: a busy-wait of known length, at most
//! [`SettleLimit::MAX`], measured on the caller's clock.
//!
//! A settle repeats an attempt until it yields a value or the limit has
//! passed; the first attempt always runs, and so does one attempt after the
//! limit was seen, so an interrupt between an attempt and the time check
//! never ends a settle whose hardware completed meanwhile. It is the only busy-wait a driver
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
/// The caller's clock type is preserved through the wait.
pub struct Settle<'clock, C: Clock + ?Sized> {
    limit: SettleLimit,
    clock: &'clock C,
}

impl<C: Clock + ?Sized> Clone for Settle<'_, C> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<C: Clock + ?Sized> Copy for Settle<'_, C> {}

impl<'clock, C: Clock + ?Sized> Settle<'clock, C> {
    /// Settle within `limit` as `clock` measures it.
    pub const fn new(limit: SettleLimit, clock: &'clock C) -> Self {
        Self { limit, clock }
    }

    /// The settle's limit.
    pub const fn limit(&self) -> SettleLimit {
        self.limit
    }
}

impl<C: Clock + ?Sized> core::fmt::Debug for Settle<'_, C> {
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
/// passed; the first attempt always runs, and the last one runs after the
/// limit was seen.
pub fn settle<C: Clock + ?Sized, T>(
    settle: Settle<'_, C>,
    mut attempt: impl FnMut() -> Option<T>,
) -> Result<T, Unsettled> {
    let start = settle.clock.now();
    loop {
        // Read the time before the attempt: the attempt that follows a
        // passed limit is the last one.
        let passed = settle.clock.now().saturating_duration_since(start) >= settle.limit.limit();
        if let Some(value) = attempt() {
            return Ok(value);
        }
        if passed {
            return Err(Unsettled);
        }
    }
}

#[cfg(test)]
mod tests;
