//! PHY baseband register operations over the generated baseband
//! configuration, front-end, PBus and clock blocks.
//!
//! Each operation keeps the complete ROM/blob body's order of fresh-read
//! updates. A whole-word image that writes undeclared bits stays a
//! generated PAC word operation.

#![forbid(unsafe_code)]

use oer_esp32s31_pac::{
    phy_baseband_config_oracle::{self, PhyBasebandConfigOracle},
    phy_clock_oracle::PhyClockOracle,
    phy_fedata_recovered::PhyFedataRecovered,
    phy_pbus::PhyPbus,
};
use oer_mmio::{Bits, Placement};

use crate::phy::i2c::PhyAdcRate;

/// Opaque capture of the two fields owned by TX-DC PWDET calibration.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxDcPwdetFields {
    table_low: u8,
    calibration: u8,
}

/// Opaque capture of the TX-IQ tone-control field state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxIqToneControlFields {
    selector_high: u8,
    low_reserved_clear_opaque: Bits<2>,
    negated_step_or_attenuation: u8,
    tone_enable_or_arm: bool,
    txiq_mismatch_mode_opaque: Bits<3>,
    middle_reserved_clear_opaque: Bits<2>,
    txiq_polarity_image: Bits<4>,
    high_nibble_opaque: Bits<4>,
}

impl Default for TxIqToneControlFields {
    /// The capture of an all-zero word.
    fn default() -> Self {
        Self {
            selector_high: 0,
            low_reserved_clear_opaque: Bits::constant::<0>(),
            negated_step_or_attenuation: 0,
            tone_enable_or_arm: false,
            txiq_mismatch_mode_opaque: Bits::constant::<0>(),
            middle_reserved_clear_opaque: Bits::constant::<0>(),
            txiq_polarity_image: Bits::constant::<0>(),
            high_nibble_opaque: Bits::constant::<0>(),
        }
    }
}

impl TxIqToneControlFields {
    /// Every field of the first tone path's control, from one read.
    pub fn capture<P: Placement>(bb: &PhyBasebandConfigOracle<P>) -> Self {
        let control = bb.read_tone_path_0_control();
        Self {
            selector_high: control.selector_high(),
            low_reserved_clear_opaque: control.bits_8_9_opaque(),
            negated_step_or_attenuation: control.negated_step_or_attenuation(),
            tone_enable_or_arm: control.tone_enable_or_arm(),
            txiq_mismatch_mode_opaque: control.bits_19_21_opaque(),
            middle_reserved_clear_opaque: control.bits_22_23_opaque(),
            txiq_polarity_image: control.txiq_polarity_image(),
            high_nibble_opaque: control.bits_28_31_opaque(),
        }
    }

    /// Store every captured field of the first tone path's control, as
    /// `phy_txiq_get_mis_pwr` restores the word.
    ///
    /// The fields cover the whole word, so the stored word is the capture.
    /// The vendor stores it without reading; this update reads the word
    /// first, a side-effect-free read of a control register.
    pub fn restore<P: Placement>(self, bb: &mut PhyBasebandConfigOracle<P>) {
        bb.modify_tone_path_0_control(|w| {
            w.set_selector_high(self.selector_high)
                .set_bits_8_9_opaque(self.low_reserved_clear_opaque)
                .set_negated_step_or_attenuation(self.negated_step_or_attenuation)
                .set_tone_enable_or_arm(self.tone_enable_or_arm)
                .set_bits_19_21_opaque(self.txiq_mismatch_mode_opaque)
                .set_bits_22_23_opaque(self.middle_reserved_clear_opaque)
                .set_txiq_polarity_image(self.txiq_polarity_image)
                .set_bits_28_31_opaque(self.high_nibble_opaque);
        });
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

/// The low bits of one signed coefficient's two's-complement byte, as the
/// vendor's field store keeps them.
const fn coefficient_bits<const WIDTH: u32>(coefficient: i8) -> Bits<WIDTH> {
    Bits::truncate(coefficient as u8 as u32)
}

/// A ten-bit tone selector's two low bits.
const fn tone_selector_low(selector: u16) -> Bits<2> {
    Bits::truncate(selector as u32)
}

/// A ten-bit tone selector's upper eight bits.
const fn tone_selector_high(selector: u16) -> u8 {
    (selector >> 2) as u8
}

/// The first tone path's control fields of one vendor edge.
pub(crate) struct TonePath0 {
    pub(crate) selector_high: u8,
    pub(crate) negated_step: u8,
    pub(crate) enabled: bool,
    pub(crate) mismatch_mode: Bits<3>,
    pub(crate) polarity: Bits<4>,
}

/// One fresh-read RMW of the first tone path's control: the selector's
/// upper byte, the negated step, the enable, the mismatch mode and the
/// polarity, with both reserved lanes written zero; the high nibble keeps
/// the value read.
pub(crate) fn configure_tone_path_0<P: Placement>(
    bb: &mut PhyBasebandConfigOracle<P>,
    path: TonePath0,
) {
    bb.modify_tone_path_0_control(|w| {
        w.set_selector_high(path.selector_high)
            .set_bits_8_9_opaque(Bits::constant::<0>())
            .set_negated_step_or_attenuation(path.negated_step)
            .set_tone_enable_or_arm(path.enabled)
            .set_bits_19_21_opaque(path.mismatch_mode)
            .set_bits_22_23_opaque(Bits::constant::<0>())
            .set_txiq_polarity_image(path.polarity);
    });
}

/// Apply both fresh-read edges of complete ROM `phy_fe_txrx_reset`.
pub fn reset_frontend_txrx<P: Placement>(fedata: &mut PhyFedataRecovered<P>) {
    // SOURCE: ROM_REV0_PHY_BLOBRAY_GAPS.
    fedata.modify_word_0040_opaque(|w| {
        w.set_tx_rx_reset_bit_25(false)
            .set_tx_rx_reset_bit_26(false);
    });
    // SOURCE: ROM_REV0_PHY_BLOBRAY_GAPS.
    fedata.modify_word_0040_opaque(|w| {
        w.set_tx_rx_reset_bit_25(true).set_tx_rx_reset_bit_26(true);
    });
}

/// Enable both RX- and TX-IQ correction modes through two fresh RMWs.
///
/// Complete rev0 ROM `phy_iq_corr_enable` at `0x2f82_7d8c` sets both
/// recovered mode bits in each word while preserving all coefficients.
pub fn enable_iq_correction_modes<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>) {
    // SOURCE: ROM_REV0_PHY_FE_REG_INIT, ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_iq_correction_control(|w| {
        w.set_rx_iq_correction_mode_low(true)
            .set_rx_iq_correction_mode_high(true);
    });
    // SOURCE: ROM_REV0_PHY_FE_REG_INIT, ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_iq_correction_aux(|w| {
        w.set_tx_iq_correction_mode_low(true)
            .set_tx_iq_correction_mode_high(true);
    });
}

