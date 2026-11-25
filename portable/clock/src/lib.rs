#![no_std]
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]

//! The ports of the image's monotonic clock: a [`Clock`] reads it and a
//! [`Timer`] waits for a deadline on it. Portable code takes both as ports;
//! an adapter binds them to an executor's time driver and a host model binds
//! them to virtual time. The values they read and wait for are
//! `oer-portable-time`'s.
//!
//! A runtime, a port's backend, a driver, the platform and a model take this
//! crate; a sans-IO core, a codec and a dictionary take time as values and
//! never depend on it.

use core::future::Future;

use oer_portable_time::{Duration, Instant};

/// How far past [`Instant::EPOCH`] a [`Clock`] starts: 2^31 µs, about
/// 35.8 minutes.
///
/// Radio code meets 32-bit hardware counters at its boundary: the MAC local
/// time, the TSF low word, the Bluetooth LE and IEEE 802.15.4 timers. A
/// clock that started at zero would keep a truncated instant below 2^31 for
/// the first ~35 minutes, so a wrap, a truncation or a signed-difference
/// compare at that boundary would fail only in long runs. Starting at
/// 2^31 µs sets bit 31 of the low word from the first read, so such code
/// meets the wrap at once, in tests and on the bench alike.
/// [`Instant::EPOCH`] stays the axis origin; a clock adds this offset to
/// its own source.
pub const CLOCK_EPOCH_OFFSET: Duration = Duration::from_micros(1 << 31);

/// A deadline lies past the representable range of monotonic time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimeOverflow;

/// Reads the image's monotonic time.
///
/// The image's clock and the default virtual clocks start
/// [`CLOCK_EPOCH_OFFSET`] past [`Instant::EPOCH`]; a model clock may start
/// where its test places it. Successive reads never decrease. The
/// resolution is the clock's own; a clock states it where it is bound, and
/// callers must not assume a read changes between two nearby calls.
pub trait Clock {
    /// The current instant.
    fn now(&self) -> Instant;

    /// The instant `duration` from now, or [`TimeOverflow`].
    fn deadline_after(&self, duration: Duration) -> Result<Instant, TimeOverflow> {
        self.now().checked_add(duration).ok_or(TimeOverflow)
    }
}

/// Waits on the image's monotonic time.
///
/// The returned future completes at or after `deadline` and immediately for
/// a deadline already reached. Dropping it cancels the wait and has no
/// other effect, so a timer can bound any other future through `select`.
pub trait Timer: Clock {
    /// Wait until `deadline`.
    fn wait_until(&self, deadline: Instant) -> impl Future<Output = ()>;

    /// Wait for `duration` from now, or fail without waiting when the
    /// deadline would overflow.
    fn wait_for(&self, duration: Duration) -> impl Future<Output = Result<(), TimeOverflow>> {
        let deadline = self.deadline_after(duration);
        async move {
            self.wait_until(deadline?).await;
            Ok(())
        }
    }
}

impl<T: Clock + ?Sized> Clock for &T {
    fn now(&self) -> Instant {
        (**self).now()
    }
}

impl<T: Timer + ?Sized> Timer for &T {
    fn wait_until(&self, deadline: Instant) -> impl Future<Output = ()> {
        (**self).wait_until(deadline)
    }
}

#[cfg(test)]
mod tests;
