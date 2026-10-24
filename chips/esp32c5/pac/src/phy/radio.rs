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

use crate::generated::peripheral_ownership::PhyRadioPeripherals;
#[cfg(oer_probe)]
use crate::generated::{
    PhyDacScale, PhyPowerDetectorMode, PhyRateBit, PhyRxEvmEnable, PhyRxEvmParameter, PhyRxIqScale,
    PhySar2PowerDetectorCapacitor,
};

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
    peripherals: PhyRadioPeripherals,
}

impl PhyRadioRegisters {
    pub(crate) const fn new(peripherals: PhyRadioPeripherals) -> Self {
        Self { peripherals }
    }
}

#[cfg(oer_probe)]
impl PhyRadioRegisters {
    fn pmu(&mut self) -> &mut crate::generated::pmu_radio::PmuRadio {
        &mut self.peripherals.pmu_radio
    }

    fn baseband(&mut self) -> &mut crate::generated::phy_baseband_config::PhyBasebandConfig {
        &mut self.peripherals.phy_baseband_config
    }

    /// Power the RF analog I2C blocks as `phy_open_i2c_xpd` does: power the
    /// 5 GHz transceiver, TX RF, RX PBus, clock generator and PLL I2C, tie
    /// the BB I2C power high, power and reset-pulse the peripheral I2C when
    /// it was off, and release its reset when it is still held.
    pub fn power_analog_i2c(&mut self) {
        self.pmu().power_rf_analog_i2c();
        self.pmu().tie_high_bb_analog_i2c_power();
        if !self.pmu().read_rf_pwc().xpd_perif_i2c() {
            self.pmu().power_peripheral_analog_i2c();
            self.pmu().hold_peripheral_analog_i2c_reset();
            self.pmu().release_peripheral_analog_i2c_reset();
        }
        if !self.pmu().read_rf_pwc().perif_i2c_rstb() {
            self.pmu().release_peripheral_analog_i2c_reset();
        }
    }

    /// Power the 5 GHz clock generator, as `phy_rf_init` does after
    /// `phy_open_i2c_xpd`.
    pub fn power_5g_clock_generator(&mut self) {
        self.pmu().power_5g_clock_generator();
    }

    /// The baseband half of `phy_adc_rate_set(rate)`: bit 1 and then bit 0
    /// of the configuration word take the rate bit.
    pub fn set_adc_rate_bits(&mut self, rate: bool) {
        let bit = if rate {
            PhyRateBit::Set
        } else {
            PhyRateBit::Clear
        };
        self.baseband().set_phy_adc_rate_high(bit);
        self.baseband().set_phy_adc_rate_low(bit);
    }

    /// The baseband half of `phy_dac_rate_set`: clear bit 3 and then bit 2.
    pub fn clear_dac_rate_bits(&mut self) {
        self.baseband().clear_phy_dac_rate_high();
        self.baseband().clear_phy_dac_rate_low();
    }

    /// `phy_open_fe_bb_clk`: open the front-end clock gate, the front-end
    /// and baseband clock enables and the baseband clock gate.
    pub fn open_front_end_baseband_clocks(&mut self) {
        self.baseband().open_phy_front_end_clock_gate();
        self.baseband().open_phy_fe_bb_clock();
        self.baseband().open_phy_baseband_clock_gate();
    }

    /// `phy_iq_swap_set`, with `high_band` the phy_param byte 0x2A, which the
    /// channel setters (`phy_chip_set_chan`) set when the channel frequency
    /// is at least 3001 MHz.
    pub fn set_iq_swap(&mut self, high_band: bool) {
        if high_band {
            self.baseband().clear_phy_front_end_init_0c08_first();
        } else {
            self.baseband().set_phy_front_end_init_0c08();
        }
        self.baseband().clear_phy_rx_dco_calibration_control();
    }

    /// `phy_fe_reg_init`, with `high_band` the phy_param byte 0x2A being set
    /// and `rx_iq_scale` the phy_param byte 0x28A.
    pub fn initialize_front_end(&mut self, high_band: bool, rx_iq_scale: u8) {
        let baseband = self.baseband();
        baseband.enable_phy_front_end_0894();
        baseband.clear_phy_front_end_clear_first();
        baseband.set_phy_table_memory_base_index();
        baseband.enable_phy_front_end_init();
        baseband.set_phy_rx_iq_correction_modes();
        baseband.set_phy_tx_iq_correction_modes();
        baseband.set_phy_rx_iq_scale_high(PhyRxIqScale::Zero);
        baseband.set_phy_rx_iq_scale_low(PhyRxIqScale::Zero);
        if high_band {
            baseband.set_phy_front_end_iq_swap();
        } else {
            baseband.clear_phy_front_end_iq_swap();
        }
        baseband.set_phy_front_end_init_0c20();
        baseband.set_phy_pa_on_bt_delay();
        self.initialize_power_detector();
        self.set_dac_scale(true);
        self.set_rx_iq_scale(rx_iq_scale);
    }

