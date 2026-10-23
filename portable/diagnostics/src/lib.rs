#![no_std]
#![forbid(unsafe_code)]

//! Typed boot diagnostics: a transport-independent record codec and an
//! optional synchronous line frame. HIL puts the codec bytes in its own
//! frame; only the host renders text. No storage or transport is installed.

pub mod console;
pub mod record;

pub use console::{Console, DecodeError, PREFIX, decode};
pub use record::{BootSmokePassed, MAX_PAYLOAD, PreviousPanic, Record};

#[cfg(test)]
extern crate std;
