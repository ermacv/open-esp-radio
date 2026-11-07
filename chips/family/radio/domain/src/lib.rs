//! Shared RF membership, epoch, power and clock drivers over chip-owned ports.
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
    clippy::disallowed_methods,
    clippy::disallowed_macros
)]
#![cfg_attr(
    test,
    allow(
        clippy::arithmetic_side_effects,
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable
    )
)]

#[cfg(test)]
extern crate std;

pub mod baseline;
pub mod clocks;
pub mod domain;
pub mod epoch;
pub mod membership;
pub mod modem;
pub mod power;

#[cfg(test)]
mod tests;
