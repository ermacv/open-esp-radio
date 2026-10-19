//! Espressif proprietary PHY policy, independent of ESP-NOW framing.
//!
//! These are not IEEE legacy rates or MCS values. Bitrates follow Espressif's
//! `wifi_phy_rate_t` API; no descriptor encoding or publication authority is
//! implied. A chip backend must separately prove its PLCP/vector mapping.
//! See <https://docs.espressif.com/projects/esp-idf/en/latest/esp32s31/api-reference/network/esp_wifi.html>.

/// Requested Espressif Long Range on-air rate, not a chip register value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspressifLongRangeRate {
    Kbps250,
    Kbps500,
}
