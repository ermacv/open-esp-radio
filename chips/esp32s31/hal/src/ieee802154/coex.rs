//! Software-coexistence priorities of the IEEE 802.15.4 MAC.
//!
//! In a build with software coexistence the pinned driver publishes the MAC's
//! two PTI fields through libcoexist: `ieee802154_mac_init` sets the ACK PTI
//! to the middle level and the TX/RX PTI to the idle scene, and each
//! operation start switches the TX/RX PTI to its scene
//! (`ieee802154_set_txrx_pti` of `esp_ieee802154_util.c`, ESP-IDF
//! `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`). The priority of a level is
//! read from the arbiter's shared table ([`CoexPtiTable::ieee802154_pti`]).
//! A build without software coexistence disables both fields instead
//! (`ieee802154_ll_disable_coex`).

use crate::coex::{CoexPtiTable, Ieee802154CoexLevel};
pub use oer_espressif_ieee802154_engine::coex::{
    Ieee802154CoexPriorities, Ieee802154CoexScene, Ieee802154Coexistence,
};

/// Level of each TX/RX scene (`esp_ieee802154_coex_config_t`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154CoexConfig {
    /// Level of the idle scene.
    pub idle: Ieee802154CoexLevel,
    /// Level of immediate transmission and reception.
    pub txrx: Ieee802154CoexLevel,
    /// Level of timed transmission and reception.
    pub txrx_at: Ieee802154CoexLevel,
}

impl Ieee802154CoexConfig {
    /// The driver's default `s_coex_config`.
    pub const VENDOR: Self = Self {
        idle: Ieee802154CoexLevel::Idle,
        txrx: Ieee802154CoexLevel::Low,
        txrx_at: Ieee802154CoexLevel::Middle,
    };
}

impl Default for Ieee802154CoexConfig {
    fn default() -> Self {
        Self::VENDOR
    }
}

/// Level `ieee802154_mac_init` gives the ACK PTI.
pub const IEEE802154_ACK_COEX_LEVEL: Ieee802154CoexLevel = Ieee802154CoexLevel::Middle;

/// Resolve `config` and the ACK level against `table`: the priorities the
/// ESP32-S31 MAC publishes.
pub const fn resolve_priorities(
    config: Ieee802154CoexConfig,
    table: &CoexPtiTable,
) -> Ieee802154CoexPriorities {
    Ieee802154CoexPriorities::new(
        table.ieee802154_pti(config.idle),
        table.ieee802154_pti(config.txrx),
        table.ieee802154_pti(config.txrx_at),
        table.ieee802154_pti(IEEE802154_ACK_COEX_LEVEL),
    )
}

#[cfg(test)]
mod tests;
