//! Owned ESP32-S31 PHY AGC register leaves.
//!
//! Register layout and legal field images live in the generated PAC. This
//! module retains only cross-peripheral sequencing and the async delay
//! boundaries owned by upper PHY state machines.

#![forbid(unsafe_code)]

#[cfg(target_arch = "riscv32")]
use crate::{phy_blocks_mut, system::owner::SharedPhyAccess};

pub mod ops;

/// Apply complete rev0 ROM `phy_bb_agc_reg_update`.
///
/// The body at `0x2f82_860e`, size `0xa6`, performs fourteen internal radio
/// operations before its official-platform enable edge.
#[cfg(target_arch = "riscv32")]
pub fn update_baseband_registers(registers: &mut impl SharedPhyAccess) {
    crate::radio::phy::sequence::update_agc(phy_blocks_mut(registers));
}

/// Apply complete rev0 ROM `phy_enable_agc`.
#[cfg(target_arch = "riscv32")]
pub fn enable(registers: &mut impl SharedPhyAccess) {
    ops::enable(&mut phy_blocks_mut(registers).phy_agc_oracle);
}

/// Apply complete rev0 ROM `phy_disable_agc`.
#[cfg(target_arch = "riscv32")]
pub fn disable(registers: &mut impl SharedPhyAccess) {
    ops::disable(&mut phy_blocks_mut(registers).phy_agc_oracle);
}

/// Apply all ten fresh-read updates of complete `phy_agc_reg_init`.
#[cfg(target_arch = "riscv32")]
pub fn initialize_registers(
    registers: &mut impl SharedPhyAccess,
    parameter_121: u8,
    parameter_120: u8,
) {
    ops::initialize_registers(
        &mut phy_blocks_mut(registers).phy_agc_oracle,
        parameter_121,
        parameter_120,
    );
}

/// Apply complete pinned `phy_set_rx_comp_new`.
#[cfg(target_arch = "riscv32")]
pub fn configure_rx_compensation(registers: &mut impl SharedPhyAccess) {
    ops::configure_rx_compensation(&mut phy_blocks_mut(registers).phy_agc_oracle);
}

/// Pulse the complete pinned `phy_dc_mem_clr` field.
#[cfg(target_arch = "riscv32")]
pub fn clear_dc_memory(registers: &mut impl SharedPhyAccess) {
    ops::clear_dc_memory(&mut phy_blocks_mut(registers).phy_agc_oracle);
}

/// Apply the two MMIO updates after the 1 µs PBus work-mode edge.
#[cfg(target_arch = "riscv32")]
pub fn configure_pbus_work_mode_pulse(registers: &mut impl SharedPhyAccess) {
    ops::configure_pbus_work_mode_pulse(&mut phy_blocks_mut(registers).phy_agc_oracle);
}

/// Clear the shared pulse after the caller-owned 2 µs edge.
#[cfg(target_arch = "riscv32")]
pub fn clear_pbus_work_mode_pulse(registers: &mut impl SharedPhyAccess) {
    ops::clear_pbus_work_mode_pulse(&mut phy_blocks_mut(registers).phy_agc_oracle);
}

/// Apply all three fresh-read updates of complete rev0 ROM `phy_ant_init`.
#[cfg(target_arch = "riscv32")]
pub fn configure_antenna(registers: &mut impl SharedPhyAccess) {
    ops::configure_antenna(&mut phy_blocks_mut(registers).phy_agc_oracle);
}

/// Apply the enabling branch of complete rev0 ROM `phy_rfrx_sat_rst`.
#[cfg(target_arch = "riscv32")]
pub fn enable_rf_rx_saturation(registers: &mut impl SharedPhyAccess) {
    ops::enable_rf_rx_saturation(&mut phy_blocks_mut(registers).phy_agc_oracle);
}

/// Apply the disabling branch of complete rev0 ROM `phy_rfrx_sat_rst`.
#[cfg(target_arch = "riscv32")]
pub fn disable_rf_rx_saturation(registers: &mut impl SharedPhyAccess) {
    ops::disable_rf_rx_saturation(&mut phy_blocks_mut(registers).phy_agc_oracle);
}

/// Publish both final limits from complete pinned `phy_set_rx_gain_table`.
#[cfg(target_arch = "riscv32")]
pub fn configure_rx_gain_limits(registers: &mut impl SharedPhyAccess, wifi_last_index: u8) {
    ops::configure_rx_gain_limits(
        &mut phy_blocks_mut(registers).phy_agc_oracle,
        wifi_last_index,
    );
}

/// Publish one Wi-Fi AGC saturation-gain word to both destinations.
#[cfg(target_arch = "riscv32")]
pub fn set_saturation_gain(registers: &mut impl SharedPhyAccess, value: u32) {
    ops::set_saturation_gain(&mut phy_blocks_mut(registers).phy_agc_oracle, value);
}

/// Apply complete pinned `phy_reg_update_new` and its finite children.
#[cfg(target_arch = "riscv32")]
pub fn update_post_initialization(registers: &mut impl SharedPhyAccess) {
    ops::update_post_initialization(&mut phy_blocks_mut(registers).phy_agc_oracle);
}

/// Apply the enabling branch of complete rev0 ROM `phy_rx_11b_opt`.
#[cfg(target_arch = "riscv32")]
pub fn enable_rx_11b_optimization(registers: &mut impl SharedPhyAccess) {
    ops::enable_rx_11b_optimization(&mut phy_blocks_mut(registers).phy_agc_oracle);
}

/// Apply the disabling branch of complete rev0 ROM `phy_rx_11b_opt`.
#[cfg(target_arch = "riscv32")]
pub fn disable_rx_11b_optimization(registers: &mut impl SharedPhyAccess) {
    ops::disable_rx_11b_optimization(&mut phy_blocks_mut(registers).phy_agc_oracle);
}
