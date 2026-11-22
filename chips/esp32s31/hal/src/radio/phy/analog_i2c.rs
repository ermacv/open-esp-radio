//! Owned PMU atoms and the family analog-I2C power port.

use crate::system::owner::SharedPhyAccess;
use oer_chips_family_radio_phy::analog_i2c::AnalogI2cPowerAccess;

/// Exclusive borrowed PMU atoms; no register or restore authority escapes.
pub struct PowerPort<'a, T: ?Sized>(&'a mut T);

/// Borrow the current PHY owner's power atoms for a family operation.
pub fn power_port<T: SharedPhyAccess + ?Sized>(owner: &mut T) -> PowerPort<'_, T> {
    PowerPort(owner)
}

// The family port's one translation of its flags into the atoms.
impl<T: SharedPhyAccess + ?Sized> AnalogI2cPowerAccess for PowerPort<'_, T> {
    fn set_rf_circuit_power(&mut self, enabled: bool) {
        if enabled {
            SharedPhyAccess::power_on_rf_circuits(self.0);
        } else {
            SharedPhyAccess::power_off_rf_circuits(self.0);
        }
    }

    /// The tie is a write trigger: its release writes nothing (the vendor's
    /// clearing RMW has no counterpart in that access class).
    fn set_bb_i2c_power_tie(&mut self, enabled: bool) {
        if enabled {
            SharedPhyAccess::tie_baseband_i2c_power(self.0);
        }
    }

    fn analog_i2c_is_powered(&self) -> bool {
        SharedPhyAccess::analog_i2c_is_powered(self.0)
    }

    fn set_analog_i2c_power(&mut self, enabled: bool) {
        if enabled {
            SharedPhyAccess::power_on_analog_i2c(self.0);
        } else {
            SharedPhyAccess::power_off_analog_i2c(self.0);
        }
    }

    fn analog_i2c_reset_is_released(&self) -> bool {
        SharedPhyAccess::analog_i2c_reset_is_released(self.0)
    }

    fn set_analog_i2c_reset_released(&mut self, released: bool) {
        if released {
            SharedPhyAccess::release_analog_i2c_reset(self.0);
        } else {
            SharedPhyAccess::assert_analog_i2c_reset(self.0);
        }
    }
}

/// Clear the RF-circuit power image inside complete `phy_xpd_rf_new`.
///
/// The surrounding close transaction owns the adjacent baseband, I2C and
/// clock operations; this leaf performs only the instruction-evidenced PMU
/// update.
pub fn power_off_rf_circuits(registers: &mut impl SharedPhyAccess) {
    SharedPhyAccess::power_off_rf_circuits(registers);
}

/// Complete the ROM frontend/baseband clock leaf after internal radio gates.
///
/// SOURCE\[complete rev0 ROM `phy_open_fe_bb_clk`]. The PMU update is the
/// fourth and final operation; the first three undocumented radio gates are
/// executed by the recovered radio PAC before this call.
pub fn enable_frontend_baseband_power(registers: &mut impl SharedPhyAccess) {
    SharedPhyAccess::enable_frontend_baseband_power(registers);
}
