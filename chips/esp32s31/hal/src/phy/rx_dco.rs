//! Owned ESP32-S31 RX-DCO control-field access.
//!
//! The primary source is complete pinned
//! `libphy.a[phy_rx_cal.o]::phy_xtal_duty_cal`, size `0x3a6`. Complete rev0
//! ROM `phy_pbus_rx_dco_cal` at `0x2f82_8f44`, size `0x228`, independently
//! uses the same field around its bounded measurement graph.

#[cfg(target_arch = "riscv32")]
use crate::{
    owner::{SharedPhyAccess, phy_parts_mut},
    phy::restore::{RxDcoControlPrepareError, RxDcoControlRestoreError},
};

/// Retain and clear the RX-DCO calibration-control field.
///
/// Complete pinned `phy_xtal_duty_cal` saves bits 23:22 and clears them with
/// one fresh read before entering the nested RX-DCO measurement. The route
/// restore slot owns the saved field and supports the two reviewed nesting
/// levels.
#[cfg(target_arch = "riscv32")]
pub fn prepare_control_restore(
    registers: &mut impl SharedPhyAccess,
) -> Result<(), RxDcoControlPrepareError> {
    let (phy, restore) = phy_parts_mut(registers);
    restore.prepare_rx_dco_with(|| phy.capture_and_clear_rx_dco_control())
}

/// Restore the captured RX-DCO calibration-control bits.
///
/// Complete pinned `phy_xtal_duty_cal` performs one fresh read and replaces
/// only the generated field after the nested measurement. The saved value
/// never leaves the HAL.
#[cfg(target_arch = "riscv32")]
pub fn restore_control(
    registers: &mut impl SharedPhyAccess,
) -> Result<(), RxDcoControlRestoreError> {
    let (phy, restore) = phy_parts_mut(registers);
    restore.restore_rx_dco_with(|field| phy.restore_rx_dco_control(field))
}
