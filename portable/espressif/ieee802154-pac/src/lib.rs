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

mod events;
mod status;
mod transition;
mod values;

pub use events::*;
pub use status::*;
pub use transition::*;
pub use values::*;

#[cfg(test)]
mod tests;
