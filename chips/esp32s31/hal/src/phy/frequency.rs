//! Owned ESP32-S31 PHY frequency and channel transitions.
//!
//! The generated radio PAC owns every custom MMIO leaf in this module. This
//! HAL layer retains only semantic sequencing and coordination with the
//! official platform PAC. Register identities and field provenance live in
//! `registers/esp32s31/published/radio.svd`.

#[cfg(target_arch = "riscv32")]
use crate::{
    owner::{PhyInitializationAccess, SharedPhyAccess},
    phy_pac_mut,
};

pub use oer_esp32s31_pac::PhyFrequencyI2cNumberAddresses;

/// Clear the shared Wi-Fi control state at the start of common PHY init.
///
/// Pinned `libphy.a[phy_init.o]::register_chipv7_phy`, size `0x1e6`, performs
/// this update before `phy_force_txrx_off`.
#[cfg(target_arch = "riscv32")]
pub fn prepare_common_phy_control(registers: &mut impl PhyInitializationAccess) {
    crate::ieee80211::baseband::prepare_cold_start(phy_pac_mut(registers));
}

/// Select the two-bit baseband mode used by the Rust cold-init transition.
#[cfg(target_arch = "riscv32")]
pub fn set_baseband_mode(registers: &mut impl SharedPhyAccess, mode: u8) {
    let registers = phy_pac_mut(registers);
    registers.set_frequency_baseband_mode(mode);
}

/// Pulse complete rev0 ROM `phy_freq_module_resetn`.
#[cfg(target_arch = "riscv32")]
pub fn reset_module(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.reset_frequency_module();
}

/// Select whether hardware owns frequency updates.
#[cfg(target_arch = "riscv32")]
pub fn set_hardware_control(registers: &mut impl SharedPhyAccess, enabled: bool) {
    let registers = phy_pac_mut(registers);
    registers.set_hardware_frequency_control(enabled);
}

/// Apply complete rev0 ROM `phy_freq_reg_init(2, 4)`.
#[cfg(target_arch = "riscv32")]
pub fn initialize_registers(registers: &mut impl SharedPhyAccess, parameter_override: bool) {
    let registers = phy_pac_mut(registers);
    registers.initialize_frequency_registers(parameter_override);
}

/// Apply complete rev0 ROM `phy_freq_i2c_mem_write`.
#[cfg(target_arch = "riscv32")]
pub fn write_memory(registers: &mut impl SharedPhyAccess, address: u16, value: u32, mode: u8) {
    let registers = phy_pac_mut(registers);
    registers.write_frequency_memory(address, value, mode);
}

/// Apply complete rev0 ROM `phy_read_rf_freq_mem`.
#[cfg(target_arch = "riscv32")]
pub fn read_memory(registers: &mut impl SharedPhyAccess, address: u16, mode: u8) -> u32 {
    let registers = phy_pac_mut(registers);
    registers.read_frequency_memory(address, mode)
}

/// Restore the frequency-control channel index without starting a switch.
#[cfg(target_arch = "riscv32")]
pub fn restore_channel_index(registers: &mut impl SharedPhyAccess, frequency_index: u8) {
    let registers = phy_pac_mut(registers);
    registers.set_frequency_channel_index(frequency_index);
}

/// Publish complete rev0 ROM `phy_freq_i2c_num_addr`.
#[cfg(target_arch = "riscv32")]
pub fn configure_i2c_number_addresses(
    registers: &mut impl SharedPhyAccess,
    addresses: PhyFrequencyI2cNumberAddresses,
) {
    let registers = phy_pac_mut(registers);
    registers.configure_frequency_i2c_number_addresses(addresses);
}

/// Publish the pre-delay half of complete rev0 ROM `phy_freq_chan_en_sw`.
#[cfg(target_arch = "riscv32")]
pub fn start_channel_switch(registers: &mut impl SharedPhyAccess, frequency_index: u8) {
    let registers = phy_pac_mut(registers);
    registers.start_frequency_channel_switch(frequency_index);
}

/// Complete the caller-delayed clear from rev0 ROM `phy_freq_chan_en_sw`.
#[cfg(target_arch = "riscv32")]
pub fn clear_channel_switch(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.clear_frequency_channel_switch();
}

/// Sample the channel parent's frequency-ready word exactly once.
#[cfg(target_arch = "riscv32")]
pub fn sample_frequency_ready(registers: &mut impl SharedPhyAccess) -> bool {
    let registers = phy_pac_mut(registers);
    registers.frequency_ready()
}

/// Apply complete rev0 ROM `phy_nrx_freq_set`.
#[cfg(target_arch = "riscv32")]
pub fn configure_nrx_frequency(registers: &mut impl SharedPhyAccess, frequency: u32) {
    let registers = phy_pac_mut(registers);
    registers.configure_nrx_frequency(frequency);
}

/// Apply the two NRX writes inside complete rev0 ROM `phy_bb_reg_init`.
#[cfg(target_arch = "riscv32")]
pub fn initialize_nrx_baseband(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.initialize_nrx_baseband();
}

/// Set the shared Wi-Fi baseband bit used by `phy_bb_reg_init`.
#[cfg(target_arch = "riscv32")]
pub fn set_baseband_init_control(registers: &mut impl SharedPhyAccess) {
    crate::ieee80211::baseband::set_baseband_init_control(phy_pac_mut(registers));
}

/// Apply complete rev0 ROM `phy_bb_bss_cbw40` and its finite children.
#[cfg(target_arch = "riscv32")]
pub fn configure_bss_cbw(registers: &mut impl SharedPhyAccess, cbw: u8) {
    let registers = phy_pac_mut(registers);
    registers.configure_bss_cbw_prefix(cbw);
    crate::ieee80211::baseband::set_bss_cbw_40_digital(registers, cbw != 0);
    registers.configure_bss_cbw_suffix(cbw);
}

/// Publish the TX-cap readback exactly as ROM `phy_i2c_master_mem_txcap`.
#[cfg(target_arch = "riscv32")]
pub fn publish_tx_cap(registers: &mut impl SharedPhyAccess, value: u8) {
    let registers = phy_pac_mut(registers);
    registers.publish_frequency_tx_cap(value);
}

/// Apply complete rev0 ROM `phy_bb_cbw_chan_cfg`.
#[cfg(target_arch = "riscv32")]
pub fn configure_channel_cbw(registers: &mut impl SharedPhyAccess, cbw: u32) {
    let registers = phy_pac_mut(registers);
    registers.configure_channel_cbw(cbw);
}

/// Apply complete rev0 ROM `phy_wifi_enable_set`.
#[cfg(target_arch = "riscv32")]
pub fn set_wifi_enabled(registers: &mut impl PhyInitializationAccess, enabled: bool) {
    crate::ieee80211::baseband::set_wifi_enabled(phy_pac_mut(registers), enabled);
}

/// Apply complete rev0 ROM `phy_mac_enable_bb`.
#[cfg(target_arch = "riscv32")]
pub fn enable_mac_baseband(registers: &mut impl PhyInitializationAccess) {
    crate::ieee80211::baseband::enable_mac_baseband(phy_pac_mut(registers));
}

/// Apply complete rev0 ROM `phy_bt_filter_reg`.
#[cfg(target_arch = "riscv32")]
pub fn configure_bt_filter(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.configure_bt_filter();
}

/// Complete the observations after the caller's two-microsecond settling edge.
#[cfg(target_arch = "riscv32")]
pub fn observe_software_frequency_boundary(registers: &mut impl SharedPhyAccess) {
    phy_pac_mut(registers).observe_software_frequency_boundary();
}
