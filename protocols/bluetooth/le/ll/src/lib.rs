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

//! Hardware-independent Bluetooth Low Energy Link Layer building blocks.
//!
//! This crate owns connection/channel progression, duplicate filtering,
//! control and data-length procedures, encryption procedures, packet counters,
//! event geometry and receive authentication:
//!
//! - [`timing`]: connection-event geometry over the backend's
//!   [`ConnectionAllowances`](oer_bluetooth_le_port::ConnectionAllowances)
//!   (first and recurring peripheral events, sleep-clock window widening) and
//!   legacy advertising channel geometry after the vendor item geometry, with
//!   [`EpochExhausted`](timing::EpochExhausted) when a result leaves the radio
//!   epoch;
//! - [`receive`]: authentication of received data channel PDUs before Link
//!   Layer and Host dispatch.
//!
//! Over-the-air fields and codecs live in `oer-bluetooth-le-wire`; shared DTM
//! window geometry and the backend's timing allowances belong to
//! `oer-bluetooth-le-port`. A Controller composes these pieces with
//! `oer-bluetooth-le-cipher` for AES-CCM. This core has no HCI, MMIO, DMA or
//! executor dependencies.

#[cfg(test)]
extern crate std;

pub mod connection;
pub mod control;
pub mod data_length;
pub mod receive;
pub mod scanning;
pub mod security;
pub mod timing;
