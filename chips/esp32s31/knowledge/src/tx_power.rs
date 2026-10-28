//! ESP32-S31 transmit-power levels: the BTBB provider's level set, which
//! IEEE 802.15.4 and Bluetooth LE select from.
//!
//! The IEEE 802.15.4 resolution itself is chip-neutral and lives in the
//! family `oer-espressif-ieee802154-mac-values` crate.

/// The BTBB provider's transmit-power levels, in dBm.
///
/// Source: pinned `libbtbb.a` sha256
/// 9bd762f68df0c894d27c542f01591aad21ee800b69fd6403c8ec6226ceaa8895,
/// `bt_bb_v2.o` `bt_bb_get_tx_pwr_table`, which writes the count 16 and fills
/// its static `power_arr` with `phy_get_data_sat(x, 80, -60)` for
/// `x = -24, -21, ..., 21`. Every value lies inside the clamp, so the table is
/// the unclamped sequence. IEEE 802.15.4 selects its `TXPOWER` code from it and
/// the BLE PHY environment records it; the values are provider levels, not an
/// RF calibration result.
pub const TX_POWER_LEVELS_DBM: [i8; 16] = [
    -24, -21, -18, -15, -12, -9, -6, -3, 0, 3, 6, 9, 12, 15, 18, 21,
];
