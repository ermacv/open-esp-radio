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
//! - [`timing`]: connection-event geometry over the backend's
//!   [`ConnectionAllowances`](oer_bluetooth_le_port::ConnectionAllowances)
//!   (first and recurring peripheral events, sleep-clock window widening) and
//!   legacy advertising channel geometry after the vendor item geometry, with
//!   [`EpochExhausted`](timing::EpochExhausted) when a result leaves the radio
//!   epoch;
//! - [`receive`]: authentication of received data channel PDUs before Link
//!   Layer and Host dispatch;
//! - [`dtm`]: one-credit Reset/Rand/DTM 1M HCI service over the LE port,
//!   with durable command, session and response ownership.

#[cfg(test)]
extern crate std;

/// The one-credit HCI DTM service over the portable LE port.
pub mod dtm;
pub mod receive;
pub mod timing;
