//! AGC register operations over the generated PHY AGC block.
//!
//! Each operation keeps the vendor body's order of fresh-read updates. A
//! whole-word image that writes undeclared bits stays a generated PAC word
//! operation (`initialize_agc_update_*`).

#![forbid(unsafe_code)]

use oer_esp32s31_pac::phy_agc_oracle::PhyAgcOracle;
use oer_mmio::{Bits, Placement};

/// Complete rev0 ROM `phy_reg_update_new` saturation-gain image.
///
/// SOURCE: BLOB_LIBPHY_PHY_REG_UPDATE_NEW.
const POST_INITIALIZATION_SATURATION_GAIN: u32 = 0x0818_212d;

/// Complete `phy_set_rx_gain_table` clamps the final limit to this index.
///
/// SOURCE: BLOB_LIBPHY_PHY_SET_RX_GAIN_TABLE.
const RX_GAIN_TABLE_FINAL_LIMIT_MAXIMUM: u8 = 0x4c;

/// Complete `phy_agc_reg_init` adds this offset to parameter 120.
///
/// SOURCE: ROM_REV0_PHY_AGC.
const PARAMETER_120_OFFSET: u8 = 0x50;

/// A seven-bit gain index from a byte, as the vendor's `& 0x7f` keeps it.
const fn gain_index(value: u8) -> Bits<7> {
    Bits::truncate(value as u32)
}

/// Apply all fourteen internal MMIO edges of `phy_bb_agc_reg_update`.
pub fn update_baseband_registers<P: Placement>(agc: &mut PhyAgcOracle<P>) {
    agc.initialize_agc_update_8070();
    agc.initialize_agc_update_78a4();
    // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
    agc.modify_rx_11b_mode_control(|w| {
        w.set_bit_26_opaque(false);
    });
    agc.initialize_agc_update_8010();
    agc.initialize_agc_update_8018();
    agc.initialize_agc_update_801c();
    agc.initialize_agc_update_8020();
    agc.initialize_agc_update_8028();
    agc.initialize_agc_update_802c();
    // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
    agc.modify_agc_update_8078_control(|w| {
        w.set_bits_20_22_opaque(Bits::constant::<0x7>());
    });
    // SOURCE: ROM_REV0_PHY_AGC.
    agc.modify_rx_11b_path_control_0(|w| {
        w.set_rx_sense(0xfe)
            .set_bits_8_13_opaque(Bits::constant::<0x21>())
            .set_bits_14_15_opaque(Bits::constant::<0x3>())
            .set_bits_16_21_opaque(Bits::constant::<0x3f>())
            .set_bits_22_31_opaque(Bits::constant::<0x3f8>());
    });
    agc.initialize_agc_update_7048();
    agc.initialize_rx_11b_window_control();
    // SOURCE: ROM_REV0_PHY_AGC.
    agc.modify_rx_11b_path_control_1(|w| {
        w.set_bits_0_3_opaque(Bits::constant::<0x3>())
            .set_bits_4_9_opaque(Bits::constant::<0x15>())
            .set_bits_10_15_opaque(Bits::constant::<0x21>())
            .set_bits_16_31_opaque(0xb220);
    });
}

/// Select the complete rev0 ROM AGC enable or disable sequence.
pub fn set_enabled<P: Placement>(agc: &mut PhyAgcOracle<P>, enabled: bool) {
    if !enabled {
        // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
        agc.modify_agc_antenna_control(|w| {
            w.set_bit_29_opaque(true);
        });
        return;
    }
    // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
    agc.modify_agc_antenna_control(|w| {
        w.set_bit_29_opaque(false);
    });
    // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
    agc.modify_agc_shared_control(|w| {
        w.set_forced_gain_enable(true);
    });
    // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
    agc.modify_agc_shared_control(|w| {
        w.set_forced_gain_enable(false);
    });
}

