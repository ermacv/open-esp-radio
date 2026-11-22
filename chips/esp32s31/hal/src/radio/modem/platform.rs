//! Route-owned PMU and upstream radio clock transactions.

use oer_chips_esp32s31_pac::hp_sys_clkrst::HpSysClkrstRadio;

use oer_portable_mmio::Placement;

use crate::{radio::modem::syscon::ModemSysconPowerBaseline, radio::phy::RadioPhyRegisters};

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
}

/// The cold-power baseline captured before the first Wi-Fi or shared
/// Bluetooth power edge: each field one decoded register field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiPowerBaseline {
    modem_register_bus_clock: oer_chips_esp32s31_pac::hp_sys_clkrst::modem_ctrl0::RegModemClkEn,
    pll_source: PlatformPllSourceBaseline,
    modem_syscon: ModemSysconPowerBaseline,
}

impl WifiPowerBaseline {
    const fn new(
        modem_register_bus_clock: oer_chips_esp32s31_pac::hp_sys_clkrst::modem_ctrl0::RegModemClkEn,
        pll_source: PlatformPllSourceBaseline,
        modem_syscon: ModemSysconPowerBaseline,
    ) -> Self {
        Self {
            modem_register_bus_clock,
            pll_source,
            modem_syscon,
        }
    }

    /// Construct one distinguishable baseline for a host test.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn for_host_test(
        modem_register_bus_clock: oer_chips_esp32s31_pac::hp_sys_clkrst::modem_ctrl0::RegModemClkEn,
    ) -> Self {
        Self::new(
            modem_register_bus_clock,
            PlatformPllSourceBaseline::default(),
            ModemSysconPowerBaseline::DISABLED,
        )
    }

    const fn modem_register_bus_clock(
        self,
    ) -> oer_chips_esp32s31_pac::hp_sys_clkrst::modem_ctrl0::RegModemClkEn {
        self.modem_register_bus_clock
    }

    const fn pll_source(self) -> PlatformPllSourceBaseline {
        self.pll_source
    }

    const fn modem_syscon(self) -> ModemSysconPowerBaseline {
        self.modem_syscon
    }
}

impl<P: crate::radio::phy::registers::RadioPlacement> RadioPhyRegisters<P> {
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

    /// Publish the active modem ICG state before opening module clock gates.
    ///
    /// SOURCE: ESP_HAL_63E97BE_S31_MODEM_CLOCK_MAP; ESP-IDF 4d80c1b2
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
        // SOURCE: ESP_IDF_4D59230D_S31_PMU_ACTIVE_ICG.
        use oer_chips_esp32s31_pac::pmu::hp_active_icg_modem::HpActiveDigIcgModemCode;
        self.peripherals.pmu_radio.modify_hp_active_icg_modem(|w| {
            w.set_hp_active_dig_icg_modem_code(HpActiveDigIcgModemCode::Active);
        });
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
                    oer_chips_esp32s31_pac::pmu::hp_active_icg_modem::HpActiveDigIcgModemCode::Active,
                ),
            modem_register_bus_clock_enabled: matches!(
                modem_register_bus_clock_enabled,
                oer_chips_esp32s31_pac::hp_sys_clkrst::modem_ctrl0::RegModemClkEn::Enabled
            ),
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
    pub fn power_on_rf_circuits(&mut self) {
        super::ops::power_on_rf_circuits(&mut self.peripherals.pmu_radio);
    }

    #[doc(hidden)]
    pub fn power_off_rf_circuits(&mut self) {
        super::ops::power_off_rf_circuits(&mut self.peripherals.pmu_radio);
    }

    #[doc(hidden)]
    pub fn tie_baseband_i2c_power(&mut self) {
        super::ops::tie_baseband_i2c_power(&mut self.peripherals.pmu_radio);
    }

    #[doc(hidden)]
    pub fn analog_i2c_is_powered(&self) -> bool {
        self.peripherals
            .pmu_radio
            .read_ana_peri_pwr_ctrl()
            .xpd_perif_i2c()
    }

    #[doc(hidden)]
    pub fn power_on_analog_i2c(&mut self) {
        super::ops::power_on_analog_i2c(&mut self.peripherals.pmu_radio);
    }

    #[doc(hidden)]
    pub fn power_off_analog_i2c(&mut self) {
        super::ops::power_off_analog_i2c(&mut self.peripherals.pmu_radio);
    }

    #[doc(hidden)]
    pub fn analog_i2c_reset_is_released(&self) -> bool {
        self.peripherals
            .pmu_radio
            .read_ana_peri_pwr_ctrl()
            .rstb_perif_i2c()
    }

    #[doc(hidden)]
    pub fn release_analog_i2c_reset(&mut self) {
        super::ops::release_analog_i2c_reset(&mut self.peripherals.pmu_radio);
    }

    #[doc(hidden)]
    pub fn assert_analog_i2c_reset(&mut self) {
        super::ops::assert_analog_i2c_reset(&mut self.peripherals.pmu_radio);
    }

    #[doc(hidden)]
    pub fn enable_frontend_baseband_power(&mut self) {
        super::ops::enable_frontend_baseband_power(&mut self.peripherals.pmu_radio);
    }
}

