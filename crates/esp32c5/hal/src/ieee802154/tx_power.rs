//! ESP32-C5 IEEE 802.15.4 transmit-power levels.
//!
//! The resolution itself is chip-neutral
//! ([`oer_espressif_ieee802154_engine::tx_power`]); the level set is the BTBB
//! provider's, recovered from the ESP32-C5 vendor library.

#![deny(missing_docs)]

use oer_espressif_ieee802154_engine::tx_power::Ieee802154TxPowerLevels;

/// The ESP32-C5 BTBB provider's transmit-power levels, in dBm.
///
/// Source: pinned ESP32-C5 `libbtbb.a` sha256
/// 9cbaf5bce18e6190dfab3213d2ecc65e2a5dd8bbde48bf5218dbbf47cc23da11,
/// `bt_bb_v2.o` `bt_bb_get_tx_pwr_table`, size `0x90`, which writes the count
/// 16 and, while `phy_param` byte `0x434` is zero, fills its static
/// `power_arr` with `phy_get_data_sat(3 * i - 24, 20, -15)` for `i = 0..16`,
/// the values clamped to -15..=20. That byte is the BT TX low-power flag: its
/// `.data` initial value in `libphy.a` sha256
/// dbf33c418c8d408d4005c849d12a1432deea82e2e5e57de3c8ddf914d104fffb
/// (`phy_init.o`) is zero and only `phy_set_bttx_low_power` writes it, which
/// the pinned ESP-IDF never calls. With the flag set the provider would
/// instead copy its sixteen-byte `.rodata` table; that configuration is not
/// modelled. IEEE 802.15.4 selects its `TXPOWER` code from this set.
pub const TX_POWER_LEVELS_DBM: [i8; 16] = [
    -15, -15, -15, -15, -12, -9, -6, -3, 0, 3, 6, 9, 12, 15, 18, 20,
];

/// The ESP32-C5 provider's level set as the engine's validated levels.
///
/// The index into this set is the MAC `TXPOWER` field code, which fits the
/// five-bit field.
pub const ESP32C5_TX_POWER_LEVELS: Ieee802154TxPowerLevels<'static> =
    match Ieee802154TxPowerLevels::new(&TX_POWER_LEVELS_DBM) {
        Ok(levels) => levels,
        Err(_) => panic!("the reviewed ESP32-C5 provider levels are valid"),
    };

#[cfg(test)]
mod tests {
    use super::*;
    use oer_espressif_ieee802154_engine::channel::Ieee802154Channel;

    /// Requests floor to a provider level, and every selected index fits
    /// the five-bit power field.
    #[test]
    fn the_esp32c5_provider_levels_resolve_every_request() {
        let channel = Ieee802154Channel::new(20).expect("standard channel");
        let levels = ESP32C5_TX_POWER_LEVELS;
        assert_eq!(levels.len(), 16);
        assert_eq!(levels.resolve(channel, 20).selected_provider_index(), 15);
        assert_eq!(levels.resolve(channel, 0).selected_provider_index(), 8);
        // The four clamped entries at -15 dBm: the scan takes the greatest.
        assert_eq!(levels.resolve(channel, -15).selected_provider_index(), 0);
        assert_eq!(levels.resolve(channel, -14).selected_provider_index(), 3);
        for request in i8::MIN..=i8::MAX {
            assert!(levels.resolve(channel, request).selected_provider_index() <= 0x1f);
        }
    }
}
