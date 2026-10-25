//! Owned ESP32-S31 internal PHY clock-gate leaves.
//!
//! Platform clock/reset and power policy stay in the integration layer. This
//! module only sequences radio-internal registers.

#[cfg(target_arch = "riscv32")]
use crate::{owner::SharedPhyAccess, phy_blocks_mut};

pub mod ops;

/// Clear the shared Wi-Fi enable bit as RF shutdown does.
///
/// SOURCE\[BLOB_LIBPHY_PHY_XPD_RF_NEW_20260907]. This is one preserving
/// register transaction that keeps every other `WIFI_BB_CFG` bit; it must not
/// be decomposed into role-level toggles.
#[cfg(target_arch = "riscv32")]
pub fn clear_rf_baseband_control(registers: &mut impl SharedPhyAccess) {
    ops::clear_rf_baseband_control(&mut phy_blocks_mut(registers).modem_syscon_radio);
}

/// Apply complete pinned `libphy.a[phy_init.o]::phy_close_fe_bb_clk`.
#[cfg(target_arch = "riscv32")]
pub fn close_frontend_baseband(registers: &mut impl SharedPhyAccess) {
    ops::close_frontend_baseband_clocks(&mut phy_blocks_mut(registers).phy_clock_oracle);
}
