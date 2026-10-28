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

//! Portable Bluetooth LE Controller services and pure protocol pieces.
//! All production transitions are synchronous. No transport, executor, clock
//! implementation, scheduler or hardware is owned here; LE runtime awaits port IO.
//!
//! - [`bootstrap`]: reset-scoped Host configuration of the software-only HCI
//!   bootstrap subset over the `oer-bluetooth-hci` codecs;
//! - [`dtm`]: one-credit Reset/Rand/DTM 1M HCI service over the LE port,
//!   with durable command, session and response ownership.

#[cfg(test)]
extern crate std;

pub mod bootstrap;
/// The one-credit HCI DTM service over the portable LE port.
pub mod dtm;