/// Publish both complete pinned RX-compensation fields in order.
pub fn configure_rx_compensation<P: Placement>(agc: &mut PhyAgcOracle<P>) {
    // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
    agc.modify_agc_shared_control(|w| {
        w.set_bits_0_7_opaque(0xed);
    });
    // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
    agc.modify_rx_compensation_high_control(|w| {
        w.set_bits_24_31_opaque(0xed);
    });
}

/// Pulse the DC-memory clear field through two fresh RMWs.
pub fn clear_dc_memory<P: Placement>(agc: &mut PhyAgcOracle<P>) {
    // SOURCE: BLOB_LIBPHY_PHY_DC_MEM_CLR, S31_ESP_PACS_SVD.
    agc.modify_dc_memory_control(|w| {
        w.set_bit_20_opaque(true);
    });
    // SOURCE: BLOB_LIBPHY_PHY_DC_MEM_CLR, S31_ESP_PACS_SVD.
    agc.modify_dc_memory_control(|w| {
        w.set_bit_20_opaque(false);
    });
}

/// Apply the two MMIO edges after the PBus work-mode 1 µs delay.
pub fn configure_pbus_work_mode_pulse<P: Placement>(agc: &mut PhyAgcOracle<P>) {
    // SOURCE: ROM_REV0_PHY_PBUS, S31_ESP_PACS_SVD.
    agc.modify_agc_shared_control(|w| {
        w.set_forced_gain_index(0x32);
    });
    // SOURCE: ROM_REV0_PHY_PBUS, S31_ESP_PACS_SVD.
    agc.modify_agc_shared_control(|w| {
        w.set_forced_gain_enable(true);
    });
}

/// Clear the PBus work-mode pulse after the caller-owned 2 µs delay.
pub fn clear_pbus_work_mode_pulse<P: Placement>(agc: &mut PhyAgcOracle<P>) {
    // SOURCE: ROM_REV0_PHY_PBUS, S31_ESP_PACS_SVD.
    agc.modify_agc_shared_control(|w| {
        w.set_forced_gain_enable(false);
    });
}

/// Apply all ten fresh-read updates of complete `phy_agc_reg_init`.
///
/// SOURCE: ROM_REV0_PHY_AGC. The first two limits take parameter 121 less
/// one, the gain index parameter 121 itself, each kept to seven bits.
pub fn initialize_registers<P: Placement>(
    agc: &mut PhyAgcOracle<P>,
    parameter_121: u8,
    parameter_120: u8,
) {
    let limit = gain_index(parameter_121.wrapping_sub(1));
    agc.modify_rx_gain_limit_control(|w| {
        w.set_initial_gain_index(limit);
    });
    agc.modify_agc_gain_limit_low(|w| {
        w.set_initial_gain_index(limit);
    });
    agc.modify_agc_shared_control(|w| {
        w.set_wifi_max_gain_index(gain_index(parameter_121));
    });
    // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
    agc.modify_agc_saturation_control(|w| {
        w.set_bits_0_18_opaque(Bits::constant::<0xbb8>());
    });
    agc.modify_agc_parameter_control(|w| {
        w.set_bits_4_11_opaque(parameter_121);
    });
    agc.modify_agc_parameter_control(|w| {
        w.set_bits_12_19_opaque(parameter_120.wrapping_add(PARAMETER_120_OFFSET));
    });
    // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
    agc.modify_agc_shared_control(|w| {
        w.set_forced_gain_index(0x32);
    });
    // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
    agc.modify_agc_shared_control(|w| {
        w.set_forced_gain_enable(true);
    });
    // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
    agc.modify_agc_shared_control(|w| {
        w.set_forced_gain_enable(false);
    });
    // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
    agc.modify_agc_init_high_control(|w| {
        w.set_bits_24_31_opaque(0xd2);
    });
}

