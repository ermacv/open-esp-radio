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

//! Typed trace events, encoding and caller-owned record buffers.
//!
//! Each event has a domain, event id, channel and two-word encoding shared by
//! producers and decoders. [`oldest_first`] orders a caller's record slice.
//! This crate has no global recorder, placement, persistence or boot policy.

mod decode;
mod event;
mod store;

pub use decode::{Described, Describer, EventSet};
pub use event::{Channel, Domain, Event, InvalidChannel, Kind};
pub use store::{Record, oldest_first};

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
mod tests;
