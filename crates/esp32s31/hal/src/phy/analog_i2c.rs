//! ESP32-S31 PMU operations used by the open PHY.
//!
//! Operation order comes from complete ROM/blob bodies. Register ownership
//! and field decoding remain behind the affine shared-PHY PAC capability.

use crate::owner::SharedPhyAccess;

/// Private seam used to verify the recovered operation sequence without
/// exposing a second register-owner abstraction.
trait AnalogI2cPowerAccess {
    fn set_rf_circuit_power(&mut self, enabled: bool);
    fn set_bb_i2c_power_tie(&mut self, enabled: bool);
    fn analog_i2c_is_powered(&self) -> bool;
    fn set_analog_i2c_power(&mut self, enabled: bool);
    fn analog_i2c_reset_is_released(&self) -> bool;
    fn set_analog_i2c_reset_released(&mut self, released: bool);
}

impl<T: SharedPhyAccess + ?Sized> AnalogI2cPowerAccess for T {
    fn set_rf_circuit_power(&mut self, enabled: bool) {
        SharedPhyAccess::set_rf_circuit_power(self, enabled);
    }

    fn set_bb_i2c_power_tie(&mut self, enabled: bool) {
        SharedPhyAccess::set_bb_i2c_power_tie(self, enabled);
    }

    fn analog_i2c_is_powered(&self) -> bool {
        SharedPhyAccess::analog_i2c_is_powered(self)
    }

    fn set_analog_i2c_power(&mut self, enabled: bool) {
        SharedPhyAccess::set_analog_i2c_power(self, enabled);
    }

    fn analog_i2c_reset_is_released(&self) -> bool {
        SharedPhyAccess::analog_i2c_reset_is_released(self)
    }

    fn set_analog_i2c_reset_released(&mut self, released: bool) {
        SharedPhyAccess::set_analog_i2c_reset_released(self, released);
    }
}

fn prepare_open_i2c_pre_delay_sequence(registers: &mut impl AnalogI2cPowerAccess) {
    registers.set_rf_circuit_power(false);
    registers.set_bb_i2c_power_tie(false);
}

fn complete_open_i2c_power_and_reset_sequence(registers: &mut impl AnalogI2cPowerAccess) {
    registers.set_rf_circuit_power(true);
    registers.set_bb_i2c_power_tie(true);

    if !registers.analog_i2c_is_powered() {
        registers.set_analog_i2c_power(true);
        registers.set_analog_i2c_reset_released(false);
        registers.set_analog_i2c_reset_released(true);
    }
    if !registers.analog_i2c_reset_is_released() {
        registers.set_analog_i2c_reset_released(true);
    }
}

/// Apply the two PMU updates before the 100 us analog-I2C power delay.
///
/// SOURCE\[`libphy.a[phy_reg.o]::phy_open_i2c_xpd_new`, offsets
/// `0x2e..0x4e`]. It powers down the RF circuit and releases the forced
/// baseband analog-I2C power tie before the delay.
pub fn prepare_open_i2c_pre_delay(registers: &mut impl SharedPhyAccess) {
    prepare_open_i2c_pre_delay_sequence(registers);
}

/// Clear the RF-circuit power image inside complete `phy_xpd_rf_new`.
///
/// The surrounding close transaction owns the adjacent baseband, I2C and
/// clock operations; this leaf performs only the instruction-evidenced PMU
/// update.
pub fn power_off_rf_circuits(registers: &mut impl SharedPhyAccess) {
    SharedPhyAccess::set_rf_circuit_power(registers, false);
}

/// Power the RF/analog-I2C circuits and release the peripheral-I2C reset.
///
/// SOURCE\[complete `libphy.a[phy_reg.o]::phy_open_i2c_xpd_new`]. When
/// analog-I2C was powered down, reset is explicitly asserted before release;
/// this edge is deliberately not collapsed into one final write.
pub fn complete_open_i2c_power_and_reset(registers: &mut impl SharedPhyAccess) {
    complete_open_i2c_power_and_reset_sequence(registers);
}

/// Complete the ROM frontend/baseband clock leaf after internal radio gates.
///
/// SOURCE\[complete rev0 ROM `phy_open_fe_bb_clk`]. The PMU update is the
/// fourth and final operation; the first three undocumented radio gates are
/// executed by the recovered radio PAC before this call.
pub fn enable_frontend_baseband_power(registers: &mut impl SharedPhyAccess) {
    SharedPhyAccess::enable_frontend_baseband_power(registers);
}

#[cfg(test)]
mod tests;
