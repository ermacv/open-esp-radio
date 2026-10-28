// SPDX-License-Identifier: Apache-2.0
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
//! Closed typed IEEE 802.15.4 MAC values and transactions shared between
//! chips.
//!
//! The ESP32-S31 and ESP32-C5 place MAC blocks of the same design at their
//! own addresses, with a few fields of different width and a chip-specific
//! interrupt route. Their chip PACs own the generated register blocks and the
//! raw register access; this crate owns what is the same on both: the
//! semantic values, events, abort reasons and observations the typed MAC
//! surface speaks, and the order of the interrupt activation and teardown
//! transactions. It contains no address and no register access.
#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

mod buffers;
mod channel;
mod events;
mod mac;
mod status;
mod transition;
mod tx_power;
mod values;

pub use buffers::*;
pub use channel::*;
pub use events::*;
pub use mac::*;
pub use status::*;
pub use transition::*;
pub use tx_power::*;
pub use values::*;

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    reason = "tests assert behavior and compare with reference arithmetic"
)]
mod tests;
