//! ESP32-S31 IEEE 802.15.4 transmit-power levels.
//!
//! The resolution itself is chip-neutral
//! ([`oer_espressif_ieee802154_engine::tx_power`]); the level set is the BTBB
//! provider's, recovered from the ESP32-S31 vendor library.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use oer_espressif_ieee802154_engine::tx_power::Ieee802154TxPowerLevels;

/// The ESP32-S31 BTBB provider's level set,
/// [`crate::phy::baseband::TX_POWER_LEVELS_DBM`].
///
/// The index into this set is the MAC `TXPOWER` field code.
pub const ESP32S31_TX_POWER_LEVELS: Ieee802154TxPowerLevels<'static> =
    match Ieee802154TxPowerLevels::new(&crate::phy::baseband::TX_POWER_LEVELS_DBM) {
        Ok(levels) => levels,
        Err(_) => panic!("the reviewed ESP32-S31 provider levels are valid"),
    };

#[cfg(test)]
mod tests;
