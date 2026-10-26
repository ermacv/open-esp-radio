//! Route-owned PMU and upstream radio clock transactions.

use core::num::NonZeroU32;
use oer_esp32s31_pac::hp_sys_clkrst::HpSysClkrstRadio;

use oer_mmio::Placement;

use crate::{
    modem::syscon::ModemSysconPowerBaseline, modem::values::ModemSysconClockGateState,
    phy::RadioPhyRegisters,
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
    /// The five fields from one read of `MODEM_CONF`.
    fn capture<P: Placement>(registers: &HpSysClkrstRadio<P>) -> Self {
        let modem_conf = registers.read_modem_conf();
        Self {
            modem_apb_clock_enabled: modem_conf.modem_apb_clk_en(),
            modem_source_clock_enabled: modem_conf.modem_clk_en(),
            modem_pll_selected: modem_conf.modem_clk_source_sel(),
            modem_pll_clock_enabled: modem_conf.modem_pll_clk_en(),
            modem_xtal_clock_enabled: modem_conf.modem_xtal_clk_en(),
        }
    }

    /// One fresh-read RMW of the five captured fields; every other bit of
    /// `MODEM_CONF`, the modem reset included, keeps the value read.
    fn restore<P: Placement>(self, registers: &mut HpSysClkrstRadio<P>) {
        registers.modify_modem_conf(|w| {
            w.set_modem_apb_clk_en(self.modem_apb_clock_enabled)
                .set_modem_clk_en(self.modem_source_clock_enabled)
                .set_modem_clk_source_sel(self.modem_pll_selected)
                .set_modem_pll_clk_en(self.modem_pll_clock_enabled)
                .set_modem_xtal_clk_en(self.modem_xtal_clock_enabled);
        });
    }

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

    /// Construct one distinguishable baseline for a host test.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn for_host_test(modem_register_bus_clock_enabled: bool) -> Self {
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

impl<P: crate::phy::registers::RadioPlacement> RadioPhyRegisters<P> {
    /// Capture every non-monotonic field changed by the cold-power path.
    #[doc(hidden)]
    pub fn capture_wifi_power_baseline(&self) -> WifiPowerBaseline {
        WifiPowerBaseline::new(
            self.peripherals
                .hp_sys_clkrst_radio
                .read_modem_ctrl0()
                .reg_modem_clk_en(),
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

        super::ops::restore_modem_register_bus_clock(
            &mut self.peripherals.hp_sys_clkrst_radio,
            if baseline.modem_register_bus_clock_enabled() {
                ModemSysconClockGateState::Enabled
            } else {
                ModemSysconClockGateState::Disabled
            },
        );
        if self
            .peripherals
            .hp_sys_clkrst_radio
            .read_modem_ctrl0()
            .reg_modem_clk_en()
            != baseline.modem_register_bus_clock_enabled()
        {
            return Err(WifiPowerRestoreReadback::ModemRegisterBusClock);
        }
        Ok(())
    }

    /// Publish the active modem ICG state before opening module clock gates.
    ///
    /// SOURCE: ESP_HAL_63E97BE_S31_MODEM_CLOCK_MAP; ESP-IDF 4d59230d
    /// `components/esp_hw_support/port/esp32s31/pmu_init.c:93,137-138`.
    /// The maps select code two, so module gates alone cannot admit register
    /// access with the reset code zero.
    /// SOURCE: hw10-20261026-radio-start-23-fe-clock and
    /// hw10-20261026-radio-start-25-active-icg;
    /// `oer-notes/research/hw-radio-start.md`.
    pub(crate) fn publish_modem_power_state(&mut self) {
        self.select_hp_active_modem_icg();
        self.apply_modem_icg_selection();
        self.apply_sleep_icg_selection();
    }

    #[doc(hidden)]
    pub fn select_hp_active_modem_icg(&mut self) {
        self.peripherals.pmu_radio.select_hp_active_modem_icg();
    }

    #[doc(hidden)]
    pub fn apply_modem_icg_selection(&mut self) {
        super::ops::apply_modem_icg_selection(&mut self.peripherals.pmu_radio);
    }

    #[doc(hidden)]
    pub fn apply_sleep_icg_selection(&mut self) {
        super::ops::apply_sleep_icg_selection(&mut self.peripherals.pmu_radio);
    }

    #[doc(hidden)]
    pub fn enable_modem_register_bus_clock(&mut self) {
        super::ops::enable_modem_register_bus_clock(&mut self.peripherals.hp_sys_clkrst_radio);
    }

    #[doc(hidden)]
    pub fn configure_modem_source_clocks(&mut self) {
        super::ops::configure_modem_source_clocks(&mut self.peripherals.hp_sys_clkrst_radio);
    }

    #[doc(hidden)]
    pub fn platform_clock_power_observation(&self) -> PlatformClockPowerObservation {
        let hp_active_icg_code = self
            .peripherals
            .pmu_radio
            .read_hp_active_icg_modem()
            .hp_active_dig_icg_modem_code_variant();
        let modem_register_bus_clock_enabled = self
            .peripherals
            .hp_sys_clkrst_radio
            .read_modem_ctrl0()
            .reg_modem_clk_en();
        PlatformClockPowerObservation {
            hp_active_icg_selected: hp_active_icg_code
                == Some(
                    oer_esp32s31_pac::pmu::hp_active_icg_modem::HpActiveDigIcgModemCode::Active,
                ),
            modem_register_bus_clock_enabled,
            modem_source_clocks_configured: modem_source_clocks_configured(
                &self.peripherals.hp_sys_clkrst_radio,
            ),
        }
    }

    /// Capture the upstream PLL-source fields a route may change.
    #[doc(hidden)]
    pub fn platform_pll_source_baseline(&self) -> PlatformPllSourceBaseline {
        PlatformPllSourceBaseline::capture(&self.peripherals.hp_sys_clkrst_radio)
    }

    /// Restore one captured upstream PLL-source baseline.
    #[doc(hidden)]
    pub fn restore_platform_pll_source_baseline(&mut self, baseline: PlatformPllSourceBaseline) {
        baseline.restore(&mut self.peripherals.hp_sys_clkrst_radio);
    }

    #[doc(hidden)]
    pub fn set_rf_circuit_power(&mut self, enabled: bool) {
        if enabled {
            super::ops::power_on_rf_circuits(&mut self.peripherals.pmu_radio);
        } else {
            super::ops::power_off_rf_circuits(&mut self.peripherals.pmu_radio);
        }
    }

    #[doc(hidden)]
    pub fn set_bb_i2c_power_tie(&mut self, enabled: bool) {
        super::ops::set_baseband_i2c_power_tie(&mut self.peripherals.pmu_radio, enabled);
    }

    #[doc(hidden)]
    pub fn analog_i2c_is_powered(&self) -> bool {
        self.peripherals
            .pmu_radio
            .read_ana_peri_pwr_ctrl()
            .xpd_perif_i2c()
    }

    #[doc(hidden)]
    pub fn set_analog_i2c_power(&mut self, enabled: bool) {
        if enabled {
            super::ops::power_on_analog_i2c(&mut self.peripherals.pmu_radio);
        } else {
            super::ops::power_off_analog_i2c(&mut self.peripherals.pmu_radio);
        }
    }

    #[doc(hidden)]
    pub fn analog_i2c_reset_is_released(&self) -> bool {
        self.peripherals
            .pmu_radio
            .read_ana_peri_pwr_ctrl()
            .rstb_perif_i2c()
    }

    #[doc(hidden)]
    pub fn set_analog_i2c_reset_released(&mut self, released: bool) {
        if released {
            super::ops::release_analog_i2c_reset(&mut self.peripherals.pmu_radio);
        } else {
            super::ops::assert_analog_i2c_reset(&mut self.peripherals.pmu_radio);
        }
    }

    #[doc(hidden)]
    pub fn enable_frontend_baseband_power(&mut self) {
        super::ops::enable_frontend_baseband_power(&mut self.peripherals.pmu_radio);
    }
}

// SOURCE: ESP_IDF_4D59230D_S31_MODEM_PLL_GATE: open selects PLL with XTAL off.
fn modem_source_clocks_configured<P: Placement>(registers: &HpSysClkrstRadio<P>) -> bool {
    let conf = registers.read_modem_conf();
    conf.modem_apb_clk_en()
        && !conf.modem_rst_en()
        && conf.modem_clk_en()
        && conf.modem_clk_source_sel()
        && conf.modem_pll_clk_en()
        && !conf.modem_xtal_clk_en()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_esp32s31_pac::hp_sys_clkrst::HpSysClkrstRadioWords;
    use oer_mmio::Registers;

    #[test]
    fn source_readback_accepts_the_pinned_open_state_only() {
        let words = HpSysClkrstRadioWords::WORDS.iter().copied().max().unwrap() / 4 + 1;
        let mut memory = std::vec![u32::MAX; words];
        let mut registers = HpSysClkrstRadio::host(&mut memory).unwrap();
        assert!(!modem_source_clocks_configured(&registers));
        registers.prepare_open_modem_pll_source_gate();
        assert!(!modem_source_clocks_configured(&registers));
        registers.open_modem_pll_source_gate();
        assert!(modem_source_clocks_configured(&registers));
        // The former open state left XTAL enabled as well as PLL.
        registers.modify_modem_conf(|w| {
            w.set_modem_xtal_clk_en(true);
        });
        assert!(!modem_source_clocks_configured(&registers));
        registers.prepare_close_modem_pll_source_gate();
        assert!(!modem_source_clocks_configured(&registers));
        registers.close_modem_pll_source_gate();
        assert!(!modem_source_clocks_configured(&registers));
    }
}
