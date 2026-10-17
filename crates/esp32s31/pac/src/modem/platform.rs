//! Route-owned PMU and upstream radio clock transactions.

use core::num::NonZeroU32;

use crate::{
    RadioPhyRegisters, generated::ModemSysconClockGateState,
    modem::syscon::ModemSysconPowerBaseline,
};

/// Semantic readback of the system-clock prerequisites shared by all radios.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PlatformClockPowerObservation {
    pub hp_active_icg_selected: bool,
    pub modem_register_bus_clock_enabled: bool,
    pub modem_source_clocks_configured: bool,
}

/// Opaque readback of the upstream PLL-source fields a route may change.
///
/// The modem reset is not part of it: the first power-up since boot releases
/// it and nothing reasserts it, as ESP-IDF's PHY enable/disable never does.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PlatformPllSourceBaseline {
    modem_apb_clock_enabled: bool,
    modem_source_clock_enabled: bool,
    modem_pll_selected: bool,
    modem_pll_clock_enabled: bool,
    modem_xtal_clock_enabled: bool,
}

impl PlatformPllSourceBaseline {
    const BIT_COUNT: u32 = 5;

    fn bits(self) -> u32 {
        u32::from(self.modem_apb_clock_enabled)
            | (u32::from(self.modem_source_clock_enabled) << 1)
            | (u32::from(self.modem_pll_selected) << 2)
            | (u32::from(self.modem_pll_clock_enabled) << 3)
            | (u32::from(self.modem_xtal_clock_enabled) << 4)
    }

    fn from_bits(bits: u32) -> Self {
        Self {
            modem_apb_clock_enabled: bits & (1 << 0) != 0,
            modem_source_clock_enabled: bits & (1 << 1) != 0,
            modem_pll_selected: bits & (1 << 2) != 0,
            modem_pll_clock_enabled: bits & (1 << 3) != 0,
            modem_xtal_clock_enabled: bits & (1 << 4) != 0,
        }
    }
}

/// Packed semantic cold-power baseline captured before the first Wi-Fi or
/// shared Bluetooth power edge.
///
/// The stored non-zero representation reserves zero for `Option::None` and
/// keeps its independent boolean fields in four bytes. Register geometry does
/// not enter this value; each bit represents one named decoded field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiPowerBaseline(NonZeroU32);

impl WifiPowerBaseline {
    const MODEM_BUS_SHIFT: u32 = 0;
    const PLL_SOURCE_SHIFT: u32 = 1;
    const MODEM_SYSCON_SHIFT: u32 = Self::PLL_SOURCE_SHIFT + PlatformPllSourceBaseline::BIT_COUNT;

    fn new(
        modem_register_bus_clock_enabled: bool,
        pll_source: PlatformPllSourceBaseline,
        modem_syscon: ModemSysconPowerBaseline,
    ) -> Self {
        let bits = (u32::from(modem_register_bus_clock_enabled) << Self::MODEM_BUS_SHIFT)
            | (pll_source.bits() << Self::PLL_SOURCE_SHIFT)
            | (modem_syscon.bits() << Self::MODEM_SYSCON_SHIFT);
        debug_assert!(
            bits < (1 << (Self::MODEM_SYSCON_SHIFT + ModemSysconPowerBaseline::BIT_COUNT))
        );
        Self(NonZeroU32::new(bits + 1).expect("encoded Wi-Fi power baseline is non-zero"))
    }

    fn bits(self) -> u32 {
        self.0.get() - 1
    }

    /// Construct one distinguishable baseline inside a validation image.
    #[cfg(any(test, feature = "validation-probes"))]
    #[doc(hidden)]
    pub fn for_validation(modem_register_bus_clock_enabled: bool) -> Self {
        Self::new(
            modem_register_bus_clock_enabled,
            PlatformPllSourceBaseline::default(),
            ModemSysconPowerBaseline::default(),
        )
    }

    fn modem_register_bus_clock_enabled(self) -> bool {
        self.bits() & (1 << Self::MODEM_BUS_SHIFT) != 0
    }

