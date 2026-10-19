//! Owned ESP32-S31 PHY AGC register leaves.
//!
//! Register layout and legal field images live in the generated PAC. This
//! module retains only cross-peripheral sequencing and the async delay
//! boundaries owned by upper PHY state machines.

#![forbid(unsafe_code)]

use crate::{owner::SharedPhyAccess, phy_pac, phy_pac_mut, types::PhyFtmEnableVendorArgument};

#[cfg(target_arch = "riscv32")]
use crate::types::ForcedRxGain;

/// Affine obligation to restore the FTM enable field to its exact prior bit.
#[must_use = "the exact prior FTM enable bit must be restored"]
#[derive(Debug, Eq, PartialEq)]
pub struct FtmEnableRestore {
    previous: bool,
}

impl FtmEnableRestore {
    pub const fn previous(&self) -> bool {
        self.previous
    }

    pub fn restore(self, registers: &mut impl SharedPhyAccess) {
        set_ftm_enabled(registers, self.previous);
    }
}

/// Apply complete rev0 ROM `phy_bb_agc_reg_update`.
///
/// The body at `0x2f82_860e`, size `0xa6`, performs fourteen internal radio
/// operations before its official-platform enable edge.
#[cfg(target_arch = "riscv32")]
pub fn update_baseband_registers(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.update_agc_baseband_registers();
    crate::ieee80211::baseband::enable_agc_register_update(registers);
}

/// Select complete rev0 ROM `phy_enable_agc` or `phy_disable_agc`.
#[cfg(target_arch = "riscv32")]
pub fn set_enabled(registers: &mut impl SharedPhyAccess, enabled: bool) {
    let registers = phy_pac_mut(registers);
    registers.set_agc_enabled(enabled);
}

/// Apply all ten fresh-read updates of complete `phy_agc_reg_init`.
#[cfg(target_arch = "riscv32")]
pub fn initialize_registers(
    registers: &mut impl SharedPhyAccess,
    parameter_121: u8,
    parameter_120: u8,
) {
    let registers = phy_pac_mut(registers);
    registers.initialize_agc_registers(parameter_121, parameter_120);
}

/// Apply complete pinned `phy_set_rx_comp_new`.
#[cfg(target_arch = "riscv32")]
pub fn configure_rx_compensation(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.configure_rx_compensation();
}

/// Pulse the complete pinned `phy_dc_mem_clr` field.
#[cfg(target_arch = "riscv32")]
pub fn clear_dc_memory(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.clear_agc_dc_memory();
}

/// Apply the two MMIO updates after the 1 µs PBus work-mode edge.
#[cfg(target_arch = "riscv32")]
pub fn configure_pbus_work_mode_pulse(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.configure_pbus_work_mode_pulse();
}

/// Clear the shared pulse after the caller-owned 2 µs edge.
#[cfg(target_arch = "riscv32")]
pub fn clear_pbus_work_mode_pulse(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.clear_pbus_work_mode_pulse();
}

/// Apply all three fresh-read updates of complete rev0 ROM `phy_ant_init`.
#[cfg(target_arch = "riscv32")]
pub fn configure_antenna(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.configure_agc_antenna();
}

/// Apply complete rev0 ROM `phy_ant_dft_cfg`.
#[cfg(target_arch = "riscv32")]
pub fn configure_antenna_diversity(registers: &mut impl SharedPhyAccess, enabled: bool) {
    let registers = phy_pac_mut(registers);
    registers.configure_antenna_diversity(enabled);
}

/// Apply complete rev0 ROM `phy_force_rx_gain`.
#[cfg(target_arch = "riscv32")]
pub fn configure_forced_rx_gain(
    registers: &mut impl SharedPhyAccess,
    enabled: bool,
    gain: ForcedRxGain,
) {
    let registers = phy_pac_mut(registers);
    registers.configure_forced_rx_gain(enabled, gain);
}

/// Apply complete rev0 ROM `phy_force_rx_gain` from its unmodified vendor ABI.
#[cfg(target_arch = "riscv32")]
pub fn configure_forced_rx_gain_from_vendor_arguments(
    registers: &mut impl SharedPhyAccess,
    enabled: u32,
    gain: u32,
) {
    let registers = phy_pac_mut(registers);
    registers.configure_forced_rx_gain_from_vendor_arguments(enabled, gain);
}

/// Apply either complete branch of rev0 ROM `phy_rfrx_sat_rst`.
#[cfg(target_arch = "riscv32")]
pub fn configure_rf_rx_saturation(registers: &mut impl SharedPhyAccess, enabled: bool) {
    let registers = phy_pac_mut(registers);
    registers.configure_rf_rx_saturation(enabled);
}

/// Publish both final limits from complete pinned `phy_set_rx_gain_table`.
#[cfg(target_arch = "riscv32")]
pub fn configure_rx_gain_limits(registers: &mut impl SharedPhyAccess, wifi_last_index: u8) {
    let registers = phy_pac_mut(registers);
    registers.configure_rx_gain_limits(wifi_last_index);
}

/// Publish one Wi-Fi AGC saturation-gain word to both destinations.
#[cfg(target_arch = "riscv32")]
pub fn set_saturation_gain(registers: &mut impl SharedPhyAccess, value: u32) {
    let registers = phy_pac_mut(registers);
    registers.set_agc_saturation_gain(value);
}

/// Apply complete pinned `libphy.a[phy_reg.o]::phy_set_ftm_en`.
pub fn set_ftm_enabled(registers: &mut impl SharedPhyAccess, enabled: bool) {
    let registers = phy_pac_mut(registers);
    registers.set_ftm_enabled(enabled);
}

/// Apply complete `phy_set_ftm_en` semantics to one raw vendor ABI argument.
pub fn set_ftm_enabled_from_vendor_argument(registers: &mut impl SharedPhyAccess, input: u32) {
    let registers = phy_pac_mut(registers);
    registers.set_ftm_enabled_from_vendor_argument(PhyFtmEnableVendorArgument::new(input));
}

/// Read the exact bit written by [`set_ftm_enabled`].
pub fn ftm_enabled(registers: &impl SharedPhyAccess) -> bool {
    phy_pac(registers).ftm_enabled()
}

/// Set the FTM leaf while returning its exact previous state for rollback.
pub fn prepare_ftm_enabled(
    registers: &mut impl SharedPhyAccess,
    enabled: bool,
) -> FtmEnableRestore {
    let previous = ftm_enabled(registers);
    set_ftm_enabled(registers, enabled);
    FtmEnableRestore { previous }
}

/// Apply complete pinned `phy_reg_update_new` and its finite children.
#[cfg(target_arch = "riscv32")]
pub fn update_post_initialization(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.update_agc_post_initialization();
}

/// Apply either complete branch of rev0 ROM `phy_rx_11b_opt`.
#[cfg(target_arch = "riscv32")]
pub fn configure_rx_11b_optimization(registers: &mut impl SharedPhyAccess, enabled: bool) {
    let registers = phy_pac_mut(registers);
    registers.configure_rx_11b_optimization(enabled);
}
