//! Owned ESP32-S31 PHY/baseband configuration leaves.
//!
//! The operations in this module preserve every fresh-read update from the
//! complete rev0 ROM and pinned `libphy.a` bodies while delegating register
//! layout and legal field images to the generated PAC.

#![forbid(unsafe_code)]

#[cfg(target_arch = "riscv32")]
use crate::{owner::SharedPhyAccess, phy_pac_mut, types::CfrValue};

/// Enable the two IQ-correction modes selected by PHY initialization.
///
/// Complete rev0 ROM `phy_iq_corr_enable` at `0x2f82_7d8c`, size `0x24`,
/// sets RX- and TX-IQ mode fields with two independent fresh-read updates.
#[cfg(target_arch = "riscv32")]
pub fn enable_iq_correction(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.enable_iq_correction_modes();
}

/// Preserve the two fresh status publications at RXIQ root entry.
///
/// Complete pinned `libphy.a[phy_rx_gain.o]::phy_rxiq_cal_init`, size
/// `0x198`, sets the shared status/clock word's bits 14 and 15 through two
/// independent reads.
#[cfg(target_arch = "riscv32")]
pub fn configure_rxiq_root_status(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.configure_rxiq_root_status();
}

/// Apply one complete RXIQ root correction-mode prefix or suffix.
///
/// Each branch retains the complete pinned parent's four separately ordered
/// fresh-read field updates.
#[cfg(target_arch = "riscv32")]
pub fn configure_rxiq_root_correction(registers: &mut impl SharedPhyAccess, begin: bool) {
    let registers = phy_pac_mut(registers);
    registers.configure_rxiq_root_correction(begin);
}

/// Configure the fourteen-edge baseband TX-power tracking leaf.
///
/// Basis: complete pinned
/// `libphy.a[phy_reg.o]::phy_bb_txpwr_track`, size `0xf4`.
#[cfg(target_arch = "riscv32")]
pub fn configure_tx_power_tracking(registers: &mut impl SharedPhyAccess, enabled: bool) {
    let registers = phy_pac_mut(registers);
    registers.configure_tx_power_tracking(enabled);
}

/// Apply complete pinned `libphy.a[phy_reg.o]::phy_config_hccfr`.
#[cfg(target_arch = "riscv32")]
pub fn configure_hccfr(registers: &mut impl SharedPhyAccess, enabled: bool, value: CfrValue) {
    let registers = phy_pac_mut(registers);
    registers.configure_hccfr(enabled, value);
}

/// Apply complete pinned `phy_config_hccfr` from its unmodified vendor ABI.
#[cfg(target_arch = "riscv32")]
pub fn configure_hccfr_from_vendor_arguments(
    registers: &mut impl SharedPhyAccess,
    enabled: u32,
    value: u32,
) {
    let registers = phy_pac_mut(registers);
    registers.configure_hccfr_from_vendor_arguments(enabled, value);
}

/// Apply either complete branch of pinned `libphy.a[phy_reg.o]::phy_iccfr_en`.
#[cfg(target_arch = "riscv32")]
pub fn configure_iccfr_gate(registers: &mut impl SharedPhyAccess, enabled: bool) {
    let registers = phy_pac_mut(registers);
    registers.configure_iccfr_gate(enabled);
}

/// Apply complete pinned `libphy.a[phy_reg.o]::phy_force_iccfr`.
#[cfg(target_arch = "riscv32")]
pub fn configure_forced_iccfr(
    registers: &mut impl SharedPhyAccess,
    mode: bool,
    enabled: bool,
    value: CfrValue,
) {
    let registers = phy_pac_mut(registers);
    registers.configure_forced_iccfr(mode, enabled, value);
}

/// Apply complete pinned `phy_force_iccfr` from its unmodified vendor ABI.
#[cfg(target_arch = "riscv32")]
pub fn configure_forced_iccfr_from_vendor_arguments(
    registers: &mut impl SharedPhyAccess,
    mode: u32,
    enabled: u32,
    value: u32,
) {
    let registers = phy_pac_mut(registers);
    registers.configure_forced_iccfr_from_vendor_arguments(mode, enabled, value);
}

