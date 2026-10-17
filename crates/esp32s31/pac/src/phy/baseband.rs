//! Ownership-bound access to recovered ESP32-S31 PHY/baseband registers.
//!
//! Register layout and legal field images come from
//! `registers/esp32s31/published/radio.svd`. Complete ROM/blob bodies cited there define the
//! finite operation order.

#![deny(unsafe_code)]

use crate::{RadioPhyRegisters, generated::PhyTxPowerTrackingState};

fn vendor_register_argument(input: u32) -> crate::generated::PhyVendorRegisterArgument {
    crate::generated::PhyVendorRegisterArgument::new(input)
        .expect("every u32 fits the complete generated vendor-argument domain")
}

/// Opaque capture of the two fields owned by TX-DC PWDET calibration.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxDcPwdetFields {
    table_low: u8,
    calibration: u8,
}

/// Opaque capture of the TX-IQ tone-control field state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxIqToneControlFields {
    selector_high: u8,
    low_reserved_clear_opaque: u8,
    negated_step_or_attenuation: u8,
    tone_enable_or_arm: bool,
    txiq_mismatch_mode_opaque: u8,
    middle_reserved_clear_opaque: u8,
    txiq_polarity_image: u8,
    high_nibble_opaque: u8,
}

/// Opaque capture of the two-bit RX-DCO calibration control field.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxDcoControlField(u8);

impl RxDcoControlField {
    pub(crate) const fn from_capture(value: u8) -> Self {
        Self(value)
    }

    pub(crate) const fn bits(self) -> u8 {
        self.0
    }

    /// Construct one distinguishable capture inside a validation image.
    #[cfg(any(test, feature = "validation-probes"))]
    #[doc(hidden)]
    pub const fn for_validation(value: u8) -> Self {
        Self(value)
    }
}

impl TxDcPwdetFields {
    /// Construct one distinguishable capture inside a validation image.
    #[cfg(any(test, feature = "validation-probes"))]
    #[doc(hidden)]
    pub const fn for_validation(table_low: u8, calibration: u8) -> Self {
        Self {
            table_low,
            calibration,
        }
    }
}

pub(crate) const fn decode_noise_floor_quarter_db(raw_low_twelve: u16) -> i32 {
    // Complete ROM `phy_read_hw_noisefloor` subtracts 0x1000 from the
    // generated twelve-bit field unconditionally and shifts by two.
    let signed_sixteenth_db = raw_low_twelve as i32 - 0x1000;
    signed_sixteenth_db >> 2
}

pub(crate) const fn quarter_db_to_dbm(quarter_db: i32) -> i8 {
    // Complete blob `wDev_GetNoiseFloor` consumes the quarter-dB ROM result,
    // adds two, shifts by two and stores a byte.
    ((quarter_db + 2) >> 2) as i8
}

const fn saturate_tx_iq_gain(coefficient: i8) -> i8 {
    // Complete ROM `phy_txiq_set_reg` deliberately excludes the most
    // negative two's-complement endpoint before publishing the six-bit
    // field. This differs from merely retaining the low bits of an `i8`.
    if coefficient < -31 {
        -31
    } else if coefficient > 31 {
        31
    } else {
        coefficient
    }
}

const fn saturate_tx_iq_phase(coefficient: i8) -> i8 {
    // The seven-bit phase field has the analogous symmetric ROM range.
    if coefficient < -63 {
        -63
    } else if coefficient > 63 {
        63
    } else {
        coefficient
    }
}

fn iq_coefficient_image(coefficient: i8) -> crate::generated::PhyIqCoefficientImageByte {
    crate::generated::PhyIqCoefficientImageByte::new(u32::from(coefficient as u8))
        .expect("one coefficient byte fits the complete generated IQ domain")
}

fn tone_byte(value: u8) -> crate::generated::PhyToneByteImage {
    crate::generated::PhyToneByteImage::new(u32::from(value))
        .expect("one byte fits the complete generated tone-byte domain")
}

fn tone_two_bit(value: u32) -> crate::generated::PhyToneTwoBitImage {
    crate::generated::PhyToneTwoBitImage::new(value)
        .expect("reviewed tone transaction supplies a complete two-bit image")
}

fn tone_three_bit(value: u32) -> crate::generated::PhyToneThreeBitImage {
    crate::generated::PhyToneThreeBitImage::new(value)
        .expect("reviewed tone transaction supplies a complete three-bit image")
}

