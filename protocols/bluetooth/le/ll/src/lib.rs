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
//! control and data-length procedures, encryption procedures and packet counters.
//! Over-the-air fields and codecs live in `oer-bluetooth-le-wire`; shared DTM
//! window geometry belongs to `oer-bluetooth-le-port`. A Controller composes
//! these pieces with `oer-bluetooth-le-cipher` for AES-CCM. This core has no
//! HCI, MMIO, DMA or executor dependencies.

#[cfg(test)]
extern crate std;

pub mod connection;
pub mod control;
pub mod data_length;
pub mod scanning;
pub mod security;
