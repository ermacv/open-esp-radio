//! ESP32-S31 system Wi-Fi/baseband control used by the open PHY.
//!
//! `MODEM_SYSCON` is owned by the custom affine radio route. This module owns
//! the instruction-evidenced operation order over that closed PAC capability.

use oer_esp32s31_pac::{RadioPhyRegisters, WifiBasebandAgcUpdate};

/// Clear the shared Wi-Fi control state before the open cold transition.
///
/// SOURCE\[`libphy.a[phy_init.o]::register_chipv7_phy`, size `0x1e6`].
/// Complete ROM `phy_wifi_enable_set` independently identifies the Wi-Fi
/// enable control; the other cleared control remains semantically unknown.
pub fn prepare_cold_start(registers: &mut RadioPhyRegisters) {
    registers.clear_cold_start_wifi_control();
}

/// Apply the single low update bit used by complete ROM `phy_bb_reg_init`.
pub fn set_baseband_init_control(registers: &mut RadioPhyRegisters) {
    registers.set_bb_agc_update_mode(WifiBasebandAgcUpdate::Initialization);
}

/// Select the digital BSS bandwidth branch used by `phy_bb_bss_cbw40_dig`.
pub fn set_bss_cbw_40_digital(registers: &mut RadioPhyRegisters, enabled: bool) {
    registers.set_bss_cbw_40_digital(enabled);
}

/// Apply complete ROM `phy_wifi_enable_set`.
pub fn set_wifi_enabled(registers: &mut RadioPhyRegisters, enabled: bool) {
    registers.set_wifi_baseband_enabled(enabled);
}

/// Apply the final system-register update from ROM `phy_bb_agc_reg_update`.
pub fn enable_agc_register_update(registers: &mut RadioPhyRegisters) {
    registers.set_bb_agc_update_mode(WifiBasebandAgcUpdate::RegisterUpdatesEnabled);
}

/// Apply complete ROM `phy_mac_enable_bb`.
///
/// SOURCE\[rev0 ROM `phy_mac_enable_bb` at `0x2f82_7836`, size `0x2a`].
/// The three separate calls deliberately preserve its three fresh RMW edges.
pub fn enable_mac_baseband(registers: &mut RadioPhyRegisters) {
    registers.enable_mac_baseband();
}