/// Apply the complete four-edge RXIQ correction prefix or suffix.
pub fn configure_rxiq_root_correction<P: Placement>(
    bb: &mut PhyBasebandConfigOracle<P>,
    pbus: &mut PhyPbus<P>,
    begin: bool,
) {
    if begin {
        // SOURCE: BLOB_LIBPHY_PHY_RXIQ_CAL_INIT, S31_ESP_PACS_SVD.
        bb.modify_iq_correction_control(|w| {
            w.set_rx_iq_correction_mode_low(true);
        });
        // SOURCE: BLOB_LIBPHY_PHY_RXIQ_CAL_INIT, S31_ESP_PACS_SVD.
        bb.modify_iq_correction_aux(|w| {
            w.set_tx_iq_correction_mode_low(true);
        });
        // SOURCE: BLOB_LIBPHY_PHY_RXIQ_CAL_INIT, S31_ESP_PACS_SVD.
        bb.modify_iq_correction_control(|w| {
            w.set_rx_iq_correction_mode_high(false);
        });
        // SOURCE: BLOB_LIBPHY_PHY_RXIQ_CAL_INIT, S31_ESP_PACS_SVD.
        bb.modify_iq_correction_aux(|w| {
            w.set_tx_iq_correction_mode_high(false);
        });
    } else {
        // SOURCE: BLOB_LIBPHY_PHY_RXIQ_CAL_INIT, S31_ESP_PACS_SVD.
        bb.modify_iq_correction_control(|w| {
            w.set_rx_iq_correction_mode_high(true);
        });
        // SOURCE: BLOB_LIBPHY_PHY_RXIQ_CAL_INIT, S31_ESP_PACS_SVD.
        bb.modify_iq_correction_aux(|w| {
            w.set_tx_iq_correction_mode_high(true);
        });
        // SOURCE: BLOB_LIBPHY_PHY_RXIQ_CAL_INIT, S31_ESP_PACS_SVD.
        bb.modify_iq_correction_control(|w| {
            w.set_rx_iq_correction_mode_low(false);
        });
        crate::phy::pbus::ops::clear_rxiq_root_status_second(pbus);
    }
}

/// Configure all fourteen ordered TX-power tracking RMW edges.
///
/// SOURCE: BLOB_LIBPHY_PHY_BASEBAND_CONFIG.
pub fn configure_tx_power_tracking<P: Placement>(
    bb: &mut PhyBasebandConfigOracle<P>,
    enabled: bool,
) {
    bb.modify_tx_power_track_control_0(|w| {
        w.set_track_enable(enabled);
    });
    // SOURCE: BLOB_LIBPHY_PHY_BASEBAND_CONFIG, S31_ESP_PACS_SVD.
    bb.modify_tx_power_track_control_0(|w| {
        w.set_bits_1_4_opaque(Bits::constant::<0x0>());
    });
    // SOURCE: BLOB_LIBPHY_PHY_BASEBAND_CONFIG, S31_ESP_PACS_SVD.
    bb.modify_tx_power_track_control_0(|w| {
        w.set_bits_5_9_opaque(Bits::constant::<0x1f>());
    });

    // The complete body clears the adjacent bits through separate reads.
    // SOURCE: BLOB_LIBPHY_PHY_BASEBAND_CONFIG, S31_ESP_PACS_SVD.
    bb.modify_tx_power_track_control_1(|w| {
        w.set_bit_0_opaque(false);
    });
    // SOURCE: BLOB_LIBPHY_PHY_BASEBAND_CONFIG, S31_ESP_PACS_SVD.
    bb.modify_tx_power_track_control_1(|w| {
        w.set_bit_1_opaque(false);
    });

    // SOURCE: BLOB_LIBPHY_PHY_BASEBAND_CONFIG, S31_ESP_PACS_SVD.
    bb.modify_tx_power_track_control_3(|w| {
        w.set_bits_8_15_opaque(0x79);
    });
    // SOURCE: BLOB_LIBPHY_PHY_BASEBAND_CONFIG, S31_ESP_PACS_SVD.
    bb.modify_tx_power_track_control_3(|w| {
        w.set_bits_0_7_opaque(0x83);
    });
    // SOURCE: BLOB_LIBPHY_PHY_BASEBAND_CONFIG, S31_ESP_PACS_SVD.
    bb.modify_tx_power_track_control_2(|w| {
        w.set_bits_24_31_opaque(0x8d);
    });
    // SOURCE: BLOB_LIBPHY_PHY_BASEBAND_CONFIG, S31_ESP_PACS_SVD.
    bb.modify_tx_power_track_control_2(|w| {
        w.set_bits_16_23_opaque(0x96);
    });
    // SOURCE: BLOB_LIBPHY_PHY_BASEBAND_CONFIG, S31_ESP_PACS_SVD.
    bb.modify_tx_power_track_control_2(|w| {
        w.set_bits_8_15_opaque(0xa0);
    });
    // SOURCE: BLOB_LIBPHY_PHY_BASEBAND_CONFIG, S31_ESP_PACS_SVD.
    bb.modify_tx_power_track_control_2(|w| {
        w.set_bits_0_7_opaque(0xb1);
    });
    // SOURCE: BLOB_LIBPHY_PHY_BASEBAND_CONFIG, S31_ESP_PACS_SVD.
    bb.modify_tx_power_track_control_1(|w| {
        w.set_bits_23_30_opaque(0xbe);
    });
    // SOURCE: BLOB_LIBPHY_PHY_BASEBAND_CONFIG, S31_ESP_PACS_SVD.
    bb.modify_tx_power_track_control_1(|w| {
        w.set_bits_15_22_opaque(0xd2);
    });
    // SOURCE: BLOB_LIBPHY_PHY_BASEBAND_CONFIG, S31_ESP_PACS_SVD.
    bb.modify_tx_power_track_control_1(|w| {
        w.set_bits_7_14_opaque(0xe6);
    });
}