    fn pll_source(self) -> PlatformPllSourceBaseline {
        PlatformPllSourceBaseline::from_bits(self.bits() >> Self::PLL_SOURCE_SHIFT)
    }

    fn modem_syscon(self) -> ModemSysconPowerBaseline {
        ModemSysconPowerBaseline::from_bits(self.bits() >> Self::MODEM_SYSCON_SHIFT)
    }
}

/// Exact stage whose restored cold-power baseline did not read back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiPowerRestoreReadback {
    ModemSyscon,
    ModemSourceClocks,
    ModemRegisterBusClock,
}

impl RadioPhyRegisters {
    /// Capture every non-monotonic field changed by the cold-power path.
    #[doc(hidden)]
    pub fn capture_wifi_power_baseline(&self) -> WifiPowerBaseline {
        WifiPowerBaseline::new(
            crate::svd::field_read::observe_modem_register_bus_clock(
                &self.peripherals.hp_sys_clkrst_radio,
            ),
            self.platform_pll_source_baseline(),
            self.modem_syscon_power_baseline(),
        )
    }

    /// Restore one captured cold-power baseline and verify each stage.
    ///
    /// ICG state maps are monotonic global initialization and deliberately
    /// remain installed, matching the vendor modem-clock manager.
    #[doc(hidden)]
    pub fn restore_wifi_power_baseline(
        &mut self,
        baseline: WifiPowerBaseline,
    ) -> Result<(), WifiPowerRestoreReadback> {
        self.restore_modem_syscon_power_baseline(baseline.modem_syscon());
        if self.modem_syscon_power_baseline() != baseline.modem_syscon() {
            return Err(WifiPowerRestoreReadback::ModemSyscon);
        }

        self.restore_platform_pll_source_baseline(baseline.pll_source());
        if self.platform_pll_source_baseline() != baseline.pll_source() {
            return Err(WifiPowerRestoreReadback::ModemSourceClocks);
        }

        crate::generated::restore_modem_register_bus_clock(
            &self.peripherals.hp_sys_clkrst_radio,
            if baseline.modem_register_bus_clock_enabled() {
                ModemSysconClockGateState::Enabled
            } else {
                ModemSysconClockGateState::Disabled
            },
        );
        if crate::svd::field_read::observe_modem_register_bus_clock(
            &self.peripherals.hp_sys_clkrst_radio,
        ) != baseline.modem_register_bus_clock_enabled()
        {
            return Err(WifiPowerRestoreReadback::ModemRegisterBusClock);
        }
        Ok(())
    }

    #[doc(hidden)]
    pub fn select_hp_active_modem_icg(&mut self) {
        crate::svd::fixed_register_image::select_hp_active_modem_icg(&self.peripherals.pmu_radio);
    }

    #[doc(hidden)]
    pub fn apply_modem_icg_selection(&mut self) {
        crate::svd::fixed_register_image::apply_modem_icg_selection(&self.peripherals.pmu_radio);
    }

    #[doc(hidden)]
    pub fn apply_sleep_icg_selection(&mut self) {
        crate::svd::fixed_register_image::apply_sleep_icg_selection(&self.peripherals.pmu_radio);
    }

    #[doc(hidden)]
    pub fn enable_modem_register_bus_clock(&mut self) {
        crate::generated::enable_modem_register_bus_clock(&self.peripherals.hp_sys_clkrst_radio);
    }

    #[doc(hidden)]
    pub fn configure_modem_source_clocks(&mut self) {
        crate::generated::configure_modem_source_clocks(&self.peripherals.hp_sys_clkrst_radio);
    }

