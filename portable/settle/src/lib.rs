#![no_std]
#![forbid(unsafe_code)]
#![expect(clippy::cast_possible_truncation, reason = "a level's task")]

//! The one named settle: a busy-wait of known length, at most
//! [`SettleLimit::MAX`], counted in the cycles of the caller's counter.
//!
//! A settle repeats an attempt until it yields a value or the limit has
//! passed; the first attempt always runs, and so does one attempt after the
//! limit was seen, so an interrupt between an attempt and the counter check
//! never ends a settle whose hardware completed meanwhile. It is the only busy-wait a driver
//! may make, and it counts against the step bound of its context. A wait of
//! unknown length is a timer-woken step with a deadline instead.

use oer_portable_time::Duration;

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

    /// The limit in whole microseconds: at most 20.
    pub const fn micros(self) -> u32 {
        // A limit is at most `MAX`, 20 µs: it fits a `u32`.
        const {
            if Self::MAX.as_micros() > u32::MAX as u64 {
                panic!("a settle's longest limit fits a u32 of microseconds");
            }
        };
        self.0.as_micros() as u32
    }
}

/// The counter a settle is counted on: the cycles of a limit, and one
/// wrapping sample. Its samples run at the rate the cycles are counted in,
/// and a settle starts and ends on one counter.
pub trait SettleCounter {
    /// The cycles of `limit`, rounded up: a settle is at least its limit.
    fn cycles(&self, limit: SettleLimit) -> u32;
    /// One sample of the counter; it wraps.
    fn sample(&self) -> u32;
}

/// A lent counter counts as its owner's.
impl<K: SettleCounter + ?Sized> SettleCounter for &K {
    fn cycles(&self, limit: SettleLimit) -> u32 {
        (**self).cycles(limit)
    }
    fn sample(&self) -> u32 {
        (**self).sample()
    }
}

/// One settle: its limit and the counter that counts it.
pub struct Settle<'counter, K: SettleCounter + ?Sized> {
    limit: SettleLimit,
    counter: &'counter K,
}

impl<K: SettleCounter + ?Sized> Clone for Settle<'_, K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K: SettleCounter + ?Sized> Copy for Settle<'_, K> {}

impl<'counter, K: SettleCounter + ?Sized> Settle<'counter, K> {
    /// Settle within `limit` as `counter` counts it.
    pub const fn new(limit: SettleLimit, counter: &'counter K) -> Self {
        Self { limit, counter }
    }

    /// The settle's limit.
    pub const fn limit(&self) -> SettleLimit {
        self.limit
    }
}

impl<K: SettleCounter + ?Sized> core::fmt::Debug for Settle<'_, K> {
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
pub fn settle<K: SettleCounter + ?Sized, T>(
    settle: Settle<'_, K>,
    mut attempt: impl FnMut() -> Option<T>,
) -> Result<T, Unsettled> {
    let cycles = settle.counter.cycles(settle.limit);
    let start = settle.counter.sample();
    loop {
        // Sample before the attempt: the attempt that follows a passed
        // limit is the last one. A wrapping difference stays exact over a
        // settle's few thousand cycles.
        let passed = settle.counter.sample().wrapping_sub(start) >= cycles;
        if let Some(value) = attempt() {
            return Ok(value);
        }
        if passed {
            return Err(Unsettled);
        }
    }
}

/// Wait out a settle's whole limit: the hold of a named length, an attempt
/// that never yields. It returns once the limit has passed on the settle's
/// counter.
pub fn hold<K: SettleCounter + ?Sized>(length: Settle<'_, K>) {
    let Err(Unsettled) = settle(length, || None::<core::convert::Infallible>);
}
