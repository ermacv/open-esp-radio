//! Exact bounded Bluetooth baseband initialization transaction.
//!
//! This module is deliberately crate-private. The transaction is the observed
//! MMIO portion of `bt_bb_v2_init_cmplx(1)`, but it is not a controller-enable
//! API: the vendor lifecycle first completes common PHY initialization. The
//! transaction belongs to the shared radio owner, which Bluetooth and IEEE
//! 802.15.4 reach through the HAL's reference-counted BTBB acquisition; it
//! closes the body with a device fence.

#![forbid(unsafe_code)]

use oer_esp32s31_pac::{
    SharedRadioRegisters, bt_v3_2_baseband::BtV3_2BasebandRest,
    phy_baseband_config_oracle::PhyBasebandConfigOracle, phy_btagc_recovered::PhyBtagcRecovered,
    shared_baseband_tx_timing::SharedBasebandTxTiming,
    shared_radio_init_control::SharedRadioInitControl, zbbb_radio_control::ZbbbRadioControl,
};
use oer_mmio::{Bits, Placement};

/// Execute only the exact MMIO path of vendor `bt_bb_v2_init_cmplx(1)`.
///
/// The BTBB baseband is shared by Bluetooth and IEEE 802.15.4, so this
/// transaction takes the shared radio owner. The caller-provided byte is the
/// positional value read by the vendor body from the real linked `phy_param`
/// object at offset `0x120`. Its higher-level meaning remains unassigned. The
/// vendor diagnostic print selected by argument one is intentionally outside
/// this hardware transaction and is omitted from the comparison effect
/// contract. After the body, this edge adds exactly one device fence before
/// returning to its lifecycle owner.
///
/// SOURCE: BLOB_LIBBTBB_BT_BB_V2_INIT.
pub(super) fn initialize_btbb_v2_arg_one(shared: &mut SharedRadioRegisters, gain_parameter: u8) {
    let radio_phy = &mut shared.radio_phy.peripherals;
    let (btagc, oracle) = (
        &mut radio_phy.phy_btagc_recovered,
        &mut radio_phy.phy_baseband_config_oracle,
    );
    let blocks = &mut shared.shared_radio;
    let mut port = BluetoothBasebandV2Transaction {
        btagc,
        oracle,
        baseband: &mut blocks.bt_v3_2_baseband,
        init_control: &mut blocks.shared_radio_init_control,
        tx_timing: &mut blocks.shared_baseband_tx_timing,
        zbbb: &mut blocks.zbbb_radio_control,
    };
    execute_standalone_bluetooth_transition(&mut port, gain_parameter);
}

trait BluetoothBasebandV2TransitionPort {
    fn execute_body(&mut self, gain_parameter: u8);
    fn order_device_accesses(&mut self);
}

fn execute_standalone_bluetooth_transition<Port>(port: &mut Port, gain_parameter: u8)
where
    Port: BluetoothBasebandV2TransitionPort,
{
    port.execute_body(gain_parameter);
    port.order_device_accesses();
}

/// One borrow-scoped view of the exact generated blocks BTBB v2 touches,
/// wherever they lie.
struct BluetoothBasebandV2Transaction<'a, A, O, B, I, T, Z>
where
    A: Placement,
    O: Placement,
    B: Placement,
    I: Placement,
    T: Placement,
    Z: Placement,
{
    btagc: &'a mut PhyBtagcRecovered<A>,
    oracle: &'a mut PhyBasebandConfigOracle<O>,
    baseband: &'a mut BtV3_2BasebandRest<B>,
    init_control: &'a mut SharedRadioInitControl<I>,
    tx_timing: &'a mut SharedBasebandTxTiming<T>,
    zbbb: &'a mut ZbbbRadioControl<Z>,
}

impl<A, O, B, I, T, Z> BluetoothBasebandV2TransitionPort
    for BluetoothBasebandV2Transaction<'_, A, O, B, I, T, Z>