/// Apply all three fresh-read antenna initialization updates.
pub fn configure_antenna<P: Placement>(agc: &mut PhyAgcOracle<P>) {
    // SOURCE: ROM_REV0_PHY_ANT_INIT, S31_ESP_PACS_SVD.
    agc.modify_antenna_control_0(|w| {
        w.set_bits_0_10_opaque(Bits::constant::<0x0>())
            .set_bit_12_opaque(false);
    });
    // SOURCE: ROM_REV0_PHY_ANT_INIT, S31_ESP_PACS_SVD.
    agc.modify_agc_antenna_control(|w| {
        w.set_bits_11_17_opaque(Bits::constant::<0x34>());
    });
    // SOURCE: ROM_REV0_PHY_ANT_INIT, S31_ESP_PACS_SVD.
    agc.modify_antenna_control_2(|w| {
        w.set_bits_8_15_opaque(0x1e).set_bits_24_31_opaque(0x1e);
    });
}

/// Apply either complete branch of rev0 ROM `phy_rfrx_sat_rst`.
pub fn configure_rf_rx_saturation<P: Placement>(agc: &mut PhyAgcOracle<P>, enabled: bool) {
    agc.initialize_rf_rx_saturation_config();
    if enabled {
        // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
        agc.modify_agc_saturation_control(|w| {
            w.set_bit_19_opaque(true)
                .set_bit_24_opaque(true)
                .set_bit_28_opaque(true)
                .set_bits_30_31_opaque(Bits::constant::<0x3>());
        });
        // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
        agc.modify_agc_saturation_control(|w| {
            w.set_bits_0_18_opaque(Bits::constant::<0x800>());
        });
    } else {
        // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
        agc.modify_agc_saturation_control(|w| {
            w.set_bit_19_opaque(false)
                .set_bit_24_opaque(false)
                .set_bit_28_opaque(false)
                .set_bits_30_31_opaque(Bits::constant::<0x0>());
        });
        // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
        agc.modify_agc_saturation_control(|w| {
            w.set_bits_0_18_opaque(Bits::constant::<0x400>());
        });
    }
}

/// Publish both final RX-gain limits through separate fresh RMWs.
///
/// SOURCE: BLOB_LIBPHY_PHY_SET_RX_GAIN_TABLE. The index takes the last
/// index, the limit the last index clamped to
/// [`RX_GAIN_TABLE_FINAL_LIMIT_MAXIMUM`], each kept to seven bits.
pub fn configure_rx_gain_limits<P: Placement>(agc: &mut PhyAgcOracle<P>, wifi_last_index: u8) {
    agc.modify_agc_shared_control(|w| {
        w.set_wifi_max_gain_index(gain_index(wifi_last_index));
    });
    let limit = gain_index(if wifi_last_index < RX_GAIN_TABLE_FINAL_LIMIT_MAXIMUM {
        wifi_last_index
    } else {
        RX_GAIN_TABLE_FINAL_LIMIT_MAXIMUM
    });
    agc.modify_rx_gain_limit_control(|w| {
        w.set_initial_gain_index(limit);
    });
}

/// Publish one saturation-gain word to both recovered destinations.
pub fn set_saturation_gain<P: Placement>(agc: &mut PhyAgcOracle<P>, value: u32) {
    agc.write_saturation_gain_low(|w| {
        w.set_value(value);
    });
    agc.write_saturation_gain_high(|w| {
        w.set_value(value);
    });
}

/// Apply the complete pinned `phy_set_ftm_en` one-bit image.
pub fn set_ftm_enabled<P: Placement>(agc: &mut PhyAgcOracle<P>, enabled: bool) {
    agc.modify_ftm_control(|w| {
        w.set_enable(enabled);
    });
}

/// Read back the single field owned by `phy_set_ftm_en`.
pub fn ftm_enabled<P: Placement>(agc: &PhyAgcOracle<P>) -> bool {
    agc.read_ftm_control().enable()
}