/// Configure both I²C TX-rate fields and the four gain-compensation bytes.
pub fn configure_i2c_tx_rate<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>) {
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_i2c_tx_rate_control(|w| {
        w.set_bits_18_25_opaque(0x55);
    });
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_i2c_tx_rate_control(|w| {
        w.set_bits_0_1_opaque(Bits::constant::<0x2>());
    });
    restore_tx_gain_compensation(bb);
}

/// Configure the complete baseband watchdog leaf.
pub fn configure_watchdog<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>) {
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_baseband_watchdog_control(|w| {
        w.set_bits_0_15_opaque(0xaa).set_bit_30_opaque(true);
    });
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_baseband_watchdog_enable(|w| {
        w.set_watchdog_enable(true);
    });
}

/// Enable all four recovered automatic noise-floor controls.
pub fn configure_noise_floor_auto<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>) {
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_noise_floor_control(|w| {
        w.set_bit_23_opaque(true);
    });
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_noise_floor_control(|w| {
        w.set_bit_28_opaque(true);
    });
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_noise_floor_enable_0(|w| {
        w.set_bit_0_opaque(true);
    });
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_noise_floor_enable_1(|w| {
        w.set_bit_0_opaque(true);
    });
}

/// Apply all six ordered PA-on configuration operations.
pub fn configure_tx_pa_on<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>) {
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_baseband_tx_pa_control(|w| {
        w.set_bits_11_20_opaque(Bits::constant::<0x14>());
    });
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_tx_pa_control_0(|w| {
        w.set_bits_8_15_opaque(0x78);
    });
    bb.initialize_tx_pa_table();
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_baseband_tx_pa_timing(|w| {
        w.set_bits_0_9_opaque(Bits::constant::<0x1e>());
    });
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_tx_pa_control_1(|w| {
        w.set_bits_16_31_opaque(0xa0e);
    });
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_tx_pa_control_1(|w| {
        w.set_pa_on_bt_delay(0xc8);
    });
}

/// Apply the local prefix of complete rev0 ROM `phy_bb_reg_init`.
pub fn initialize_baseband_prefix<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>) {
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_word_7400_opaque(|w| {
        w.set_bits_13_14_opaque(Bits::constant::<0x3>());
    });
}

/// Apply the twelve local middle edges of complete `phy_bb_reg_init`.
pub fn initialize_baseband_middle<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>) {
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_word_7808_opaque(|w| {
        w.set_bits_7_13_opaque(Bits::constant::<0x60>());
    });
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_word_78dc_opaque(|w| {
        w.set_bits_7_13_opaque(Bits::constant::<0x2>());
    });
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_word_78e4_opaque(|w| {
        w.set_bit_22_opaque(false);
    });
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_baseband_tx_pa_timing(|w| {
        w.set_bits_12_19_opaque(0x0);
    });
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_word_790c_opaque(|w| {
        w.set_bit_11_opaque(false);
    });
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_word_7ca8_opaque(|w| {
        w.set_bit_20_opaque(true);
    });
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_word_7980_opaque(|w| {
        w.set_bit_25_opaque(false);
    });

    // Complete ROM updates the adjacent mode bits through separate reads.
    // SOURCE: ROM_REV0_PHY_CBW_FEATURE_WATCHDOG, ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_word_7890_opaque(|w| {
        w.set_he_ru26_good_response_disable(false);
    });
    // SOURCE: ROM_REV0_PHY_CBW_FEATURE_WATCHDOG, ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_word_7890_opaque(|w| {
        w.set_he_ru26_good_response_enable(true);
    });
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_word_7a28_opaque(|w| {
        w.set_bit_22_opaque(false);
    });
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_word_7cd0_opaque(|w| {
        w.set_bit_0_opaque(true)
            .set_bit_1_opaque(true)
            .set_bit_2_opaque(true)
            .set_bit_3_opaque(true)
            .set_bits_16_19_opaque(Bits::constant::<0xf>());
    });
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_baseband_tx_pa_control(|w| {
        w.set_bit_9_opaque(true);
    });
}

/// Apply the five local tail edges of complete `phy_bb_reg_init`.
pub fn initialize_baseband_tail<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>) {
    // Complete ROM clears bits 7:6 and bit 8 through separate reads.
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_word_743c_opaque(|w| {
        w.set_bits_6_7_opaque(Bits::constant::<0x0>());
    });
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_word_743c_opaque(|w| {
        w.set_bit_8_opaque(false);
    });
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_word_7428_opaque(|w| {
        w.set_bit_14_opaque(true);
    });
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_word_7428_opaque(|w| {
        w.set_bits_8_13_opaque(Bits::constant::<0x15>());
    });
    // SOURCE: ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_word_7cd0_opaque(|w| {
        w.set_bit_0_opaque(true)
            .set_bit_1_opaque(true)
            .set_bit_2_opaque(true)
            .set_bit_3_opaque(true)
            .set_bits_16_19_opaque(Bits::constant::<0xf>());
    });
}