/// Apply complete rev0 ROM `phy_btbb_wifi_bb_cfg2`.
#[cfg(target_arch = "riscv32")]
pub fn configure_bt_wifi_baseband(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.configure_bt_wifi_baseband();
}

/// Apply complete rev0 ROM `phy_chan_dump_cfg`.
#[cfg(target_arch = "riscv32")]
pub fn configure_channel_dump(
    registers: &mut impl SharedPhyAccess,
    value: u32,
    enabled: u32,
    mode: u32,
) {
    let registers = phy_pac_mut(registers);
    registers.configure_channel_dump(value, enabled, mode);
}

/// Apply complete rev0 ROM `phy_dac_rate_set`.
#[cfg(target_arch = "riscv32")]
pub fn configure_dac_rate(
    registers: &mut impl SharedPhyAccess,
    rate: oer_esp32s31_pac::PhyAdcRate,
) {
    let registers = phy_pac_mut(registers);
    registers.configure_dac_rate(rate);
}

/// Apply complete rev0 ROM `phy_fe_reg_init`.
///
/// The finite body performs seventeen ordered MMIO updates with the table
/// memory base-index update retained between its prefix and suffix.
#[cfg(target_arch = "riscv32")]
pub fn initialize_front_end(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.initialize_front_end_prefix();
    registers.configure_table_memory_base_index(0xa0);
    registers.initialize_front_end_suffix();
}

/// Apply both fresh-read updates of complete rev0 ROM `phy_fe_txrx_reset`.
#[cfg(target_arch = "riscv32")]
pub fn reset_frontend_txrx(registers: &mut impl SharedPhyAccess) {
    phy_pac_mut(registers).reset_frontend_txrx();
}

/// Apply complete pinned `libphy.a[phy_reg.o]::phy_fe_reg_update`.
#[cfg(target_arch = "riscv32")]
pub fn update_front_end(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.update_front_end();
}

/// Apply complete pinned `libphy.a[phy_reg.o]::phy_stop_tx_tone_new`.
///
/// This leaf intentionally does not restore the two DAC-scale fields owned by
/// the longer ROM `phy_stop_tx_tone(1)` operation.
#[cfg(target_arch = "riscv32")]
pub fn stop_tx_tone(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.stop_calibration_tone_paths();
}

/// Configure PHY I²C TX-rate fields and TX-gain compensation bytes.
///
/// Complete rev0 ROM `phy_i2c_txrate_init` at `0x2f82_86d0`, size `0x38`,
/// performs two rate updates, then dispatches to complete pinned
/// `phy_txgain_comp_pacfg_new(1)` for four ordered byte updates.
#[cfg(target_arch = "riscv32")]
pub fn configure_i2c_tx_rate(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.configure_i2c_tx_rate();
}

/// Apply complete pinned `phy_txgain_comp_pacfg_new(1)` without coupling it
/// to calibration-tone setup.
#[cfg(target_arch = "riscv32")]
pub fn restore_tx_gain_compensation(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.restore_tx_gain_compensation();
}

/// Configure the complete baseband watchdog leaf.
///
/// Basis: complete rev0 ROM `phy_bb_wdg_cfg` at `0x2f82_7860`, size `0x2c`.
#[cfg(target_arch = "riscv32")]
pub fn configure_watchdog(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.configure_baseband_watchdog();
}

/// Apply complete ROM `phy_vht_support`.
#[cfg(target_arch = "riscv32")]
pub fn set_vht_support(registers: &mut impl SharedPhyAccess, input: u32) {
    let registers = phy_pac_mut(registers);
    registers.set_vht_support(input);
}

/// Apply complete ROM `phy_csidump_force_lltf_cfg`.
#[cfg(target_arch = "riscv32")]
pub fn set_csi_dump_force_lltf(registers: &mut impl SharedPhyAccess, input: u32) {
    let registers = phy_pac_mut(registers);
    registers.set_csi_dump_force_lltf(input);
}

/// Apply complete ROM `phy_hemu_ru26_good_res`.
#[cfg(target_arch = "riscv32")]
pub fn configure_he_ru26_good_response(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.configure_he_ru26_good_response();
}