fn tone_four_bit(value: u32) -> crate::generated::PhyToneFourBitImage {
    crate::generated::PhyToneFourBitImage::new(value)
        .expect("reviewed tone transaction supplies a complete four-bit image")
}

fn tone_selector_high(selector: u16) -> crate::generated::PhyToneByteImage {
    let selector = crate::generated::PhyToneSelector::new(u32::from(selector))
        .expect("tone selector must fit the generated ten-bit domain");
    crate::generated::PhyToneByteImage::new(selector.get() / 4)
        .expect("upper eight selector bits fit the generated tone-byte domain")
}

impl RadioPhyRegisters {
    /// Apply both fresh-read edges of complete ROM `phy_fe_txrx_reset`.
    pub fn reset_frontend_txrx(&mut self) {
        let registers = &self.peripherals.phy_fedata_recovered;
        crate::generated::clear_phy_frontend_txrx_reset_state(registers);
        crate::generated::assert_phy_frontend_txrx_reset_state(registers);
    }

    /// Enable both RX- and TX-IQ correction modes through two fresh RMWs.
    ///
    /// Complete rev0 ROM `phy_iq_corr_enable` at `0x2f82_7d8c` sets both
    /// recovered mode bits in each word while preserving all coefficients.
    pub fn enable_iq_correction_modes(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::enable_rx_iq_correction_modes(bb);
        crate::generated::enable_tx_iq_correction_modes(bb);
    }

    /// Publish both RXIQ root status bits through independent fresh RMWs.
    pub fn configure_rxiq_root_status(&mut self) {
        let pbus = &self.peripherals.phy_pbus;
        crate::generated::set_pbus_rxiq_status_first(pbus);
        crate::generated::set_pbus_rxiq_status_second(pbus);
    }

