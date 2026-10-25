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

use oer_esp32s31_pac::fe::PhyRxDcoOracle;
use oer_mmio::{Bits, Placement};

/// Opaque capture of the two-bit RX-DCO calibration control field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxDcoControlField(Bits<2>);

impl Default for RxDcoControlField {
    /// The capture of a clear field.
    fn default() -> Self {
        Self(Bits::constant::<0>())
    }
}

impl RxDcoControlField {
    /// Construct one distinguishable capture for a host test.
    #[cfg(test)]
    pub(crate) const fn for_host_test(value: u32) -> Self {
        Self(Bits::truncate(value))
    }
}

/// Capture the RX-DCO control field, then clear it with a fresh RMW.
pub fn capture_and_clear<P: Placement>(rx_dco: &mut PhyRxDcoOracle<P>) -> RxDcoControlField {
    let saved = rx_dco.read_control().bits_22_23_opaque();
    // SOURCE: BLOB_LIBPHY_PHY_XTAL_DUTY_CAL, ROM_REV0_PHY_RX_DCO_CAL.
    rx_dco.modify_control(|w| {
        w.set_bits_22_23_opaque(Bits::constant::<0x0>());
    });
    RxDcoControlField(saved)
}

/// Restore one captured RX-DCO control field with a fresh RMW.
pub fn restore<P: Placement>(rx_dco: &mut PhyRxDcoOracle<P>, field: RxDcoControlField) {
    rx_dco.modify_control(|w| {
        w.set_bits_22_23_opaque(field.0);
    });
}

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
    restore.prepare_rx_dco_with(|| capture_and_clear(&mut phy.peripherals.phy_rx_dco_oracle))
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
    restore.restore_rx_dco_with(|field| restore(&mut phy.peripherals.phy_rx_dco_oracle, field))
}
