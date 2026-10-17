#![no_std]
#![forbid(unsafe_code)]

//! Chip-neutral monotonic and radio time.
//!
//! Two time axes cross the radio contracts:
//!
//! - [`Instant`] and [`Duration`] are the image's monotonic time, the one
//!   axis every executor task waits on. A [`Clock`] reads it and a [`Timer`]
//!   waits for a deadline on it. Portable code takes both as ports; an
//!   adapter binds them to an executor's time driver and a host model binds
//!   them to virtual time.
//! - [`RadioInstant`] is a coordinate in its owner's radio domain and
//!   epoch; [`RadioDuration`] is a non-negative span within that domain.
//!   Instants of different domains cannot be compared or subtracted. The owner
//!   validates additional interface, instance or generation identity.
//!   Raw microseconds imply no image-monotonic or wall-clock relationship;
//!   the existing clock relations own conversion between domains.
//! - [`Coordinate`] names an instant of either axis by its microseconds and
//!   span type, the only view a clock relation takes to carry time across.
//!
//! Both axes represent microseconds in `u64`. Radio spans include every
//! value from zero through `u64::MAX`; [`RadioWindow`] additionally requires
//! a non-zero span and a representable exclusive endpoint.
//! Checked arithmetic returns `None` or [`TimeOverflow`] when a result would
//! leave the representable range, and window construction returns
//! [`WindowError`]. The monotonic axis also exposes explicit saturation via
//! [`Instant::saturating_add`] for deadlines and
//! [`Instant::saturating_duration_since`]; radio arithmetic never saturates.
//!
//! # Features
//!
//! The crate has no dependency and no default feature. Two optional
//! features give [`Duration`] a wire form, for a port's typed bounds that
//! travel from the firmware to the host:
//!
//! - `serde`: [`Duration`] serializes transparently as its `u64`
//!   microseconds, and deserializes from them;
//! - `schema`: its `postcard_schema::Schema` is that of `u64`, for a host
//!   protocol's message lock; it enables `serde`.

use core::future::Future;

mod coordinate;
pub mod radio;

pub use coordinate::Coordinate;
pub use radio::{NonZeroRadioDuration, RadioDuration, RadioInstant, RadioWindow, WindowError};

/// A point on the image's monotonic time, in microseconds since an epoch
/// the [`Clock`] defines.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Instant(u64);

impl Instant {
    /// The clock's epoch.
    pub const EPOCH: Self = Self(0);

    /// Construct an instant from microseconds since the epoch.
    pub const fn from_micros(micros: u64) -> Self {
        Self(micros)
    }

    /// Microseconds since the epoch.
    pub const fn as_micros(self) -> u64 {
        self.0
    }

    /// The instant `duration` later, or `None` past the representable range.
    pub const fn checked_add(self, duration: Duration) -> Option<Self> {
        match self.0.checked_add(duration.0) {
            Some(micros) => Some(Self(micros)),
            None => None,
        }
    }

    /// The deadline `duration` later, or the end of representable time when
    /// it lies past it: such a deadline never comes, and `u64` microseconds
    /// outlast any image. Use it for deadlines, never to measure time.
    pub const fn saturating_add(self, duration: Duration) -> Self {
        Self(self.0.saturating_add(duration.0))
    }

    /// The instant `duration` earlier, or `None` before the epoch.
    pub const fn checked_sub(self, duration: Duration) -> Option<Self> {
        match self.0.checked_sub(duration.0) {
            Some(micros) => Some(Self(micros)),
            None => None,
        }
    }

    /// The time from `earlier` to `self`, or `None` when `earlier` is later.
    pub const fn checked_duration_since(self, earlier: Self) -> Option<Duration> {
        match self.0.checked_sub(earlier.0) {
            Some(micros) => Some(Duration(micros)),
            None => None,
        }
    }

    /// The time from `earlier` to `self`, zero when `earlier` is later.
    pub const fn saturating_duration_since(self, earlier: Self) -> Duration {
        Duration(self.0.saturating_sub(earlier.0))
    }
}

/// A non-negative span of monotonic time, in microseconds. With the
/// `serde` feature it is written as that `u64`.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Duration(u64);

impl Duration {
    /// No time.
    pub const ZERO: Self = Self(0);

    /// Construct a duration from microseconds.
    pub const fn from_micros(micros: u64) -> Self {
        Self(micros)
    }

    /// Construct a duration from milliseconds; every `u32` fits.
    pub const fn from_millis(millis: u32) -> Self {
        Self(millis as u64 * 1_000)
    }

    /// Construct a duration from seconds; every `u32` fits.
    pub const fn from_secs(secs: u32) -> Self {
        Self(secs as u64 * 1_000_000)
    }

    /// Microseconds.
    pub const fn as_micros(self) -> u64 {
        self.0
    }

    /// The sum, or `None` past the representable range.
    pub const fn checked_add(self, other: Self) -> Option<Self> {
        match self.0.checked_add(other.0) {
            Some(micros) => Some(Self(micros)),
            None => None,
        }
    }

    /// The difference, zero when `other` is longer.
    pub const fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }
}

/// A deadline lies past the representable range of monotonic time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimeOverflow;

/// Reads the image's monotonic time.
///
/// Successive reads never decrease. The resolution is the clock's own; a
/// clock states it where it is bound, and callers must not assume a read
/// changes between two nearby calls.
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

#[cfg(feature = "serde")]
mod wire;

#[cfg(test)]
mod tests;