/// Complete ROM `phy_pwdet_reg_init`'s power-detector reference.
///
/// SOURCE: ROM_REV0_PHY_POWER_DETECTOR.
const POWER_DETECTOR_INITIAL_REFERENCE: u16 = 0xaaaa;

/// Complete ROM `phy_en_pwdet`'s power-detector reference.
///
/// SOURCE: ROM_REV0_PHY_POWER_DETECTOR.
const POWER_DETECTOR_ENABLED_REFERENCE: u16 = 0x016a;

/// Apply the five internal-MMIO stores of complete ROM `phy_pwdet_reg_init`.
pub fn initialize_power_detector_registers<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>) {
    bb.initialize_power_detector_table_0();
    bb.initialize_power_detector_table_1();
    // SOURCE: ROM_REV0_PHY_POWER_DETECTOR, S31_ESP_PACS_SVD.
    bb.modify_power_detector_control(|w| {
        w.set_bits_4_11_opaque(0x50);
    });
    write_power_detector_reference(bb, POWER_DETECTOR_INITIAL_REFERENCE);
    // SOURCE: ROM_REV0_PHY_POWER_DETECTOR, S31_ESP_PACS_SVD.
    bb.modify_power_detector_control(|w| {
        w.set_bits_20_22_opaque(Bits::constant::<0x2>());
    });
}

/// Apply the internal-MMIO portion of complete ROM `phy_en_pwdet`.
pub fn configure_power_detector_enabled<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>) {
    // SOURCE: ROM_REV0_PHY_POWER_DETECTOR, S31_ESP_PACS_SVD.
    bb.modify_power_detector_control(|w| {
        w.set_bit_2_opaque(false);
    });
    // SOURCE: ROM_REV0_PHY_POWER_DETECTOR, S31_ESP_PACS_SVD.
    bb.modify_power_detector_control(|w| {
        w.set_bit_1_opaque(false);
    });
    // SOURCE: ROM_REV0_PHY_POWER_DETECTOR, S31_ESP_PACS_SVD.
    bb.modify_power_detector_control(|w| {
        w.set_bit_3_opaque(false);
    });
    // SOURCE: ROM_REV0_PHY_POWER_DETECTOR, S31_ESP_PACS_SVD.
    bb.modify_power_detector_sar_control_status(|w| {
        w.set_bits_12_13_opaque(Bits::constant::<0x3>());
    });
    // SOURCE: ROM_REV0_PHY_POWER_DETECTOR, S31_ESP_PACS_SVD.
    bb.modify_power_detector_sar_control_status(|w| {
        w.set_bit_9_opaque(false);
    });
    write_power_detector_reference(bb, POWER_DETECTOR_ENABLED_REFERENCE);
}

/// Set the final background-control bit after PWDET enable.
pub fn enable_power_detector_background_control<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>) {
    // SOURCE: ROM_REV0_PHY_POWER_DETECTOR, S31_ESP_PACS_SVD.
    bb.modify_power_detector_control(|w| {
        w.set_bit_16_opaque(true);
    });
}

/// Capture the two fields owned by TX-DC PWDET calibration.
pub fn capture_txdc_power_detector<P: Placement>(
    bb: &PhyBasebandConfigOracle<P>,
) -> TxDcPwdetFields {
    TxDcPwdetFields {
        table_low: bb.read_power_detector_table_1().bits_0_7_opaque(),
        calibration: bb.read_power_detector_control().bits_4_11_opaque(),
    }
}

/// Publish the two temporary TX-DC PWDET calibration fields.
pub fn apply_txdc_power_detector_calibration<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>) {
    // SOURCE: ROM_REV0_PHY_POWER_DETECTOR, S31_ESP_PACS_SVD.
    bb.modify_power_detector_table_1(|w| {
        w.set_bits_0_7_opaque(0xf0);
    });
    // SOURCE: ROM_REV0_PHY_POWER_DETECTOR, S31_ESP_PACS_SVD.
    bb.modify_power_detector_control(|w| {
        w.set_bits_4_11_opaque(0x78);
    });
}

/// Select TX-DC SAR mode one after the initial PBus setup.
pub fn configure_txdc_power_detector_sar<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>) {
    // SOURCE: ROM_REV0_PHY_POWER_DETECTOR, S31_ESP_PACS_SVD.
    bb.modify_power_detector_sar_control_status(|w| {
        w.set_bits_12_13_opaque(Bits::constant::<0x1>());
    });
}

/// Restore captured TX-DC fields and select final SAR mode.
pub fn restore_txdc_power_detector<P: Placement>(
    bb: &mut PhyBasebandConfigOracle<P>,
    fields: TxDcPwdetFields,
) {
    bb.modify_power_detector_table_1(|w| {
        w.set_bits_0_7_opaque(fields.table_low);
    });
    bb.modify_power_detector_control(|w| {
        w.set_bits_4_11_opaque(fields.calibration);
    });
    // SOURCE: ROM_REV0_PHY_POWER_DETECTOR, S31_ESP_PACS_SVD.
    bb.modify_power_detector_sar_control_status(|w| {
        w.set_bits_12_13_opaque(Bits::constant::<0x3>());
    });
}

/// Publish one zero-extended power-detector reference word.
pub fn write_power_detector_reference<P: Placement>(
    bb: &mut PhyBasebandConfigOracle<P>,
    value: u16,
) {
    bb.write_power_detector_reference(|w| {
        w.set_reference_code(value);
    });
}

/// Pulse the power-detector SAR trigger through two fresh RMW edges.
pub fn trigger_power_detector_sar<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>) {
    // SOURCE: ROM_REV0_PHY_POWER_DETECTOR, S31_ESP_PACS_SVD.
    bb.modify_power_detector_control(|w| {
        w.set_sar_trigger(false);
    });
    // SOURCE: ROM_REV0_PHY_POWER_DETECTOR, S31_ESP_PACS_SVD.
    bb.modify_power_detector_control(|w| {
        w.set_sar_trigger(true);
    });
}

