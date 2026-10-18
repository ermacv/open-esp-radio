#![no_std]
#![forbid(unsafe_code)]

//! ESP32-S31 Wi-Fi station composition.
//!
//! This crate composes chip PHY/MAC-backend owners for station operation. It must not
//! depend on Embassy, a network stack, board allocation or HIL protocols.

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod test_support;

pub mod association;
pub mod attempt;
pub mod connected;
pub mod connected_control;
pub mod connected_rx;
/// The coexistence requests of connection frames, from the Espressif policy.
pub use oer_espressif_ieee80211_policy::connection_coex;
pub mod control_tx;
pub mod ftm;
pub mod join;
/// The station power manager of `oer-ieee80211-sta`.
pub use oer_ieee80211_sta::modem_sleep;
pub mod peer;
mod peer_policy;
pub mod scan;
pub mod scan_tx;
pub mod single_mpdu_tx;
pub mod standalone_esp_now_rx;
pub mod tx_epoch;
pub mod wpa2;

/// Local capability claims and scan-to-hardware channel selection.
pub mod profile;

/// Chip-specific channel, receive policy and beacon-monitor operations.
pub mod hardware;
