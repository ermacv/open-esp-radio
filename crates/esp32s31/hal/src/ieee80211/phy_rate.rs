//! Validation leaves of the Wi-Fi-owned PHY words.
//!
//! The low-rate receive gate and the noise-floor measurement belong to the
//! Wi-Fi register set; production reaches them through the Wi-Fi MAC HAL.
//! These leaves let vendor comparison drive the complete ROM bodies alone.

use oer_esp32s31_pac::WifiRadioRegisters;

/// Select either complete ROM low-rate configuration leaf
/// (`phy_enable_low_rate` or `phy_disable_low_rate`).
pub fn set_low_rate_enabled(registers: &mut WifiRadioRegisters, enabled: bool) {
    registers.configure_phy_low_rate(enabled);
}

/// Read complete ROM `phy_is_low_rate_enabled`.
pub fn low_rate_enabled(registers: &WifiRadioRegisters) -> bool {
    registers.phy_low_rate_enabled()
}

/// Apply complete rev0 ROM `phy_rx11blr_cfg`.
pub fn configure_rx_11b_low_rate(registers: &mut WifiRadioRegisters, input: u32) {
    registers.configure_rx_11b_low_rate(input);
}

/// Read complete rev0 ROM `phy_read_hw_noisefloor` as signed quarter-dB.
pub fn read_hardware_noise_floor(registers: &WifiRadioRegisters) -> i32 {
    registers.read_noise_floor_quarter_db()
}
