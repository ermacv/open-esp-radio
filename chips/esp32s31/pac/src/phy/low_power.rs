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
        self.peripherals
            .lp_aon_clkrst
            .select_lp_power_detector_circuit_mode(LpPowerDetectorCircuitMode::Initialization);
    }

    /// Select encoding two used by complete TX-calibration debug mode.
    pub fn select_power_detector_calibration_mode(&mut self) {
        self.peripherals
            .lp_aon_clkrst
            .select_lp_power_detector_circuit_mode(LpPowerDetectorCircuitMode::Calibration);
    }

    /// Enable the LP temperature-sensor register bank.
    pub fn enable_temperature_sensor_register_bank(&mut self) {
        self.peripherals
            .lp_tsens
            .enable_lp_tsens_register_bank(LpTsensRegisterBankEnable::Enabled);
    }

    /// Enable the LP temperature-sensor peripheral clock.
    pub fn set_temperature_sensor_clock(&mut self, enabled: bool) {
        self.peripherals
            .lp_periclkrst
            .enable_lp_tsens_peripheral_clock(if enabled {
                LpTsensPeripheralClockEnable::Enabled
            } else {
                LpTsensPeripheralClockEnable::Disabled
            });
    }

    /// Assert or release the first-owner TSENS peripheral reset.
    pub fn set_temperature_sensor_reset(&mut self, asserted: bool) {
        if asserted {
            self.peripherals.lp_periclkrst.assert_lp_tsens_reset();
        } else {
            self.peripherals.lp_periclkrst.release_lp_tsens_reset();
        }
    }

    /// Enable the blob-evidenced PHY readout path.
    pub fn enable_temperature_sensor_phy_readout(&mut self) {
        self.peripherals
            .lp_tsens
            .enable_lp_tsens_phy_readout(LpTsensPhyReadoutEnable::Enabled);
    }

    /// Enable the blob-evidenced PHY conversion path.
    pub fn enable_temperature_sensor_phy_conversion(&mut self) {
        self.peripherals
            .lp_tsens
            .enable_lp_tsens_phy_conversion(LpTsensPhyConversionEnable::Enabled);
    }

    /// Power the sensor through the complete-ROM-evidenced control bit.
    pub fn enable_temperature_sensor_power(&mut self) {
        self.peripherals
            .lp_tsens
            .enable_lp_tsens_power(LpTsensPowerEnable::Enabled);
    }

    /// Power down the temperature sensor through complete ROM
    /// `phy_set_tsens_power_(0)` semantics.
    pub fn disable_temperature_sensor_power(&mut self) {
        self.peripherals
            .lp_tsens
            .enable_lp_tsens_power(LpTsensPowerEnable::Disabled);
    }

    /// Sample and return the unsigned temperature code exactly once.
    pub fn read_temperature_sensor_code(&self) -> u8 {
        self.peripherals.lp_tsens.read_ctrl().out()
    }
}
