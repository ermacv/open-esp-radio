//! Route-owned TSENS and peripheral SAR-I2C resources.
//!
//! SOURCE: pinned IDF `phy_override.c::phy_set_tsens_power`,
//! `sar_tsens_ctrl.c`, `sar_periph_ctrl_common.c` and S31
//! `regi2c_ctrl_ll.h`. The linked override, rather than the MMIO-only
//! libphy/ROM leaf, owns the last-release/first-acquire power/reset edges.

use super::restore::Restore;
use crate::owner::{SharedPhyAccess, phy_parts_mut};
use oer_esp32s31_pac::RadioPhyRegisters;

pub mod ops;

/// Named minimum settle between asserting SAR-I2C reset and enabling power.
pub const SAR_I2C_RESET_SETTLE_MICROS: u32 = 1;

/// Why the physical temperature/SAR-I2C owner could not change epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TemperaturePowerError {
    AlreadyAcquired,
    NotAcquired,
    /// Reset started but its minimum settle did not complete. Reset is held;
    /// the route cannot be reused or returned to the neutral root.
    ResetSettleFailed,
    Poisoned,
    /// A calibration must restore its fields before the resource is released.
    RestorePending,
}

trait TemperaturePort {
    fn cold_readout_prefix(&mut self);
    fn sar_reset_released(&mut self, released: bool);
    fn sar_power(&mut self, enabled: bool);
    fn sensor_clock(&mut self, enabled: bool);
    fn sensor_reset(&mut self, asserted: bool);
    fn sensor_power(&mut self, enabled: bool);
}

impl TemperaturePort for RadioPhyRegisters {
    fn cold_readout_prefix(&mut self) {
        // SOURCE: complete phy_tsens_read_init, before its tail call to the
        // linked IDF phy_set_tsens_power override. Preserve all four RMWs.
        let blocks = &mut self.peripherals;
        ops::enable_register_bank(&mut blocks.lp_tsens);
        ops::set_clock(&mut blocks.lp_periclkrst, true);
        ops::enable_phy_readout(&mut blocks.lp_tsens);
        ops::enable_phy_conversion(&mut blocks.lp_tsens);
    }
    fn sar_reset_released(&mut self, released: bool) {
        self.set_analog_i2c_reset_released(released);
    }
    fn sar_power(&mut self, enabled: bool) {
        self.set_analog_i2c_power(enabled);
    }
    fn sensor_clock(&mut self, enabled: bool) {
        ops::set_clock(&mut self.peripherals.lp_periclkrst, enabled);
    }
    fn sensor_reset(&mut self, asserted: bool) {
        ops::set_reset(&mut self.peripherals.lp_periclkrst, asserted);
    }
    fn sensor_power(&mut self, enabled: bool) {
        ops::set_power(&mut self.peripherals.lp_tsens, enabled);
    }
}

// Cold RF init acquires TSENS before calibration can own a restore slot.
// Keeping the restore payload inside the acquired state expresses that
// lifetime without a second field or an independent SAR-I2C reference count.
impl Restore {
    pub(crate) const fn blocks_release(&self) -> bool {
        !matches!(self, Self::TemperatureIdle)
    }

    fn acquire_with(
        &mut self,
        port: &mut impl TemperaturePort,
        settle_reset: impl FnOnce() -> bool,
        cold: bool,
    ) -> Result<(), TemperaturePowerError> {
        match self {
            Self::TemperaturePoisoned => return Err(TemperaturePowerError::Poisoned),
            Self::TemperatureIdle => {}
            _ => return Err(TemperaturePowerError::AlreadyAcquired),
        }
        *self = Self::TemperaturePoisoned;
        if cold {
            port.cold_readout_prefix();
        }
        port.sar_reset_released(false);
        if !settle_reset() {
            return Err(TemperaturePowerError::ResetSettleFailed);
        }
        port.sar_power(true);
        port.sar_reset_released(true);
        port.sensor_clock(true);
        port.sensor_reset(true);
        port.sensor_reset(false);
        port.sensor_power(true);
        *self = Self::Empty;
        Ok(())
    }

    fn release_with(
        &mut self,
        port: &mut impl TemperaturePort,
    ) -> Result<(), TemperaturePowerError> {
        match self {
            Self::TemperatureIdle => return Err(TemperaturePowerError::NotAcquired),
            Self::TemperaturePoisoned => return Err(TemperaturePowerError::Poisoned),
            Self::Empty => {}
            _ => return Err(TemperaturePowerError::RestorePending),
        }
        port.sensor_power(false);
        port.sensor_clock(false);
        port.sar_power(false);
        *self = Self::TemperatureIdle;
        Ok(())
    }
}

/// Configure the cold read path and acquire TSENS/SAR-I2C at the existing
/// RF-init action. `settle_reset` completes the named 1 µs hardware settle.
pub fn initialize(
    registers: &mut impl SharedPhyAccess,
    settle_reset: impl FnOnce() -> bool,
) -> Result<(), TemperaturePowerError> {
    let (phy, state) = phy_parts_mut(registers);
    state.resources_mut().acquire_with(phy, settle_reset, true)
}

/// First acquire after RF close: reset SAR-I2C, then clock/reset/enable TSENS.
pub fn acquire(
    registers: &mut impl SharedPhyAccess,
    settle_reset: impl FnOnce() -> bool,
) -> Result<(), TemperaturePowerError> {
    let (phy, state) = phy_parts_mut(registers);
    state.resources_mut().acquire_with(phy, settle_reset, false)
}

/// Last release after RF close: disable TSENS, its clock, then SAR-I2C power.
pub fn release(registers: &mut impl SharedPhyAccess) -> Result<(), TemperaturePowerError> {
    let (phy, state) = phy_parts_mut(registers);
    state.resources_mut().release_with(phy)
}

/// Sample one unsigned sensor code.
pub fn read_code(registers: &impl SharedPhyAccess) -> u8 {
    ops::read_code(&crate::phy_blocks(registers).lp_tsens)
}

#[cfg(test)]
mod tests;
