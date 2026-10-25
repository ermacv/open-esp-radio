//! Exact bounded Bluetooth baseband initialization transaction.
//!
//! This module is deliberately crate-private. The transaction is the observed
//! MMIO portion of `bt_bb_v2_init_cmplx(1)`, but it is not a controller-enable
//! API: the vendor lifecycle first completes common PHY initialization. The
//! transaction belongs to the shared radio owner, which Bluetooth and IEEE
//! 802.15.4 reach through the HAL's reference-counted BTBB acquisition; it
//! closes the body with a device fence.

#![deny(unsafe_code)]

use crate::{SharedRadioRegisters, device_fence};

impl SharedRadioRegisters {
    /// Execute only the exact MMIO path of vendor `bt_bb_v2_init_cmplx(1)`.
    ///
    /// The BTBB baseband is shared by Bluetooth and IEEE 802.15.4, so this
    /// transaction belongs to the shared radio owner. The caller-provided byte
    /// is the positional value read by the vendor body from the real linked
    /// `phy_param` object at offset `0x120`. Its higher-level meaning remains
    /// unassigned. The vendor diagnostic print selected by argument one is
    /// intentionally outside this hardware transaction and is omitted from
    /// the comparison effect contract. After the body, this edge adds exactly
    /// one device fence before returning to its lifecycle owner.
    ///
    /// Common-PHY and clock ordering do not belong to the PAC. The lifecycle
    /// above HAL is the only safe production caller and retains those owners
    /// while this finite register transaction executes.
    #[doc(hidden)]
    pub fn initialize_btbb_v2_arg_one(&mut self, gain_parameter: u8) {
        let mut port = BluetoothBasebandV2Transaction {
            radio_phy: &mut self.radio_phy.peripherals,
            shared_radio: &mut self.shared_radio,
        };
        execute_standalone_bluetooth_transition(&mut port, gain_parameter);
    }
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

/// One borrow-scoped view of the exact generated owners touched by BTBB v2.
struct BluetoothBasebandV2Transaction<'a> {
    radio_phy: &'a mut crate::generated::peripheral_ownership::RadioPhyPeripherals,
    shared_radio: &'a mut crate::ownership::SharedRadioBlocks,
}

impl BluetoothBasebandV2TransitionPort for BluetoothBasebandV2Transaction<'_> {
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
        device_fence();
    }
}

