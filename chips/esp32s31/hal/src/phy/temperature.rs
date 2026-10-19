//! Owned ESP32-S31 temperature-sensor MMIO leaves.
//!
//! The primary sources are pinned `libphy.a[phy_tsens.o]::phy_tsens_read_init`,
//! size `0x36`, and the complete rev0 ROM `phy_set_tsens_power_` body at
//! `0x2f82_5dc8`, size `0x1c`. Temperature-code identity is independently
//! proven by complete ROM `phy_tsens_code_read` at `0x2f82_5ee0`, size
//! `0x0c`, and `phy_tsens_temp_read_local` at `0x2f82_5f1e`, size `0x5e`.

/// Official LP peripheral capabilities needed by PHY temperature handling.
pub trait PhyTemperatureSystemControl {
    /// Enable the LP temperature-sensor register bank.
    fn enable_temperature_sensor_register_bank(&mut self);
    /// Enable the LP temperature-sensor peripheral clock.
    fn enable_temperature_sensor_clock(&mut self);
    /// Set the blob-evidenced readout-enable field.
    fn enable_temperature_sensor_phy_readout(&mut self);
    /// Set the blob-evidenced conversion-enable field.
    fn enable_temperature_sensor_phy_conversion(&mut self);
    /// Power the temperature sensor through the official control field.
    fn enable_temperature_sensor_power(&mut self);
    /// Power down the temperature sensor after the RF domain is closed.
    fn disable_temperature_sensor_power(&mut self);
    /// Sample the unsigned temperature code exactly once.
    fn read_temperature_sensor_code(&self) -> u8;
}

impl<T: crate::owner::SharedPhyAccess> PhyTemperatureSystemControl for T {
    fn enable_temperature_sensor_register_bank(&mut self) {
        crate::phy_pac_mut(self).enable_temperature_sensor_register_bank();
    }

    fn enable_temperature_sensor_clock(&mut self) {
        crate::phy_pac_mut(self).enable_temperature_sensor_clock();
    }

    fn enable_temperature_sensor_phy_readout(&mut self) {
        crate::phy_pac_mut(self).enable_temperature_sensor_phy_readout();
    }

    fn enable_temperature_sensor_phy_conversion(&mut self) {
        crate::phy_pac_mut(self).enable_temperature_sensor_phy_conversion();
    }

    fn enable_temperature_sensor_power(&mut self) {
        crate::phy_pac_mut(self).enable_temperature_sensor_power();
    }

    fn disable_temperature_sensor_power(&mut self) {
        crate::phy_pac_mut(self).disable_temperature_sensor_power();
    }

    fn read_temperature_sensor_code(&self) -> u8 {
        crate::phy_pac(self).read_temperature_sensor_code()
    }
}

/// Configure the temperature-sensor read path.
///
/// Complete pinned `phy_tsens_read_init` performs five independent
/// read/modify/write transactions in this order: sensor-control bit 0,
/// system-control bit 30, sensor-control bit 23, sensor-control bit 9, then
/// sensor power bit 22. The final transaction is the inlined semantics of
/// complete ROM `phy_set_tsens_power_(1)`. Both archive ABI arguments are
/// ignored and therefore do not cross this safe boundary.
pub fn initialize(platform: &mut impl PhyTemperatureSystemControl) {
    platform.enable_temperature_sensor_register_bank();
    platform.enable_temperature_sensor_clock();
    platform.enable_temperature_sensor_phy_readout();
    platform.enable_temperature_sensor_phy_conversion();
    platform.enable_temperature_sensor_power();
}

/// Sample the unsigned temperature code exactly once.
///
/// Complete ROM `phy_tsens_code_read` and `phy_tsens_temp_read_local` both
/// read `SENSOR_CODE_POWER` once and zero-extend its low-byte `CODE` field.
/// Readiness, conversion arithmetic and range selection remain in the
/// caller-driven PHY transition.
pub fn read_code(platform: &impl PhyTemperatureSystemControl) -> u8 {
    platform.read_temperature_sensor_code()
}

/// Restore temperature-sensor power during retained PHY wake.
///
/// Unlike [`initialize`], current-vendor `phy_wakeup_init` only reasserts the
/// power field. The register-bank, clock, readout and conversion setup remain
/// retained across the matching RF-close transition.
pub fn power_up(platform: &mut impl PhyTemperatureSystemControl) {
    platform.enable_temperature_sensor_power();
}

/// Power down the temperature sensor after the shared RF close transaction.
///
/// This is deliberately separate from RF close because Espressif's open
/// lifecycle wrapper owns this outer system-policy edge.
pub fn power_down(platform: &mut impl PhyTemperatureSystemControl) {
    platform.disable_temperature_sensor_power();
}

#[cfg(test)]
mod tests;
