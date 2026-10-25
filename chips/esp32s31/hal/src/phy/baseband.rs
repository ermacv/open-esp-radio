//! Owned ESP32-S31 PHY/baseband configuration leaves.
//!
//! The operations in this module preserve every fresh-read update from the
//! complete rev0 ROM and pinned `libphy.a` bodies while delegating register
//! layout and legal field images to the generated PAC.

#![forbid(unsafe_code)]

#[cfg(target_arch = "riscv32")]
use crate::{owner::SharedPhyAccess, phy_blocks_mut, types::CfrValue};

pub mod ops;

pub use ops::{TxDcPwdetFields, TxIqToneControlFields};

/// Enable the two IQ-correction modes selected by PHY initialization.
///
/// Complete rev0 ROM `phy_iq_corr_enable` at `0x2f82_7d8c`, size `0x24`,
/// sets RX- and TX-IQ mode fields with two independent fresh-read updates.
#[cfg(target_arch = "riscv32")]
pub fn enable_iq_correction(registers: &mut impl SharedPhyAccess) {
    ops::enable_iq_correction_modes(&mut phy_blocks_mut(registers).phy_baseband_config_oracle);
}

/// Preserve the two fresh status publications at RXIQ root entry.
///
/// Complete pinned `libphy.a[phy_rx_gain.o]::phy_rxiq_cal_init`, size
/// `0x198`, sets the shared status/clock word's bits 14 and 15 through two
/// independent reads.
#[cfg(target_arch = "riscv32")]
pub fn configure_rxiq_root_status(registers: &mut impl SharedPhyAccess) {
    crate::phy::pbus::ops::configure_rxiq_root_status(&mut phy_blocks_mut(registers).phy_pbus);
}

/// Apply one complete RXIQ root correction-mode prefix or suffix.
///
/// Each branch retains the complete pinned parent's four separately ordered
/// fresh-read field updates.
#[cfg(target_arch = "riscv32")]
pub fn configure_rxiq_root_correction(registers: &mut impl SharedPhyAccess, begin: bool) {
    crate::phy::sequence::configure_rxiq_root_correction(phy_blocks_mut(registers), begin);
}

/// Configure the fourteen-edge baseband TX-power tracking leaf.
///
/// Basis: complete pinned
/// `libphy.a[phy_reg.o]::phy_bb_txpwr_track`, size `0xf4`.
#[cfg(target_arch = "riscv32")]
pub fn configure_tx_power_tracking(registers: &mut impl SharedPhyAccess, enabled: bool) {
    ops::configure_tx_power_tracking(
        &mut phy_blocks_mut(registers).phy_baseband_config_oracle,
        enabled,
    );
}

/// Apply complete pinned `libphy.a[phy_reg.o]::phy_config_hccfr`.
#[cfg(target_arch = "riscv32")]
pub fn configure_hccfr(registers: &mut impl SharedPhyAccess, enabled: bool, value: CfrValue) {
    ops::configure_hccfr(
        &mut phy_blocks_mut(registers).phy_baseband_config_oracle,
        enabled,
        value,
    );
}

/// Apply either complete branch of pinned `libphy.a[phy_reg.o]::phy_iccfr_en`.
#[cfg(target_arch = "riscv32")]
pub fn configure_iccfr_gate(registers: &mut impl SharedPhyAccess, enabled: bool) {
    ops::configure_iccfr_gate(
        &mut phy_blocks_mut(registers).phy_baseband_config_oracle,
        enabled,
    );
}

/// Apply complete pinned `libphy.a[phy_reg.o]::phy_force_iccfr`.
#[cfg(target_arch = "riscv32")]
pub fn configure_forced_iccfr(
    registers: &mut impl SharedPhyAccess,
    mode: bool,
    enabled: bool,
    value: CfrValue,
) {
    ops::configure_forced_iccfr(
        &mut phy_blocks_mut(registers).phy_baseband_config_oracle,
        mode,
        enabled,
        value,
    );
}

