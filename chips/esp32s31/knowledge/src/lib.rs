#![expect(clippy::cast_possible_truncation, reason = "the rows no step rewrites")]
//! ESP32-S31 chip knowledge: the tables and pure functions the HAL, the
//! drivers and the host tools share, without hardware access.
//!
//! - [`calibration_cache`]: the hardware-free S31 calibration record codec.
//! - [`ieee802154`]: the IEEE 802.15.4 receive sensitivity and RSSI
//!   compensation.
//! - [`reset_cause`]: why an HP core last left reset.
//! - [`tx_power`]: the BTBB provider's transmit-power levels.
#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]

pub mod calibration_cache;

/// Hardware-free modem clock profile.
pub mod modem_clock;

pub mod ieee802154;

pub mod phy_relation;

pub mod reset_cause;

pub mod tx_power;

#[cfg(test)]
extern crate std;
