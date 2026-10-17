//! The instants of a clock domain as microsecond coordinates, for the
//! clock relations that carry time between two domains.

use crate::{Duration, Instant, RadioDuration, RadioInstant};

/// An instant of one clock domain, with that domain's span type.
///
/// A clock relation projects a coordinate of one domain into another by
/// their microseconds and widens the result by spans of the target domain.
/// Only a relation reads these microseconds across domains; arithmetic
/// within a domain stays on the instant and span types themselves.
pub trait Coordinate: Copy + Ord {
    /// A non-negative span of this domain.
    type Span: Copy;

    /// The coordinate's microseconds in its domain's epoch.
    fn micros(self) -> u64;

    /// The coordinate `micros` microseconds into its domain's epoch.
    fn at_micros(micros: u64) -> Self;

    /// A span of `micros` microseconds of this domain.
    fn span(micros: u64) -> Self::Span;

    /// The microseconds of a span of this domain.
    fn span_micros(span: Self::Span) -> u64;
}

impl Coordinate for Instant {
    type Span = Duration;

    fn micros(self) -> u64 {
        self.as_micros()
    }

    fn at_micros(micros: u64) -> Self {
        Self::from_micros(micros)
    }

    fn span(micros: u64) -> Duration {
        Duration::from_micros(micros)
    }

    fn span_micros(span: Duration) -> u64 {
        span.as_micros()
    }
}

impl<D> Coordinate for RadioInstant<D> {
    type Span = RadioDuration<D>;

    fn micros(self) -> u64 {
        self.as_micros()
    }

    fn at_micros(micros: u64) -> Self {
        Self::from_micros(micros)
    }

    fn span(micros: u64) -> RadioDuration<D> {
        RadioDuration::from_micros(micros)
    }

    fn span_micros(span: RadioDuration<D>) -> u64 {
        span.as_micros()
    }
}