/// Apply complete rev0 ROM `phy_fe_reg_init`.
///
/// The finite body performs seventeen ordered MMIO updates with the table
/// memory base-index update retained between its prefix and suffix.
#[cfg(target_arch = "riscv32")]
pub fn initialize_front_end(registers: &mut impl SharedPhyAccess) {
    crate::phy::sequence::initialize_front_end(phy_blocks_mut(registers));
}

/// Apply both fresh-read updates of complete rev0 ROM `phy_fe_txrx_reset`.
#[cfg(target_arch = "riscv32")]
pub fn reset_frontend_txrx(registers: &mut impl SharedPhyAccess) {
    ops::reset_frontend_txrx(&mut phy_blocks_mut(registers).phy_fedata_recovered);
}

/// Apply complete pinned `libphy.a[phy_reg.o]::phy_fe_reg_update`.
#[cfg(target_arch = "riscv32")]
pub fn update_front_end(registers: &mut impl SharedPhyAccess) {
    ops::update_front_end(&mut phy_blocks_mut(registers).phy_baseband_config_oracle);
}

/// Configure PHY I²C TX-rate fields and TX-gain compensation bytes.
///
/// Complete rev0 ROM `phy_i2c_txrate_init` at `0x2f82_86d0`, size `0x38`,
/// performs two rate updates, then dispatches to complete pinned
/// `phy_txgain_comp_pacfg_new(1)` for four ordered byte updates.
#[cfg(target_arch = "riscv32")]
pub fn configure_i2c_tx_rate(registers: &mut impl SharedPhyAccess) {
    ops::configure_i2c_tx_rate(&mut phy_blocks_mut(registers).phy_baseband_config_oracle);
}

/// Apply complete pinned `phy_txgain_comp_pacfg_new(1)` without coupling it
/// to calibration-tone setup.
#[cfg(target_arch = "riscv32")]
pub fn restore_tx_gain_compensation(registers: &mut impl SharedPhyAccess) {
    ops::restore_tx_gain_compensation(&mut phy_blocks_mut(registers).phy_baseband_config_oracle);
}

/// Configure the complete baseband watchdog leaf.
///
/// Basis: complete rev0 ROM `phy_bb_wdg_cfg` at `0x2f82_7860`, size `0x2c`.
#[cfg(target_arch = "riscv32")]
pub fn configure_watchdog(registers: &mut impl SharedPhyAccess) {
    ops::configure_watchdog(&mut phy_blocks_mut(registers).phy_baseband_config_oracle);
}

/// Enable the four recovered automatic noise-floor controls.
///
/// Basis: complete rev0 ROM `phy_noise_floor_auto_set` at `0x2f82_7d3c`,
/// size `0x36`.
#[cfg(target_arch = "riscv32")]
pub fn configure_noise_floor_auto(registers: &mut impl SharedPhyAccess) {
    ops::configure_noise_floor_auto(&mut phy_blocks_mut(registers).phy_baseband_config_oracle);
}

/// Apply the complete baseband register initialization leaf.
///
/// Complete rev0 ROM `phy_bb_reg_init` at `0x2f82_79c6`, size `0x140`,
/// supplies all local writes. Calls to the already owned NRX and platform
/// baseband-control leaves remain at their original positions.
#[cfg(target_arch = "riscv32")]
pub fn initialize_baseband(registers: &mut impl SharedPhyAccess) {
    crate::phy::sequence::initialize_baseband(phy_blocks_mut(registers));
}

/// Apply the complete six-edge PA-on configuration leaf.
///
/// Basis: complete rev0 ROM `phy_tx_paon_set` at `0x2f82_764c`, size `0x78`.
#[cfg(target_arch = "riscv32")]
pub fn configure_tx_pa_on(registers: &mut impl SharedPhyAccess) {
    ops::configure_tx_pa_on(&mut phy_blocks_mut(registers).phy_baseband_config_oracle);
}

/// Publish both signed digital gains through the shared PHY owner.
#[cfg(target_arch = "riscv32")]
pub fn configure_forced_digital_gain(
    registers: &mut impl SharedPhyAccess,
    enabled: bool,
    gain_0: i8,
    gain_1: i8,
) {
    ops::configure_forced_digital_gain(
        &mut phy_blocks_mut(registers).phy_clock_oracle,
        enabled,
        gain_0,
        gain_1,
    );
}