where
    A: Placement,
    O: Placement,
    B: Placement,
    I: Placement,
    T: Placement,
    Z: Placement,
{
    fn execute_body(&mut self, gain_parameter: u8) {
        self.initialize_baseband_v2_tx();
        self.initialize_baseband_v2_rx(gain_parameter);
        self.initialize_gaussian_1m_coefficients();
        self.initialize_gaussian_2m_coefficients();
        self.initialize_baseband_tx_timing();
        self.initialize_baseband_coexistence_defaults();
        self.initialize_baseband_cca_defaults();
        self.initialize_shared_receive_image();
    }

    fn order_device_accesses(&mut self) {
        oer_mmio::fence();
    }
}

impl<A, O, B, I, T, Z> BluetoothBasebandV2Transaction<'_, A, O, B, I, T, Z>
where
    A: Placement,
    O: Placement,
    B: Placement,
    I: Placement,
    T: Placement,
    Z: Placement,
{
    fn initialize_baseband_v2_tx(&mut self) {
        let baseband = &mut *self.baseband;
        baseband.modify_rx_setup_argument(|w| {
            w.set_tx_argument_0(oer_mmio::Bits::constant::<0x0>());
        });
        baseband.modify_rx_setup_image_0(|w| {
            w.set_tx_setup_image(oer_mmio::Bits::constant::<0x0>());
        });
        baseband.modify_gaussian_2m_coefficient_and_tx_config(|w| {
            w.set_tx_set_low_byte(0xf5);
        });
    }

    fn initialize_baseband_v2_rx(&mut self, gain_parameter: u8) {
        self.initialize_baseband_rx_setup();
        self.initialize_receive_compensation();
        self.initialize_receive_gain_offsets();
        self.initialize_receive_gain(gain_parameter);
        self.initialize_receive_rssi_thresholds();
        self.initialize_receive_targets();
        self.initialize_receive_restart();
        self.initialize_receive_recorrection();
        self.initialize_receive_detection();

        let baseband = &mut *self.baseband;
        baseband.modify_rx_correlator_control(|w| {
            w.set_config_value(oer_mmio::Bits::constant::<0x8>());
        });
        baseband.modify_rx_dpo_control(|w| {
            w.set_bit_19_opaque(false);
        });
        baseband.modify_rx_dpo_control(|w| {
            w.set_config_value(oer_mmio::Bits::constant::<0x6>());
        });
        baseband.modify_rx_filter_select(|w| {
            w.set_select_1(oer_mmio::Bits::constant::<0x2>());
        });
        baseband.modify_rx_filter_select(|w| {
            w.set_select_5(oer_mmio::Bits::constant::<0x1>());
        });
        baseband.modify_rx_filter_select(|w| {
            w.set_select_4(oer_mmio::Bits::constant::<0x2>());
        });
        baseband.modify_rx_filter_select(|w| {
            w.set_select_3(oer_mmio::Bits::constant::<0x0>());
        });
        baseband.modify_rx_filter_select(|w| {
            w.set_select_2(oer_mmio::Bits::constant::<0x0>());
        });
        baseband.modify_rx_filter_select(|w| {
            w.set_select_0(oer_mmio::Bits::constant::<0x2>());
        });

        self.initialize_shared_receive_prefix();
        let baseband = &mut *self.baseband;
        baseband.modify_rx_correlator_control(|w| {
            w.set_bit_13_opaque(false);
        });
        self.initialize_receive_saturation_gain();
    }

    /// Vendor `bt_bb_v2_rx_set` ends by tail-calling `bt_agc_sat_gain` with
    /// the finite image `0x0a1b272e`.
    fn initialize_receive_saturation_gain(&mut self) {
        let btagc = &mut *self.btagc;
        btagc.initialize_bluetooth_receive_saturation_gain_config();
        btagc.modify_word_004c_opaque(|w| {
            w.set_sat_gain_bits_0_18(oer_mmio::Bits::constant::<0x1770>());
        });
        btagc.initialize_bluetooth_receive_saturation_gain_image_0();
        btagc.initialize_bluetooth_receive_saturation_gain_image_1();
        btagc.modify_word_004c_opaque(|w| {
            w.set_bit_19_opaque(true);
            w.set_bit_24_opaque(true);
            w.set_bit_26_opaque(true);
            w.set_bit_28_opaque(true);
            w.set_bit_30_opaque(true);
            w.set_bit_31_opaque(true);
        });
        btagc.modify_cte_re_gain(|w| {
            w.set_bit_3_opaque(false);
        });
        btagc.modify_word_0088_opaque(|w| {
            w.set_bit_30_opaque(true);
        });
    }

    fn initialize_baseband_rx_setup(&mut self) {
        let baseband = &mut *self.baseband;
        baseband.modify_rx_setup_argument(|w| {
            w.set_argument_0(oer_mmio::Bits::constant::<0x4>());
        });
        baseband.modify_rx_setup_image_0(|w| {
            w.set_config_image(oer_mmio::Bits::constant::<0x199a>());
        });
        self.init_control.modify_control(|w| {
            w.set_bit_31_opaque(false);
        });
        baseband.modify_rx_setup_control_1(|w| {
            w.set_bit_2_opaque(true);
        });

        let btagc = &mut *self.btagc;
        btagc.modify_word_00d0_opaque(|w| {
            w.set_config_value_high(oer_mmio::Bits::constant::<0x14>());
        });
        btagc.modify_word_00d0_opaque(|w| {
            w.set_config_value_low(oer_mmio::Bits::constant::<0x14>());
        });
        btagc.modify_word_00d4_opaque(|w| {
            w.set_config_value(oer_mmio::Bits::constant::<0x3c0>());
        });
        btagc.modify_word_008c_opaque(|w| {
            w.set_bit_29_opaque(false);
        });
        btagc.modify_word_0088_opaque(|w| {
            w.set_bit_29_opaque(false);
        });
        btagc.modify_word_0088_opaque(|w| {
            w.set_bit_18_opaque(true);
        });
        baseband.modify_tx_cca_control_1(|w| {
            w.set_rx_setup_enable(true);
        });
        baseband.modify_tx_cca_control_2(|w| {
            w.set_rx_setup_disable(false);
        });
        btagc.modify_cte_dc_shift(|w| {
            w.set_rx_config_value(oer_mmio::Bits::constant::<0x7>());
        });
        btagc.modify_cte_dc_shift(|w| {
            w.set_dc_shift_max(oer_mmio::Bits::constant::<0x7>());
        });
        baseband.modify_tx_cca_control_2(|w| {
            w.set_rx_setup_enable(true);
        });
        btagc.modify_word_004c_opaque(|w| {
            w.set_bit_26_opaque(false);
        });
        btagc.modify_word_008c_opaque(|w| {
            w.set_bit_29_opaque(false);
        });
        baseband.modify_rx_setup_control_0(|w| {
            w.set_bit_0_opaque(true);
        });

        let shared = &mut *self.zbbb;
        shared.modify_rx_setup_control(|w| {
            w.set_bit_8_opaque(false);
        });
        shared.modify_rx_setup_image(|w| {
            w.set_bits_12_31_opaque(oer_mmio::Bits::constant::<0x1cb>());
        });
        shared.modify_rx_setup_control(|w| {
            w.set_bits_0_1_opaque(oer_mmio::Bits::constant::<0x0>());
        });
    }

    fn initialize_receive_compensation(&mut self) {
        let btagc = &mut *self.btagc;
        btagc.modify_rx_comp_control(|w| {
            w.set_dynamic_bits_7_13(oer_mmio::Bits::constant::<0x6>());
        });
        btagc.modify_rx_comp_control(|w| {
            w.set_dynamic_bits_0_6(oer_mmio::Bits::constant::<0x6>());
        });
        btagc.modify_rx_comp_control(|w| {
            w.set_finite_bits_24_28(oer_mmio::Bits::constant::<0x6>());
        });
        btagc.modify_rx_comp_control(|w| {
            w.set_finite_bits_19_23(oer_mmio::Bits::constant::<0x0>());
        });
        btagc.modify_rx_comp_control(|w| {
            w.set_finite_bits_14_18(oer_mmio::Bits::constant::<0x8>());
        });
    }

    fn initialize_receive_gain_offsets(&mut self) {
        let btagc = &mut *self.btagc;
        btagc.modify_word_0048_opaque(|w| {
            w.set_bits_5_9_opaque(oer_mmio::Bits::constant::<0x4>());
        });
        btagc.modify_word_0048_opaque(|w| {
            w.set_bits_0_4_opaque(oer_mmio::Bits::constant::<0x4>());
        });
        btagc.modify_word_0048_opaque(|w| {
            w.set_gain_offset_0(0x50);
        });
        btagc.modify_word_0048_opaque(|w| {
            w.set_gain_offset_1(0x50);
        });
        btagc.modify_word_0068_opaque(|w| {
            w.set_gain_offset_1(0x50);
        });
        btagc.modify_word_0068_opaque(|w| {
            w.set_gain_offset_0(0x50);
        });
    }

    fn initialize_receive_gain(&mut self, parameter: u8) {
        self.oracle.modify_word_7cd0_opaque(|w| {
            w.set_bit_0_opaque(true);
            w.set_bit_1_opaque(true);
            w.set_bit_2_opaque(true);
            w.set_bit_3_opaque(true);
            w.set_bits_16_19_opaque(oer_mmio::Bits::constant::<0xf>());
        });
        // The force field takes the byte's low seven bits, the gain image the
        // byte less five, wrapping, in its low seven bits.
        let parameter = u32::from(parameter);
        self.btagc.modify_rx_gain_force(|w| {
            w.set_dynamic_bits_0_6(Bits::truncate(parameter));
        });
        self.btagc.modify_agc_gain_image(|w| {
            w.set_dynamic_bits_2_8(Bits::truncate(parameter.wrapping_sub(5)));
        });
    }

    fn initialize_receive_rssi_thresholds(&mut self) {
        let btagc = &mut *self.btagc;
        btagc.modify_word_00a0_opaque(|w| {
            w.set_bits_16_23_opaque(0x9c);
        });
        btagc.modify_word_00a8_opaque(|w| {
            w.set_bits_17_24_opaque(0x9c);
        });
        btagc.modify_word_00a8_opaque(|w| {
            w.set_bits_4_11_opaque(0x88);
        });
        btagc.modify_word_00b8_opaque(|w| {
            w.set_bits_12_19_opaque(0x88);
        });
    }

    fn initialize_receive_targets(&mut self) {
        let btagc = &mut *self.btagc;
        btagc.modify_cte_agc_target(|w| {
            w.set_baseband_target_value(oer_mmio::Bits::constant::<0x1d4>());
        });
        btagc.modify_word_00b0_opaque(|w| {
            w.set_target_bits_23_31(oer_mmio::Bits::constant::<0x1d4>());
        });
        btagc.modify_rx_gain_force(|w| {
            w.set_agc_target(oer_mmio::Bits::constant::<0x1d4>());
        });
        btagc.modify_word_008c_opaque(|w| {
            w.set_target_bits_0_8(oer_mmio::Bits::constant::<0x1d4>());
        });
        btagc.modify_word_008c_opaque(|w| {
            w.set_target_bits_9_17(oer_mmio::Bits::constant::<0x1dc>());
        });
        btagc.modify_cca_config(|w| {
            w.set_target_bits_14_22(oer_mmio::Bits::constant::<0x1ce>());
        });
        btagc.modify_word_00b4_opaque(|w| {
            w.set_target_bits_23_31(oer_mmio::Bits::constant::<0x1ce>());
        });
    }

    fn initialize_receive_restart(&mut self) {
        let btagc = &mut *self.btagc;
        btagc.modify_word_0088_opaque(|w| {
            w.set_bit_30_opaque(false);
        });
        btagc.modify_word_00bc_opaque(|w| {
            w.set_restart_bit_31(true);
        });
        btagc.modify_word_0084_opaque(|w| {
            w.set_finite_bits_24_31(0xf4);
        });

        btagc.modify_word_00e0_opaque(|w| {
            w.set_bits_24_31_opaque(0x14);
        });
        btagc.modify_word_00e0_opaque(|w| {
            w.set_bits_16_23_opaque(0xe);
        });
        btagc.modify_word_00e0_opaque(|w| {
            w.set_bits_8_15_opaque(0xe);
        });
        btagc.modify_word_00e4_opaque(|w| {
            w.set_bits_16_23_opaque(0xec);
        });
        btagc.modify_word_00e4_opaque(|w| {
            w.set_bits_8_15_opaque(0xf2);
        });
        btagc.modify_word_00e4_opaque(|w| {
            w.set_bits_0_7_opaque(0xf2);
        });
        btagc.modify_word_00e0_opaque(|w| {
            w.set_bits_0_7_opaque(0xf);
        });
        btagc.modify_word_00e8_opaque(|w| {
            w.set_bits_24_31_opaque(0xf1);
        });
        btagc.modify_word_00ec_opaque(|w| {
            w.set_bits_24_31_opaque(0x2d);
        });
        btagc.modify_word_00ec_opaque(|w| {
            w.set_bits_8_15_opaque(0xd3);
        });
        btagc.modify_word_00f0_opaque(|w| {
            w.set_bits_24_31_opaque(0xa6);
        });
        btagc.modify_word_00e4_opaque(|w| {
            w.set_bits_24_31_opaque(0x1e);
        });
        btagc.modify_word_00e8_opaque(|w| {
            w.set_bits_16_23_opaque(0xe2);
        });
        btagc.modify_word_00ec_opaque(|w| {
            w.set_bits_16_23_opaque(0x28);
        });
        btagc.modify_word_00ec_opaque(|w| {
            w.set_bits_0_7_opaque(0xd8);
        });
        btagc.modify_word_00f0_opaque(|w| {
            w.set_bits_16_23_opaque(0xa6);
        });

        btagc.modify_word_0100_opaque(|w| {
            w.set_bits_20_23_opaque(oer_mmio::Bits::constant::<0x6>());
        });
        btagc.modify_word_0100_opaque(|w| {
            w.set_bits_24_27_opaque(oer_mmio::Bits::constant::<0x6>());
        });
        btagc.modify_word_0100_opaque(|w| {
            w.set_bits_16_19_opaque(oer_mmio::Bits::constant::<0x8>());
        });
        btagc.modify_word_0100_opaque(|w| {
            w.set_bits_12_15_opaque(oer_mmio::Bits::constant::<0xa>());
        });
        btagc.modify_word_0100_opaque(|w| {
            w.set_bits_28_31_opaque(oer_mmio::Bits::constant::<0x6>());
        });
        btagc.modify_word_00e8_opaque(|w| {
            w.set_bits_10_15_opaque(oer_mmio::Bits::constant::<0x3>());
        });

        let baseband = &mut *self.baseband;
        baseband.modify_rx_correlator_control(|w| {
            w.set_rx_setup_enable(true);
        });
        baseband.modify_agc_restart_control(|w| {
            w.set_bit_21_opaque(true);
        });
        baseband.modify_agc_restart_control(|w| {
            w.set_bit_20_opaque(true);
        });
        baseband.modify_agc_restart_control(|w| {
            w.set_bit_19_opaque(true);
        });
        baseband.modify_agc_restart_control(|w| {
            w.set_bit_10_opaque(true);
        });
        baseband.modify_agc_restart_control(|w| {
            w.set_bit_9_opaque(true);
        });
        baseband.modify_agc_restart_control(|w| {
            w.set_bit_8_opaque(true);
        });
        baseband.modify_agc_restart_control(|w| {
            w.set_bit_7_opaque(true);
        });
        baseband.modify_agc_restart_control(|w| {
            w.set_bit_6_opaque(true);
        });

        btagc.modify_word_00dc_opaque(|w| {
            w.set_restart_bit_31(true);
        });
        btagc.modify_word_00dc_opaque(|w| {
            w.set_restart_bit_30(true);
        });
        btagc.modify_word_00dc_opaque(|w| {
            w.set_restart_bit_29(true);
        });
        btagc.modify_word_00dc_opaque(|w| {
            w.set_restart_bit_27(true);
        });
        btagc.modify_word_00dc_opaque(|w| {
            w.set_restart_bit_28(true);
        });

        btagc.modify_word_00f8_opaque(|w| {
            w.set_bits_2_7_opaque(oer_mmio::Bits::constant::<0xc>());
        });
        btagc.modify_word_00f8_opaque(|w| {
            w.set_bits_8_13_opaque(oer_mmio::Bits::constant::<0xc>());
        });
        btagc.modify_word_00f8_opaque(|w| {
            w.set_bits_14_19_opaque(oer_mmio::Bits::constant::<0x20>());
        });
        btagc.modify_word_00f8_opaque(|w| {
            w.set_bits_26_31_opaque(oer_mmio::Bits::constant::<0xc>());
        });
        btagc.modify_word_00f8_opaque(|w| {
            w.set_bits_20_25_opaque(oer_mmio::Bits::constant::<0x5>());
        });
    }

    fn initialize_receive_recorrection(&mut self) {
        let btagc = &mut *self.btagc;
        btagc.modify_word_00b4_opaque(|w| {
            w.set_recorrect_bit_8(true);
        });
        btagc.modify_word_00b4_opaque(|w| {
            w.set_recorrect_bits_9_13(oer_mmio::Bits::constant::<0xa>());
        });
        btagc.modify_word_00bc_opaque(|w| {
            w.set_recorrect_bit_5(true);
            w.set_recorrect_bit_7(true);
        });
        btagc.modify_word_0068_opaque(|w| {
            w.set_bit_13_opaque(true);
        });
        btagc.modify_word_00bc_opaque(|w| {
            w.set_recorrect_bit_6(false);
            w.set_recorrect_bit_8(false);
        });
        btagc.modify_word_0068_opaque(|w| {
            w.set_bit_5_opaque(true);
        });
        btagc.modify_word_0068_opaque(|w| {
            w.set_bits_11_12_opaque(oer_mmio::Bits::constant::<0x3>());
        });
        btagc.modify_word_0068_opaque(|w| {
            w.set_bits_6_10_opaque(oer_mmio::Bits::constant::<0x14>());
        });
        btagc.modify_word_00b0_opaque(|w| {
            w.set_recorrect_bits_18_22(oer_mmio::Bits::constant::<0x18>());
        });
        btagc.modify_word_00b0_opaque(|w| {
            w.set_recorrect_bits_13_17(oer_mmio::Bits::constant::<0x18>());
        });
        btagc.modify_word_0068_opaque(|w| {
            w.set_bits_0_4_opaque(oer_mmio::Bits::constant::<0x14>());
        });
        btagc.modify_word_006c_opaque(|w| {
            w.set_finite_bits_24_31(0xf);
        });
    }

    fn initialize_receive_detection(&mut self) {
        let btagc = &mut *self.btagc;
        btagc.modify_word_00a0_opaque(|w| {
            w.set_bits_24_27_opaque(oer_mmio::Bits::constant::<0x4>());
        });
        btagc.modify_word_00c0_opaque(|w| {
            w.set_bits_5_9_opaque(oer_mmio::Bits::constant::<0xa>());
        });
        btagc.modify_word_00a8_opaque(|w| {
            w.set_bits_12_16_opaque(oer_mmio::Bits::constant::<0x4>());
        });
        btagc.modify_word_00a8_opaque(|w| {
            w.set_bits_0_3_opaque(oer_mmio::Bits::constant::<0x7>());
        });
        btagc.modify_word_00c0_opaque(|w| {
            w.set_bits_10_14_opaque(oer_mmio::Bits::constant::<0xa>());
        });
        btagc.modify_word_00b8_opaque(|w| {
            w.set_bits_8_11_opaque(oer_mmio::Bits::constant::<0x7>());
        });
        btagc.modify_word_00c4_opaque(|w| {
            w.set_bits_10_14_opaque(oer_mmio::Bits::constant::<0xf>());
        });
        btagc.modify_word_00c0_opaque(|w| {
            w.set_bits_24_31_opaque(0x9c);
        });
        btagc.modify_word_00c0_opaque(|w| {
            w.set_bits_20_23_opaque(oer_mmio::Bits::constant::<0x7>());
        });
        btagc.modify_word_00c0_opaque(|w| {
            w.set_bits_15_19_opaque(oer_mmio::Bits::constant::<0xa>());
        });
        btagc.modify_word_00c4_opaque(|w| {
            w.set_bits_24_31_opaque(0x9c);
        });
        btagc.modify_word_00c4_opaque(|w| {
            w.set_bits_20_23_opaque(oer_mmio::Bits::constant::<0xa>());
        });
        btagc.modify_word_00c4_opaque(|w| {
            w.set_bits_15_19_opaque(oer_mmio::Bits::constant::<0xf>());
        });
    }

    fn initialize_gaussian_1m_coefficients(&mut self) {
        let baseband = &mut *self.baseband;
        baseband.modify_gaussian_1m_coefficient_0(|w| {
            w.set_bits_28_31_opaque(oer_mmio::Bits::constant::<0x0>());
        });
        baseband.modify_gaussian_1m_coefficient_0(|w| {
            w.set_bits_23_27_opaque(oer_mmio::Bits::constant::<0x0>());
        });
        baseband.modify_gaussian_1m_coefficient_0(|w| {
            w.set_bits_17_22_opaque(oer_mmio::Bits::constant::<0x0>());
        });
        baseband.modify_gaussian_1m_coefficient_0(|w| {
            w.set_bits_10_16_opaque(oer_mmio::Bits::constant::<0x3>());
        });
        baseband.modify_gaussian_1m_coefficient_0(|w| {
            w.set_bits_2_9_opaque(0x13);
        });
        baseband.modify_gaussian_1m_coefficient_1(|w| {
            w.set_bits_23_31_opaque(oer_mmio::Bits::constant::<0x5f>());
        });
        baseband.modify_gaussian_1m_coefficient_1(|w| {
            w.set_bits_13_22_opaque(oer_mmio::Bits::constant::<0x140>());
        });
        baseband.modify_gaussian_1m_coefficient_1(|w| {
            w.set_bits_2_12_opaque(oer_mmio::Bits::constant::<0x2f2>());
        });
        baseband.modify_gaussian_1m_coefficient_2(|w| {
            w.set_bits_21_31_opaque(oer_mmio::Bits::constant::<0x50d>());
        });
        baseband.modify_gaussian_1m_coefficient_2(|w| {
            w.set_bits_10_20_opaque(oer_mmio::Bits::constant::<0x6bf>());
        });
        baseband.modify_gaussian_1m_coefficient_3(|w| {
            w.set_bits_21_31_opaque(oer_mmio::Bits::constant::<0x7a0>());
        });
        baseband.modify_gaussian_1m_coefficient_3(|w| {
            w.set_bits_10_20_opaque(oer_mmio::Bits::constant::<0x7e9>());
        });
    }

    fn initialize_gaussian_2m_coefficients(&mut self) {
        let baseband = &mut *self.baseband;
        baseband.modify_gaussian_2m_coefficient_and_tx_config(|w| {
            w.set_gaussian_bits_25_31(oer_mmio::Bits::constant::<0x0>());
        });
        baseband.modify_gaussian_2m_coefficient_and_tx_config(|w| {
            w.set_gaussian_bits_17_24(0x7);
        });
        baseband.modify_gaussian_2m_coefficient_and_tx_config(|w| {
            w.set_gaussian_bits_8_16(oer_mmio::Bits::constant::<0x67>());
        });
        baseband.modify_gaussian_2m_coefficient_1(|w| {
            w.set_bits_22_31_opaque(oer_mmio::Bits::constant::<0x24d>());
        });
        baseband.modify_gaussian_2m_coefficient_1(|w| {
            w.set_bits_11_21_opaque(oer_mmio::Bits::constant::<0x58e>());
        });
        baseband.modify_gaussian_2m_coefficient_1(|w| {
            w.set_bits_0_10_opaque(oer_mmio::Bits::constant::<0x76d>());
        });
    }

    fn initialize_baseband_tx_timing(&mut self) {
        self.oracle.modify_tx_pa_control_1(|w| {
            w.set_pa_on_bt_delay(0x96);
        });
        let baseband = &mut *self.baseband;
        baseband.modify_le_tx_on_delay(|w| {
            w.set_bits_16_18_opaque(oer_mmio::Bits::constant::<0x0>());
            w.set_encoded_value_minus_10(0x1d);
        });
        baseband.modify_tx_cca_control_0(|w| {
            w.set_bits_20_22_opaque(oer_mmio::Bits::constant::<0x0>());
            w.set_period_argument_0_minus_argument_1_image(0x17);
        });
        baseband.modify_tx_cca_control_0(|w| {
            w.set_period_argument_0_image(oer_mmio::Bits::constant::<0x157>());
        });
        self.tx_timing.modify_auxiliary_tx_on_delay(|w| {
            w.set_encoded_image(oer_mmio::Bits::constant::<0x190>());
        });
    }

    fn initialize_baseband_coexistence_defaults(&mut self) {
        self.baseband.modify_coex_config(|w| {
            w.set_bit_18_opaque(true);
        });
        self.baseband.modify_coex_config(|w| {
            w.set_bit_20_opaque(true);
        });
    }

    fn initialize_baseband_cca_defaults(&mut self) {
        self.btagc.modify_cca_config(|w| {
            w.set_config_value_0(oer_mmio::Bits::constant::<0x1ce>());
        });
        self.btagc.modify_cca_config(|w| {
            w.set_config_value_1(oer_mmio::Bits::constant::<0x1e>());
        });
        self.btagc.modify_cca_config(|w| {
            w.set_bit_23_opaque(true);
        });
        self.baseband.modify_tx_cca_control_1(|w| {
            w.set_default_config_value(oer_mmio::Bits::constant::<0x5>());
        });
    }

    fn initialize_shared_receive_prefix(&mut self) {
        let shared = &mut *self.zbbb;
        shared.modify_shared_rx_init_image_0(|w| {
            w.set_bits_21_31_opaque(oer_mmio::Bits::constant::<0x792>());
        });
        shared.modify_rx_setup_control(|w| {
            w.set_bits_10_20_opaque(oer_mmio::Bits::constant::<0x79c>());
        });
        shared.modify_shared_rx_init_image_1(|w| {
            w.set_bits_0_10_opaque(oer_mmio::Bits::constant::<0x7a6>());
        });
        shared.modify_shared_rx_init_image_0(|w| {
            w.set_bits_13_20_opaque(0xa6);
        });
        shared.modify_shared_rx_init_image_4(|w| {
            w.set_bits_20_30_opaque(oer_mmio::Bits::constant::<0x7e1>());
        });
        shared.modify_shared_rx_init_image_4(|w| {
            w.set_bits_9_19_opaque(oer_mmio::Bits::constant::<0x7ed>());
        });
        shared.modify_shared_rx_init_image_2(|w| {
            w.set_bits_8_31_opaque(oer_mmio::Bits::constant::<0x7a120>());
        });
        shared.modify_shared_rx_init_image_3(|w| {
            w.set_bits_8_31_opaque(oer_mmio::Bits::constant::<0xf85edf>());
        });
        shared.modify_shared_rx_init_control_0(|w| {
            w.set_bit_1_opaque(false);
        });
        shared.modify_shared_rx_init_control_1(|w| {
            w.set_bit_30_opaque(true);
        });
        shared.modify_shared_rx_init_image_2(|w| {
            w.set_bits_0_7_opaque(0xf1);
        });
        shared.modify_shared_rx_init_image_4(|w| {
            w.set_bit_0_opaque(true);
        });
    }

    fn initialize_shared_receive_image(&mut self) {
        self.initialize_shared_receive_prefix();
        self.zbbb.modify_rx_setup_image(|w| {
            w.set_bits_12_31_opaque(oer_mmio::Bits::constant::<0x19f>());
        });
    }
}

#[cfg(test)]
mod tests;
