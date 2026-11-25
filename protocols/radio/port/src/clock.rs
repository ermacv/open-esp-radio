//! A port's radio clock and its relation to the image's monotonic time.

use core::fmt;

use oer_portable_time::{ClockRelation, Duration, Monotonic, RelationSample, Stamp};

/// A radio instant with the generation of its clock's relation to the
/// image's monotonic time.
///
/// A port's generation changes whenever that relation breaks (a wake from
/// sleep), so a stamp converts only with a [`ClockSample`] of its own
/// generation.
pub type RadioStamp<D> = Stamp<D, Monotonic>;

/// One paired reading of a port's radio clock (the source) and the image's
/// monotonic clock (the target), read by the backend when asked.
pub type ClockSample<D> = RelationSample<D, Monotonic>;

/// A port's radio clock of domain `D`: how finely it counts and how it
/// relates to the image's monotonic time.
pub struct ClockInfo<D> {
    /// The smallest step two readings can differ by.
    pub resolution: Duration,
    /// The relation of the radio clock (the source) to the monotonic clock
    /// (the target); `None` when none is known, and the port's instants
    /// compare only with each other.
    pub monotonic: Option<ClockRelation<D, Monotonic>>,
}

impl<D> ClockInfo<D> {
    /// A radio clock that is the image's monotonic clock, counting in
    /// microseconds.
    pub const MONOTONIC_MICROS: Self = Self {
        resolution: Duration::from_micros(1),
        monotonic: Some(ClockRelation::IDENTICAL),
    };
}

impl<D> Clone for ClockInfo<D> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D> Copy for ClockInfo<D> {}

impl<D> PartialEq for ClockInfo<D> {
    fn eq(&self, other: &Self) -> bool {
        self.resolution == other.resolution && self.monotonic == other.monotonic
    }
}

impl<D> Eq for ClockInfo<D> {}

impl<D> fmt::Debug for ClockInfo<D> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClockInfo")
            .field("resolution", &self.resolution)
            .field("monotonic", &self.monotonic)
            .finish()
    }
}