/// Read the SVD-described power-detector readiness field.
pub fn power_detector_ready<P: Placement>(bb: &PhyBasebandConfigOracle<P>) -> bool {
    bb.read_power_detector_sar_control_status().sar_ready() == Bits::constant::<0b111>()
}

/// Read the SVD-described power-detector SAR sample field.
pub fn power_detector_sar_sample<P: Placement>(bb: &PhyBasebandConfigOracle<P>) -> u16 {
    let [low, high, _, _] = bb
        .read_power_detector_sar_result()
        .sar_sample()
        .get()
        .to_le_bytes();
    u16::from_le_bytes([low, high])
}

fn clear_tx_gain_compensation<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>) {
    // The vendor stores zero without reading; every bit of the word is a
    // field, so this update reads the word first, a side-effect-free read
    // of a control register, and stores the same zero.
    // SOURCE: BLOB_LIBPHY_PHY_BASEBAND_CONFIG.
    bb.modify_tx_gain_compensation(|w| {
        w.set_bits_0_7_opaque(0x0)
            .set_bits_8_15_opaque(0x0)
            .set_bits_16_23_opaque(0x0)
            .set_bits_24_31_opaque(0x0);
    });
    bb.clear_tx_gain_compensation_aux();
}

/// Apply complete pinned `phy_txgain_comp_pacfg_new(1)` as four ordered
/// fresh-read byte updates.
pub fn restore_tx_gain_compensation<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>) {
    // SOURCE: BLOB_LIBPHY_CALIBRATION_20260907, S31_ESP_PACS_SVD.
    bb.modify_tx_gain_compensation(|w| {
        w.set_bits_0_7_opaque(0x0);
    });
    // SOURCE: BLOB_LIBPHY_CALIBRATION_20260907, S31_ESP_PACS_SVD.
    bb.modify_tx_gain_compensation(|w| {
        w.set_bits_8_15_opaque(0xfb);
    });
    // SOURCE: BLOB_LIBPHY_CALIBRATION_20260907, S31_ESP_PACS_SVD.
    bb.modify_tx_gain_compensation(|w| {
        w.set_bits_16_23_opaque(0x3);
    });
    // SOURCE: BLOB_LIBPHY_CALIBRATION_20260907, S31_ESP_PACS_SVD.
    bb.modify_tx_gain_compensation(|w| {
        w.set_bits_24_31_opaque(0x0);
    });
}

fn configure_tone_selectors<P: Placement>(
    bb: &mut PhyBasebandConfigOracle<P>,
    path_0: u16,
    path_1: u16,
) {
    bb.modify_tone_selector_control(|w| {
        w.set_path_0_selector_low(tone_selector_low(path_0));
    });
    bb.modify_tone_selector_control(|w| {
        w.set_path_1_selector_low(tone_selector_low(path_1));
    });
}

fn configure_tone_paths<P: Placement>(
    bb: &mut PhyBasebandConfigOracle<P>,
    enabled: bool,
    path_0_selector: u16,
    path_0_step: u8,
) {
    configure_tone_path_0(
        bb,
        TonePath0 {
            selector_high: tone_selector_high(path_0_selector),
            negated_step: path_0_step.wrapping_neg(),
            enabled,
            mismatch_mode: Bits::constant::<0>(),
            polarity: Bits::constant::<0>(),
        },
    );
    // SOURCE: BLOB_LIBPHY_PHY_START_TX_TONE_STEP_NEW, ROM_REV0_PHY_START_STOP_TX_TONE, S31_ESP_PACS_SVD.
    bb.modify_tone_path_1_control(|w| {
        w.set_selector_high(phy_baseband_config_oracle::tone_path_1_control::SelectorHigh::Cleared)
            .set_bits_8_9_opaque(
                phy_baseband_config_oracle::tone_path_1_control::Bits8_9Opaque::Cleared,
            )
            .set_negated_step_or_attenuation(
                phy_baseband_config_oracle::tone_path_1_control::NegatedStepOrAttenuation::Cleared,
            )
            .set_tone_enable_or_arm(false)
            .set_bits_19_27_opaque(
                phy_baseband_config_oracle::tone_path_1_control::Bits19_27Opaque::Cleared,
            );
    });
}

/// Set digital-gain forcing before publishing both signed gain bytes.
/// Disabling retains the supplied gain values for the next calibration.
pub fn configure_forced_digital_gain<P: Placement>(
    clock: &mut PhyClockOracle<P>,
    enabled: bool,
    gain_0: i8,
    gain_1: i8,
) {
    clock.modify_table_memory_index_source(|w| {
        w.set_force_digital_gain_enable(enabled);
    });
    clock.modify_table_memory_index_source(|w| {
        w.set_forced_digital_gain_0(gain_0 as u8);
    });
    clock.modify_table_memory_index_source(|w| {
        w.set_forced_digital_gain_1(gain_1 as u8);
    });
}

/// Program the calibration tone, restoring TX gain only when stopping it.
///
/// This preserves every fresh-read/write edge in
/// `libphy.a[phy_reg.o]::phy_start_tx_tone_step_new` and its
/// `phy_txgain_comp_pacfg_new` child.
pub fn configure_calibration_tone<P: Placement>(
    bb: &mut PhyBasebandConfigOracle<P>,
    enabled: bool,
    selector: u16,
    step: u8,
) {
    // SOURCE: ROM_REV0_PHY_START_STOP_TX_TONE, S31_ESP_PACS_SVD.
    bb.modify_front_end_and_tone_stop_control(|w| {
        w.set_bits_0_1_opaque(Bits::constant::<0x0>());
    });
    clear_tx_gain_compensation(bb);
    configure_tone_selectors(bb, selector, 0);
    configure_tone_paths(bb, enabled, selector, step);
    if !enabled {
        // SOURCE: BLOB_LIBPHY_PHY_START_TX_TONE_STEP_NEW, ROM_REV0_PHY_START_STOP_TX_TONE, S31_ESP_PACS_SVD.
        bb.modify_front_end_and_tone_stop_control(|w| {
            w.set_bits_0_1_opaque(Bits::constant::<0x3>());
        });
        restore_tx_gain_compensation(bb);
    }
}

