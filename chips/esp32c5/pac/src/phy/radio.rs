//! PMU analog I2C power and the PHY baseband configuration words.
//!
//! SOURCE: reviewed evidence `C5_BLOB_LIBPHY_RF_INIT_LEAVES` (ESP32-C5
//! `libphy[phy_reg.o]::phy_open_i2c_xpd`,
//! `libphy[phy_i2c.o]::phy_dac_rate_set, libphy[phy_i2c.o]::phy_adc_rate_set`) with the
//! PMU fields of `ESP_IDF_4D59230D_C5_PMU`, and `C5_BLOB_LIBPHY_RF_INIT_LEAVES_2`
//! (`libphy[phy_init.o]::phy_open_fe_bb_clk`, `phy_iq_swap_set`, `libphy[phy_reg.o]::phy_fe_reg_init`,
//! `libphy[phy_pwdet.o]::phy_pwdet_reg_init`, `phy_dac_scale_set`, `phy_rxiq_scale_set`,
//! `libphy[phy_pwdet.o]::phy_pwdet_sar2_init`) with `ESP_IDF_4D59230D_C5_LP_AON_SARADC`, and
//! `C5_BLOB_LIBPHY_RF_INIT_LEAVES_4` (the `phy_rf_init` prologue,
//! `phy_set_tsens_power`, `phy_set_tsens_pwr`, `libphy[phy_tsens.o]::phy_tsens_read_init`) with
//! `ESP_IDF_4D59230D_C5_PCR_TSENS`, and `C5_BLOB_LIBPHY_RF_INIT_LEAVES_5`
//! (`phy_rxevm_init_cfg`, `phy_rxevm_reset_mem`).

#![forbid(unsafe_code)]

#[cfg(oer_probe)]
use crate::generated::{
    self, PhyDacScale, PhyPowerDetectorMode, PhyRateBit, PhyRxEvmEnable, PhyRxEvmParameter,
    PhyRxIqScale, PhySar2PowerDetectorCapacitor,
};
use crate::svd;

/// Unique owner of the PHY's PMU fields and baseband configuration words.
#[must_use = "dropping the PHY radio owner loses its register authority"]
pub struct PhyRadioRegisters {
    #[cfg_attr(
        not(oer_probe),
        expect(
            dead_code,
            reason = "the product capability reserves the PHY registers; only probes drive them"
        )
    )]
    peripherals: svd::peripheral_ownership::PhyRadioPeripherals,
}

impl PhyRadioRegisters {
    pub(crate) const fn new(peripherals: svd::peripheral_ownership::PhyRadioPeripherals) -> Self {
        Self { peripherals }
    }
}

#[cfg(oer_probe)]
impl PhyRadioRegisters {
    fn pmu(&self) -> &svd::PmuRadio {
        &self.peripherals.pmu_radio
    }

    fn baseband(&self) -> &svd::PhyBasebandConfig {
        &self.peripherals.phy_baseband_config
    }

    /// Power the RF analog I2C blocks as `phy_open_i2c_xpd` does: power the
    /// 5 GHz transceiver, TX RF, RX PBus, clock generator and PLL I2C, tie
    /// the BB I2C power high, power and reset-pulse the peripheral I2C when
    /// it was off, and release its reset when it is still held.
    pub fn power_analog_i2c(&mut self) {
        generated::power_rf_analog_i2c(self.pmu());
        generated::tie_high_bb_analog_i2c_power(self.pmu());
        if !svd::field_read::observe_peripheral_analog_i2c_power(self.pmu()) {
            generated::power_peripheral_analog_i2c(self.pmu());
            generated::hold_peripheral_analog_i2c_reset(self.pmu());
            generated::release_peripheral_analog_i2c_reset(self.pmu());
        }
        if !svd::field_read::observe_peripheral_analog_i2c_released(self.pmu()) {
            generated::release_peripheral_analog_i2c_reset(self.pmu());
        }
    }

    /// Power the 5 GHz clock generator, as `phy_rf_init` does after
    /// `phy_open_i2c_xpd`.
    pub fn power_5g_clock_generator(&mut self) {
        generated::power_5g_clock_generator(self.pmu());
    }

