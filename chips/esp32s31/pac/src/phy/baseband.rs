//! Ownership-bound access to recovered ESP32-S31 PHY/baseband registers.
//!
//! Register layout and legal field images come from
//! `chips/esp32s31/registers/published/radio.svd`. Complete ROM/blob bodies cited there define the
//! finite operation order.

#![deny(unsafe_code)]

use crate::{RadioPhyRegisters, generated::PhyTxPowerTrackingState};

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

impl TxIqToneControlFields {
    /// Every field of the first tone path's control, from one read.
    fn capture<P: oer_mmio::Placement>(
        bb: &crate::generated::phy_baseband_config_oracle::PhyBasebandConfigOracle<P>,
    ) -> Self {
        let control = bb.read_tone_path_0_control();
        Self {
            selector_high: control.selector_high(),
            low_reserved_clear_opaque: crate::field_byte(control.bits_8_9_opaque()),
            negated_step_or_attenuation: control.negated_step_or_attenuation(),
            tone_enable_or_arm: control.tone_enable_or_arm(),
            txiq_mismatch_mode_opaque: crate::field_byte(control.bits_19_21_opaque()),
            middle_reserved_clear_opaque: crate::field_byte(control.bits_22_23_opaque()),
            txiq_polarity_image: crate::field_byte(control.txiq_polarity_image()),
            high_nibble_opaque: crate::field_byte(control.bits_28_31_opaque()),
        }
    }

    /// Store every captured field of the first tone path's control from
    /// zero, as `phy_txiq_get_mis_pwr` restores the word.
    fn restore<P: oer_mmio::Placement>(
        self,
        bb: &mut crate::generated::phy_baseband_config_oracle::PhyBasebandConfigOracle<P>,
    ) {
        bb.restore_phy_txiq_tone_control(
            self.selector_high,
            oer_mmio::Bits::truncate(u32::from(self.low_reserved_clear_opaque)),
            self.negated_step_or_attenuation,
            self.tone_enable_or_arm,
            oer_mmio::Bits::truncate(u32::from(self.txiq_mismatch_mode_opaque)),
            oer_mmio::Bits::truncate(u32::from(self.middle_reserved_clear_opaque)),
            oer_mmio::Bits::truncate(u32::from(self.txiq_polarity_image)),
            oer_mmio::Bits::truncate(u32::from(self.high_nibble_opaque)),
        );
    }
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

    /// Construct one distinguishable capture for a host test.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub const fn for_host_test(value: u8) -> Self {
        Self(value)
    }
}

impl TxDcPwdetFields {
    /// Construct one distinguishable capture for a host test.
    #[cfg(test)]
    #[doc(hidden)]
    pub const fn for_host_test(table_low: u8, calibration: u8) -> Self {
        Self {
            table_low,
            calibration,
        }
    }
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

fn tone_four_bit(value: u32) -> crate::generated::PhyToneFourBitImage {
    crate::generated::PhyToneFourBitImage::new(value)
        .expect("reviewed tone transaction supplies a complete four-bit image")
}

/// The upper eight bits of a ten-bit tone selector.
fn tone_selector_high(selector: u16) -> u8 {
    let selector = crate::generated::PhyToneSelector::new(u32::from(selector))
        .expect("tone selector must fit the generated ten-bit domain");
    crate::field_byte(oer_mmio::Bits::<8>::truncate(selector.get() / 4))
}

/// The first tone path's control fields of one vendor edge.
struct TonePath0 {
    selector_high: u8,
    negated_step: u8,
    enabled: bool,
    mismatch_mode: oer_mmio::Bits<3>,
    polarity: oer_mmio::Bits<4>,
}

/// One fresh-read RMW of the first tone path's control: the selector's
/// upper byte, the negated step, the enable, the mismatch mode and the
/// polarity, with both reserved lanes written zero; the high nibble keeps
/// the value read.
fn configure_tone_path_0<P: oer_mmio::Placement>(
    bb: &mut crate::generated::phy_baseband_config_oracle::PhyBasebandConfigOracle<P>,
    path: TonePath0,
) {
    bb.modify_tone_path_0_control(|w| {
        w.set_selector_high(path.selector_high)
            .set_bits_8_9_opaque(oer_mmio::Bits::constant::<0>())
            .set_negated_step_or_attenuation(path.negated_step)
            .set_tone_enable_or_arm(path.enabled)
            .set_bits_19_21_opaque(path.mismatch_mode)
            .set_bits_22_23_opaque(oer_mmio::Bits::constant::<0>())
            .set_txiq_polarity_image(path.polarity);
    });
}

impl RadioPhyRegisters {
    /// Apply both fresh-read edges of complete ROM `phy_fe_txrx_reset`.
    pub fn reset_frontend_txrx(&mut self) {
        let registers = &mut self.peripherals.phy_fedata_recovered;
        registers.clear_phy_frontend_txrx_reset_state();
        registers.assert_phy_frontend_txrx_reset_state();
    }