impl<P: crate::radio::phy::registers::RadioPlacement>
    oer_chips_family_radio_power::baseline::PowerBaselinePort for RadioPhyRegisters<P>
{
    type Baseline = WifiPowerBaseline;
    fn restore_syscon(&mut self, baseline: WifiPowerBaseline) {
        self.restore_modem_syscon_power_baseline(baseline.modem_syscon());
    }
    fn syscon_matches(&self, baseline: WifiPowerBaseline) -> bool {
        self.modem_syscon_power_baseline() == baseline.modem_syscon()
    }
    fn restore_source(&mut self, baseline: WifiPowerBaseline) {
        self.restore_platform_pll_source_baseline(baseline.pll_source());
    }
    fn source_matches(&self, baseline: WifiPowerBaseline) -> bool {
        self.platform_pll_source_baseline() == baseline.pll_source()
    }
    fn restore_bus(&mut self, baseline: WifiPowerBaseline) {
        super::ops::restore_modem_register_bus_clock(
            &mut self.peripherals.hp_sys_clkrst_radio,
            baseline.modem_register_bus_clock(),
        );
    }
    fn bus_matches(&self, baseline: WifiPowerBaseline) -> bool {
        self.peripherals
            .hp_sys_clkrst_radio
            .read_modem_ctrl0()
            .reg_modem_clk_en()
            == baseline.modem_register_bus_clock()
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
    use oer_chips_esp32s31_pac::hp_sys_clkrst::HpSysClkrstRadioWords;
    use oer_portable_mmio::Registers;

    #[test]
    fn active_icg_selection_preserves_inherited_bits_and_other_pmu_words() {
        use oer_chips_esp32s31_pac::pmu::{PmuRadio, hp_active_icg_modem};

        for (inherited, selected) in [
            (0x0000_0000, 0x8000_0000),
            (0x4000_0000, 0x8000_0000),
            (0x8000_0000, 0x8000_0000),
            (0xc000_0000, 0x8000_0000),
            (0xffff_ffff, 0xbfff_ffff),
            (0x5555_5555, 0x9555_5555),
            (0xaaaa_aaaa, 0xaaaa_aaaa),
        ] {
            crate::radio::phy::registers::host::with_registers(inherited, |registers| {
                registers.select_hp_active_modem_icg();
                let pmu = &registers.peripherals.pmu_radio;
                assert_eq!(
                    pmu.read_hp_active_icg_modem()
                        .hp_active_dig_icg_modem_code_variant(),
                    Some(hp_active_icg_modem::HpActiveDigIcgModemCode::Active),
                );
                for (index, &offset) in
                    PmuRadio::<oer_portable_mmio::Host<'_>>::RADIO_PHY_REGISTER_IMAGE_OFFSETS
                        .iter()
                        .enumerate()
                {
                    let expected = if offset == hp_active_icg_modem::OFFSET as u32 {
                        selected
                    } else {
                        inherited
                    };
                    assert_eq!(pmu.radio_phy_register_image(index), Some(expected));
                }
            });
        }
    }

    #[test]
    fn a_captured_baseline_restores_the_fields_it_was_captured_from() {
        for fill in [0, 0x5555_5555, 0xaaaa_aaaa, u32::MAX] {
            crate::radio::phy::registers::host::with_registers(fill, |registers| {
                let captured = registers.capture_wifi_power_baseline();
                registers.enable_modem_register_bus_clock();
                registers.configure_modem_source_clocks();
                registers.enable_phy_calibration_clocks();
                registers.select_phy_i2c_160mhz_source();
                if fill == 0 {
                    assert_ne!(
                        registers.capture_wifi_power_baseline(),
                        captured,
                        "the power-up changes the captured fields"
                    );
                }
                assert_eq!(
                    oer_chips_family_radio_power::baseline::restore(registers, captured),
                    Ok(())
                );
                assert_eq!(registers.capture_wifi_power_baseline(), captured);
                assert_eq!(
                    registers
                        .peripherals
                        .hp_sys_clkrst_radio
                        .read_modem_ctrl0()
                        .reg_modem_clk_en(),
                    if fill & 1 == 0 {
                        oer_chips_esp32s31_pac::hp_sys_clkrst::modem_ctrl0::RegModemClkEn::Disabled
                    } else {
                        oer_chips_esp32s31_pac::hp_sys_clkrst::modem_ctrl0::RegModemClkEn::Enabled
                    },
                    "the bus clock returns to its fill"
                );
            });
        }
    }

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
        // An open state with both XTAL and PLL enabled is rejected.
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