    /// `phy_pwdet_reg_init`.
    pub fn initialize_power_detector(&mut self) {
        let baseband = self.baseband();
        baseband.set_phy_power_detector_word_810();
        baseband.set_phy_power_detector_word_814();
        baseband.set_phy_power_detector_calibration_field();
        baseband.set_phy_power_detector_reference();
        baseband.set_phy_power_detector_mode(PhyPowerDetectorMode::RegisterInitialization);
        self.peripherals
            .apb_saradc_radio
            .drive_saradc2_from_power_detector();
        self.peripherals
            .lp_aon_radio
            .set_sar2_power_detector_capacitor(PhySar2PowerDetectorCapacitor::Four);
    }

    /// `phy_dac_scale_set(full)`.
    pub fn set_dac_scale(&mut self, full: bool) {
        let scale = if full {
            PhyDacScale::Full
        } else {
            PhyDacScale::Zero
        };
        self.baseband().set_phy_dac_scale_high(scale);
        self.baseband().set_phy_dac_scale_low(scale);
    }

    /// `phy_rxiq_scale_set`, with `selection` the phy_param byte 0x28A.
    pub fn set_rx_iq_scale(&mut self, selection: u8) {
        let (high, low) = match selection {
            1 => (PhyRxIqScale::MinusSix, PhyRxIqScale::Zero),
            2 => (PhyRxIqScale::Zero, PhyRxIqScale::MinusSix),
            _ => (PhyRxIqScale::Zero, PhyRxIqScale::Zero),
        };
        self.baseband().set_phy_rx_iq_scale_high(high);
        self.baseband().set_phy_rx_iq_scale_low(low);
    }

    /// `phy_pwdet_sar2_init`, with `high_band` the phy_param byte 0x2A being
    /// set.
    pub fn initialize_power_detector_sar2(&mut self, high_band: bool) {
        let baseband = self.baseband();
        baseband.set_phy_power_detector_sar_mode();
        baseband.clear_phy_power_detector_sar_config();
        baseband.set_phy_power_detector_reference();
        baseband.set_phy_power_detector_mode(PhyPowerDetectorMode::Sar2Initialization);
        self.peripherals
            .lp_aon_radio
            .set_sar2_power_detector_capacitor(if high_band {
                PhySar2PowerDetectorCapacitor::Four
            } else {
                PhySar2PowerDetectorCapacitor::Two
            });
    }

    /// The register writes of the `phy_rf_init` prologue after
    /// `phy_open_i2c_xpd`: power the 5 GHz clock generator, enable the SAR
    /// ADC register clock and set the low byte of PCR word 0x14C.
    pub fn open_rf_initialization_clocks(&mut self) {
        self.pmu().power_5g_clock_generator();
        self.peripherals.pcr_radio.enable_saradc_register_clock();
        self.peripherals.pcr_radio.set_pcr_register_014c_low_byte();
    }

    /// `phy_set_tsens_power(on)`.
    pub fn set_temperature_sensor_power(&mut self, on: bool) {
        let bit = if on {
            PhyRateBit::Set
        } else {
            PhyRateBit::Clear
        };
        self.peripherals.apb_saradc_radio.set_tsens_power(bit);
    }

    /// `phy_set_tsens_pwr`: power the temperature sensor and select its clock.
    pub fn power_temperature_sensor(&mut self) {
        self.set_temperature_sensor_power(true);
        self.peripherals.apb_saradc_radio.select_tsens_clock();
    }

    /// `phy_tsens_read_init`: enable the SAR ADC and temperature-sensor
    /// clocks, release the sensor's reset, select its clock and power it.
    pub fn initialize_temperature_sensor(&mut self) {
        let pcr = &mut self.peripherals.pcr_radio;
        pcr.enable_saradc_clocks();
        pcr.enable_tsens_clock();
        pcr.release_tsens_reset();
        self.peripherals.apb_saradc_radio.select_tsens_clock();
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
        baseband.set_phy_rx_evm_mode();
        baseband.set_phy_rx_evm_third_parameter(third);
        baseband.set_phy_rx_evm_second_parameter(second);
        baseband.set_phy_rx_evm_enable(if enable {
            PhyRxEvmEnable::Enabled
        } else {
            PhyRxEvmEnable::Disabled
        });
        baseband.clear_phy_rx_evm_word_7920();
        self.reset_rx_evm_memory();
    }

    /// `phy_rxevm_reset_mem`: pulse both RX EVM memory resets.
    pub fn reset_rx_evm_memory(&mut self) {
        let baseband = self.baseband();
        baseband.assert_phy_rx_evm_memory_reset();
        baseband.release_phy_rx_evm_memory_reset();
        baseband.assert_phy_rx_evm_memory_reset_2();
        baseband.release_phy_rx_evm_memory_reset_2();
    }
}
