//! ESP32-S31 chip knowledge: the tables and pure functions the HAL, the
//! drivers and the host tools share, without hardware access.
//!
//! - [`tx_power`]: the BTBB provider's transmit-power levels.
#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod phy_relation;

pub mod tx_power;
