//! Closed LP-system capabilities used by the shared ESP32-S31 PHY lifecycle.
//!
//! The raw register blocks remain inside [`crate::RadioPhyRegisters`]. Public
//! callers can request only complete ROM/libphy-evidenced transitions and one
//! read-only temperature-code observation; no generic LP register accessor is
//! exposed.

#![forbid(unsafe_code)]

use crate::{
    RadioPhyRegisters,
    generated::{
        LpPowerDetectorCircuitMode, LpTsensPeripheralClockEnable, LpTsensPhyConversionEnable,
        LpTsensPhyReadoutEnable, LpTsensPowerEnable, LpTsensRegisterBankEnable,
    },
};

impl RadioPhyRegisters {
    /// Select encoding four used by complete power-detector initialization and
    /// enable bodies.
    pub fn select_power_detector_initialization_mode(&mut self) {
        crate::generated::select_lp_power_detector_circuit_mode(
            &self.peripherals.lp_aon_clkrst,
            LpPowerDetectorCircuitMode::Initialization,
        );
    }

    /// Select encoding two used by complete TX-calibration debug mode.
    pub fn select_power_detector_calibration_mode(&mut self) {
        crate::generated::select_lp_power_detector_circuit_mode(
            &self.peripherals.lp_aon_clkrst,
            LpPowerDetectorCircuitMode::Calibration,
        );
    }

    /// Enable the LP temperature-sensor register bank.
    pub fn enable_temperature_sensor_register_bank(&mut self) {
        crate::generated::enable_lp_tsens_register_bank(
            &self.peripherals.lp_tsens,
            LpTsensRegisterBankEnable::Enabled,
        );
    }

    /// Enable the LP temperature-sensor peripheral clock.
    pub fn enable_temperature_sensor_clock(&mut self) {
        crate::generated::enable_lp_tsens_peripheral_clock(
            &self.peripherals.lp_periclkrst,
            LpTsensPeripheralClockEnable::Enabled,
        );
    }

    /// Enable the blob-evidenced PHY readout path.
    pub fn enable_temperature_sensor_phy_readout(&mut self) {
        crate::generated::enable_lp_tsens_phy_readout(
            &self.peripherals.lp_tsens,
            LpTsensPhyReadoutEnable::Enabled,
        );
    }

    /// Enable the blob-evidenced PHY conversion path.
    pub fn enable_temperature_sensor_phy_conversion(&mut self) {
        crate::generated::enable_lp_tsens_phy_conversion(
            &self.peripherals.lp_tsens,
            LpTsensPhyConversionEnable::Enabled,
        );
    }

    /// Power the sensor through the complete-ROM-evidenced control bit.
    pub fn enable_temperature_sensor_power(&mut self) {
        crate::generated::enable_lp_tsens_power(
            &self.peripherals.lp_tsens,
            LpTsensPowerEnable::Enabled,
        );
    }

    /// Power down the temperature sensor through complete ROM
    /// `phy_set_tsens_power_(0)` semantics.
    pub fn disable_temperature_sensor_power(&mut self) {
        crate::generated::enable_lp_tsens_power(
            &self.peripherals.lp_tsens,
            LpTsensPowerEnable::Disabled,
        );
    }

    /// Sample and return the unsigned temperature code exactly once.
    pub fn read_temperature_sensor_code(&self) -> u8 {
        crate::svd::field_read::read_lp_temperature_sensor_code(&self.peripherals.lp_tsens)
    }
}
