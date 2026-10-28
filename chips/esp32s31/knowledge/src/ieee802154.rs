//! ESP32-S31 IEEE 802.15.4 radio facts the shared engine takes from the chip.

/// ESP32-S31 `IEEE802154_RX_SENSITIVITY` in dBm
/// (`esp_ieee802154_get_receive_sensitivity`).
///
/// SOURCE: ESP-IDF `ieee802154_ll.h` of the ESP32-S31 (public ESP-IDF
/// driver).
pub const RECEIVE_SENSITIVITY_DBM: i8 = -104;

/// ESP32-S31 `IEEE802154_RSSI_COMPENSATION_VALUE` in dBm.
///
/// SOURCE: ESP-IDF `ieee802154_ll.h` of the ESP32-S31 (public ESP-IDF
/// driver).
pub const RSSI_COMPENSATION_DBM: i8 = 0;
