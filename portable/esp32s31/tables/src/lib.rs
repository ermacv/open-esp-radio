//! ESP32-S31 chip knowledge: the tables and pure functions the HAL, the
//! drivers and the host tools share, without hardware access.
//!
//! - [`tx_power`]: the BTBB provider's transmit-power levels;
//! - [`ieee802154_coex`]: the software-coexistence priorities the IEEE
//!   802.15.4 MAC publishes.
#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod ieee802154_coex;
pub mod tx_power;
