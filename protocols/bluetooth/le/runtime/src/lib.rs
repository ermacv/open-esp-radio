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

//! Executor-independent Bluetooth LE HCI and DTM orchestration.
//! The composition owns installation, shutdown, hardware and task spawning.

#[cfg(test)]
extern crate std;

pub mod dtm;
pub mod host;