/// Apply `phy_reg_update_new` while preserving the independently owned FTM
/// mode.
pub fn update_post_initialization<P: Placement>(agc: &mut PhyAgcOracle<P>) {
    // SOURCE: BLOB_LIBPHY_PHY_REG_UPDATE_NEW, S31_ESP_PACS_SVD.
    agc.modify_agc_saturation_control(|w| {
        w.set_bit_26_opaque(true);
    });
    set_saturation_gain(agc, POST_INITIALIZATION_SATURATION_GAIN);
    // SOURCE: BLOB_LIBPHY_PHY_REG_UPDATE_NEW, S31_ESP_PACS_SVD.
    agc.modify_rx_11b_window_control(|w| {
        w.set_bits_0_8_opaque(Bits::constant::<0x1c0>());
    });
    // SOURCE: BLOB_LIBPHY_PHY_REG_UPDATE_NEW, S31_ESP_PACS_SVD.
    agc.modify_post_init_rx_control(|w| {
        w.set_bits_0_6_opaque(Bits::constant::<0x17>());
    });
    // SOURCE: BLOB_LIBPHY_PHY_REG_UPDATE_NEW, S31_ESP_PACS_SVD.
    agc.modify_post_init_rx_control(|w| {
        w.set_bits_7_13_opaque(Bits::constant::<0x17>());
    });
}

/// Apply either complete branch of rev0 ROM `phy_rx_11b_opt`.
pub fn configure_rx_11b_optimization<P: Placement>(agc: &mut PhyAgcOracle<P>, enabled: bool) {
    if enabled {
        // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
        agc.modify_rx_11b_path_control_0(|w| {
            w.set_bits_16_21_opaque(Bits::constant::<0x3f>());
        });
        // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
        agc.modify_rx_11b_path_control_0(|w| {
            w.set_bits_8_13_opaque(Bits::constant::<0x21>());
        });
        // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
        agc.modify_rx_11b_path_control_1(|w| {
            w.set_bits_10_15_opaque(Bits::constant::<0x21>());
        });
        // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
        agc.modify_rx_11b_path_control_1(|w| {
            w.set_bits_0_3_opaque(Bits::constant::<0x3>());
        });
        // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
        agc.modify_rx_11b_mode_control(|w| {
            w.set_bits_12_15_opaque(Bits::constant::<0x9>());
        });
    } else {
        // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
        agc.modify_rx_11b_path_control_0(|w| {
            w.set_bits_16_21_opaque(Bits::constant::<0x3e>());
        });
        // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
        agc.modify_rx_11b_path_control_0(|w| {
            w.set_bits_8_13_opaque(Bits::constant::<0x18>());
        });
        // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
        agc.modify_rx_11b_path_control_1(|w| {
            w.set_bits_10_15_opaque(Bits::constant::<0x18>());
        });
        // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
        agc.modify_rx_11b_path_control_1(|w| {
            w.set_bits_0_3_opaque(Bits::constant::<0x4>());
        });
        // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
        agc.modify_rx_11b_mode_control(|w| {
            w.set_bits_12_15_opaque(Bits::constant::<0x6>());
        });
    }
    // SOURCE: ROM_REV0_PHY_AGC, S31_ESP_PACS_SVD.
    agc.modify_rx_11b_window_control(|w| {
        w.set_bits_0_8_opaque(Bits::constant::<0x1c8>());
    });
}

/// Apply complete rev0 ROM `phy_force_rx_gain`.
pub fn configure_forced_rx_gain<P: Placement>(
    agc: &mut PhyAgcOracle<P>,
    enabled: bool,
    gain: crate::types::ForcedRxGain,
) {
    agc.modify_agc_shared_control(|w| {
        w.set_forced_gain_index(gain_byte(gain));
    });
    agc.modify_agc_shared_control(|w| {
        w.set_forced_gain_enable(enabled);
    });
}

/// The forced gain's byte.
const fn gain_byte(gain: crate::types::ForcedRxGain) -> u8 {
    gain.get().to_le_bytes()[0]
}