/// Program the ROM power-control tone with DAC scale and TX gain disabled.
pub fn configure_power_control_tone<P: Placement>(
    bb: &mut PhyBasebandConfigOracle<P>,
    selector: u16,
    step: u8,
) {
    // SOURCE: ROM_REV0_PHY_START_STOP_TX_TONE, S31_ESP_PACS_SVD.
    bb.modify_front_end_and_tone_stop_control(|w| {
        w.set_bits_0_1_opaque(Bits::constant::<0x0>());
    });
    // SOURCE: ROM_REV0_PHY_START_STOP_TX_TONE, S31_ESP_PACS_SVD.
    bb.modify_dac_scale_control(|w| {
        w.set_bits_16_23_opaque(0x0);
    });
    // SOURCE: ROM_REV0_PHY_START_STOP_TX_TONE, S31_ESP_PACS_SVD.
    bb.modify_dac_scale_control(|w| {
        w.set_bits_8_15_opaque(0x0);
    });
    clear_tx_gain_compensation(bb);
    configure_tone_selectors(bb, selector, 0);
    configure_tone_paths(bb, true, selector, step);
}

/// Configure one of the two complete TX-IQ mismatch-power polarity edges.
///
/// SOURCE: ROM_REV0_PHY_TXIQ_GET_MIS_PWR. The first edge selects mismatch
/// mode five and polarity four or zero; the second writes the second-path
/// polarity eight or one.
pub fn configure_txiq_mismatch_power<P: Placement>(
    bb: &mut PhyBasebandConfigOracle<P>,
    first: bool,
    polarity: bool,
    attenuation: u8,
    selector: u16,
) {
    if first {
        configure_tone_path_0(
            bb,
            TonePath0 {
                selector_high: tone_selector_high(selector),
                negated_step: attenuation.wrapping_neg(),
                enabled: true,
                mismatch_mode: Bits::constant::<5>(),
                polarity: if polarity {
                    Bits::constant::<4>()
                } else {
                    Bits::constant::<0>()
                },
            },
        );
        bb.modify_tone_selector_control(|w| {
            w.set_path_0_selector_low(tone_selector_low(selector));
        });
    } else {
        let second: Bits<4> = if polarity {
            Bits::constant::<8>()
        } else {
            Bits::constant::<1>()
        };
        bb.modify_tone_path_0_control(|w| {
            w.set_txiq_polarity_image(second);
        });
    }
}

/// Set or clear the shared first-path arm bit for one PWDET sample.
pub fn set_power_detector_tone_armed<P: Placement>(
    bb: &mut PhyBasebandConfigOracle<P>,
    armed: bool,
) {
    bb.modify_tone_path_0_control(|w| {
        w.set_tone_enable_or_arm(armed);
    });
}

/// Stop both tone paths and restore the two DAC-scale fields.
pub fn stop_power_detector_tone<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>) {
    stop_calibration_tone_paths(bb);
    // SOURCE: ROM_REV0_PHY_START_STOP_TX_TONE, S31_ESP_PACS_SVD.
    bb.modify_dac_scale_control(|w| {
        w.set_bits_16_23_opaque(0xff);
    });
    // SOURCE: ROM_REV0_PHY_START_STOP_TX_TONE, S31_ESP_PACS_SVD.
    bb.modify_dac_scale_control(|w| {
        w.set_bits_8_15_opaque(0xff);
    });
}

/// Stop both tone paths without changing their DAC-scale fields.
///
/// This is the complete pinned `libphy.a` `phy_stop_tx_tone_new` leaf.
/// The longer ROM `phy_stop_tx_tone(1)` composes this exact prefix with
/// two additional DAC-scale restores in [`stop_power_detector_tone`].
pub fn stop_calibration_tone_paths<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>) {
    // SOURCE: BLOB_LIBPHY_PHY_START_TX_TONE_STEP_NEW, ROM_REV0_PHY_START_STOP_TX_TONE, S31_ESP_PACS_SVD.
    bb.modify_tone_path_0_control(|w| {
        w.set_tone_enable_or_arm(false);
    });
    // SOURCE: BLOB_LIBPHY_PHY_START_TX_TONE_STEP_NEW, ROM_REV0_PHY_START_STOP_TX_TONE, S31_ESP_PACS_SVD.
    bb.modify_tone_path_1_control(|w| {
        w.set_tone_enable_or_arm(false);
    });
    // SOURCE: BLOB_LIBPHY_PHY_START_TX_TONE_STEP_NEW, ROM_REV0_PHY_START_STOP_TX_TONE, S31_ESP_PACS_SVD.
    bb.modify_front_end_and_tone_stop_control(|w| {
        w.set_bits_0_1_opaque(Bits::constant::<0x3>());
    });
}

/// Enter or complete the TX-IQ correction phase with one fresh RMW.
///
/// Complete ROM `phy_rfcal_txiq` clears the high mode bit while setting
/// the low bit on entry. Its completion edge sets only the high bit.
pub fn configure_tx_iq_correction<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>, begin: bool) {
    if begin {
        // SOURCE: ROM_REV0_PHY_RFCAL_TXIQ, S31_ESP_PACS_SVD.
        bb.modify_iq_correction_aux(|w| {
            w.set_tx_iq_correction_mode_low(true)
                .set_tx_iq_correction_mode_high(false);
        });
    } else {
        // SOURCE: ROM_REV0_PHY_RFCAL_TXIQ, S31_ESP_PACS_SVD.
        bb.modify_iq_correction_aux(|w| {
            w.set_tx_iq_correction_mode_high(true);
        });
    }
}

/// Select the RX-IQ calibration mode with one fresh RMW.
pub fn configure_rx_iq_calibration_mode<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>) {
    // SOURCE: ROM_REV0_PHY_IQ_ESTIMATOR, S31_ESP_PACS_SVD.
    bb.modify_iq_correction_control(|w| {
        w.set_rx_iq_correction_mode_low(true)
            .set_rx_iq_correction_mode_high(false);
    });
}

