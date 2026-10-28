//! The engine's typed selection from the chip's hardware-free provider facts.
use oer_esp32s31_knowledge::tx_power::TX_POWER_LEVELS_DBM;
use oer_espressif_ieee802154_mac_values::Ieee802154TxPowerLevels;

/// The ESP32-S31 BTBB provider's level set, [`TX_POWER_LEVELS_DBM`].
///
/// The index into this set is the MAC `TXPOWER` field code.
pub const ESP32S31_TX_POWER_LEVELS: Ieee802154TxPowerLevels<'static> =
    match Ieee802154TxPowerLevels::new(&TX_POWER_LEVELS_DBM) {
        Ok(levels) => levels,
        Err(_) => panic!("the reviewed ESP32-S31 provider levels are valid"),
    };

#[cfg(test)]
mod tests;
