#![expect(
    clippy::cast_possible_truncation,
    reason = "R3 cleans the cores and the contracts (tasks/312)"
)]
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

//! Typed trace events, encoding, caller-owned record buffers and the
//! observer contract.
//!
//! Each event has a domain, event id, channel and two-word encoding shared by
//! producers and decoders. [`oldest_first`] orders a caller's record slice.
//! An owner reports events through [`observe!`] to an [`Observer`], by
//! default [`Noop`]; without `cfg(oer_trace)` an observation point leaves no
//! code. This crate has no global recorder, placement, persistence or boot
//! policy.

mod decode;
mod event;
mod observe;
pub mod profile;
mod store;
pub mod tune;

pub use decode::{Described, Describer, EventSet};
pub use event::{Channel, Domain, Event, InvalidChannel, Kind};
pub use observe::{Noop, Observer};
pub use store::{Record, oldest_first};

#[cfg(test)]
mod tests;