/// Publish one signed TX-IQ gain coefficient using the ROM saturation.
pub fn set_tx_iq_gain_coefficient<P: Placement>(
    bb: &mut PhyBasebandConfigOracle<P>,
    coefficient: i8,
) {
    let value = coefficient_bits(saturate_tx_iq_gain(coefficient));
    bb.modify_iq_correction_aux(|w| {
        w.set_tx_iq_gain_coefficient(value);
    });
}

/// Publish one signed TX-IQ phase coefficient using the ROM saturation.
pub fn set_tx_iq_phase_coefficient<P: Placement>(
    bb: &mut PhyBasebandConfigOracle<P>,
    coefficient: i8,
) {
    let value = coefficient_bits(saturate_tx_iq_phase(coefficient));
    bb.modify_iq_correction_aux(|w| {
        w.set_tx_iq_phase_coefficient(value);
    });
}

/// Publish one signed RX-IQ gain coefficient using the ROM truncation.
pub fn set_rx_iq_gain_coefficient<P: Placement>(
    bb: &mut PhyBasebandConfigOracle<P>,
    coefficient: i8,
) {
    bb.modify_iq_correction_control(|w| {
        w.set_rx_iq_gain_coefficient(coefficient_bits(coefficient));
    });
}

/// Publish one signed RX-IQ phase coefficient using the ROM truncation.
pub fn set_rx_iq_phase_coefficient<P: Placement>(
    bb: &mut PhyBasebandConfigOracle<P>,
    coefficient: i8,
) {
    bb.modify_iq_correction_control(|w| {
        w.set_rx_iq_phase_coefficient(coefficient_bits(coefficient));
    });
}

/// Trigger one TX-DC comparator measurement using three fresh RMW edges.
pub fn trigger_tx_dc_measurement<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>) {
    // SOURCE: ROM_REV0_PHY_TXDC_CAL, S31_ESP_PACS_SVD.
    bb.modify_tx_dc_measurement_control_status(|w| {
        w.set_measurement_enable(true);
    });
    // SOURCE: ROM_REV0_PHY_TXDC_CAL, S31_ESP_PACS_SVD.
    bb.modify_tx_dc_measurement_control_status(|w| {
        w.set_measurement_start(false);
    });
    // SOURCE: ROM_REV0_PHY_TXDC_CAL, S31_ESP_PACS_SVD.
    bb.modify_tx_dc_measurement_control_status(|w| {
        w.set_measurement_start(true);
    });
}

/// Sample the TX-DC ready bit exactly once.
pub fn tx_dc_measurement_is_ready<P: Placement>(bb: &PhyBasebandConfigOracle<P>) -> bool {
    bb.read_tx_dc_measurement_control_status()
        .measurement_ready()
}

/// Preserve the complete ROM's independent I and Q comparator reads.
pub fn sample_tx_dc_comparators<P: Placement>(bb: &PhyBasebandConfigOracle<P>) -> [bool; 2] {
    [
        bb.read_tx_dc_measurement_control_status()
            .i_comparator_high(),
        bb.read_tx_dc_measurement_control_status()
            .q_comparator_high(),
    ]
}

/// Clear TX-DC enable and start through two fresh RMW edges.
pub fn clear_tx_dc_measurement<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>) {
    // SOURCE: ROM_REV0_PHY_TXDC_CAL, S31_ESP_PACS_SVD.
    bb.modify_tx_dc_measurement_control_status(|w| {
        w.set_measurement_enable(false);
    });
    // SOURCE: ROM_REV0_PHY_TXDC_CAL, S31_ESP_PACS_SVD.
    bb.modify_tx_dc_measurement_control_status(|w| {
        w.set_measurement_start(false);
    });
}

/// Publish the two-register suffix of complete ROM `phy_adc_rate_set`.
///
/// The ROM body at `0x2f82_a6d2`, size `0x4a`, uses two fresh reads to
/// publish the selected rate into the two recovered bits: one for the high
/// rate, zero for the low.
pub fn configure_adc_rate<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>, rate: PhyAdcRate) {
    let high = matches!(rate, PhyAdcRate::High);
    bb.modify_adc_rate_and_front_end_control(|w| {
        w.set_bit_1_opaque(high);
    });
    bb.modify_adc_rate_and_front_end_control(|w| {
        w.set_bit_0_opaque(high);
    });
}

/// Apply the four front-end initialization edges before table-memory setup.
///
/// This is the exact prefix of complete rev0 ROM `phy_fe_reg_init` at
/// `0x2f82_7740`, size `0xf6`. The table-memory edge remains between this
/// operation and [`initialize_front_end_suffix`].
pub fn initialize_front_end_prefix<P: Placement>(
    bb: &mut PhyBasebandConfigOracle<P>,
    pbus: &mut PhyPbus<P>,
) {
    crate::phy::pbus::ops::initialize_front_end(pbus);
    // SOURCE: BLOB_LIBPHY_PHY_FE_REG_UPDATE, ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
    bb.modify_word_0c08_opaque(|w| {
        w.set_bit_25_opaque(true);
    });
    // SOURCE: BLOB_LIBPHY_PHY_FE_REG_UPDATE, ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
    bb.modify_word_0c08_opaque(|w| {
        w.set_bit_26_opaque(true);
    });
    // SOURCE: ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
    bb.modify_front_end_clear_control(|w| {
        w.set_bit_8_opaque(false);
    });
}

