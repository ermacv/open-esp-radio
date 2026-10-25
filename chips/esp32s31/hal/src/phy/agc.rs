//! Owned ESP32-S31 PHY AGC register leaves.
//!
//! Register layout and legal field images live in the generated PAC. This
//! module retains only cross-peripheral sequencing and the async delay
//! boundaries owned by upper PHY state machines.

#![forbid(unsafe_code)]

use crate::{owner::SharedPhyAccess, phy_blocks, phy_blocks_mut};

pub mod ops;

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
    crate::phy::sequence::update_agc(phy_blocks_mut(registers));
}

/// Select complete rev0 ROM `phy_enable_agc` or `phy_disable_agc`.
#[cfg(target_arch = "riscv32")]
pub fn set_enabled(registers: &mut impl SharedPhyAccess, enabled: bool) {
    ops::set_enabled(&mut phy_blocks_mut(registers).phy_agc_oracle, enabled);
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

/// Apply complete rev0 ROM `phy_force_rx_gain`.
#[cfg(target_arch = "riscv32")]
pub fn configure_forced_rx_gain(
    registers: &mut impl SharedPhyAccess,
    enabled: bool,
    gain: ForcedRxGain,
) {
    ops::configure_forced_rx_gain(&mut phy_blocks_mut(registers).phy_agc_oracle, enabled, gain);
}

/// Apply either complete branch of rev0 ROM `phy_rfrx_sat_rst`.
#[cfg(target_arch = "riscv32")]
pub fn configure_rf_rx_saturation(registers: &mut impl SharedPhyAccess, enabled: bool) {
    ops::configure_rf_rx_saturation(&mut phy_blocks_mut(registers).phy_agc_oracle, enabled);
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

/// Apply complete pinned `libphy.a[phy_reg.o]::phy_set_ftm_en`.
pub fn set_ftm_enabled(registers: &mut impl SharedPhyAccess, enabled: bool) {
    ops::set_ftm_enabled(&mut phy_blocks_mut(registers).phy_agc_oracle, enabled);
}

/// Read the exact bit written by [`set_ftm_enabled`].
pub fn ftm_enabled(registers: &impl SharedPhyAccess) -> bool {
    ops::ftm_enabled(&phy_blocks(registers).phy_agc_oracle)
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
    ops::update_post_initialization(&mut phy_blocks_mut(registers).phy_agc_oracle);
}

/// Apply either complete branch of rev0 ROM `phy_rx_11b_opt`.
#[cfg(target_arch = "riscv32")]
pub fn configure_rx_11b_optimization(registers: &mut impl SharedPhyAccess, enabled: bool) {
    ops::configure_rx_11b_optimization(&mut phy_blocks_mut(registers).phy_agc_oracle, enabled);
}
