//! Analog-I2C power/reset order. SOURCE: complete libphy phy_open_i2c_xpd_new.

pub trait AnalogI2cPowerAccess {
    fn set_rf_circuit_power(&mut self, enabled: bool);
    fn set_bb_i2c_power_tie(&mut self, enabled: bool);
    fn analog_i2c_is_powered(&self) -> bool;
    fn set_analog_i2c_power(&mut self, enabled: bool);
    fn analog_i2c_reset_is_released(&self) -> bool;
    fn set_analog_i2c_reset_released(&mut self, released: bool);
}

pub fn prepare_open_i2c_pre_delay(registers: &mut impl AnalogI2cPowerAccess) {
    registers.set_rf_circuit_power(false);
    registers.set_bb_i2c_power_tie(false);
}

pub fn complete_open_i2c_power_and_reset(registers: &mut impl AnalogI2cPowerAccess) {
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

#[cfg(test)]
mod tests;