impl BluetoothBasebandV2Transaction<'_> {
    fn initialize_baseband_v2_tx(&mut self) {
        let baseband = &mut self.shared_radio.bt_v3_2_baseband;
        baseband.initialize_bluetooth_baseband_tx_argument();
        baseband.initialize_bluetooth_baseband_tx_setup_image();
        baseband.initialize_bluetooth_baseband_tx_low_byte();
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

        let baseband = &mut self.shared_radio.bt_v3_2_baseband;
        baseband.initialize_bluetooth_receive_correlator();
        baseband.initialize_bluetooth_receive_dpo_bit_19();
        baseband.initialize_bluetooth_receive_dpo_value();
        baseband.initialize_bluetooth_receive_filter_1();
        baseband.initialize_bluetooth_receive_filter_5();
        baseband.initialize_bluetooth_receive_filter_4();
        baseband.initialize_bluetooth_receive_filter_3();
        baseband.initialize_bluetooth_receive_filter_2();
        baseband.initialize_bluetooth_receive_filter_0();

        self.initialize_shared_receive_prefix();
        let baseband = &mut self.shared_radio.bt_v3_2_baseband;
        baseband.initialize_bluetooth_receive_correlator_final();
        self.initialize_receive_saturation_gain();
    }

    /// Vendor `bt_bb_v2_rx_set` ends by tail-calling `bt_agc_sat_gain` with
    /// the finite image `0x0a1b272e`.
    fn initialize_receive_saturation_gain(&mut self) {
        let btagc = &mut self.radio_phy.phy_btagc_recovered;
        btagc.initialize_bluetooth_receive_saturation_gain_config();
        btagc.initialize_bluetooth_receive_saturation_gain_004c_low();
        btagc.initialize_bluetooth_receive_saturation_gain_image_0();
        btagc.initialize_bluetooth_receive_saturation_gain_image_1();
        btagc.set_bluetooth_receive_saturation_gain_004c_bits();
        btagc.clear_bluetooth_receive_saturation_gain_cte_bit_3();
        btagc.set_bluetooth_receive_saturation_gain_0088_bit_30();
    }

    fn initialize_baseband_rx_setup(&mut self) {
        let baseband = &mut self.shared_radio.bt_v3_2_baseband;
        baseband.initialize_bluetooth_receive_setup_argument();
        baseband.initialize_bluetooth_receive_setup_image();
        self.shared_radio
            .shared_radio_init_control
            .initialize_bluetooth_receive_setup_shared_control();
        baseband.set_bluetooth_receive_setup_control_1_bit_2();

        let btagc = &mut self.radio_phy.phy_btagc_recovered;
        btagc.initialize_bluetooth_receive_setup_agc_00d0_high();
        btagc.initialize_bluetooth_receive_setup_agc_00d0_low();
        btagc.initialize_bluetooth_receive_setup_agc_00d4();
        btagc.initialize_bluetooth_receive_setup_008c_bit_29_initial();
        btagc.initialize_bluetooth_receive_setup_0088_bit_29();
        btagc.initialize_bluetooth_receive_setup_0088_bit_18();
        baseband.initialize_bluetooth_receive_setup_cca_1();
        baseband.initialize_bluetooth_receive_setup_cca_2_disable();
        btagc.initialize_bluetooth_receive_setup_cte_value();
        btagc.initialize_bluetooth_receive_setup_cte_max();
        baseband.initialize_bluetooth_receive_setup_cca_2_enable();
        btagc.initialize_bluetooth_receive_setup_004c_bit_26();
        btagc.initialize_bluetooth_receive_setup_008c_bit_29_final();
        baseband.set_bluetooth_receive_setup_control_0_bit_0();

        let shared = &mut self.shared_radio.zbbb_radio_control;
        shared.initialize_bluetooth_receive_setup_zbbb_bit_8();
        shared.initialize_bluetooth_receive_setup_zbbb_image();
        shared.initialize_bluetooth_receive_setup_zbbb_low();
    }

    fn initialize_receive_compensation(&mut self) {
        let btagc = &mut self.radio_phy.phy_btagc_recovered;
        btagc.initialize_bluetooth_receive_compensation_7_13();
        btagc.initialize_bluetooth_receive_compensation_0_6();
        btagc.initialize_bluetooth_receive_compensation_24_28();
        btagc.initialize_bluetooth_receive_compensation_19_23();
        btagc.initialize_bluetooth_receive_compensation_14_18();
    }

    fn initialize_receive_gain_offsets(&mut self) {
        let btagc = &mut self.radio_phy.phy_btagc_recovered;
        btagc.initialize_bluetooth_receive_gain_offset_0_bits_5_9();
        btagc.initialize_bluetooth_receive_gain_offset_0_bits_0_4();
        btagc.initialize_bluetooth_receive_gain_offset_0_bits_16_23();
        btagc.initialize_bluetooth_receive_gain_offset_0_bits_24_31();
        btagc.initialize_bluetooth_receive_gain_offset_1_bits_24_31();
        btagc.initialize_bluetooth_receive_gain_offset_1_bits_16_23();
    }

    fn initialize_receive_gain(&mut self, parameter: u8) {
        self.radio_phy
            .phy_baseband_config_oracle
            .initialize_bluetooth_receive_gain_baseband();
        let parameter =
            crate::generated::BluetoothBasebandGainParameterByte::new(u32::from(parameter))
                .expect("every byte belongs to the complete reviewed domain");
        self.radio_phy
            .phy_btagc_recovered
            .initialize_bluetooth_receive_gain_force(parameter);
        self.radio_phy
            .phy_btagc_recovered
            .initialize_bluetooth_receive_gain_image(parameter);
    }

    fn initialize_receive_rssi_thresholds(&mut self) {
        let btagc = &mut self.radio_phy.phy_btagc_recovered;
        btagc.initialize_bluetooth_receive_rssi_threshold_00a0();
        btagc.initialize_bluetooth_receive_rssi_threshold_00a8_high();
        btagc.initialize_bluetooth_receive_rssi_threshold_00a8_low();
        btagc.initialize_bluetooth_receive_rssi_threshold_00b8();
    }

    fn initialize_receive_targets(&mut self) {
        let btagc = &mut self.radio_phy.phy_btagc_recovered;
        btagc.initialize_bluetooth_receive_target_cte();
        btagc.initialize_bluetooth_receive_target_00b0();
        btagc.initialize_bluetooth_receive_target_gain_force();
        btagc.initialize_bluetooth_receive_target_008c_low();
        btagc.initialize_bluetooth_receive_target_008c_high();
        btagc.initialize_bluetooth_receive_target_cca();
        btagc.initialize_bluetooth_receive_target_00b4();
    }

    fn initialize_receive_restart(&mut self) {
        let btagc = &mut self.radio_phy.phy_btagc_recovered;
        btagc.initialize_bluetooth_receive_restart_config_0088();
        btagc.initialize_bluetooth_receive_restart_control_00bc();
        btagc.initialize_bluetooth_receive_restart_config_0084();

        btagc.initialize_bluetooth_receive_restart_e0_24_31();
        btagc.initialize_bluetooth_receive_restart_e0_16_23();
        btagc.initialize_bluetooth_receive_restart_e0_8_15();
        btagc.initialize_bluetooth_receive_restart_e4_16_23();
        btagc.initialize_bluetooth_receive_restart_e4_8_15();
        btagc.initialize_bluetooth_receive_restart_e4_0_7();
        btagc.initialize_bluetooth_receive_restart_e0_0_7();
        btagc.initialize_bluetooth_receive_restart_e8_24_31();
        btagc.initialize_bluetooth_receive_restart_ec_24_31();
        btagc.initialize_bluetooth_receive_restart_ec_8_15();
        btagc.initialize_bluetooth_receive_restart_f0_24_31();
        btagc.initialize_bluetooth_receive_restart_e4_24_31();
        btagc.initialize_bluetooth_receive_restart_e8_16_23();
        btagc.initialize_bluetooth_receive_restart_ec_16_23();
        btagc.initialize_bluetooth_receive_restart_ec_0_7();
        btagc.initialize_bluetooth_receive_restart_f0_16_23();

        btagc.initialize_bluetooth_receive_restart_100_20_23();
        btagc.initialize_bluetooth_receive_restart_100_24_27();
        btagc.initialize_bluetooth_receive_restart_100_16_19();
        btagc.initialize_bluetooth_receive_restart_100_12_15();
        btagc.initialize_bluetooth_receive_restart_100_28_31();
        btagc.initialize_bluetooth_receive_restart_e8_10_15();

        let baseband = &mut self.shared_radio.bt_v3_2_baseband;
        baseband.initialize_bluetooth_receive_restart_correlator();
        baseband.initialize_bluetooth_receive_restart_baseband_21();
        baseband.initialize_bluetooth_receive_restart_baseband_20();
        baseband.initialize_bluetooth_receive_restart_baseband_19();
        baseband.initialize_bluetooth_receive_restart_baseband_10();
        baseband.initialize_bluetooth_receive_restart_baseband_9();
        baseband.initialize_bluetooth_receive_restart_baseband_8();
        baseband.initialize_bluetooth_receive_restart_baseband_7();
        baseband.initialize_bluetooth_receive_restart_baseband_6();

        btagc.initialize_bluetooth_receive_restart_btagc_31();
        btagc.initialize_bluetooth_receive_restart_btagc_30();
        btagc.initialize_bluetooth_receive_restart_btagc_29();
        btagc.initialize_bluetooth_receive_restart_btagc_27();
        btagc.initialize_bluetooth_receive_restart_btagc_28();

        btagc.initialize_bluetooth_receive_restart_f8_2_7();
        btagc.initialize_bluetooth_receive_restart_f8_8_13();
        btagc.initialize_bluetooth_receive_restart_f8_14_19();
        btagc.initialize_bluetooth_receive_restart_f8_26_31();
        btagc.initialize_bluetooth_receive_restart_f8_20_25();
    }

    fn initialize_receive_recorrection(&mut self) {
        let btagc = &mut self.radio_phy.phy_btagc_recovered;
        btagc.initialize_bluetooth_receive_recorrection_00b4_bit_8();
        btagc.initialize_bluetooth_receive_recorrection_00b4_bits_9_13();
        btagc.initialize_bluetooth_receive_recorrection_00bc_set_pair();
        btagc.initialize_bluetooth_receive_recorrection_gain_bit_13();
        btagc.initialize_bluetooth_receive_recorrection_00bc_clear_pair();
        btagc.initialize_bluetooth_receive_recorrection_gain_bit_5();
        btagc.initialize_bluetooth_receive_recorrection_gain_bits_11_12();
        btagc.initialize_bluetooth_receive_recorrection_gain_bits_6_10();
        btagc.initialize_bluetooth_receive_recorrection_00b0_bits_18_22();
        btagc.initialize_bluetooth_receive_recorrection_00b0_bits_13_17();
        btagc.initialize_bluetooth_receive_recorrection_gain_bits_0_4();
        btagc.initialize_bluetooth_receive_recorrection_control_006c();
    }

    fn initialize_receive_detection(&mut self) {
        let btagc = &mut self.radio_phy.phy_btagc_recovered;
        btagc.initialize_bluetooth_receive_detection_00a0();
        btagc.initialize_bluetooth_receive_detection_00c0_bits_5_9();
        btagc.initialize_bluetooth_receive_detection_00a8_middle();
        btagc.initialize_bluetooth_receive_detection_00a8_low();
        btagc.initialize_bluetooth_receive_detection_00c0_bits_10_14();
        btagc.initialize_bluetooth_receive_detection_00b8();
        btagc.initialize_bluetooth_receive_detection_00c4_bits_10_14();
        btagc.initialize_bluetooth_receive_detection_00c0_bits_24_31();
        btagc.initialize_bluetooth_receive_detection_00c0_bits_20_23();
        btagc.initialize_bluetooth_receive_detection_00c0_bits_15_19();
        btagc.initialize_bluetooth_receive_detection_00c4_bits_24_31();
        btagc.initialize_bluetooth_receive_detection_00c4_bits_20_23();
        btagc.initialize_bluetooth_receive_detection_00c4_bits_15_19();
    }

    fn initialize_gaussian_1m_coefficients(&mut self) {
        let baseband = &mut self.shared_radio.bt_v3_2_baseband;
        baseband.initialize_bluetooth_gaussian_1m_0_bits_28_31();
        baseband.initialize_bluetooth_gaussian_1m_0_bits_23_27();
        baseband.initialize_bluetooth_gaussian_1m_0_bits_17_22();
        baseband.initialize_bluetooth_gaussian_1m_0_bits_10_16();
        baseband.initialize_bluetooth_gaussian_1m_0_bits_2_9();
        baseband.initialize_bluetooth_gaussian_1m_1_bits_23_31();
        baseband.initialize_bluetooth_gaussian_1m_1_bits_13_22();
        baseband.initialize_bluetooth_gaussian_1m_1_bits_2_12();
        baseband.initialize_bluetooth_gaussian_1m_2_bits_21_31();
        baseband.initialize_bluetooth_gaussian_1m_2_bits_10_20();
        baseband.initialize_bluetooth_gaussian_1m_3_bits_21_31();
        baseband.initialize_bluetooth_gaussian_1m_3_bits_10_20();
    }

    fn initialize_gaussian_2m_coefficients(&mut self) {
        let baseband = &mut self.shared_radio.bt_v3_2_baseband;
        baseband.initialize_bluetooth_gaussian_2m_0_bits_25_31();
        baseband.initialize_bluetooth_gaussian_2m_0_bits_17_24();
        baseband.initialize_bluetooth_gaussian_2m_0_bits_8_16();
        baseband.initialize_bluetooth_gaussian_2m_1_bits_22_31();
        baseband.initialize_bluetooth_gaussian_2m_1_bits_11_21();
        baseband.initialize_bluetooth_gaussian_2m_1_bits_0_10();
    }

    fn initialize_baseband_tx_timing(&mut self) {
        self.radio_phy
            .phy_baseband_config_oracle
            .initialize_bluetooth_tx_pa_delay();
        let baseband = &mut self.shared_radio.bt_v3_2_baseband;
        baseband.initialize_bluetooth_le_tx_delay();
        baseband.initialize_bluetooth_tx_cca_period_difference();
        baseband.initialize_bluetooth_tx_cca_period_argument();
        self.shared_radio
            .shared_baseband_tx_timing
            .initialize_bluetooth_shared_tx_delay();
    }

    fn initialize_baseband_coexistence_defaults(&mut self) {
        self.shared_radio
            .bt_v3_2_baseband
            .initialize_bluetooth_baseband_coexistence_18();
        self.shared_radio
            .bt_v3_2_baseband
            .initialize_bluetooth_baseband_coexistence_20();
    }

    fn initialize_baseband_cca_defaults(&mut self) {
        self.radio_phy
            .phy_btagc_recovered
            .initialize_bluetooth_baseband_cca_value_0();
        self.radio_phy
            .phy_btagc_recovered
            .initialize_bluetooth_baseband_cca_value_1();
        self.radio_phy
            .phy_btagc_recovered
            .initialize_bluetooth_baseband_cca_bit_23();
        self.shared_radio
            .bt_v3_2_baseband
            .initialize_bluetooth_baseband_cca_default();
    }

    fn initialize_shared_receive_prefix(&mut self) {
        let shared = &mut self.shared_radio.zbbb_radio_control;
        shared.initialize_shared_receive_image_0_high();
        shared.initialize_shared_receive_control_image();
        shared.initialize_shared_receive_image_1_low();
        shared.initialize_shared_receive_image_0_middle();
        shared.initialize_shared_receive_image_4_high();
        shared.initialize_shared_receive_image_4_middle();
        shared.initialize_shared_receive_image_2_high();
        shared.initialize_shared_receive_image_3_high();
        shared.initialize_shared_receive_control_0();
        shared.initialize_shared_receive_control_1();
        shared.initialize_shared_receive_image_2_low();
        shared.initialize_shared_receive_image_4_low();
    }

    fn initialize_shared_receive_image(&mut self) {
        self.initialize_shared_receive_prefix();
        self.shared_radio
            .zbbb_radio_control
            .initialize_bluetooth_shared_receive_final_image();
    }
}

#[cfg(test)]
mod tests;
