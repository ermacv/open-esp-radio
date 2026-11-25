//! Route-owned TSENS and peripheral SAR-I2C resources.
//!
//! SOURCE: pinned IDF `phy_override.c::phy_set_tsens_power`,
//! `sar_tsens_ctrl.c`, `sar_periph_ctrl_common.c` and S31
//! `regi2c_ctrl_ll.h`. The linked override, rather than the MMIO-only
//! libphy/ROM leaf, owns the last-release/first-acquire power/reset edges.

use super::restore::Restore;
use crate::radio::phy::RadioPhyRegisters;
use crate::system::owner::{SharedPhyAccess, phy_parts_mut};
use oer_chips_esp32s31_pac::{
    dictionaries::Reset, lp_peri_clkrst::tsens_ctrl::LpTsensClkEn, tsens::ctrl::PowerUp,
};

pub mod ops;

/// Named minimum settle between asserting SAR-I2C reset and enabling power.
pub const SAR_I2C_RESET_SETTLE: oer_portable_settle::SettleLimit =
    match oer_portable_settle::SettleLimit::new(oer_portable_time::Duration::from_micros(1)) {
        Some(limit) => limit,
        None => panic!("the SAR-I2C reset settle lies within the longest a settle may wait"),
    };

/// Why the physical temperature/SAR-I2C owner could not change epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TemperaturePowerError {
    AlreadyAcquired,
    NotAcquired,
    /// A calibration must restore its fields before the resource is released.
    RestorePending,
}

/// Whether an acquisition first configures the cold readout path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Acquisition {
    /// RF initialization: the readout path is configured first.
    Cold,
    /// After an RF close: the path is configured already.
    Warm,
}

trait TemperaturePort {
    fn cold_readout_prefix(&mut self);
    fn hold_sar_reset(&mut self);
    fn release_sar_reset(&mut self);
    fn power_on_sar(&mut self);
    fn power_off_sar(&mut self);
    fn sensor_clock(&mut self, clock: LpTsensClkEn);
    fn sensor_reset(&mut self, reset: Reset);
    fn sensor_power(&mut self, power: PowerUp);
}

impl<P: crate::radio::phy::registers::RadioPlacement> TemperaturePort for RadioPhyRegisters<P> {
    fn cold_readout_prefix(&mut self) {
        // SOURCE: complete phy_tsens_read_init, before its tail call to the
        // linked IDF phy_set_tsens_power override. Preserve all four RMWs.
        let blocks = &mut self.peripherals;
        ops::enable_register_bank(&mut blocks.lp_tsens);
        ops::set_clock(&mut blocks.lp_periclkrst, LpTsensClkEn::Enabled);
        ops::enable_phy_readout(&mut blocks.lp_tsens);
        ops::enable_phy_conversion(&mut blocks.lp_tsens);
    }
    fn hold_sar_reset(&mut self) {
        self.assert_analog_i2c_reset();
    }
    fn release_sar_reset(&mut self) {
        self.release_analog_i2c_reset();
    }
    fn power_on_sar(&mut self) {
        self.power_on_analog_i2c();
    }
    fn power_off_sar(&mut self) {
        self.power_off_analog_i2c();
    }
    fn sensor_clock(&mut self, clock: LpTsensClkEn) {
        ops::set_clock(&mut self.peripherals.lp_periclkrst, clock);
    }
    fn sensor_reset(&mut self, reset: Reset) {
        ops::set_reset(&mut self.peripherals.lp_periclkrst, reset);
    }
    fn sensor_power(&mut self, power: PowerUp) {
        ops::set_power(&mut self.peripherals.lp_tsens, power);
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
        settle_reset: impl FnOnce(),
        acquisition: Acquisition,
    ) -> Result<(), TemperaturePowerError> {
        match self {
            Self::TemperatureIdle => {}
            _ => return Err(TemperaturePowerError::AlreadyAcquired),
        }
        if acquisition == Acquisition::Cold {
            port.cold_readout_prefix();
        }
        port.hold_sar_reset();
        settle_reset();
        port.power_on_sar();
        port.release_sar_reset();
        port.sensor_clock(LpTsensClkEn::Enabled);
        port.sensor_reset(Reset::Asserted);
        port.sensor_reset(Reset::Released);
        port.sensor_power(PowerUp::Enabled);
        *self = Self::Empty;
        Ok(())
    }

    fn release_with(
        &mut self,
        port: &mut impl TemperaturePort,
    ) -> Result<(), TemperaturePowerError> {
        match self {
            Self::TemperatureIdle => return Err(TemperaturePowerError::NotAcquired),
            Self::Empty => {}
            _ => return Err(TemperaturePowerError::RestorePending),
        }
        port.sensor_power(PowerUp::Disabled);
        port.sensor_clock(LpTsensClkEn::Disabled);
        port.power_off_sar();
        *self = Self::TemperatureIdle;
        Ok(())
    }
}

/// Configure the cold read path and acquire TSENS/SAR-I2C at the existing
/// RF-init action. `settle_reset` completes the named 1 µs hardware settle.
pub fn initialize(
    registers: &mut impl SharedPhyAccess,
    settle_reset: impl FnOnce(),
) -> Result<(), TemperaturePowerError> {
    let (phy, state) = phy_parts_mut(registers);
    state
        .resources_mut()
        .acquire_with(phy, settle_reset, Acquisition::Cold)
}

/// First acquire after RF close: reset SAR-I2C, then clock/reset/enable TSENS.
pub fn acquire(
    registers: &mut impl SharedPhyAccess,
    settle_reset: impl FnOnce(),
) -> Result<(), TemperaturePowerError> {
    let (phy, state) = phy_parts_mut(registers);
    state
        .resources_mut()
        .acquire_with(phy, settle_reset, Acquisition::Warm)
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