/// Apply the twelve front-end initialization edges after table-memory setup.
///
/// Complete rev0 ROM `phy_fe_reg_init` performs every update below using
/// a fresh read. Repeated sets are retained because intermediate device
/// states are observable hardware behavior.
pub fn initialize_front_end_suffix<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>) {
    // SOURCE: ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
    bb.modify_front_end_and_tone_stop_control(|w| {
        w.set_bit_2_opaque(true);
    });
    // SOURCE: ROM_REV0_PHY_FE_REG_INIT, ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_iq_correction_control(|w| {
        w.set_rx_iq_correction_mode_low(true)
            .set_rx_iq_correction_mode_high(true);
    });
    // SOURCE: ROM_REV0_PHY_FE_REG_INIT, ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
    bb.modify_iq_correction_aux(|w| {
        w.set_tx_iq_correction_mode_low(true)
            .set_tx_iq_correction_mode_high(true);
    });
    // SOURCE: ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
    bb.modify_front_end_clear_control(|w| {
        w.set_bit_11_opaque(false);
    });
    // SOURCE: ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
    bb.modify_adc_rate_and_front_end_control(|w| {
        w.set_bit_1_opaque(true);
    });
    // SOURCE: ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
    bb.modify_adc_rate_and_front_end_control(|w| {
        w.set_bit_0_opaque(true);
    });
    // SOURCE: ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
    bb.modify_tx_pa_control_0(|w| {
        w.set_bits_0_7_opaque(0x4);
    });
    // SOURCE: ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
    bb.modify_adc_rate_and_front_end_control(|w| {
        w.set_bit_0_opaque(true);
    });
    // SOURCE: ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
    bb.modify_adc_rate_and_front_end_control(|w| {
        w.set_bit_1_opaque(true);
    });
    // SOURCE: ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
    bb.modify_iq_correction_control(|w| {
        w.set_bit_31_opaque(true);
    });
    // SOURCE: ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
    bb.modify_iq_correction_aux(|w| {
        w.set_bit_15_opaque(true);
    });
    // SOURCE: ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
    bb.modify_word_0c20_opaque(|w| {
        w.set_bits_0_7_opaque(0x57);
    });
}

/// Apply complete pinned `libphy.a[phy_reg.o]::phy_fe_reg_update`.
///
/// The `0x32`-byte body performs exactly three fresh-read RMW edges and
/// has no ROM-only DAC-scale tail.
pub fn update_front_end<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>) {
    // SOURCE: BLOB_LIBPHY_PHY_FE_REG_UPDATE, ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
    bb.modify_word_0c08_opaque(|w| {
        w.set_bit_25_opaque(true);
    });
    // SOURCE: BLOB_LIBPHY_PHY_FE_REG_UPDATE, ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
    bb.modify_word_0c08_opaque(|w| {
        w.set_bit_26_opaque(true);
    });
    // SOURCE: BLOB_LIBPHY_PHY_FE_REG_UPDATE, S31_ESP_PACS_SVD.
    bb.modify_adc_rate_and_front_end_control(|w| {
        w.set_bit_0_opaque(true).set_bit_1_opaque(true);
    });
}

/// Select the direct-register prefix or cleanup state of RX-gain DC
/// calibration.
///
/// Complete rev0 ROM `phy_set_rx_gain_cal_dc` at `0x2f82_9858`, size
/// `0x206`, sets bits 6:5 to `0b11` before entering the bounded
/// calibration graph and clears them to `0b00` in its common cleanup.
/// The field's narrower electrical meaning is not independently proved.
pub fn set_rx_gain_dc_calibration<P: Placement>(
    bb: &mut PhyBasebandConfigOracle<P>,
    enabled: bool,
) {
    use phy_baseband_config_oracle::rx_gain_dc_control::Bits5_6Opaque as State;
    bb.modify_rx_gain_dc_control(|w| {
        w.set_bits_5_6_opaque(if enabled {
            State::Enabled
        } else {
            State::Disabled
        });
    });
}

/// Publish both fields of complete pinned `phy_config_hccfr` in order.
pub fn configure_hccfr<P: Placement>(
    bb: &mut PhyBasebandConfigOracle<P>,
    enabled: bool,
    value: crate::types::CfrValue,
) {
    use phy_baseband_config_oracle::hccfr_control::Enable;
    bb.modify_hccfr_control(|w| {
        w.set_enable(if enabled {
            Enable::Enabled
        } else {
            Enable::Disabled
        });
    });
    bb.modify_hccfr_value(|w| {
        w.set_value(cfr_bits(value));
    });
}

/// Apply either complete branch of pinned `phy_iccfr_en`.
pub fn configure_iccfr_gate<P: Placement>(bb: &mut PhyBasebandConfigOracle<P>, enabled: bool) {
    use phy_baseband_config_oracle::iccfr_enable_control::Gate;
    bb.modify_iccfr_enable_control(|w| {
        w.set_gate(if enabled {
            Gate::Enabled
        } else {
            Gate::Disabled
        });
    });
}

/// Publish all five fields and the tail gate of pinned `phy_force_iccfr`.
pub fn configure_forced_iccfr<P: Placement>(
    bb: &mut PhyBasebandConfigOracle<P>,
    mode: bool,
    enabled: bool,
    value: crate::types::CfrValue,
) {
    use phy_baseband_config_oracle::iccfr_force_control::ForceEnable;
    bb.modify_iccfr_force_control(|w| {
        w.set_force_mode_high(mode);
    });
    bb.modify_iccfr_force_control(|w| {
        w.set_force_enable(if enabled {
            ForceEnable::Enabled
        } else {
            ForceEnable::Disabled
        });
    });
    // SOURCE: BLOB_LIBPHY_PHY_CFR_CONTROL, S31_ESP_PACS_SVD.
    bb.modify_iccfr_force_control(|w| {
        w.set_force_trigger(true);
    });
    bb.modify_iccfr_force_control(|w| {
        w.set_force_mode_low(mode);
    });
    bb.modify_iccfr_force_control(|w| {
        w.set_force_value(cfr_bits(value));
    });
    configure_iccfr_gate(bb, enabled);
}

/// A CFR value's twelve bits.
const fn cfr_bits(value: crate::types::CfrValue) -> Bits<12> {
    Bits::truncate(value.get())
}

#[cfg(test)]
mod tests;
