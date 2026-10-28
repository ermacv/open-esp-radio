#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
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
mod store;

pub use decode::{Described, Describer, EventSet};
pub use event::{Channel, Domain, Event, InvalidChannel, Kind};
pub use observe::{Noop, Observer};
pub use store::{Record, oldest_first};

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
mod tests;