/// Apply complete ROM `phy_freq_band_reg_set`, including its VHT tail.
#[cfg(target_arch = "riscv32")]
pub fn set_frequency_band(registers: &mut impl SharedPhyAccess, input: u32) {
    let registers = phy_pac_mut(registers);
    registers.set_frequency_band(input);
}

/// Apply complete ROM `phy_bbtx_outfilter`.
#[cfg(target_arch = "riscv32")]
pub fn configure_tx_output_filter(
    registers: &mut impl SharedPhyAccess,
    input_0: u32,
    input_1: u32,
    input_2: u32,
) {
    let registers = phy_pac_mut(registers);
    registers.configure_tx_output_filter(input_0, input_1, input_2);
}

/// Apply complete ROM `phy_bb_wdt_rst_enable`.
#[cfg(target_arch = "riscv32")]
pub fn set_watchdog_reset_enabled(registers: &mut impl SharedPhyAccess, input: u32) {
    let registers = phy_pac_mut(registers);
    registers.set_baseband_watchdog_reset_enabled(input);
}

/// Apply complete ROM `phy_bb_wdt_int_enable`.
#[cfg(target_arch = "riscv32")]
pub fn set_watchdog_interrupt_enabled(registers: &mut impl SharedPhyAccess, input: u32) {
    let registers = phy_pac_mut(registers);
    registers.set_baseband_watchdog_interrupt_enabled(input);
}

/// Apply complete ROM `phy_bb_wdt_timeout_clear`.
#[cfg(target_arch = "riscv32")]
pub fn clear_watchdog_timeout(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.clear_baseband_watchdog_timeout();
}

/// Apply complete ROM `phy_bb_wdt_get_status`.
#[cfg(target_arch = "riscv32")]
pub fn watchdog_status(registers: &mut impl SharedPhyAccess) -> u32 {
    let registers = phy_pac_mut(registers);
    registers.baseband_watchdog_status()
}

/// Apply complete ROM `phy_lltf_mask_en`.
#[cfg(target_arch = "riscv32")]
pub fn configure_lltf_mask(registers: &mut impl SharedPhyAccess, input_0: u32, input_1: u32) {
    let registers = phy_pac_mut(registers);
    registers.configure_lltf_mask(input_0, input_1);
}

/// Enable the four recovered automatic noise-floor controls.
///
/// Basis: complete rev0 ROM `phy_noise_floor_auto_set` at `0x2f82_7d3c`,
/// size `0x36`.
#[cfg(target_arch = "riscv32")]
pub fn configure_noise_floor_auto(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.configure_noise_floor_auto();
}

/// Apply the complete baseband register initialization leaf.
///
/// Complete rev0 ROM `phy_bb_reg_init` at `0x2f82_79c6`, size `0x140`,
/// supplies all local writes. Calls to the already owned NRX and platform
/// baseband-control leaves remain at their original positions.
#[cfg(target_arch = "riscv32")]
pub fn initialize_baseband(registers: &mut impl SharedPhyAccess) {
    use crate::phy::frequency;

    phy_pac_mut(registers).initialize_baseband_prefix();
    frequency::initialize_nrx_baseband(registers);
    phy_pac_mut(registers).initialize_baseband_middle();
    frequency::set_baseband_init_control(registers);
    phy_pac_mut(registers).initialize_baseband_tail();
}

/// Apply the complete six-edge PA-on configuration leaf.
///
/// Basis: complete rev0 ROM `phy_tx_paon_set` at `0x2f82_764c`, size `0x78`.
#[cfg(target_arch = "riscv32")]
pub fn configure_tx_pa_on(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.configure_tx_pa_on();
}

/// Publish both signed digital gains through the shared PHY owner.
#[cfg(target_arch = "riscv32")]
pub fn configure_forced_digital_gain(
    registers: &mut impl SharedPhyAccess,
    enabled: bool,
    gain_0: i8,
    gain_1: i8,
) {
    phy_pac_mut(registers).configure_forced_digital_gain(enabled, gain_0, gain_1);
}
