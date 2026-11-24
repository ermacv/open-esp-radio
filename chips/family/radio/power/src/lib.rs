#![expect(
    clippy::fn_params_excessive_bools,
    clippy::struct_excessive_bools,
    reason = "the PHY under one owner: its state, its operations, clocks and power"
)]
//! Shared RF membership, epoch, power and clock drivers over chip-owned ports.
#![no_std]
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
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