    /// The baseband half of `phy_adc_rate_set(rate)`: bit 1 and then bit 0
    /// of the configuration word take the rate bit.
    pub fn set_adc_rate_bits(&mut self, rate: bool) {
        let bit = if rate {
            PhyRateBit::Set
        } else {
            PhyRateBit::Clear
        };
        generated::set_phy_adc_rate_high(self.baseband(), bit);
        generated::set_phy_adc_rate_low(self.baseband(), bit);
    }

    /// The baseband half of `phy_dac_rate_set`: clear bit 3 and then bit 2.
    pub fn clear_dac_rate_bits(&mut self) {
        generated::clear_phy_dac_rate_high(self.baseband());
        generated::clear_phy_dac_rate_low(self.baseband());
    }

    /// `phy_open_fe_bb_clk`: open the front-end clock gate, the front-end
    /// and baseband clock enables and the baseband clock gate.
    pub fn open_front_end_baseband_clocks(&mut self) {
        svd::fixed_register_image::open_phy_front_end_clock_gate(self.baseband());
        generated::open_phy_fe_bb_clock(self.baseband());
        svd::fixed_register_image::open_phy_baseband_clock_gate(self.baseband());
    }

    /// `phy_iq_swap_set`, with `high_band` the phy_param byte 0x2A, which the
    /// channel setters (`phy_chip_set_chan`) set when the channel frequency
    /// is at least 3001 MHz.
    pub fn set_iq_swap(&mut self, high_band: bool) {
        if high_band {
            generated::clear_phy_front_end_init_0c08_first(self.baseband());
        } else {
            generated::set_phy_front_end_init_0c08(self.baseband());
        }
        generated::clear_phy_rx_dco_calibration_control(self.baseband());
    }

    /// `phy_fe_reg_init`, with `high_band` the phy_param byte 0x2A being set
    /// and `rx_iq_scale` the phy_param byte 0x28A.
    pub fn initialize_front_end(&mut self, high_band: bool, rx_iq_scale: u8) {
        let baseband = self.baseband();
        generated::enable_phy_front_end_0894(baseband);
        generated::clear_phy_front_end_clear_first(baseband);
        generated::set_phy_table_memory_base_index(baseband);
        generated::enable_phy_front_end_init(baseband);
        generated::set_phy_rx_iq_correction_modes(baseband);
        generated::set_phy_tx_iq_correction_modes(baseband);
        generated::set_phy_rx_iq_scale_high(baseband, PhyRxIqScale::Zero);
        generated::set_phy_rx_iq_scale_low(baseband, PhyRxIqScale::Zero);
        if high_band {
            generated::set_phy_front_end_iq_swap(baseband);
        } else {
            generated::clear_phy_front_end_iq_swap(baseband);
        }
        generated::set_phy_front_end_init_0c20(baseband);
        generated::set_phy_pa_on_bt_delay(baseband);
        self.initialize_power_detector();
        self.set_dac_scale(true);
        self.set_rx_iq_scale(rx_iq_scale);
    }

    /// `phy_pwdet_reg_init`.
    pub fn initialize_power_detector(&mut self) {
        let baseband = self.baseband();
        svd::fixed_register_image::set_phy_power_detector_word_810(baseband);
        svd::fixed_register_image::set_phy_power_detector_word_814(baseband);
        generated::set_phy_power_detector_calibration_field(baseband);
        svd::fixed_register_image::set_phy_power_detector_reference(baseband);
        generated::set_phy_power_detector_mode(
            baseband,
            PhyPowerDetectorMode::RegisterInitialization,
        );
        generated::drive_saradc2_from_power_detector(&self.peripherals.apb_saradc_radio);
        generated::set_sar2_power_detector_capacitor(
            &self.peripherals.lp_aon_radio,
            PhySar2PowerDetectorCapacitor::Four,
        );
    }

    /// `phy_dac_scale_set(full)`.
    pub fn set_dac_scale(&mut self, full: bool) {
        let scale = if full {
            PhyDacScale::Full
        } else {
            PhyDacScale::Zero
        };
        generated::set_phy_dac_scale_high(self.baseband(), scale);
        generated::set_phy_dac_scale_low(self.baseband(), scale);
    }

