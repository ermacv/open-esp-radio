//! Safe, ownership-bound access to the recovered modem clock/reset registers.
//!
//! Register layout and field positions come from `chips/esp32s31/registers/published/radio.svd`.
//! Operation values are independently evidenced by the qualified ESP32-S31
//! `esp-hal` clock implementation. The complete cold-boot
//! ordering intentionally remains in the HAL crate.

#![forbid(unsafe_code)]

use crate::RadioPhyRegisters;

impl RadioPhyRegisters {
    /// Clear the Wi-Fi enable bit, the only baseband-control bit complete
    /// `phy_xpd_rf_new` changes.
    ///
    /// SOURCE\[BLOB_LIBPHY_PHY_XPD_RF_NEW_20260907]; the vendor leaf performs
    /// one preserving update of `WIFI_BB_CFG` that clears bit one alone
    /// before its one-microsecond settle.
    pub fn clear_phy_rf_baseband_control(&mut self) {
        crate::generated::clear_phy_rf_baseband_control(&self.peripherals.modem_syscon_radio);
    }

    /// Release the baseband analog-I2C power tie, the only immediate-clock
    /// bit complete `phy_xpd_rf_new` changes.
    ///
    /// SOURCE\[BLOB_LIBPHY_PHY_XPD_RF_NEW_20260907]; the vendor leaf clears
    /// bit 28 of `IMM_HP_CK_POWER_0` in one preserving update and keeps the
    /// crystal ties in bits 31:30.
    pub fn clear_phy_rf_immediate_clock_power(&mut self) {
        crate::generated::clear_phy_rf_immediate_clock_power(&self.peripherals.pmu_radio);
    }

    /// Enable or disable the recovered PHY calibration clock.
    ///
    /// SOURCE\[BLOB_LIBPHY_PHY_BB_INIT]; the complete parent sets this bit
    /// before executing the baseband calibration sequence.
    pub fn set_phy_calibration_clock(&mut self, enabled: bool) {
        if enabled {
            crate::generated::enable_phy_calibration_clock(&self.peripherals.phy_clock_oracle);
        } else {
            crate::generated::disable_phy_calibration_clock(&self.peripherals.phy_clock_oracle);
        }
    }

    /// Open the three undocumented front-end and baseband radio clock gates.
    ///
    /// SOURCE\[ROM_REV0_PHY_OPEN_FE_BB_CLK]; these are the first three
    /// operations in the complete ROM body. Its fourth PMU operation belongs
    /// to the official platform PAC and is sequenced by the HAL.
    pub fn open_frontend_baseband_internal_clocks(&mut self) {
        crate::svd::fixed_register_image::open_frontend_clock_gates(
            &self.peripherals.phy_clock_oracle,
        );

        crate::generated::open_frontend_baseband_control(&self.peripherals.phy_clock_oracle);

        crate::svd::fixed_register_write::open_baseband_clock_gates(
            &self.peripherals.phy_clock_oracle,
        );
    }

    /// Close the recovered front-end and baseband clock gates.
    ///
    /// SOURCE\[BLOB_LIBPHY_PHY_CLOSE_FE_BB_CLK]; this is the exact inverse
    /// three-operation blob leaf. It intentionally leaves PMU power policy to
    /// the surrounding lifecycle owner, matching the source.
    pub fn close_frontend_baseband_clocks(&mut self) {
        crate::svd::fixed_register_image::close_frontend_clock_gates(
            &self.peripherals.phy_clock_oracle,
        );

        crate::generated::close_frontend_baseband_control(&self.peripherals.phy_clock_oracle);

        crate::svd::fixed_register_write::close_baseband_clock_gates(
            &self.peripherals.phy_clock_oracle,
        );
    }
}