    /// Enable both RX- and TX-IQ correction modes through two fresh RMWs.
    ///
    /// Complete rev0 ROM `phy_iq_corr_enable` at `0x2f82_7d8c` sets both
    /// recovered mode bits in each word while preserving all coefficients.
    pub fn enable_iq_correction_modes(&mut self) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        bb.enable_rx_iq_correction_modes();
        bb.enable_tx_iq_correction_modes();
    }

    /// Publish both RXIQ root status bits through independent fresh RMWs.
    pub fn configure_rxiq_root_status(&mut self) {
        let pbus = &mut self.peripherals.phy_pbus;
        pbus.set_pbus_rxiq_status_first();
        pbus.set_pbus_rxiq_status_second();
    }

    /// Apply the complete four-edge RXIQ correction prefix or suffix.
    pub fn configure_rxiq_root_correction(&mut self, begin: bool) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        if begin {
            bb.set_rxiq_root_rx_correction_mode_low();
            bb.set_rxiq_root_tx_correction_mode_low();
            bb.clear_rxiq_root_rx_correction_mode_high();
            bb.clear_rxiq_root_tx_correction_mode_high();
        } else {
            bb.set_rxiq_root_rx_correction_mode_high();
            bb.set_rxiq_root_tx_correction_mode_high();
            bb.clear_rxiq_root_rx_correction_mode_low();
            self.peripherals.phy_pbus.clear_pbus_rxiq_status_second();
        }
    }

    /// Configure all fourteen ordered TX-power tracking RMW edges.
    pub fn configure_tx_power_tracking(&mut self, enabled: bool) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        let state = if enabled {
            PhyTxPowerTrackingState::Enabled
        } else {
            PhyTxPowerTrackingState::Disabled
        };
        bb.configure_tx_power_tracking_state(state);
        bb.clear_tx_power_tracking_initial_field();
        bb.configure_tx_power_tracking_initial_field();

        // The complete body clears the adjacent bits through separate reads.
        bb.clear_tx_power_tracking_control_low();
        bb.clear_tx_power_tracking_control_high();

        bb.configure_tx_power_tracking_value_5();
        bb.configure_tx_power_tracking_value_4();
        bb.configure_tx_power_tracking_value_3();
        bb.configure_tx_power_tracking_value_2();
        bb.configure_tx_power_tracking_value_1();
        bb.configure_tx_power_tracking_value_0();
        bb.configure_tx_power_tracking_value_8();
        bb.configure_tx_power_tracking_value_7();
        bb.configure_tx_power_tracking_value_6();
    }

    /// Configure both I²C TX-rate fields and the four gain-compensation bytes.
    pub fn configure_i2c_tx_rate(&mut self) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        bb.configure_phy_i2c_tx_rate_high();
        bb.configure_phy_i2c_tx_rate_low();
        self.restore_tx_gain_compensation();
    }

    /// Configure the complete baseband watchdog leaf.
    pub fn configure_baseband_watchdog(&mut self) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        bb.configure_phy_baseband_watchdog_control();
        bb.enable_phy_baseband_watchdog();
    }

    /// Enable all four recovered automatic noise-floor controls.
    pub fn configure_noise_floor_auto(&mut self) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        bb.enable_noise_floor_auto_control_low();
        bb.enable_noise_floor_auto_control_high();
        bb.enable_noise_floor_auto_path_0();
        bb.enable_noise_floor_auto_path_1();
    }

    /// Apply all six ordered PA-on configuration operations.
    pub fn configure_tx_pa_on(&mut self) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        bb.configure_phy_tx_pa_on_field();
        bb.configure_phy_tx_pa_on_high_0();
        bb.initialize_tx_pa_table();
        bb.configure_phy_tx_pa_on_timing();
        bb.configure_phy_tx_pa_on_high_1();
        bb.configure_phy_tx_pa_on_bt_delay();
    }

    /// Apply the local prefix of complete rev0 ROM `phy_bb_reg_init`.
    pub fn initialize_baseband_prefix(&mut self) {
        self.peripherals
            .phy_baseband_config_oracle
            .initialize_phy_baseband_prefix();
    }

    /// Apply the twelve local middle edges of complete `phy_bb_reg_init`.
    pub fn initialize_baseband_middle(&mut self) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        bb.initialize_phy_baseband_7808();
        bb.initialize_phy_baseband_78dc();
        bb.clear_phy_baseband_78e4();
        bb.clear_phy_baseband_tx_pa_timing_init();
        bb.clear_phy_baseband_790c_init();
        bb.enable_phy_baseband_7ca8_init();
        bb.clear_phy_baseband_7980_init();

        // Complete ROM updates the adjacent mode bits through separate reads.
        bb.clear_phy_he_ru26_good_response_disable();
        bb.enable_phy_he_ru26_good_response();
        bb.clear_phy_baseband_7a28_init();
        bb.initialize_phy_baseband_mode_fields();
        bb.enable_phy_baseband_tx_pa_init();
    }

    /// Apply the five local tail edges of complete `phy_bb_reg_init`.
    pub fn initialize_baseband_tail(&mut self) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        // Complete ROM clears bits 7:6 and bit 8 through separate reads.
        bb.clear_phy_baseband_743c_low();
        bb.clear_phy_baseband_743c_high();
        bb.enable_phy_baseband_7428_init();
        bb.initialize_phy_baseband_7428_value();
        bb.initialize_phy_baseband_mode_fields();
    }

    /// Apply the five internal-MMIO stores of complete ROM `phy_pwdet_reg_init`.
    pub fn initialize_power_detector_registers(&mut self) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        bb.initialize_power_detector_table_0();
        bb.initialize_power_detector_table_1();
        bb.initialize_phy_power_detector_calibration();
        bb.power_detector_reference(0xaaaa);
        bb.initialize_phy_power_detector_mode();
    }

    /// Apply the internal-MMIO portion of complete ROM `phy_en_pwdet`.
    pub fn configure_power_detector_enabled(&mut self) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        bb.clear_phy_power_detector_enable_middle();
        bb.clear_phy_power_detector_enable_low();
        bb.clear_phy_power_detector_enable_high();
        bb.enable_phy_power_detector_sar_mode();
        bb.clear_phy_power_detector_sar_config();
        bb.power_detector_reference(0x016a);
    }

    /// Set the final background-control bit after PWDET enable.
    pub fn enable_power_detector_background_control(&mut self) {
        self.peripherals
            .phy_baseband_config_oracle
            .enable_phy_power_detector_background_control();
    }

    /// Capture the two fields owned by TX-DC PWDET calibration.
    pub fn capture_txdc_power_detector(&self) -> TxDcPwdetFields {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        TxDcPwdetFields {
            table_low: bb.read_power_detector_table_1().bits_0_7_opaque(),
            calibration: bb.read_power_detector_control().bits_4_11_opaque(),
        }
    }

    /// Publish the two temporary TX-DC PWDET calibration fields.
    pub fn apply_txdc_power_detector_calibration(&mut self) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        bb.prepare_phy_txdc_power_detector_table_low();
        bb.prepare_phy_txdc_power_detector_calibration();
    }

    /// Select TX-DC SAR mode one after the initial PBus setup.
    pub fn configure_txdc_power_detector_sar(&mut self) {
        self.peripherals
            .phy_baseband_config_oracle
            .select_phy_txdc_power_detector_sar_mode();
    }

    /// Restore captured TX-DC fields and select final SAR mode.
    pub fn restore_txdc_power_detector(&mut self, fields: TxDcPwdetFields) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        let table_low =
            crate::generated::PhyPowerDetectorRestoreByte::new(u32::from(fields.table_low))
                .expect("captured power-detector byte fits its generated restore domain");
        let calibration =
            crate::generated::PhyPowerDetectorRestoreByte::new(u32::from(fields.calibration))
                .expect("captured power-detector byte fits its generated restore domain");
        bb.restore_phy_txdc_power_detector_table_low(table_low);
        bb.restore_phy_txdc_power_detector_calibration(calibration);
        bb.enable_phy_power_detector_sar_mode();
    }

    /// Publish one zero-extended power-detector reference word.
    pub fn write_power_detector_reference(&mut self, value: u16) {
        self.peripherals
            .phy_baseband_config_oracle
            .power_detector_reference(value);
    }

    /// Pulse the power-detector SAR trigger through two fresh RMW edges.
    pub fn trigger_power_detector_sar(&mut self) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        bb.lower_phy_power_detector_sar_trigger();
        bb.raise_phy_power_detector_sar_trigger();
    }

    /// Read the SVD-described power-detector readiness field.
    pub fn power_detector_ready(&mut self) -> bool {
        self.peripherals
            .phy_baseband_config_oracle
            .read_power_detector_sar_control_status()
            .sar_ready()
            == oer_mmio::Bits::constant::<0b111>()
    }

    /// Read the SVD-described power-detector SAR sample field.
    pub fn power_detector_sar_sample(&mut self) -> u16 {
        crate::field_halfword(
            self.peripherals
                .phy_baseband_config_oracle
                .read_power_detector_sar_result()
                .sar_sample(),
        )
    }

    fn clear_tx_gain_compensation(&mut self) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        bb.clear_tx_gain_compensation();
        bb.clear_tx_gain_compensation_aux();
    }

    /// Apply complete pinned `phy_txgain_comp_pacfg_new(1)` as four ordered
    /// fresh-read byte updates.
    pub fn restore_tx_gain_compensation(&mut self) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        bb.restore_phy_tx_gain_compensation_byte_0();
        bb.restore_phy_tx_gain_compensation_byte_1();
        bb.restore_phy_tx_gain_compensation_byte_2();
        bb.restore_phy_tx_gain_compensation_byte_3();
    }

    fn configure_tone_selectors(&mut self, path_0: u16, path_1: u16) {
        let path_0 = crate::generated::PhyToneSelector::new(u32::from(path_0))
            .expect("tone selector must fit the generated ten-bit domain");
        let path_1 = crate::generated::PhyToneSelector::new(u32::from(path_1))
            .expect("tone selector must fit the generated ten-bit domain");
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        bb.configure_phy_tone_path_0_selector_low(path_0);
        bb.configure_phy_tone_path_1_selector_low(path_1);
    }

    fn configure_tone_paths(&mut self, enabled: bool, path_0_selector: u16, path_0_step: u8) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        configure_tone_path_0(
            bb,
            TonePath0 {
                selector_high: tone_selector_high(path_0_selector),
                negated_step: path_0_step.wrapping_neg(),
                enabled,
                mismatch_mode: oer_mmio::Bits::constant::<0>(),
                polarity: oer_mmio::Bits::constant::<0>(),
            },
        );
        bb.clear_phy_tone_path_1_low_image();
    }

    /// Set digital-gain forcing before publishing both signed gain bytes.
    /// Disabling retains the supplied gain values for the next calibration.
    pub fn configure_forced_digital_gain(&mut self, enabled: bool, gain_0: i8, gain_1: i8) {
        let clock = &mut self.peripherals.phy_clock_oracle;
        clock.modify_table_memory_index_source(|w| {
            w.set_force_digital_gain_enable(enabled);
        });
        let gain_0 = crate::generated::PhyDigitalGainImage::new(u32::from(gain_0 as u8))
            .expect("signed gain byte fits its hardware image");
        let gain_1 = crate::generated::PhyDigitalGainImage::new(u32::from(gain_1 as u8))
            .expect("signed gain byte fits its hardware image");
        clock.configure_phy_forced_digital_gain_0(gain_0);
        clock.configure_phy_forced_digital_gain_1(gain_1);
    }

    /// Program the calibration tone, restoring TX gain only when stopping it.
    ///
    /// This preserves every fresh-read/write edge in
    /// `libphy.a[phy_reg.o]::phy_start_tx_tone_step_new` and its
    /// `phy_txgain_comp_pacfg_new` child.
    pub fn configure_calibration_tone(&mut self, enabled: bool, selector: u16, step: u8) {
        self.peripherals
            .phy_baseband_config_oracle
            .clear_phy_power_control_tone_stop();
        self.clear_tx_gain_compensation();
        self.configure_tone_selectors(selector, 0);
        self.configure_tone_paths(enabled, selector, step);
        if !enabled {
            self.peripherals
                .phy_baseband_config_oracle
                .stop_phy_tone_paths();
            self.restore_tx_gain_compensation();
        }
    }

    /// Program the ROM power-control tone with DAC scale and TX gain disabled.
    pub fn configure_power_control_tone(&mut self, selector: u16, step: u8) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        bb.clear_phy_power_control_tone_stop();
        bb.clear_phy_dac_scale_high();
        bb.clear_phy_dac_scale_low();
        self.clear_tx_gain_compensation();
        self.configure_tone_selectors(selector, 0);
        self.configure_tone_paths(true, selector, step);
    }

    /// Capture the first-path TX-IQ tone-control field state.
    pub fn capture_txiq_tone_control(&self) -> TxIqToneControlFields {
        TxIqToneControlFields::capture(&self.peripherals.phy_baseband_config_oracle)
    }

    /// Restore captured TX-IQ tone-control field state.
    pub fn restore_txiq_tone_control(&mut self, fields: TxIqToneControlFields) {
        fields.restore(&mut self.peripherals.phy_baseband_config_oracle);
    }

    /// Configure one of the two complete TX-IQ mismatch-power polarity edges.
    pub fn configure_txiq_mismatch_power(
        &mut self,
        first: bool,
        polarity: bool,
        attenuation: u8,
        selector: u16,
    ) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        if first {
            configure_tone_path_0(
                bb,
                TonePath0 {
                    selector_high: tone_selector_high(selector),
                    negated_step: attenuation.wrapping_neg(),
                    enabled: true,
                    mismatch_mode: oer_mmio::Bits::constant::<5>(),
                    polarity: if polarity {
                        oer_mmio::Bits::constant::<4>()
                    } else {
                        oer_mmio::Bits::constant::<0>()
                    },
                },
            );
            let selector = crate::generated::PhyToneSelector::new(u32::from(selector))
                .expect("tone selector must fit the generated ten-bit domain");
            bb.configure_phy_tone_path_0_selector_low(selector);
        } else {
            bb.configure_phy_txiq_second_polarity(tone_four_bit(if polarity { 8 } else { 1 }));
        }
    }

    /// Set or clear the shared first-path arm bit for one PWDET sample.
    pub fn set_power_detector_tone_armed(&mut self, armed: bool) {
        let armed = if armed {
            crate::generated::PhyPowerDetectorToneArmState::Armed
        } else {
            crate::generated::PhyPowerDetectorToneArmState::Disarmed
        };
        self.peripherals
            .phy_baseband_config_oracle
            .set_phy_power_detector_tone_armed(armed);
    }

    /// Stop both tone paths and restore the two DAC-scale fields.
    pub fn stop_power_detector_tone(&mut self) {
        self.stop_calibration_tone_paths();
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        bb.restore_phy_dac_scale_high();
        bb.restore_phy_dac_scale_low();
    }

    /// Stop both tone paths without changing their DAC-scale fields.
    ///
    /// This is the complete pinned `libphy.a` `phy_stop_tx_tone_new` leaf.
    /// The longer ROM `phy_stop_tx_tone(1)` composes this exact prefix with
    /// two additional DAC-scale restores in [`Self::stop_power_detector_tone`].
    pub fn stop_calibration_tone_paths(&mut self) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        bb.disable_phy_tone_path_0();
        bb.disable_phy_tone_path_1();
        bb.stop_phy_tone_paths();
    }

    /// Enter or complete the TX-IQ correction phase with one fresh RMW.
    ///
    /// Complete ROM `phy_rfcal_txiq` clears the high mode bit while setting
    /// the low bit on entry. Its completion edge sets only the high bit.
    pub fn configure_tx_iq_correction(&mut self, begin: bool) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        if begin {
            bb.begin_phy_tx_iq_correction();
        } else {
            bb.complete_phy_tx_iq_correction();
        }
    }

    /// Select the RX-IQ calibration mode with one fresh RMW.
    pub fn configure_rx_iq_calibration_mode(&mut self) {
        self.peripherals
            .phy_baseband_config_oracle
            .configure_phy_rx_iq_calibration_mode();
    }

    /// Publish one signed TX-IQ gain coefficient using the ROM saturation.
    pub fn set_tx_iq_gain_coefficient(&mut self, coefficient: i8) {
        self.peripherals
            .phy_baseband_config_oracle
            .publish_phy_tx_iq_gain_coefficient(iq_coefficient_image(saturate_tx_iq_gain(
                coefficient,
            )));
    }

    /// Publish one signed TX-IQ phase coefficient using the ROM saturation.
    pub fn set_tx_iq_phase_coefficient(&mut self, coefficient: i8) {
        self.peripherals
            .phy_baseband_config_oracle
            .publish_phy_tx_iq_phase_coefficient(iq_coefficient_image(saturate_tx_iq_phase(
                coefficient,
            )));
    }

    /// Publish one signed RX-IQ gain coefficient using the ROM truncation.
    pub fn set_rx_iq_gain_coefficient(&mut self, coefficient: i8) {
        self.peripherals
            .phy_baseband_config_oracle
            .publish_phy_rx_iq_gain_coefficient(iq_coefficient_image(coefficient));
    }

    /// Publish one signed RX-IQ phase coefficient using the ROM truncation.
    pub fn set_rx_iq_phase_coefficient(&mut self, coefficient: i8) {
        self.peripherals
            .phy_baseband_config_oracle
            .publish_phy_rx_iq_phase_coefficient(iq_coefficient_image(coefficient));
    }

    /// Trigger one TX-DC comparator measurement using three fresh RMW edges.
    pub fn trigger_tx_dc_measurement(&mut self) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        bb.enable_phy_tx_dc_measurement();
        bb.clear_phy_tx_dc_measurement_start();
        bb.start_phy_tx_dc_measurement();
    }

    /// Sample the TX-DC ready bit exactly once.
    pub fn tx_dc_measurement_is_ready(&mut self) -> bool {
        self.peripherals
            .phy_baseband_config_oracle
            .read_tx_dc_measurement_control_status()
            .measurement_ready()
    }

    /// Preserve the complete ROM's independent I and Q comparator reads.
    pub fn sample_tx_dc_comparators(&mut self) -> [bool; 2] {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        [
            bb.read_tx_dc_measurement_control_status()
                .i_comparator_high(),
            bb.read_tx_dc_measurement_control_status()
                .q_comparator_high(),
        ]
    }

    /// Clear TX-DC enable and start through two fresh RMW edges.
    pub fn clear_tx_dc_measurement(&mut self) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        bb.disable_phy_tx_dc_measurement();
        bb.clear_phy_tx_dc_measurement_start();
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
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        bb.configure_phy_adc_rate_high(rate);
        bb.configure_phy_adc_rate_low(rate);
    }

    /// Apply the four front-end initialization edges before table-memory setup.
    ///
    /// This is the exact prefix of complete rev0 ROM `phy_fe_reg_init` at
    /// `0x2f82_7740`, size `0xf6`. The table-memory edge remains between this
    /// method and [`Self::initialize_front_end_suffix`].
    pub fn initialize_front_end_prefix(&mut self) {
        self.peripherals.phy_pbus.initialize_phy_front_end_pbus();
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        bb.initialize_phy_front_end_first();
        bb.initialize_phy_front_end_second();
        bb.clear_phy_front_end_first();
    }

    /// Apply the twelve front-end initialization edges after table-memory setup.
    ///
    /// Complete rev0 ROM `phy_fe_reg_init` performs every update below using
    /// a fresh read. Repeated sets are retained because intermediate device
    /// states are observable hardware behavior.
    pub fn initialize_front_end_suffix(&mut self) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        bb.enable_phy_front_end_init();
        bb.enable_rx_iq_correction_modes();
        bb.enable_tx_iq_correction_modes();
        bb.clear_phy_front_end_second();
        bb.enable_phy_front_end_adc_rate_high();
        bb.enable_phy_front_end_adc_rate_low();
        bb.configure_phy_front_end_low();
        bb.enable_phy_front_end_adc_rate_low();
        bb.enable_phy_front_end_adc_rate_high();
        bb.enable_phy_rx_iq_front_end_high();
        bb.enable_phy_tx_iq_front_end_high();
        bb.initialize_phy_front_end_low();
    }

    /// Apply complete pinned `libphy.a[phy_reg.o]::phy_fe_reg_update`.
    ///
    /// The `0x32`-byte body performs exactly three fresh-read RMW edges and
    /// has no ROM-only DAC-scale tail.
    pub fn update_front_end(&mut self) {
        let bb = &mut self.peripherals.phy_baseband_config_oracle;
        bb.initialize_phy_front_end_first();
        bb.initialize_phy_front_end_second();
        bb.enable_phy_front_end_adc_rates();
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
        self.peripherals
            .phy_baseband_config_oracle
            .configure_phy_rx_gain_dc_calibration(state);
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod txrx_reset_tests;
