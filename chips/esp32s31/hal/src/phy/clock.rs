//! Owned ESP32-S31 internal PHY clock-gate leaves.
//!
//! Platform clock/reset and power policy stay in the integration layer. This
//! module only sequences radio-internal registers described by the SVD/PAC.

#[cfg(target_arch = "riscv32")]
use crate::{owner::SharedPhyAccess, phy_pac_mut};

/// Clear the shared Wi-Fi enable bit as RF shutdown does.
///
/// SOURCE\[BLOB_LIBPHY_PHY_XPD_RF_NEW_20260907]. This is one preserving
/// register transaction that keeps every other `WIFI_BB_CFG` bit; it must not
/// be decomposed into role-level toggles.
#[cfg(target_arch = "riscv32")]
pub fn clear_rf_baseband_control(registers: &mut impl SharedPhyAccess) {
    phy_pac_mut(registers).clear_phy_rf_baseband_control();
}

/// Release the PMU baseband analog-I2C power tie as RF shutdown does,
/// keeping the crystal ties.
///
/// SOURCE\[BLOB_LIBPHY_PHY_XPD_RF_NEW_20260907].
#[cfg(target_arch = "riscv32")]
pub fn clear_rf_immediate_clock_power(registers: &mut impl SharedPhyAccess) {
    phy_pac_mut(registers).clear_phy_rf_immediate_clock_power();
}

/// Apply complete pinned `libphy.a[phy_init.o]::phy_close_fe_bb_clk`.
#[cfg(target_arch = "riscv32")]
pub fn close_frontend_baseband(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.close_frontend_baseband_clocks();
}