    /// `phy_rxiq_scale_set`, with `selection` the phy_param byte 0x28A.
    pub fn set_rx_iq_scale(&mut self, selection: u8) {
        let (high, low) = match selection {
            1 => (PhyRxIqScale::MinusSix, PhyRxIqScale::Zero),
            2 => (PhyRxIqScale::Zero, PhyRxIqScale::MinusSix),
            _ => (PhyRxIqScale::Zero, PhyRxIqScale::Zero),
        };
        generated::set_phy_rx_iq_scale_high(self.baseband(), high);
        generated::set_phy_rx_iq_scale_low(self.baseband(), low);
    }

    /// `phy_pwdet_sar2_init`, with `high_band` the phy_param byte 0x2A being
    /// set.
    pub fn initialize_power_detector_sar2(&mut self, high_band: bool) {
        let baseband = self.baseband();
        generated::set_phy_power_detector_sar_mode(baseband);
        generated::clear_phy_power_detector_sar_config(baseband);
        svd::fixed_register_image::set_phy_power_detector_reference(baseband);
        generated::set_phy_power_detector_mode(baseband, PhyPowerDetectorMode::Sar2Initialization);
        generated::set_sar2_power_detector_capacitor(
            &self.peripherals.lp_aon_radio,
            if high_band {
                PhySar2PowerDetectorCapacitor::Four
            } else {
                PhySar2PowerDetectorCapacitor::Two
            },
        );
    }

    /// The register writes of the `phy_rf_init` prologue after
    /// `phy_open_i2c_xpd`: power the 5 GHz clock generator, enable the SAR
    /// ADC register clock and set the low byte of PCR word 0x14C.
    pub fn open_rf_initialization_clocks(&mut self) {
        generated::power_5g_clock_generator(self.pmu());
        generated::enable_saradc_register_clock(&self.peripherals.pcr_radio);
        generated::set_pcr_register_014c_low_byte(&self.peripherals.pcr_radio);
    }

    /// `phy_set_tsens_power(on)`.
    pub fn set_temperature_sensor_power(&mut self, on: bool) {
        let bit = if on {
            PhyRateBit::Set
        } else {
            PhyRateBit::Clear
        };
        generated::set_tsens_power(&self.peripherals.apb_saradc_radio, bit);
    }

    /// `phy_set_tsens_pwr`: power the temperature sensor and select its clock.
    pub fn power_temperature_sensor(&mut self) {
        self.set_temperature_sensor_power(true);
        generated::select_tsens_clock(&self.peripherals.apb_saradc_radio);
    }

    /// `phy_tsens_read_init`: enable the SAR ADC and temperature-sensor
    /// clocks, release the sensor's reset, select its clock and power it.
    pub fn initialize_temperature_sensor(&mut self) {
        let pcr = &self.peripherals.pcr_radio;
        generated::enable_saradc_clocks(pcr);
        generated::enable_tsens_clock(pcr);
        generated::release_tsens_reset(pcr);
        generated::select_tsens_clock(&self.peripherals.apb_saradc_radio);
        self.power_temperature_sensor();
    }

    /// `phy_rxevm_init_cfg(enable, second, third)`.
    pub fn initialize_rx_evm(
        &mut self,
        enable: bool,
        second: PhyRxEvmParameter,
        third: PhyRxEvmParameter,
    ) {
        let baseband = self.baseband();
        generated::set_phy_rx_evm_mode(baseband);
        generated::set_phy_rx_evm_third_parameter(baseband, third);
        generated::set_phy_rx_evm_second_parameter(baseband, second);
        generated::set_phy_rx_evm_enable(
            baseband,
            if enable {
                PhyRxEvmEnable::Enabled
            } else {
                PhyRxEvmEnable::Disabled
            },
        );
        generated::clear_phy_rx_evm_word_7920(baseband);
        self.reset_rx_evm_memory();
    }

    /// `phy_rxevm_reset_mem`: pulse both RX EVM memory resets.
    pub fn reset_rx_evm_memory(&mut self) {
        let baseband = self.baseband();
        generated::assert_phy_rx_evm_memory_reset(baseband);
        generated::release_phy_rx_evm_memory_reset(baseband);
        generated::assert_phy_rx_evm_memory_reset_2(baseband);
        generated::release_phy_rx_evm_memory_reset_2(baseband);
    }
}