    /// Apply the complete four-edge RXIQ correction prefix or suffix.
    pub fn configure_rxiq_root_correction(&mut self, begin: bool) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        if begin {
            crate::generated::set_rxiq_root_rx_correction_mode_low(bb);
            crate::generated::set_rxiq_root_tx_correction_mode_low(bb);
            crate::generated::clear_rxiq_root_rx_correction_mode_high(bb);
            crate::generated::clear_rxiq_root_tx_correction_mode_high(bb);
        } else {
            crate::generated::set_rxiq_root_rx_correction_mode_high(bb);
            crate::generated::set_rxiq_root_tx_correction_mode_high(bb);
            crate::generated::clear_rxiq_root_rx_correction_mode_low(bb);
            crate::generated::clear_pbus_rxiq_status_second(&self.peripherals.phy_pbus);
        }
    }

    /// Configure all fourteen ordered TX-power tracking RMW edges.
    pub fn configure_tx_power_tracking(&mut self, enabled: bool) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        let state = if enabled {
            PhyTxPowerTrackingState::Enabled
        } else {
            PhyTxPowerTrackingState::Disabled
        };
        crate::generated::configure_tx_power_tracking_state(bb, state);
        crate::generated::clear_tx_power_tracking_initial_field(bb);
        crate::generated::configure_tx_power_tracking_initial_field(bb);

        // The complete body clears the adjacent bits through separate reads.
        crate::generated::clear_tx_power_tracking_control_low(bb);
        crate::generated::clear_tx_power_tracking_control_high(bb);

        crate::generated::configure_tx_power_tracking_value_5(bb);
        crate::generated::configure_tx_power_tracking_value_4(bb);
        crate::generated::configure_tx_power_tracking_value_3(bb);
        crate::generated::configure_tx_power_tracking_value_2(bb);
        crate::generated::configure_tx_power_tracking_value_1(bb);
        crate::generated::configure_tx_power_tracking_value_0(bb);
        crate::generated::configure_tx_power_tracking_value_8(bb);
        crate::generated::configure_tx_power_tracking_value_7(bb);
        crate::generated::configure_tx_power_tracking_value_6(bb);
    }

    /// Apply complete rev0 ROM `phy_btbb_wifi_bb_cfg2`.
    pub fn configure_bt_wifi_baseband(&mut self) {
        crate::generated::configure_bt_wifi_baseband_fields(
            &self.peripherals.phy_baseband_config_oracle,
        );
    }

    /// Apply complete rev0 ROM `phy_chan_dump_cfg`.
    pub fn configure_channel_dump(&mut self, value: u32, enabled: u32, mode: u32) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::configure_phy_channel_dump_value(bb, vendor_register_argument(value));
        crate::generated::configure_phy_channel_dump_mode(bb, vendor_register_argument(mode));
        crate::generated::configure_phy_channel_dump_enabled(bb, vendor_register_argument(enabled));
    }

    /// Apply complete rev0 ROM `phy_dac_rate_set`.
    pub fn configure_dac_rate(&mut self, rate: crate::PhyAdcRate) {
        self.configure_adc_rate(rate);
    }

    /// Configure both I²C TX-rate fields and the four gain-compensation bytes.
    pub fn configure_i2c_tx_rate(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::configure_phy_i2c_tx_rate_high(bb);
        crate::generated::configure_phy_i2c_tx_rate_low(bb);
        self.restore_tx_gain_compensation();
    }

    /// Configure the complete baseband watchdog leaf.
    pub fn configure_baseband_watchdog(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::configure_phy_baseband_watchdog_control(bb);
        crate::generated::enable_phy_baseband_watchdog(bb);
    }

    /// Replace the standalone PHY VHT-support bit through one fresh RMW.
    pub fn set_vht_support(&mut self, input: u32) {
        crate::generated::configure_phy_vht_support(
            &self.peripherals.phy_frequency_channel_oracle,
            vendor_register_argument(input),
        );
    }

    /// Replace the PHY CSI-dump force-LLTF bit through one fresh RMW.
    pub fn set_csi_dump_force_lltf(&mut self, input: u32) {
        crate::generated::configure_phy_csi_dump_force_lltf(
            &self.peripherals.phy_agc_oracle,
            vendor_register_argument(input),
        );
    }

    /// Apply complete ROM `phy_hemu_ru26_good_res`.
    pub fn configure_he_ru26_good_response(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::enable_phy_he_ru26_good_response(bb);
        crate::generated::clear_phy_he_ru26_good_response_disable(bb);
    }

    /// Apply complete ROM `phy_freq_band_reg_set` and its VHT tail.
    pub fn set_frequency_band(&mut self, input: u32) {
        crate::generated::configure_phy_frequency_band_inverse(
            &self.peripherals.phy_agc_oracle,
            vendor_register_argument(input),
        );
        self.set_vht_support(input);
    }

    /// Apply the three fresh RMWs of complete ROM `phy_bbtx_outfilter`.
    pub fn configure_tx_output_filter(&mut self, input_0: u32, input_1: u32, input_2: u32) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::configure_phy_tx_output_filter_0(bb, vendor_register_argument(input_0));
        crate::generated::configure_phy_tx_output_filter_1(bb, vendor_register_argument(input_1));
        crate::generated::configure_phy_tx_output_filter_2(bb, vendor_register_argument(input_2));
    }

    /// Replace the baseband watchdog reset-enable bit.
    pub fn set_baseband_watchdog_reset_enabled(&mut self, input: u32) {
        crate::generated::configure_phy_baseband_watchdog_reset(
            &self.peripherals.phy_baseband_config_oracle,
            vendor_register_argument(input),
        );
    }

    /// Replace the baseband watchdog interrupt-enable bit.
    pub fn set_baseband_watchdog_interrupt_enabled(&mut self, input: u32) {
        crate::generated::configure_phy_baseband_watchdog_interrupt(
            &self.peripherals.phy_baseband_config_oracle,
            vendor_register_argument(input),
        );
    }

    /// Set the baseband watchdog timeout-clear bit through one fresh RMW.
    pub fn clear_baseband_watchdog_timeout(&mut self) {
        crate::generated::clear_phy_baseband_watchdog_timeout(
            &self.peripherals.phy_baseband_config_oracle,
        );
    }

    /// Return the complete standalone baseband watchdog status word.
    pub fn baseband_watchdog_status(&mut self) -> u32 {
        crate::svd::field_read::observe_phy_baseband_watchdog_status(
            &self.peripherals.phy_baseband_config_oracle,
        )
    }

    /// Apply both fresh RMWs of complete ROM `phy_lltf_mask_en`.
    pub fn configure_lltf_mask(&mut self, input_0: u32, input_1: u32) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::configure_phy_lltf_mask_0(bb, vendor_register_argument(input_0));
        crate::generated::configure_phy_lltf_mask_1(bb, vendor_register_argument(input_1));
    }

    /// Enable all four recovered automatic noise-floor controls.
    pub fn configure_noise_floor_auto(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::enable_noise_floor_auto_control_low(bb);
        crate::generated::enable_noise_floor_auto_control_high(bb);
        crate::generated::enable_noise_floor_auto_path_0(bb);
        crate::generated::enable_noise_floor_auto_path_1(bb);
    }

    /// Apply all six ordered PA-on configuration operations.
    pub fn configure_tx_pa_on(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::configure_phy_tx_pa_on_field(bb);
        crate::generated::configure_phy_tx_pa_on_high_0(bb);
        crate::svd::fixed_register_image::initialize_tx_pa_table(bb);
        crate::generated::configure_phy_tx_pa_on_timing(bb);
        crate::generated::configure_phy_tx_pa_on_high_1(bb);
        crate::generated::configure_phy_tx_pa_on_bt_delay(bb);
    }

    /// Apply the local prefix of complete rev0 ROM `phy_bb_reg_init`.
    pub fn initialize_baseband_prefix(&mut self) {
        crate::generated::initialize_phy_baseband_prefix(
            &self.peripherals.phy_baseband_config_oracle,
        );
    }

    /// Apply the twelve local middle edges of complete `phy_bb_reg_init`.
    pub fn initialize_baseband_middle(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::initialize_phy_baseband_7808(bb);
        crate::generated::initialize_phy_baseband_78dc(bb);
        crate::generated::clear_phy_baseband_78e4(bb);
        crate::generated::clear_phy_baseband_tx_pa_timing_init(bb);
        crate::generated::clear_phy_baseband_790c_init(bb);
        crate::generated::enable_phy_baseband_7ca8_init(bb);
        crate::generated::clear_phy_baseband_7980_init(bb);

        // Complete ROM updates the adjacent mode bits through separate reads.
        crate::generated::clear_phy_he_ru26_good_response_disable(bb);
        crate::generated::enable_phy_he_ru26_good_response(bb);
        crate::generated::clear_phy_baseband_7a28_init(bb);
        crate::generated::initialize_phy_baseband_mode_fields(bb);
        crate::generated::enable_phy_baseband_tx_pa_init(bb);
    }

    /// Apply the five local tail edges of complete `phy_bb_reg_init`.
    pub fn initialize_baseband_tail(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        // Complete ROM clears bits 7:6 and bit 8 through separate reads.
        crate::generated::clear_phy_baseband_743c_low(bb);
        crate::generated::clear_phy_baseband_743c_high(bb);
        crate::generated::enable_phy_baseband_7428_init(bb);
        crate::generated::initialize_phy_baseband_7428_value(bb);
        crate::generated::initialize_phy_baseband_mode_fields(bb);
    }

    /// Apply the five internal-MMIO stores of complete ROM `phy_pwdet_reg_init`.
    pub fn initialize_power_detector_registers(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::svd::fixed_register_image::initialize_power_detector_table_0(bb);
        crate::svd::fixed_register_image::initialize_power_detector_table_1(bb);
        crate::generated::initialize_phy_power_detector_calibration(bb);
        crate::svd::zero_based_field_write::power_detector_reference(bb, 0xaaaa);
        crate::generated::initialize_phy_power_detector_mode(bb);
    }

    /// Apply the internal-MMIO portion of complete ROM `phy_en_pwdet`.
    pub fn configure_power_detector_enabled(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::clear_phy_power_detector_enable_middle(bb);
        crate::generated::clear_phy_power_detector_enable_low(bb);
        crate::generated::clear_phy_power_detector_enable_high(bb);
        crate::generated::enable_phy_power_detector_sar_mode(bb);
        crate::generated::clear_phy_power_detector_sar_config(bb);
        crate::svd::zero_based_field_write::power_detector_reference(bb, 0x016a);
    }

    /// Set the final background-control bit after PWDET enable.
    pub fn enable_power_detector_background_control(&mut self) {
        crate::generated::enable_phy_power_detector_background_control(
            &self.peripherals.phy_baseband_config_oracle,
        );
    }

    /// Capture the two fields owned by TX-DC PWDET calibration.
    pub fn capture_txdc_power_detector(&self) -> TxDcPwdetFields {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        TxDcPwdetFields {
            table_low: crate::svd::field_read::capture_phy_txdc_power_detector_table_low(bb),
            calibration: crate::svd::field_read::capture_phy_txdc_power_detector_calibration(bb),
        }
    }

    /// Publish the two temporary TX-DC PWDET calibration fields.
    pub fn apply_txdc_power_detector_calibration(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::prepare_phy_txdc_power_detector_table_low(bb);
        crate::generated::prepare_phy_txdc_power_detector_calibration(bb);
    }

    /// Select TX-DC SAR mode one after the initial PBus setup.
    pub fn configure_txdc_power_detector_sar(&mut self) {
        crate::generated::select_phy_txdc_power_detector_sar_mode(
            &self.peripherals.phy_baseband_config_oracle,
        );
    }

    /// Restore captured TX-DC fields and select final SAR mode.
    pub fn restore_txdc_power_detector(&mut self, fields: TxDcPwdetFields) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        let table_low =
            crate::generated::PhyPowerDetectorRestoreByte::new(u32::from(fields.table_low))
                .expect("captured power-detector byte fits its generated restore domain");
        let calibration =
            crate::generated::PhyPowerDetectorRestoreByte::new(u32::from(fields.calibration))
                .expect("captured power-detector byte fits its generated restore domain");
        crate::generated::restore_phy_txdc_power_detector_table_low(bb, table_low);
        crate::generated::restore_phy_txdc_power_detector_calibration(bb, calibration);
        crate::generated::enable_phy_power_detector_sar_mode(bb);
    }

    /// Publish one zero-extended power-detector reference word.
    pub fn write_power_detector_reference(&mut self, value: u16) {
        crate::svd::zero_based_field_write::power_detector_reference(
            &self.peripherals.phy_baseband_config_oracle,
            value,
        );
    }

    /// Pulse the power-detector SAR trigger through two fresh RMW edges.
    pub fn trigger_power_detector_sar(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::lower_phy_power_detector_sar_trigger(bb);
        crate::generated::raise_phy_power_detector_sar_trigger(bb);
    }

    /// Read the SVD-described power-detector readiness field.
    pub fn power_detector_ready(&mut self) -> bool {
        crate::svd::field_read::observe_phy_power_detector_ready(
            &self.peripherals.phy_baseband_config_oracle,
        ) == 0b111
    }

    /// Read the SVD-described power-detector SAR sample field.
    pub fn power_detector_sar_sample(&mut self) -> u16 {
        crate::svd::field_read::observe_phy_power_detector_sar_sample(
            &self.peripherals.phy_baseband_config_oracle,
        )
    }

    fn clear_tx_gain_compensation(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::svd::zero_register_write::clear_tx_gain_compensation(bb);
        crate::svd::zero_register_write::clear_tx_gain_compensation_aux(bb);
    }

    /// Apply complete pinned `phy_txgain_comp_pacfg_new(1)` as four ordered
    /// fresh-read byte updates.
    pub fn restore_tx_gain_compensation(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::restore_phy_tx_gain_compensation_byte_0(bb);
        crate::generated::restore_phy_tx_gain_compensation_byte_1(bb);
        crate::generated::restore_phy_tx_gain_compensation_byte_2(bb);
        crate::generated::restore_phy_tx_gain_compensation_byte_3(bb);
    }

    fn configure_tone_selectors(&mut self, path_0: u16, path_1: u16) {
        let path_0 = crate::generated::PhyToneSelector::new(u32::from(path_0))
            .expect("tone selector must fit the generated ten-bit domain");
        let path_1 = crate::generated::PhyToneSelector::new(u32::from(path_1))
            .expect("tone selector must fit the generated ten-bit domain");
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::configure_phy_tone_path_0_selector_low(bb, path_0);
        crate::generated::configure_phy_tone_path_1_selector_low(bb, path_1);
    }

    fn configure_tone_paths(&mut self, enabled: bool, path_0_selector: u16, path_0_step: u8) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::configure_phy_tone_path_0(
            bb,
            tone_selector_high(path_0_selector),
            tone_two_bit(0),
            tone_byte(path_0_step.wrapping_neg()),
            enabled,
            tone_three_bit(0),
            tone_two_bit(0),
            tone_four_bit(0),
        );
        crate::generated::clear_phy_tone_path_1_low_image(bb);
    }

    /// Set digital-gain forcing before publishing both signed gain bytes.
    /// Disabling retains the supplied gain values for the next calibration.
    pub fn configure_forced_digital_gain(&mut self, enabled: bool, gain_0: i8, gain_1: i8) {
        let clock = &self.peripherals.phy_clock_oracle;
        clock
            .table_memory_index_source()
            .modify(|_, w| w.force_digital_gain_enable().bit(enabled));
        let gain_0 = crate::generated::PhyDigitalGainImage::new(u32::from(gain_0 as u8))
            .expect("signed gain byte fits its hardware image");
        let gain_1 = crate::generated::PhyDigitalGainImage::new(u32::from(gain_1 as u8))
            .expect("signed gain byte fits its hardware image");
        crate::generated::configure_phy_forced_digital_gain_0(clock, gain_0);
        crate::generated::configure_phy_forced_digital_gain_1(clock, gain_1);
    }

    /// Program the calibration tone, restoring TX gain only when stopping it.
    ///
    /// This preserves every fresh-read/write edge in
    /// `libphy.a[phy_reg.o]::phy_start_tx_tone_step_new` and its
    /// `phy_txgain_comp_pacfg_new` child.
    pub fn configure_calibration_tone(&mut self, enabled: bool, selector: u16, step: u8) {
        crate::generated::clear_phy_power_control_tone_stop(
            &self.peripherals.phy_baseband_config_oracle,
        );
        self.clear_tx_gain_compensation();
        self.configure_tone_selectors(selector, 0);
        self.configure_tone_paths(enabled, selector, step);
        if !enabled {
            crate::generated::stop_phy_tone_paths(&self.peripherals.phy_baseband_config_oracle);
            self.restore_tx_gain_compensation();
        }
    }

    /// Program the ROM power-control tone with DAC scale and TX gain disabled.
    pub fn configure_power_control_tone(&mut self, selector: u16, step: u8) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::clear_phy_power_control_tone_stop(bb);
        crate::generated::clear_phy_dac_scale_high(bb);
        crate::generated::clear_phy_dac_scale_low(bb);
        self.clear_tx_gain_compensation();
        self.configure_tone_selectors(selector, 0);
        self.configure_tone_paths(true, selector, step);
    }

    /// Capture the first-path TX-IQ tone-control field state.
    pub fn capture_txiq_tone_control(&self) -> TxIqToneControlFields {
        let (
            selector_high,
            low_reserved_clear_opaque,
            negated_step_or_attenuation,
            tone_enable_or_arm,
            txiq_mismatch_mode_opaque,
            middle_reserved_clear_opaque,
            txiq_polarity_image,
            high_nibble_opaque,
        ) = crate::svd::field_snapshot_read::capture_phy_txiq_tone_control(
            &self.peripherals.phy_baseband_config_oracle,
        );
        TxIqToneControlFields {
            selector_high,
            low_reserved_clear_opaque,
            negated_step_or_attenuation,
            tone_enable_or_arm,
            txiq_mismatch_mode_opaque,
            middle_reserved_clear_opaque,
            txiq_polarity_image,
            high_nibble_opaque,
        }
    }

    /// Restore captured TX-IQ tone-control field state.
    pub fn restore_txiq_tone_control(&mut self, fields: TxIqToneControlFields) {
        crate::svd::zero_based_field_write::restore_phy_txiq_tone_control(
            &self.peripherals.phy_baseband_config_oracle,
            fields.selector_high,
            fields.low_reserved_clear_opaque,
            fields.negated_step_or_attenuation,
            fields.tone_enable_or_arm,
            fields.txiq_mismatch_mode_opaque,
            fields.middle_reserved_clear_opaque,
            fields.txiq_polarity_image,
            fields.high_nibble_opaque,
        );
    }

    /// Configure one of the two complete TX-IQ mismatch-power polarity edges.
    pub fn configure_txiq_mismatch_power(
        &mut self,
        first: bool,
        polarity: bool,
        attenuation: u8,
        selector: u16,
    ) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        if first {
            crate::generated::configure_phy_tone_path_0(
                bb,
                tone_selector_high(selector),
                tone_two_bit(0),
                tone_byte(attenuation.wrapping_neg()),
                true,
                tone_three_bit(5),
                tone_two_bit(0),
                tone_four_bit(if polarity { 4 } else { 0 }),
            );
            let selector = crate::generated::PhyToneSelector::new(u32::from(selector))
                .expect("tone selector must fit the generated ten-bit domain");
            crate::generated::configure_phy_tone_path_0_selector_low(bb, selector);
        } else {
            crate::generated::configure_phy_txiq_second_polarity(
                bb,
                tone_four_bit(if polarity { 8 } else { 1 }),
            );
        }
    }

    /// Set or clear the shared first-path arm bit for one PWDET sample.
    pub fn set_power_detector_tone_armed(&mut self, armed: bool) {
        let armed = if armed {
            crate::generated::PhyPowerDetectorToneArmState::Armed
        } else {
            crate::generated::PhyPowerDetectorToneArmState::Disarmed
        };
        crate::generated::set_phy_power_detector_tone_armed(
            &self.peripherals.phy_baseband_config_oracle,
            armed,
        );
    }

    /// Stop both tone paths and restore the two DAC-scale fields.
    pub fn stop_power_detector_tone(&mut self) {
        self.stop_calibration_tone_paths();
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::restore_phy_dac_scale_high(bb);
        crate::generated::restore_phy_dac_scale_low(bb);
    }

    /// Stop both tone paths without changing their DAC-scale fields.
    ///
    /// This is the complete pinned `libphy.a` `phy_stop_tx_tone_new` leaf.
    /// The longer ROM `phy_stop_tx_tone(1)` composes this exact prefix with
    /// two additional DAC-scale restores in [`Self::stop_power_detector_tone`].
    pub fn stop_calibration_tone_paths(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::disable_phy_tone_path_0(bb);
        crate::generated::disable_phy_tone_path_1(bb);
        crate::generated::stop_phy_tone_paths(bb);
    }

    /// Enter or complete the TX-IQ correction phase with one fresh RMW.
    ///
    /// Complete ROM `phy_rfcal_txiq` clears the high mode bit while setting
    /// the low bit on entry. Its completion edge sets only the high bit.
    pub fn configure_tx_iq_correction(&mut self, begin: bool) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        if begin {
            crate::generated::begin_phy_tx_iq_correction(bb);
        } else {
            crate::generated::complete_phy_tx_iq_correction(bb);
        }
    }

    /// Select the RX-IQ calibration mode with one fresh RMW.
    pub fn configure_rx_iq_calibration_mode(&mut self) {
        crate::generated::configure_phy_rx_iq_calibration_mode(
            &self.peripherals.phy_baseband_config_oracle,
        );
    }

    /// Publish one signed TX-IQ gain coefficient using the ROM saturation.
    pub fn set_tx_iq_gain_coefficient(&mut self, coefficient: i8) {
        crate::generated::publish_phy_tx_iq_gain_coefficient(
            &self.peripherals.phy_baseband_config_oracle,
            iq_coefficient_image(saturate_tx_iq_gain(coefficient)),
        );
    }

    /// Publish one signed TX-IQ phase coefficient using the ROM saturation.
    pub fn set_tx_iq_phase_coefficient(&mut self, coefficient: i8) {
        crate::generated::publish_phy_tx_iq_phase_coefficient(
            &self.peripherals.phy_baseband_config_oracle,
            iq_coefficient_image(saturate_tx_iq_phase(coefficient)),
        );
    }

    /// Publish one signed RX-IQ gain coefficient using the ROM truncation.
    pub fn set_rx_iq_gain_coefficient(&mut self, coefficient: i8) {
        crate::generated::publish_phy_rx_iq_gain_coefficient(
            &self.peripherals.phy_baseband_config_oracle,
            iq_coefficient_image(coefficient),
        );
    }

    /// Publish one signed RX-IQ phase coefficient using the ROM truncation.
    pub fn set_rx_iq_phase_coefficient(&mut self, coefficient: i8) {
        crate::generated::publish_phy_rx_iq_phase_coefficient(
            &self.peripherals.phy_baseband_config_oracle,
            iq_coefficient_image(coefficient),
        );
    }

    /// Trigger one TX-DC comparator measurement using three fresh RMW edges.
    pub fn trigger_tx_dc_measurement(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::enable_phy_tx_dc_measurement(bb);
        crate::generated::clear_phy_tx_dc_measurement_start(bb);
        crate::generated::start_phy_tx_dc_measurement(bb);
    }

    /// Sample the TX-DC ready bit exactly once.
    pub fn tx_dc_measurement_is_ready(&mut self) -> bool {
        crate::svd::field_read::observe_phy_tx_dc_measurement_ready(
            &self.peripherals.phy_baseband_config_oracle,
        )
    }

    /// Preserve the complete ROM's independent I and Q comparator reads.
    pub fn sample_tx_dc_comparators(&mut self) -> [bool; 2] {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        [
            crate::svd::field_read::observe_phy_tx_dc_i_comparator_high(bb),
            crate::svd::field_read::observe_phy_tx_dc_q_comparator_high(bb),
        ]
    }

    /// Clear TX-DC enable and start through two fresh RMW edges.
    pub fn clear_tx_dc_measurement(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::disable_phy_tx_dc_measurement(bb);
        crate::generated::clear_phy_tx_dc_measurement_start(bb);
    }

    /// Publish the two-register suffix of complete ROM `phy_adc_rate_set`.
    ///
    /// The ROM body at `0x2f82_a6d2`, size `0x4a`, uses two fresh reads to
    /// publish the selected semantic rate into the two recovered fields.
    pub fn configure_adc_rate(&mut self, rate: crate::PhyAdcRate) {
        let rate = match rate {
            crate::PhyAdcRate::Low => crate::generated::PhyAdcRateSelection::Low,
            crate::PhyAdcRate::High => crate::generated::PhyAdcRateSelection::High,
        };
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::configure_phy_adc_rate_high(bb, rate);
        crate::generated::configure_phy_adc_rate_low(bb, rate);
    }

    /// Apply the four front-end initialization edges before table-memory setup.
    ///
    /// This is the exact prefix of complete rev0 ROM `phy_fe_reg_init` at
    /// `0x2f82_7740`, size `0xf6`. The table-memory edge remains between this
    /// method and [`Self::initialize_front_end_suffix`].
    pub fn initialize_front_end_prefix(&mut self) {
        crate::generated::initialize_phy_front_end_pbus(&self.peripherals.phy_pbus);
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::initialize_phy_front_end_first(bb);
        crate::generated::initialize_phy_front_end_second(bb);
        crate::generated::clear_phy_front_end_first(bb);
    }

    /// Apply the twelve front-end initialization edges after table-memory setup.
    ///
    /// Complete rev0 ROM `phy_fe_reg_init` performs every update below using
    /// a fresh read. Repeated sets are retained because intermediate device
    /// states are observable hardware behavior.
    pub fn initialize_front_end_suffix(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::enable_phy_front_end_init(bb);
        crate::generated::enable_rx_iq_correction_modes(bb);
        crate::generated::enable_tx_iq_correction_modes(bb);
        crate::generated::clear_phy_front_end_second(bb);
        crate::generated::enable_phy_front_end_adc_rate_high(bb);
        crate::generated::enable_phy_front_end_adc_rate_low(bb);
        crate::generated::configure_phy_front_end_low(bb);
        crate::generated::enable_phy_front_end_adc_rate_low(bb);
        crate::generated::enable_phy_front_end_adc_rate_high(bb);
        crate::generated::enable_phy_rx_iq_front_end_high(bb);
        crate::generated::enable_phy_tx_iq_front_end_high(bb);
        crate::generated::initialize_phy_front_end_low(bb);
    }

    /// Apply complete pinned `libphy.a[phy_reg.o]::phy_fe_reg_update`.
    ///
    /// The `0x32`-byte body performs exactly three fresh-read RMW edges and
    /// has no ROM-only DAC-scale tail.
    pub fn update_front_end(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::initialize_phy_front_end_first(bb);
        crate::generated::initialize_phy_front_end_second(bb);
        crate::generated::enable_phy_front_end_adc_rates(bb);
    }

    /// Select the direct-register prefix or cleanup state of RX-gain DC calibration.
    ///
    /// Complete rev0 ROM `phy_set_rx_gain_cal_dc` at `0x2f82_9858`, size
    /// `0x206`, sets bits 6:5 to `0b11` before entering the bounded
    /// calibration graph and clears them to `0b00` in its common cleanup.
    /// The field's narrower electrical meaning is not independently proved.
    pub fn set_rx_gain_dc_calibration(&mut self, enabled: bool) {
        let state = if enabled {
            crate::generated::PhyRxGainDcCalibrationState::Enabled
        } else {
            crate::generated::PhyRxGainDcCalibrationState::Disabled
        };
        crate::generated::configure_phy_rx_gain_dc_calibration(
            &self.peripherals.phy_baseband_config_oracle,
            state,
        );
    }
}

#[cfg(test)]
mod tests;
