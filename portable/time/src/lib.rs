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

//! Chip-neutral time: the values of a clock's domain, the relation of two
//! domains, and the ports of the monotonic clock.
//!
//! - [`DomainInstant`] is an instant of the clock of domain `D`, an empty
//!   type its owner declares, and [`DomainDuration`] a non-negative span of
//!   that domain. Instants of different domains cannot be compared or
//!   subtracted. The owner validates additional interface, instance or
//!   generation identity.
//! - The image's monotonic clock is the domain [`Monotonic`]: [`Instant`] and
//!   [`Duration`] name its values, the one axis every executor task waits on.
//!   Its ports, the clock that reads it and the timer that waits on it, are
//!   `oer-portable-clock`'s: this crate holds values alone, which a codec, a
//!   dictionary and a core may take.
//! - A [`ClockRelation`] states how the clocks of two domains advance, and is
//!   the only way to carry time from one to the other. Raw microseconds of a
//!   domain imply no relation to another's.
//!
//! Every domain counts microseconds in `u64`. Spans include every value from
//! zero through `u64::MAX`; [`DomainWindow`] additionally requires a non-zero
//! span and a representable exclusive endpoint. Checked arithmetic returns
//! `None` when a result would leave the representable range, and window construction returns [`WindowError`]. The monotonic
//! domain alone also has explicit saturation, for deadlines
//! ([`Instant::saturating_add`], [`Instant::saturating_duration_since`],
//! [`Duration::saturating_sub`]); no other domain's arithmetic saturates.

mod domain;
mod relation;

pub use domain::{
    DomainDuration, DomainInstant, DomainWindow, Duration, Instant, Monotonic,
    NonZeroDomainDuration, WindowError,
};
pub use relation::{
    ClockRelation, Generation, GenerationSource, Projected, ProjectionError, RelationRate,
    RelationSample, Stamp,
};

#[cfg(test)]
mod tests;
