#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing
)]
//! ESP32-S31 chip knowledge: the tables and pure functions the HAL, the
//! drivers and the host tools share, without hardware access.
//!
//! - [`calibration_cache`]: the hardware-free S31 calibration record codec.
//! - [`reset_cause`]: why an HP core last left reset.
//! - [`tx_power`]: the BTBB provider's transmit-power levels.
#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod calibration_cache;

pub mod phy_relation;

pub mod reset_cause;

pub mod tx_power;

#[cfg(test)]
extern crate std;
