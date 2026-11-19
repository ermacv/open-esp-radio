#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]

//! Portable Bluetooth LE Controller pieces. All production transitions are
//! synchronous. No transport, executor, clock implementation, scheduler or
//! hardware is owned here.
//!
//! - [`bootstrap`]: reset-scoped Host configuration of the software-only HCI
//!   bootstrap subset over the `oer-protocols-bluetooth-hci` codecs.

#[cfg(test)]
extern crate std;

pub mod bootstrap;
