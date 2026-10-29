//! Owned PMU atoms and the family analog-I2C power port.

use crate::owner::SharedPhyAccess;
use oer_espressif_phy::analog_i2c::AnalogI2cPowerAccess;

/// Exclusive borrowed PMU atoms; no register or restore authority escapes.
pub struct PowerPort<'a, T: ?Sized>(&'a mut T);

/// Borrow the current PHY owner's power atoms for a family operation.
pub fn power_port<T: SharedPhyAccess + ?Sized>(owner: &mut T) -> PowerPort<'_, T> {
    PowerPort(owner)
}

impl<T: SharedPhyAccess + ?Sized> AnalogI2cPowerAccess for PowerPort<'_, T> {
    fn set_rf_circuit_power(&mut self, enabled: bool) {
        SharedPhyAccess::set_rf_circuit_power(self.0, enabled);
    }

    fn set_bb_i2c_power_tie(&mut self, enabled: bool) {
        SharedPhyAccess::set_bb_i2c_power_tie(self.0, enabled);
    }

    fn analog_i2c_is_powered(&self) -> bool {
        SharedPhyAccess::analog_i2c_is_powered(self.0)
    }

    fn set_analog_i2c_power(&mut self, enabled: bool) {
        SharedPhyAccess::set_analog_i2c_power(self.0, enabled);
    }

    fn analog_i2c_reset_is_released(&self) -> bool {
        SharedPhyAccess::analog_i2c_reset_is_released(self.0)
    }

    fn set_analog_i2c_reset_released(&mut self, released: bool) {
        SharedPhyAccess::set_analog_i2c_reset_released(self.0, released);
    }
}

/// Clear the RF-circuit power image inside complete `phy_xpd_rf_new`.
///
/// The surrounding close transaction owns the adjacent baseband, I2C and
/// clock operations; this leaf performs only the instruction-evidenced PMU
/// update.
pub fn power_off_rf_circuits(registers: &mut impl SharedPhyAccess) {
    SharedPhyAccess::set_rf_circuit_power(registers, false);
}

/// Complete the ROM frontend/baseband clock leaf after internal radio gates.
///
/// SOURCE\[complete rev0 ROM `phy_open_fe_bb_clk`]. The PMU update is the
/// fourth and final operation; the first three undocumented radio gates are
/// executed by the recovered radio PAC before this call.
pub fn enable_frontend_baseband_power(registers: &mut impl SharedPhyAccess) {
    SharedPhyAccess::enable_frontend_baseband_power(registers);
}
