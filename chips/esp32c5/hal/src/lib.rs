//! Ownership boundaries of the ESP32-C5 radio over its closed PAC.
//!
//! This first stage owns the IEEE 802.15.4 MAC: the owners that implement the
//! chip-neutral [`oer_espressif_ieee802154_engine::ll::Ieee802154LowLevel`] interface
//! over [`oer_esp32c5_pac`], and the ESP32-C5 data the engine needs from its
//! chip, and the shared modem clocks and MAC reset of [`modem_clock`]. The
//! PHY and the MAC lifecycle are not owned here yet.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod analog;
pub mod coex;
pub mod ieee802154;
pub mod modem_clock;