    #[doc(hidden)]
    pub fn platform_clock_power_observation(&self) -> PlatformClockPowerObservation {
        let hp_active_icg_code =
            crate::svd::field_read::observe_hp_active_modem_icg_code(&self.peripherals.pmu_radio);
        let modem_register_bus_clock_enabled =
            crate::svd::field_read::observe_modem_register_bus_clock(
                &self.peripherals.hp_sys_clkrst_radio,
            );
        let (
            modem_apb_clock_enabled,
            modem_reset_asserted,
            modem_source_clock_enabled,
            modem_pll_selected,
            modem_pll_clock_enabled,
            modem_xtal_clock_enabled,
        ) = crate::svd::field_snapshot_read::observe_modem_source_clocks(
            &self.peripherals.hp_sys_clkrst_radio,
        );
        PlatformClockPowerObservation {
            hp_active_icg_selected: hp_active_icg_code
                == u8::from(crate::svd::pmu_radio::hp_active_icg_modem::ActiveModemIcgCode::Active),
            modem_register_bus_clock_enabled,
            modem_source_clocks_configured: modem_apb_clock_enabled
                && !modem_reset_asserted
                && modem_source_clock_enabled
                && modem_pll_selected
                && modem_pll_clock_enabled
                && modem_xtal_clock_enabled,
        }
    }

    /// Capture the upstream PLL-source fields a route may change.
    #[doc(hidden)]
    pub fn platform_pll_source_baseline(&self) -> PlatformPllSourceBaseline {
        let (
            modem_apb_clock_enabled,
            _modem_reset_asserted,
            modem_source_clock_enabled,
            modem_pll_selected,
            modem_pll_clock_enabled,
            modem_xtal_clock_enabled,
        ) = crate::svd::field_snapshot_read::observe_modem_source_clocks(
            &self.peripherals.hp_sys_clkrst_radio,
        );
        PlatformPllSourceBaseline {
            modem_apb_clock_enabled,
            modem_source_clock_enabled,
            modem_pll_selected,
            modem_pll_clock_enabled,
            modem_xtal_clock_enabled,
        }
    }

    /// Restore one captured upstream PLL-source baseline.
    #[doc(hidden)]
    pub fn restore_platform_pll_source_baseline(&mut self, baseline: PlatformPllSourceBaseline) {
        crate::generated::restore_modem_source_clocks(
            &self.peripherals.hp_sys_clkrst_radio,
            baseline.modem_apb_clock_enabled,
            baseline.modem_source_clock_enabled,
            baseline.modem_pll_selected,
            baseline.modem_pll_clock_enabled,
            baseline.modem_xtal_clock_enabled,
        );
    }

    #[doc(hidden)]
    pub fn set_rf_circuit_power(&mut self, enabled: bool) {
        if enabled {
            crate::generated::power_on_rf_circuits(&self.peripherals.pmu_radio);
        } else {
            crate::generated::power_off_rf_circuits(&self.peripherals.pmu_radio);
        }
    }

    #[doc(hidden)]
    pub fn set_bb_i2c_power_tie(&mut self, enabled: bool) {
        if enabled {
            crate::generated::enable_baseband_i2c_power_tie(&self.peripherals.pmu_radio);
        } else {
            crate::generated::disable_baseband_i2c_power_tie(&self.peripherals.pmu_radio);
        }
    }

    #[doc(hidden)]
    pub fn analog_i2c_is_powered(&self) -> bool {
        crate::svd::field_read::observe_analog_i2c_power(&self.peripherals.pmu_radio)
    }

    #[doc(hidden)]
    pub fn set_analog_i2c_power(&mut self, enabled: bool) {
        if enabled {
            crate::generated::power_on_analog_i2c(&self.peripherals.pmu_radio);
        } else {
            crate::generated::power_off_analog_i2c(&self.peripherals.pmu_radio);
        }
    }

    #[doc(hidden)]
    pub fn analog_i2c_reset_is_released(&self) -> bool {
        crate::svd::field_read::observe_analog_i2c_reset_release(&self.peripherals.pmu_radio)
    }

    #[doc(hidden)]
    pub fn set_analog_i2c_reset_released(&mut self, released: bool) {
        if released {
            crate::generated::release_analog_i2c_reset(&self.peripherals.pmu_radio);
        } else {
            crate::generated::assert_analog_i2c_reset(&self.peripherals.pmu_radio);
        }
    }

    #[doc(hidden)]
    pub fn enable_frontend_baseband_power(&mut self) {
        crate::generated::enable_frontend_baseband_power(&self.peripherals.pmu_radio);
    }
}

#[cfg(test)]
mod tests;
