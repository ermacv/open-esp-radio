//! Temporary: each converted transaction against its class-accessor replacement.
#![allow(unused_imports)]
use std::vec::Vec;

fn fills(words: usize) -> Vec<Vec<u32>> {
    let mut out = std::vec![
        std::vec![0; words],
        std::vec![u32::MAX; words],
        std::vec![0x5555_5555; words],
        std::vec![0xaaaa_aaaa; words]
    ];
    for seed in 1..=4u32 {
        let mut x = seed.wrapping_mul(0x9e37_79b9);
        out.push(
            (0..words)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    x
                })
                .collect(),
        );
    }
    out
}

fn reference<L: oer_esp32s31_pac::layouts::DmaLayout>(
    address: u32,
) -> oer_esp32s31_pac::layouts::DmaRef<L> {
    crate::dma_address::model_reference(address)
}

#[test]
fn ble_controller_address_slot_high_0() {
    for fill in fills(oer_esp32s31_pac::ble_controller_address_slots::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_controller_address_slots::BleControllerAddressSlots::host(&mut old)
            .expect("block")
            .ble_controller_address_slot_high(oer_mmio::Index::constant::<2>(), 0x0001);
        oer_esp32s31_pac::ble_controller_address_slots::BleControllerAddressSlots::host(&mut new)
            .expect("block")
            .write_slot_high(oer_mmio::Index::constant::<2>(), |w| {
                w.set_octets_4_to_5(0x0001);
            });
        assert_eq!(old, new, "ble_controller_address_slot_high");
    }
}

#[test]
fn ble_controller_address_slot_high_1() {
    for fill in fills(oer_esp32s31_pac::ble_controller_address_slots::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_controller_address_slots::BleControllerAddressSlots::host(&mut old)
            .expect("block")
            .ble_controller_address_slot_high(oer_mmio::Index::constant::<2>(), 0xa55a);
        oer_esp32s31_pac::ble_controller_address_slots::BleControllerAddressSlots::host(&mut new)
            .expect("block")
            .write_slot_high(oer_mmio::Index::constant::<2>(), |w| {
                w.set_octets_4_to_5(0xa55a);
            });
        assert_eq!(old, new, "ble_controller_address_slot_high");
    }
}

#[test]
fn ble_controller_address_slot_low_0() {
    for fill in fills(oer_esp32s31_pac::ble_controller_address_slots::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_controller_address_slots::BleControllerAddressSlots::host(&mut old)
            .expect("block")
            .ble_controller_address_slot_low(oer_mmio::Index::constant::<2>(), 0x0000_0001);
        oer_esp32s31_pac::ble_controller_address_slots::BleControllerAddressSlots::host(&mut new)
            .expect("block")
            .write_slot_low(oer_mmio::Index::constant::<2>(), |w| {
                w.set_octets_0_to_3(0x0000_0001);
            });
        assert_eq!(old, new, "ble_controller_address_slot_low");
    }
}

#[test]
fn ble_controller_address_slot_low_1() {
    for fill in fills(oer_esp32s31_pac::ble_controller_address_slots::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_controller_address_slots::BleControllerAddressSlots::host(&mut old)
            .expect("block")
            .ble_controller_address_slot_low(oer_mmio::Index::constant::<2>(), 0xa55a_3cc3);
        oer_esp32s31_pac::ble_controller_address_slots::BleControllerAddressSlots::host(&mut new)
            .expect("block")
            .write_slot_low(oer_mmio::Index::constant::<2>(), |w| {
                w.set_octets_0_to_3(0xa55a_3cc3);
            });
        assert_eq!(old, new, "ble_controller_address_slot_low");
    }
}

#[test]
fn clear_ble_connection_link_state_control_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .clear_ble_connection_link_state_control();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_connection_abort_control(|w| {
                w.set_link_state_control(false);
            });
        assert_eq!(old, new, "clear_ble_connection_link_state_control");
    }
}

#[test]
fn clear_ble_connection_link_state_control_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .clear_ble_connection_link_state_control();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_connection_abort_control(|w| {
                w.set_link_state_control(false);
            });
        assert_eq!(old, new, "clear_ble_connection_link_state_control");
    }
}

#[test]
fn clear_ble_phy_init_byte_2_low_7_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .clear_ble_phy_init_byte_2_low_7();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0254_opaque(|w| {
                w.set_init_byte_2_low_7(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "clear_ble_phy_init_byte_2_low_7");
    }
}

#[test]
fn clear_ble_phy_init_byte_2_low_7_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .clear_ble_phy_init_byte_2_low_7();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0254_opaque(|w| {
                w.set_init_byte_2_low_7(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "clear_ble_phy_init_byte_2_low_7");
    }
}

#[test]
fn clear_ble_phy_init_low_byte_pair_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .clear_ble_phy_init_low_byte_pair();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0254_opaque(|w| {
                w.set_init_byte_0(0x0);
                w.set_init_byte_1(0x0);
            });
        assert_eq!(old, new, "clear_ble_phy_init_low_byte_pair");
    }
}

#[test]
fn clear_ble_phy_init_low_byte_pair_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .clear_ble_phy_init_low_byte_pair();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0254_opaque(|w| {
                w.set_init_byte_0(0x0);
                w.set_init_byte_1(0x0);
            });
        assert_eq!(old, new, "clear_ble_phy_init_low_byte_pair");
    }
}

#[test]
fn clear_ble_phy_init_zero_0074_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .clear_ble_phy_init_zero_0074();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0074_opaque(|w| {
                w.set_bits_0_31_opaque(0x0);
            });
        assert_eq!(old, new, "clear_ble_phy_init_zero_0074");
    }
}

#[test]
fn clear_ble_phy_init_zero_0074_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .clear_ble_phy_init_zero_0074();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0074_opaque(|w| {
                w.set_bits_0_31_opaque(0x0);
            });
        assert_eq!(old, new, "clear_ble_phy_init_zero_0074");
    }
}

#[test]
fn clear_ble_phy_lc_tx_on_delay_fields_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .clear_ble_phy_lc_tx_on_delay_fields();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_lc_tx_on_delay_config(|w| {
                w.set_lc_tx_on_delay(0x0);
                w.set_init_duplicate_byte(0x0);
            });
        assert_eq!(old, new, "clear_ble_phy_lc_tx_on_delay_fields");
    }
}

#[test]
fn clear_ble_phy_lc_tx_on_delay_fields_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .clear_ble_phy_lc_tx_on_delay_fields();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_lc_tx_on_delay_config(|w| {
                w.set_lc_tx_on_delay(0x0);
                w.set_init_duplicate_byte(0x0);
            });
        assert_eq!(old, new, "clear_ble_phy_lc_tx_on_delay_fields");
    }
}

#[test]
fn clear_ble_scheduler_run_event_source_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .clear_ble_scheduler_run_event_source();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .trigger_interrupt_clear_scheduler_run();
        assert_eq!(old, new, "clear_ble_scheduler_run_event_source");
    }
}

#[test]
fn clear_ble_scheduler_run_event_source_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .clear_ble_scheduler_run_event_source();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .trigger_interrupt_clear_scheduler_run();
        assert_eq!(old, new, "clear_ble_scheduler_run_event_source");
    }
}

#[test]
fn clear_bluetooth_cte_buffer_software_ownership_0() {
    for fill in fills(oer_esp32s31_pac::ble_hw_cte_ring_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_cte_ring_control::BleHwCteRingControl::host(&mut old)
            .expect("block")
            .clear_bluetooth_cte_buffer_software_ownership(oer_mmio::Index::constant::<5>());
        oer_esp32s31_pac::ble_hw_cte_ring_control::BleHwCteRingControl::host(&mut new)
            .expect("block")
            .modify_buffer_state(oer_mmio::Index::constant::<5>(), |w| {
                w.set_software_owned(false);
            });
        assert_eq!(old, new, "clear_bluetooth_cte_buffer_software_ownership");
    }
}

#[test]
fn clear_bluetooth_cte_buffer_software_ownership_1() {
    for fill in fills(oer_esp32s31_pac::ble_hw_cte_ring_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_cte_ring_control::BleHwCteRingControl::host(&mut old)
            .expect("block")
            .clear_bluetooth_cte_buffer_software_ownership(oer_mmio::Index::constant::<5>());
        oer_esp32s31_pac::ble_hw_cte_ring_control::BleHwCteRingControl::host(&mut new)
            .expect("block")
            .modify_buffer_state(oer_mmio::Index::constant::<5>(), |w| {
                w.set_software_owned(false);
            });
        assert_eq!(old, new, "clear_bluetooth_cte_buffer_software_ownership");
    }
}

#[test]
fn clear_bluetooth_hal_low_half_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_hal_low_half();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_low_half(|w| {
                w.set_config_low_16(0x0);
            });
        assert_eq!(old, new, "clear_bluetooth_hal_low_half");
    }
}

#[test]
fn clear_bluetooth_hal_low_half_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_hal_low_half();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_low_half(|w| {
                w.set_config_low_16(0x0);
            });
        assert_eq!(old, new, "clear_bluetooth_hal_low_half");
    }
}

#[test]
fn clear_bluetooth_hal_scheduler_byte_1_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_hal_scheduler_byte_1();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_scheduler_control(|w| {
                w.set_config_byte_1(0x0);
            });
        assert_eq!(old, new, "clear_bluetooth_hal_scheduler_byte_1");
    }
}

#[test]
fn clear_bluetooth_hal_scheduler_byte_1_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_hal_scheduler_byte_1();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_scheduler_control(|w| {
                w.set_config_byte_1(0x0);
            });
        assert_eq!(old, new, "clear_bluetooth_hal_scheduler_byte_1");
    }
}

#[test]
fn clear_bluetooth_hal_scheduler_config_16_20_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_hal_scheduler_config_16_20();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_scheduler_control(|w| {
                w.set_config_16_20(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "clear_bluetooth_hal_scheduler_config_16_20");
    }
}

#[test]
fn clear_bluetooth_hal_scheduler_config_16_20_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_hal_scheduler_config_16_20();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_scheduler_control(|w| {
                w.set_config_16_20(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "clear_bluetooth_hal_scheduler_config_16_20");
    }
}

#[test]
fn clear_bluetooth_hal_slot_lane_0_upper_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_hal_slot_lane_0_upper(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_0_index_high(false);
                w.set_bit_3_opaque(false);
            });
        assert_eq!(old, new, "clear_bluetooth_hal_slot_lane_0_upper");
    }
}

#[test]
fn clear_bluetooth_hal_slot_lane_0_upper_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_hal_slot_lane_0_upper(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_0_index_high(false);
                w.set_bit_3_opaque(false);
            });
        assert_eq!(old, new, "clear_bluetooth_hal_slot_lane_0_upper");
    }
}

#[test]
fn clear_bluetooth_hal_slot_lane_1_upper_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_hal_slot_lane_1_upper(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_1_index_high(false);
                w.set_bit_7_opaque(false);
            });
        assert_eq!(old, new, "clear_bluetooth_hal_slot_lane_1_upper");
    }
}

#[test]
fn clear_bluetooth_hal_slot_lane_1_upper_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_hal_slot_lane_1_upper(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_1_index_high(false);
                w.set_bit_7_opaque(false);
            });
        assert_eq!(old, new, "clear_bluetooth_hal_slot_lane_1_upper");
    }
}

#[test]
fn clear_bluetooth_hal_slot_lane_2_upper_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_hal_slot_lane_2_upper(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_2_index_high(false);
                w.set_bit_11_opaque(false);
            });
        assert_eq!(old, new, "clear_bluetooth_hal_slot_lane_2_upper");
    }
}

#[test]
fn clear_bluetooth_hal_slot_lane_2_upper_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_hal_slot_lane_2_upper(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_2_index_high(false);
                w.set_bit_11_opaque(false);
            });
        assert_eq!(old, new, "clear_bluetooth_hal_slot_lane_2_upper");
    }
}

#[test]
fn clear_bluetooth_hal_slot_lane_3_upper_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_hal_slot_lane_3_upper(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_3_index_high(false);
                w.set_bit_15_opaque(false);
            });
        assert_eq!(old, new, "clear_bluetooth_hal_slot_lane_3_upper");
    }
}

#[test]
fn clear_bluetooth_hal_slot_lane_3_upper_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_hal_slot_lane_3_upper(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_3_index_high(false);
                w.set_bit_15_opaque(false);
            });
        assert_eq!(old, new, "clear_bluetooth_hal_slot_lane_3_upper");
    }
}

#[test]
fn clear_bluetooth_hal_slot_lane_4_upper_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_hal_slot_lane_4_upper(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_4_index_high(false);
                w.set_bit_19_opaque(false);
            });
        assert_eq!(old, new, "clear_bluetooth_hal_slot_lane_4_upper");
    }
}

#[test]
fn clear_bluetooth_hal_slot_lane_4_upper_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_hal_slot_lane_4_upper(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_4_index_high(false);
                w.set_bit_19_opaque(false);
            });
        assert_eq!(old, new, "clear_bluetooth_hal_slot_lane_4_upper");
    }
}

#[test]
fn clear_bluetooth_hal_slot_lane_5_upper_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_hal_slot_lane_5_upper(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_5_index_high(false);
                w.set_bit_23_opaque(false);
            });
        assert_eq!(old, new, "clear_bluetooth_hal_slot_lane_5_upper");
    }
}

#[test]
fn clear_bluetooth_hal_slot_lane_5_upper_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_hal_slot_lane_5_upper(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_5_index_high(false);
                w.set_bit_23_opaque(false);
            });
        assert_eq!(old, new, "clear_bluetooth_hal_slot_lane_5_upper");
    }
}

#[test]
fn clear_bluetooth_hal_slot_lane_6_upper_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_hal_slot_lane_6_upper(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_6_index_high(false);
                w.set_bit_27_opaque(false);
            });
        assert_eq!(old, new, "clear_bluetooth_hal_slot_lane_6_upper");
    }
}

#[test]
fn clear_bluetooth_hal_slot_lane_6_upper_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_hal_slot_lane_6_upper(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_6_index_high(false);
                w.set_bit_27_opaque(false);
            });
        assert_eq!(old, new, "clear_bluetooth_hal_slot_lane_6_upper");
    }
}

#[test]
fn clear_bluetooth_hal_slot_lane_7_upper_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_hal_slot_lane_7_upper(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_7_index_high(false);
                w.set_bit_31_opaque(false);
            });
        assert_eq!(old, new, "clear_bluetooth_hal_slot_lane_7_upper");
    }
}

#[test]
fn clear_bluetooth_hal_slot_lane_7_upper_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_hal_slot_lane_7_upper(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_7_index_high(false);
                w.set_bit_31_opaque(false);
            });
        assert_eq!(old, new, "clear_bluetooth_hal_slot_lane_7_upper");
    }
}

#[test]
fn clear_bluetooth_low_power_controls_0_4_0() {
    for fill in fills(oer_esp32s31_pac::btdm_runtime_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btdm_runtime_control::BtdmRuntimeControl::host(&mut old)
            .expect("block")
            .clear_bluetooth_low_power_controls_0_4();
        oer_esp32s31_pac::btdm_runtime_control::BtdmRuntimeControl::host(&mut new)
            .expect("block")
            .modify_word_0058_opaque(|w| {
                w.set_control_0(false);
                w.set_control_4(false);
            });
        assert_eq!(old, new, "clear_bluetooth_low_power_controls_0_4");
    }
}

#[test]
fn clear_bluetooth_low_power_controls_0_4_1() {
    for fill in fills(oer_esp32s31_pac::btdm_runtime_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btdm_runtime_control::BtdmRuntimeControl::host(&mut old)
            .expect("block")
            .clear_bluetooth_low_power_controls_0_4();
        oer_esp32s31_pac::btdm_runtime_control::BtdmRuntimeControl::host(&mut new)
            .expect("block")
            .modify_word_0058_opaque(|w| {
                w.set_control_0(false);
                w.set_control_4(false);
            });
        assert_eq!(old, new, "clear_bluetooth_low_power_controls_0_4");
    }
}

#[test]
fn clear_bluetooth_memory_list_1_initial_control_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_memory_list_1_initial_control();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_global_rxlink_1_current(|w| {
                w.set_rx_valid(false);
            });
        assert_eq!(old, new, "clear_bluetooth_memory_list_1_initial_control");
    }
}

#[test]
fn clear_bluetooth_memory_list_1_initial_control_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_memory_list_1_initial_control();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_global_rxlink_1_current(|w| {
                w.set_rx_valid(false);
            });
        assert_eq!(old, new, "clear_bluetooth_memory_list_1_initial_control");
    }
}

#[test]
fn clear_bluetooth_memory_list_2_initial_control_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_memory_list_2_initial_control();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_global_rxlink_2_current(|w| {
                w.set_rx_valid(false);
            });
        assert_eq!(old, new, "clear_bluetooth_memory_list_2_initial_control");
    }
}

#[test]
fn clear_bluetooth_memory_list_2_initial_control_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_memory_list_2_initial_control();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_global_rxlink_2_current(|w| {
                w.set_rx_valid(false);
            });
        assert_eq!(old, new, "clear_bluetooth_memory_list_2_initial_control");
    }
}

#[test]
fn clear_bluetooth_receive_saturation_gain_cte_bit_3_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .clear_bluetooth_receive_saturation_gain_cte_bit_3();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_cte_re_gain(|w| {
                w.set_bit_3_opaque(false);
            });
        assert_eq!(
            old, new,
            "clear_bluetooth_receive_saturation_gain_cte_bit_3"
        );
    }
}

#[test]
fn clear_bluetooth_receive_saturation_gain_cte_bit_3_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .clear_bluetooth_receive_saturation_gain_cte_bit_3();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_cte_re_gain(|w| {
                w.set_bit_3_opaque(false);
            });
        assert_eq!(
            old, new,
            "clear_bluetooth_receive_saturation_gain_cte_bit_3"
        );
    }
}

#[test]
fn clear_bluetooth_scheduler_cancellation_control_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_scheduler_cancellation_control();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_word_0204_opaque(|w| {
                w.set_control_0(false);
            });
        assert_eq!(old, new, "clear_bluetooth_scheduler_cancellation_control");
    }
}

#[test]
fn clear_bluetooth_scheduler_cancellation_control_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_scheduler_cancellation_control();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_word_0204_opaque(|w| {
                w.set_control_0(false);
            });
        assert_eq!(old, new, "clear_bluetooth_scheduler_cancellation_control");
    }
}

#[test]
fn clear_bluetooth_scheduler_cancellation_hardware_list_index_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_scheduler_cancellation_hardware_list_index();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_word_036c_opaque(|w| {
                w.set_cancellation_hardware_list_index(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(
            old, new,
            "clear_bluetooth_scheduler_cancellation_hardware_list_index"
        );
    }
}

#[test]
fn clear_bluetooth_scheduler_cancellation_hardware_list_index_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_scheduler_cancellation_hardware_list_index();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_word_036c_opaque(|w| {
                w.set_cancellation_hardware_list_index(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(
            old, new,
            "clear_bluetooth_scheduler_cancellation_hardware_list_index"
        );
    }
}

#[test]
fn clear_bluetooth_scheduler_hardware_list_head_0() {
    for fill in fills(oer_esp32s31_pac::btdm_scheduler_table::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btdm_scheduler_table::BtdmSchedulerTable::host(&mut old)
            .expect("block")
            .clear_bluetooth_scheduler_hardware_list_head(oer_mmio::Index::constant::<15>());
        oer_esp32s31_pac::btdm_scheduler_table::BtdmSchedulerTable::host(&mut new)
            .expect("block")
            .modify_entry(oer_mmio::Index::constant::<15>(), |w| {
                w.set_head_pointer(
                    oer_esp32s31_pac::btdm_scheduler_table::entry::HeadPointerImage::NULL,
                );
            });
        assert_eq!(old, new, "clear_bluetooth_scheduler_hardware_list_head");
    }
}

#[test]
fn clear_bluetooth_scheduler_hardware_list_head_1() {
    for fill in fills(oer_esp32s31_pac::btdm_scheduler_table::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btdm_scheduler_table::BtdmSchedulerTable::host(&mut old)
            .expect("block")
            .clear_bluetooth_scheduler_hardware_list_head(oer_mmio::Index::constant::<15>());
        oer_esp32s31_pac::btdm_scheduler_table::BtdmSchedulerTable::host(&mut new)
            .expect("block")
            .modify_entry(oer_mmio::Index::constant::<15>(), |w| {
                w.set_head_pointer(
                    oer_esp32s31_pac::btdm_scheduler_table::entry::HeadPointerImage::NULL,
                );
            });
        assert_eq!(old, new, "clear_bluetooth_scheduler_hardware_list_head");
    }
}

#[test]
fn clear_bluetooth_scheduler_insertion_command_0_start_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_scheduler_insertion_command_0_start();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_scheduler_command_0(|w| {
                w.set_start(false);
            });
        assert_eq!(
            old, new,
            "clear_bluetooth_scheduler_insertion_command_0_start"
        );
    }
}

#[test]
fn clear_bluetooth_scheduler_insertion_command_0_start_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_scheduler_insertion_command_0_start();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_scheduler_command_0(|w| {
                w.set_start(false);
            });
        assert_eq!(
            old, new,
            "clear_bluetooth_scheduler_insertion_command_0_start"
        );
    }
}

#[test]
fn clear_bluetooth_scheduler_insertion_command_1_start_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_scheduler_insertion_command_1_start();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_scheduler_command_1(|w| {
                w.set_start(false);
            });
        assert_eq!(
            old, new,
            "clear_bluetooth_scheduler_insertion_command_1_start"
        );
    }
}

#[test]
fn clear_bluetooth_scheduler_insertion_command_1_start_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_scheduler_insertion_command_1_start();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_scheduler_command_1(|w| {
                w.set_start(false);
            });
        assert_eq!(
            old, new,
            "clear_bluetooth_scheduler_insertion_command_1_start"
        );
    }
}

#[test]
fn clear_bluetooth_scheduler_lock_modify_hardware_list_index_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_scheduler_lock_modify_hardware_list_index();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_word_036c_opaque(|w| {
                w.set_lock_modify_hardware_list_index(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(
            old, new,
            "clear_bluetooth_scheduler_lock_modify_hardware_list_index"
        );
    }
}

#[test]
fn clear_bluetooth_scheduler_lock_modify_hardware_list_index_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .clear_bluetooth_scheduler_lock_modify_hardware_list_index();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_word_036c_opaque(|w| {
                w.set_lock_modify_hardware_list_index(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(
            old, new,
            "clear_bluetooth_scheduler_lock_modify_hardware_list_index"
        );
    }
}

#[test]
fn configure_bluetooth_cte_six_buffer_limit_0() {
    for fill in fills(oer_esp32s31_pac::ble_hw_cte_ring_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_cte_ring_control::BleHwCteRingControl::host(&mut old)
            .expect("block")
            .configure_bluetooth_cte_six_buffer_limit();
        oer_esp32s31_pac::ble_hw_cte_ring_control::BleHwCteRingControl::host(&mut new)
            .expect("block")
            .modify_ring_control(|w| {
                w.set_max_buffer_number(oer_mmio::Bits::constant::<0x6>());
            });
        assert_eq!(old, new, "configure_bluetooth_cte_six_buffer_limit");
    }
}

#[test]
fn configure_bluetooth_cte_six_buffer_limit_1() {
    for fill in fills(oer_esp32s31_pac::ble_hw_cte_ring_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_cte_ring_control::BleHwCteRingControl::host(&mut old)
            .expect("block")
            .configure_bluetooth_cte_six_buffer_limit();
        oer_esp32s31_pac::ble_hw_cte_ring_control::BleHwCteRingControl::host(&mut new)
            .expect("block")
            .modify_ring_control(|w| {
                w.set_max_buffer_number(oer_mmio::Bits::constant::<0x6>());
            });
        assert_eq!(old, new, "configure_bluetooth_cte_six_buffer_limit");
    }
}

#[test]
fn configure_bluetooth_hal_control_1_high_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .configure_bluetooth_hal_control_1_high();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_wdt_config_1(|w| {
                w.set_config_15(true);
                w.set_config_18_19(oer_mmio::Bits::constant::<0x3>());
            });
        assert_eq!(old, new, "configure_bluetooth_hal_control_1_high");
    }
}

#[test]
fn configure_bluetooth_hal_control_1_high_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .configure_bluetooth_hal_control_1_high();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_wdt_config_1(|w| {
                w.set_config_15(true);
                w.set_config_18_19(oer_mmio::Bits::constant::<0x3>());
            });
        assert_eq!(old, new, "configure_bluetooth_hal_control_1_high");
    }
}

#[test]
fn configure_bluetooth_hal_control_1_low_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .configure_bluetooth_hal_control_1_low();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_wdt_config_1(|w| {
                w.set_config_3(true);
                w.set_config_6_7(oer_mmio::Bits::constant::<0x3>());
            });
        assert_eq!(old, new, "configure_bluetooth_hal_control_1_low");
    }
}

#[test]
fn configure_bluetooth_hal_control_1_low_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .configure_bluetooth_hal_control_1_low();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_wdt_config_1(|w| {
                w.set_config_3(true);
                w.set_config_6_7(oer_mmio::Bits::constant::<0x3>());
            });
        assert_eq!(old, new, "configure_bluetooth_hal_control_1_low");
    }
}

#[test]
fn disable_ble_scheduler_run_event_source_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .disable_ble_scheduler_run_event_source();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_interrupt_enable(|w| {
                w.set_scheduler_run(false);
            });
        assert_eq!(old, new, "disable_ble_scheduler_run_event_source");
    }
}

#[test]
fn disable_ble_scheduler_run_event_source_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .disable_ble_scheduler_run_event_source();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_interrupt_enable(|w| {
                w.set_scheduler_run(false);
            });
        assert_eq!(old, new, "disable_ble_scheduler_run_event_source");
    }
}

#[test]
fn disable_bluetooth_phy_etm_channel2_0() {
    for fill in fills(oer_esp32s31_pac::modem_etm::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::modem_etm::ModemEtm::host(&mut old)
            .expect("block")
            .divide()
            .1
            .disable_bluetooth_phy_etm_channel2();
        oer_esp32s31_pac::modem_etm::ModemEtm::host(&mut new)
            .expect("block")
            .divide()
            .1
            .trigger_channel_enable_clear_ch2();
        assert_eq!(old, new, "disable_bluetooth_phy_etm_channel2");
    }
}

#[test]
fn disable_bluetooth_phy_etm_channel2_1() {
    for fill in fills(oer_esp32s31_pac::modem_etm::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::modem_etm::ModemEtm::host(&mut old)
            .expect("block")
            .divide()
            .1
            .disable_bluetooth_phy_etm_channel2();
        oer_esp32s31_pac::modem_etm::ModemEtm::host(&mut new)
            .expect("block")
            .divide()
            .1
            .trigger_channel_enable_clear_ch2();
        assert_eq!(old, new, "disable_bluetooth_phy_etm_channel2");
    }
}

#[test]
fn enable_ble_phy_access_address_low_correlation_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .enable_ble_phy_access_address_low_correlation();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_le_correlation_threshold_0(|w| {
                w.set_aa_lc_enable(true);
            });
        assert_eq!(old, new, "enable_ble_phy_access_address_low_correlation");
    }
}

#[test]
fn enable_ble_phy_access_address_low_correlation_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .enable_ble_phy_access_address_low_correlation();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_le_correlation_threshold_0(|w| {
                w.set_aa_lc_enable(true);
            });
        assert_eq!(old, new, "enable_ble_phy_access_address_low_correlation");
    }
}

#[test]
fn enable_ble_phy_init_control_0400_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .enable_ble_phy_init_control_0400();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0400_opaque(|w| {
                w.set_init_enable_22(true);
            });
        assert_eq!(old, new, "enable_ble_phy_init_control_0400");
    }
}

#[test]
fn enable_ble_phy_init_control_0400_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .enable_ble_phy_init_control_0400();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0400_opaque(|w| {
                w.set_init_enable_22(true);
            });
        assert_eq!(old, new, "enable_ble_phy_init_control_0400");
    }
}

#[test]
fn enable_ble_scheduler_run_event_source_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .enable_ble_scheduler_run_event_source();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_interrupt_enable(|w| {
                w.set_scheduler_run(true);
            });
        assert_eq!(old, new, "enable_ble_scheduler_run_event_source");
    }
}

#[test]
fn enable_ble_scheduler_run_event_source_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .enable_ble_scheduler_run_event_source();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_interrupt_enable(|w| {
                w.set_scheduler_run(true);
            });
        assert_eq!(old, new, "enable_ble_scheduler_run_event_source");
    }
}

#[test]
fn enable_bluetooth_hal_control_0_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .enable_bluetooth_hal_control_0();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_wdt_config_0(|w| {
                w.set_enable(true);
            });
        assert_eq!(old, new, "enable_bluetooth_hal_control_0");
    }
}

#[test]
fn enable_bluetooth_hal_control_0_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .enable_bluetooth_hal_control_0();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_wdt_config_0(|w| {
                w.set_enable(true);
            });
        assert_eq!(old, new, "enable_bluetooth_hal_control_0");
    }
}

#[test]
fn enable_bluetooth_hal_latch_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .enable_bluetooth_hal_latch();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_latch(|w| {
                w.set_enable_31(true);
            });
        assert_eq!(old, new, "enable_bluetooth_hal_latch");
    }
}

#[test]
fn enable_bluetooth_hal_latch_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .enable_bluetooth_hal_latch();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_latch(|w| {
                w.set_enable_31(true);
            });
        assert_eq!(old, new, "enable_bluetooth_hal_latch");
    }
}

#[test]
fn enable_bluetooth_hal_scheduler_control_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .enable_bluetooth_hal_scheduler_control();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_scheduler_control(|w| {
                w.set_enable_31(true);
            });
        assert_eq!(old, new, "enable_bluetooth_hal_scheduler_control");
    }
}

#[test]
fn enable_bluetooth_hal_scheduler_control_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .enable_bluetooth_hal_scheduler_control();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_scheduler_control(|w| {
                w.set_enable_31(true);
            });
        assert_eq!(old, new, "enable_bluetooth_hal_scheduler_control");
    }
}

#[test]
fn enable_bluetooth_phy_etm_channel2_0() {
    for fill in fills(oer_esp32s31_pac::modem_etm::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::modem_etm::ModemEtm::host(&mut old)
            .expect("block")
            .divide()
            .1
            .enable_bluetooth_phy_etm_channel2();
        oer_esp32s31_pac::modem_etm::ModemEtm::host(&mut new)
            .expect("block")
            .divide()
            .1
            .trigger_channel_enable_set_ch2();
        assert_eq!(old, new, "enable_bluetooth_phy_etm_channel2");
    }
}

#[test]
fn enable_bluetooth_phy_etm_channel2_1() {
    for fill in fills(oer_esp32s31_pac::modem_etm::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::modem_etm::ModemEtm::host(&mut old)
            .expect("block")
            .divide()
            .1
            .enable_bluetooth_phy_etm_channel2();
        oer_esp32s31_pac::modem_etm::ModemEtm::host(&mut new)
            .expect("block")
            .divide()
            .1
            .trigger_channel_enable_set_ch2();
        assert_eq!(old, new, "enable_bluetooth_phy_etm_channel2");
    }
}

#[test]
fn enable_bluetooth_primary_baseline_bank_0_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_interrupt_bank::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut old)
            .expect("block")
            .enable_bluetooth_primary_baseline_bank_0();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut new)
            .expect("block")
            .modify_irq_enable_0(|w| {
                w.set_source_15(true);
            });
        assert_eq!(old, new, "enable_bluetooth_primary_baseline_bank_0");
    }
}

#[test]
fn enable_bluetooth_primary_baseline_bank_0_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_interrupt_bank::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut old)
            .expect("block")
            .enable_bluetooth_primary_baseline_bank_0();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut new)
            .expect("block")
            .modify_irq_enable_0(|w| {
                w.set_source_15(true);
            });
        assert_eq!(old, new, "enable_bluetooth_primary_baseline_bank_0");
    }
}

#[test]
fn enable_bluetooth_primary_baseline_bank_1_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_interrupt_bank::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut old)
            .expect("block")
            .enable_bluetooth_primary_baseline_bank_1();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut new)
            .expect("block")
            .modify_irq_enable_1(|w| {
                w.set_source_8(true);
                w.set_source_9(true);
                w.set_source_12(true);
            });
        assert_eq!(old, new, "enable_bluetooth_primary_baseline_bank_1");
    }
}

#[test]
fn enable_bluetooth_primary_baseline_bank_1_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_interrupt_bank::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut old)
            .expect("block")
            .enable_bluetooth_primary_baseline_bank_1();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut new)
            .expect("block")
            .modify_irq_enable_1(|w| {
                w.set_source_8(true);
                w.set_source_9(true);
                w.set_source_12(true);
            });
        assert_eq!(old, new, "enable_bluetooth_primary_baseline_bank_1");
    }
}

#[test]
fn enable_bluetooth_scheduler_run_interrupts_bank_0_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_interrupt_bank::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut old)
            .expect("block")
            .enable_bluetooth_scheduler_run_interrupts_bank_0();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut new)
            .expect("block")
            .modify_irq_enable_0(|w| {
                w.set_source_21(true);
                w.set_sources_27_28(oer_mmio::Bits::truncate(w.sources_27_28().get() | 0x3));
            });
        assert_eq!(old, new, "enable_bluetooth_scheduler_run_interrupts_bank_0");
    }
}

#[test]
fn enable_bluetooth_scheduler_run_interrupts_bank_0_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_interrupt_bank::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut old)
            .expect("block")
            .enable_bluetooth_scheduler_run_interrupts_bank_0();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut new)
            .expect("block")
            .modify_irq_enable_0(|w| {
                w.set_source_21(true);
                w.set_sources_27_28(oer_mmio::Bits::truncate(w.sources_27_28().get() | 0x3));
            });
        assert_eq!(old, new, "enable_bluetooth_scheduler_run_interrupts_bank_0");
    }
}

#[test]
fn enable_bluetooth_scheduler_run_interrupts_bank_1_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_interrupt_bank::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut old)
            .expect("block")
            .enable_bluetooth_scheduler_run_interrupts_bank_1();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut new)
            .expect("block")
            .modify_irq_enable_1(|w| {
                w.set_source_3(true);
            });
        assert_eq!(old, new, "enable_bluetooth_scheduler_run_interrupts_bank_1");
    }
}

#[test]
fn enable_bluetooth_scheduler_run_interrupts_bank_1_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_interrupt_bank::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut old)
            .expect("block")
            .enable_bluetooth_scheduler_run_interrupts_bank_1();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut new)
            .expect("block")
            .modify_irq_enable_1(|w| {
                w.set_source_3(true);
            });
        assert_eq!(old, new, "enable_bluetooth_scheduler_run_interrupts_bank_1");
    }
}

#[test]
fn fill_bluetooth_hal_low_half_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .fill_bluetooth_hal_low_half();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_low_half(|w| {
                w.set_config_low_16(0xffff);
            });
        assert_eq!(old, new, "fill_bluetooth_hal_low_half");
    }
}

#[test]
fn fill_bluetooth_hal_low_half_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .fill_bluetooth_hal_low_half();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_low_half(|w| {
                w.set_config_low_16(0xffff);
            });
        assert_eq!(old, new, "fill_bluetooth_hal_low_half");
    }
}

#[test]
fn initialize_bluetooth_baseband_cca_bit_23_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_baseband_cca_bit_23();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_cca_config(|w| {
                w.set_bit_23_opaque(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_baseband_cca_bit_23");
    }
}

#[test]
fn initialize_bluetooth_baseband_cca_bit_23_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_baseband_cca_bit_23();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_cca_config(|w| {
                w.set_bit_23_opaque(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_baseband_cca_bit_23");
    }
}

#[test]
fn initialize_bluetooth_baseband_cca_default_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_baseband_cca_default();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_tx_cca_control_1(|w| {
                w.set_default_config_value(oer_mmio::Bits::constant::<0x5>());
            });
        assert_eq!(old, new, "initialize_bluetooth_baseband_cca_default");
    }
}

#[test]
fn initialize_bluetooth_baseband_cca_default_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_baseband_cca_default();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_tx_cca_control_1(|w| {
                w.set_default_config_value(oer_mmio::Bits::constant::<0x5>());
            });
        assert_eq!(old, new, "initialize_bluetooth_baseband_cca_default");
    }
}

#[test]
fn initialize_bluetooth_baseband_cca_value_0_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_baseband_cca_value_0();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_cca_config(|w| {
                w.set_config_value_0(oer_mmio::Bits::constant::<0x1ce>());
            });
        assert_eq!(old, new, "initialize_bluetooth_baseband_cca_value_0");
    }
}

#[test]
fn initialize_bluetooth_baseband_cca_value_0_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_baseband_cca_value_0();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_cca_config(|w| {
                w.set_config_value_0(oer_mmio::Bits::constant::<0x1ce>());
            });
        assert_eq!(old, new, "initialize_bluetooth_baseband_cca_value_0");
    }
}

#[test]
fn initialize_bluetooth_baseband_cca_value_1_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_baseband_cca_value_1();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_cca_config(|w| {
                w.set_config_value_1(oer_mmio::Bits::constant::<0x1e>());
            });
        assert_eq!(old, new, "initialize_bluetooth_baseband_cca_value_1");
    }
}

#[test]
fn initialize_bluetooth_baseband_cca_value_1_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_baseband_cca_value_1();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_cca_config(|w| {
                w.set_config_value_1(oer_mmio::Bits::constant::<0x1e>());
            });
        assert_eq!(old, new, "initialize_bluetooth_baseband_cca_value_1");
    }
}

#[test]
fn initialize_bluetooth_baseband_coexistence_18_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_baseband_coexistence_18();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_coex_config(|w| {
                w.set_bit_18_opaque(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_baseband_coexistence_18");
    }
}

#[test]
fn initialize_bluetooth_baseband_coexistence_18_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_baseband_coexistence_18();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_coex_config(|w| {
                w.set_bit_18_opaque(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_baseband_coexistence_18");
    }
}

#[test]
fn initialize_bluetooth_baseband_coexistence_20_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_baseband_coexistence_20();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_coex_config(|w| {
                w.set_bit_20_opaque(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_baseband_coexistence_20");
    }
}

#[test]
fn initialize_bluetooth_baseband_coexistence_20_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_baseband_coexistence_20();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_coex_config(|w| {
                w.set_bit_20_opaque(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_baseband_coexistence_20");
    }
}

#[test]
fn initialize_bluetooth_baseband_tx_argument_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_baseband_tx_argument();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_setup_argument(|w| {
                w.set_tx_argument_0(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "initialize_bluetooth_baseband_tx_argument");
    }
}

#[test]
fn initialize_bluetooth_baseband_tx_argument_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_baseband_tx_argument();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_setup_argument(|w| {
                w.set_tx_argument_0(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "initialize_bluetooth_baseband_tx_argument");
    }
}

#[test]
fn initialize_bluetooth_baseband_tx_low_byte_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_baseband_tx_low_byte();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_2m_coefficient_and_tx_config(|w| {
                w.set_tx_set_low_byte(0xf5);
            });
        assert_eq!(old, new, "initialize_bluetooth_baseband_tx_low_byte");
    }
}

#[test]
fn initialize_bluetooth_baseband_tx_low_byte_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_baseband_tx_low_byte();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_2m_coefficient_and_tx_config(|w| {
                w.set_tx_set_low_byte(0xf5);
            });
        assert_eq!(old, new, "initialize_bluetooth_baseband_tx_low_byte");
    }
}

#[test]
fn initialize_bluetooth_baseband_tx_setup_image_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_baseband_tx_setup_image();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_setup_image_0(|w| {
                w.set_tx_setup_image(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "initialize_bluetooth_baseband_tx_setup_image");
    }
}

#[test]
fn initialize_bluetooth_baseband_tx_setup_image_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_baseband_tx_setup_image();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_setup_image_0(|w| {
                w.set_tx_setup_image(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "initialize_bluetooth_baseband_tx_setup_image");
    }
}

#[test]
fn initialize_bluetooth_gaussian_1m_0_bits_10_16_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_1m_0_bits_10_16();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_1m_coefficient_0(|w| {
                w.set_bits_10_16_opaque(oer_mmio::Bits::constant::<0x3>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_1m_0_bits_10_16");
    }
}

#[test]
fn initialize_bluetooth_gaussian_1m_0_bits_10_16_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_1m_0_bits_10_16();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_1m_coefficient_0(|w| {
                w.set_bits_10_16_opaque(oer_mmio::Bits::constant::<0x3>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_1m_0_bits_10_16");
    }
}

#[test]
fn initialize_bluetooth_gaussian_1m_0_bits_17_22_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_1m_0_bits_17_22();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_1m_coefficient_0(|w| {
                w.set_bits_17_22_opaque(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_1m_0_bits_17_22");
    }
}

#[test]
fn initialize_bluetooth_gaussian_1m_0_bits_17_22_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_1m_0_bits_17_22();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_1m_coefficient_0(|w| {
                w.set_bits_17_22_opaque(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_1m_0_bits_17_22");
    }
}

#[test]
fn initialize_bluetooth_gaussian_1m_0_bits_23_27_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_1m_0_bits_23_27();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_1m_coefficient_0(|w| {
                w.set_bits_23_27_opaque(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_1m_0_bits_23_27");
    }
}

#[test]
fn initialize_bluetooth_gaussian_1m_0_bits_23_27_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_1m_0_bits_23_27();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_1m_coefficient_0(|w| {
                w.set_bits_23_27_opaque(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_1m_0_bits_23_27");
    }
}

#[test]
fn initialize_bluetooth_gaussian_1m_0_bits_28_31_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_1m_0_bits_28_31();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_1m_coefficient_0(|w| {
                w.set_bits_28_31_opaque(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_1m_0_bits_28_31");
    }
}

#[test]
fn initialize_bluetooth_gaussian_1m_0_bits_28_31_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_1m_0_bits_28_31();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_1m_coefficient_0(|w| {
                w.set_bits_28_31_opaque(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_1m_0_bits_28_31");
    }
}

#[test]
fn initialize_bluetooth_gaussian_1m_0_bits_2_9_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_1m_0_bits_2_9();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_1m_coefficient_0(|w| {
                w.set_bits_2_9_opaque(0x13);
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_1m_0_bits_2_9");
    }
}

#[test]
fn initialize_bluetooth_gaussian_1m_0_bits_2_9_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_1m_0_bits_2_9();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_1m_coefficient_0(|w| {
                w.set_bits_2_9_opaque(0x13);
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_1m_0_bits_2_9");
    }
}

#[test]
fn initialize_bluetooth_gaussian_1m_1_bits_13_22_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_1m_1_bits_13_22();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_1m_coefficient_1(|w| {
                w.set_bits_13_22_opaque(oer_mmio::Bits::constant::<0x140>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_1m_1_bits_13_22");
    }
}

#[test]
fn initialize_bluetooth_gaussian_1m_1_bits_13_22_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_1m_1_bits_13_22();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_1m_coefficient_1(|w| {
                w.set_bits_13_22_opaque(oer_mmio::Bits::constant::<0x140>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_1m_1_bits_13_22");
    }
}

#[test]
fn initialize_bluetooth_gaussian_1m_1_bits_23_31_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_1m_1_bits_23_31();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_1m_coefficient_1(|w| {
                w.set_bits_23_31_opaque(oer_mmio::Bits::constant::<0x5f>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_1m_1_bits_23_31");
    }
}

#[test]
fn initialize_bluetooth_gaussian_1m_1_bits_23_31_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_1m_1_bits_23_31();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_1m_coefficient_1(|w| {
                w.set_bits_23_31_opaque(oer_mmio::Bits::constant::<0x5f>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_1m_1_bits_23_31");
    }
}

#[test]
fn initialize_bluetooth_gaussian_1m_1_bits_2_12_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_1m_1_bits_2_12();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_1m_coefficient_1(|w| {
                w.set_bits_2_12_opaque(oer_mmio::Bits::constant::<0x2f2>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_1m_1_bits_2_12");
    }
}

#[test]
fn initialize_bluetooth_gaussian_1m_1_bits_2_12_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_1m_1_bits_2_12();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_1m_coefficient_1(|w| {
                w.set_bits_2_12_opaque(oer_mmio::Bits::constant::<0x2f2>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_1m_1_bits_2_12");
    }
}

#[test]
fn initialize_bluetooth_gaussian_1m_2_bits_10_20_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_1m_2_bits_10_20();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_1m_coefficient_2(|w| {
                w.set_bits_10_20_opaque(oer_mmio::Bits::constant::<0x6bf>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_1m_2_bits_10_20");
    }
}

#[test]
fn initialize_bluetooth_gaussian_1m_2_bits_10_20_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_1m_2_bits_10_20();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_1m_coefficient_2(|w| {
                w.set_bits_10_20_opaque(oer_mmio::Bits::constant::<0x6bf>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_1m_2_bits_10_20");
    }
}

#[test]
fn initialize_bluetooth_gaussian_1m_2_bits_21_31_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_1m_2_bits_21_31();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_1m_coefficient_2(|w| {
                w.set_bits_21_31_opaque(oer_mmio::Bits::constant::<0x50d>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_1m_2_bits_21_31");
    }
}

#[test]
fn initialize_bluetooth_gaussian_1m_2_bits_21_31_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_1m_2_bits_21_31();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_1m_coefficient_2(|w| {
                w.set_bits_21_31_opaque(oer_mmio::Bits::constant::<0x50d>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_1m_2_bits_21_31");
    }
}

#[test]
fn initialize_bluetooth_gaussian_1m_3_bits_10_20_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_1m_3_bits_10_20();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_1m_coefficient_3(|w| {
                w.set_bits_10_20_opaque(oer_mmio::Bits::constant::<0x7e9>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_1m_3_bits_10_20");
    }
}

#[test]
fn initialize_bluetooth_gaussian_1m_3_bits_10_20_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_1m_3_bits_10_20();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_1m_coefficient_3(|w| {
                w.set_bits_10_20_opaque(oer_mmio::Bits::constant::<0x7e9>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_1m_3_bits_10_20");
    }
}

#[test]
fn initialize_bluetooth_gaussian_1m_3_bits_21_31_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_1m_3_bits_21_31();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_1m_coefficient_3(|w| {
                w.set_bits_21_31_opaque(oer_mmio::Bits::constant::<0x7a0>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_1m_3_bits_21_31");
    }
}

#[test]
fn initialize_bluetooth_gaussian_1m_3_bits_21_31_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_1m_3_bits_21_31();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_1m_coefficient_3(|w| {
                w.set_bits_21_31_opaque(oer_mmio::Bits::constant::<0x7a0>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_1m_3_bits_21_31");
    }
}

#[test]
fn initialize_bluetooth_gaussian_2m_0_bits_17_24_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_2m_0_bits_17_24();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_2m_coefficient_and_tx_config(|w| {
                w.set_gaussian_bits_17_24(0x7);
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_2m_0_bits_17_24");
    }
}

#[test]
fn initialize_bluetooth_gaussian_2m_0_bits_17_24_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_2m_0_bits_17_24();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_2m_coefficient_and_tx_config(|w| {
                w.set_gaussian_bits_17_24(0x7);
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_2m_0_bits_17_24");
    }
}

#[test]
fn initialize_bluetooth_gaussian_2m_0_bits_25_31_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_2m_0_bits_25_31();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_2m_coefficient_and_tx_config(|w| {
                w.set_gaussian_bits_25_31(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_2m_0_bits_25_31");
    }
}

#[test]
fn initialize_bluetooth_gaussian_2m_0_bits_25_31_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_2m_0_bits_25_31();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_2m_coefficient_and_tx_config(|w| {
                w.set_gaussian_bits_25_31(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_2m_0_bits_25_31");
    }
}

#[test]
fn initialize_bluetooth_gaussian_2m_0_bits_8_16_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_2m_0_bits_8_16();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_2m_coefficient_and_tx_config(|w| {
                w.set_gaussian_bits_8_16(oer_mmio::Bits::constant::<0x67>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_2m_0_bits_8_16");
    }
}

#[test]
fn initialize_bluetooth_gaussian_2m_0_bits_8_16_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_2m_0_bits_8_16();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_2m_coefficient_and_tx_config(|w| {
                w.set_gaussian_bits_8_16(oer_mmio::Bits::constant::<0x67>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_2m_0_bits_8_16");
    }
}

#[test]
fn initialize_bluetooth_gaussian_2m_1_bits_0_10_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_2m_1_bits_0_10();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_2m_coefficient_1(|w| {
                w.set_bits_0_10_opaque(oer_mmio::Bits::constant::<0x76d>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_2m_1_bits_0_10");
    }
}

#[test]
fn initialize_bluetooth_gaussian_2m_1_bits_0_10_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_2m_1_bits_0_10();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_2m_coefficient_1(|w| {
                w.set_bits_0_10_opaque(oer_mmio::Bits::constant::<0x76d>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_2m_1_bits_0_10");
    }
}

#[test]
fn initialize_bluetooth_gaussian_2m_1_bits_11_21_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_2m_1_bits_11_21();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_2m_coefficient_1(|w| {
                w.set_bits_11_21_opaque(oer_mmio::Bits::constant::<0x58e>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_2m_1_bits_11_21");
    }
}

#[test]
fn initialize_bluetooth_gaussian_2m_1_bits_11_21_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_2m_1_bits_11_21();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_2m_coefficient_1(|w| {
                w.set_bits_11_21_opaque(oer_mmio::Bits::constant::<0x58e>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_2m_1_bits_11_21");
    }
}

#[test]
fn initialize_bluetooth_gaussian_2m_1_bits_22_31_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_2m_1_bits_22_31();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_2m_coefficient_1(|w| {
                w.set_bits_22_31_opaque(oer_mmio::Bits::constant::<0x24d>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_2m_1_bits_22_31");
    }
}

#[test]
fn initialize_bluetooth_gaussian_2m_1_bits_22_31_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_gaussian_2m_1_bits_22_31();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_gaussian_2m_coefficient_1(|w| {
                w.set_bits_22_31_opaque(oer_mmio::Bits::constant::<0x24d>());
            });
        assert_eq!(old, new, "initialize_bluetooth_gaussian_2m_1_bits_22_31");
    }
}

#[test]
fn initialize_bluetooth_hal_low_20_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .initialize_bluetooth_hal_low_20(oer_mmio::Bits::truncate(0xa55a_3cc3));
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .write_hal_init_low_20(|w| {
                w.set_config_low_20(oer_mmio::Bits::truncate(0xa55a_3cc3));
            });
        assert_eq!(old, new, "initialize_bluetooth_hal_low_20");
    }
}

#[test]
fn initialize_bluetooth_hal_low_20_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .initialize_bluetooth_hal_low_20(oer_mmio::Bits::truncate(0x1));
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .write_hal_init_low_20(|w| {
                w.set_config_low_20(oer_mmio::Bits::truncate(0x1));
            });
        assert_eq!(old, new, "initialize_bluetooth_hal_low_20");
    }
}

#[test]
fn initialize_bluetooth_le_tx_delay_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_le_tx_delay();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_le_tx_on_delay(|w| {
                w.set_bits_16_18_opaque(oer_mmio::Bits::constant::<0x0>());
                w.set_encoded_value_minus_10(0x1d);
            });
        assert_eq!(old, new, "initialize_bluetooth_le_tx_delay");
    }
}

#[test]
fn initialize_bluetooth_le_tx_delay_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_le_tx_delay();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_le_tx_on_delay(|w| {
                w.set_bits_16_18_opaque(oer_mmio::Bits::constant::<0x0>());
                w.set_encoded_value_minus_10(0x1d);
            });
        assert_eq!(old, new, "initialize_bluetooth_le_tx_delay");
    }
}

#[test]
fn initialize_bluetooth_receive_compensation_0_6_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_compensation_0_6();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_rx_comp_control(|w| {
                w.set_dynamic_bits_0_6(oer_mmio::Bits::constant::<0x6>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_compensation_0_6");
    }
}

#[test]
fn initialize_bluetooth_receive_compensation_0_6_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_compensation_0_6();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_rx_comp_control(|w| {
                w.set_dynamic_bits_0_6(oer_mmio::Bits::constant::<0x6>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_compensation_0_6");
    }
}

#[test]
fn initialize_bluetooth_receive_compensation_14_18_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_compensation_14_18();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_rx_comp_control(|w| {
                w.set_finite_bits_14_18(oer_mmio::Bits::constant::<0x8>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_compensation_14_18");
    }
}

#[test]
fn initialize_bluetooth_receive_compensation_14_18_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_compensation_14_18();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_rx_comp_control(|w| {
                w.set_finite_bits_14_18(oer_mmio::Bits::constant::<0x8>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_compensation_14_18");
    }
}

#[test]
fn initialize_bluetooth_receive_compensation_19_23_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_compensation_19_23();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_rx_comp_control(|w| {
                w.set_finite_bits_19_23(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_compensation_19_23");
    }
}

#[test]
fn initialize_bluetooth_receive_compensation_19_23_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_compensation_19_23();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_rx_comp_control(|w| {
                w.set_finite_bits_19_23(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_compensation_19_23");
    }
}

#[test]
fn initialize_bluetooth_receive_compensation_24_28_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_compensation_24_28();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_rx_comp_control(|w| {
                w.set_finite_bits_24_28(oer_mmio::Bits::constant::<0x6>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_compensation_24_28");
    }
}

#[test]
fn initialize_bluetooth_receive_compensation_24_28_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_compensation_24_28();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_rx_comp_control(|w| {
                w.set_finite_bits_24_28(oer_mmio::Bits::constant::<0x6>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_compensation_24_28");
    }
}

#[test]
fn initialize_bluetooth_receive_compensation_7_13_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_compensation_7_13();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_rx_comp_control(|w| {
                w.set_dynamic_bits_7_13(oer_mmio::Bits::constant::<0x6>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_compensation_7_13");
    }
}

#[test]
fn initialize_bluetooth_receive_compensation_7_13_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_compensation_7_13();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_rx_comp_control(|w| {
                w.set_dynamic_bits_7_13(oer_mmio::Bits::constant::<0x6>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_compensation_7_13");
    }
}

#[test]
fn initialize_bluetooth_receive_correlator_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_correlator();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_correlator_control(|w| {
                w.set_config_value(oer_mmio::Bits::constant::<0x8>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_correlator");
    }
}

#[test]
fn initialize_bluetooth_receive_correlator_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_correlator();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_correlator_control(|w| {
                w.set_config_value(oer_mmio::Bits::constant::<0x8>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_correlator");
    }
}

#[test]
fn initialize_bluetooth_receive_correlator_final_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_correlator_final();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_correlator_control(|w| {
                w.set_bit_13_opaque(false);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_correlator_final");
    }
}

#[test]
fn initialize_bluetooth_receive_correlator_final_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_correlator_final();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_correlator_control(|w| {
                w.set_bit_13_opaque(false);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_correlator_final");
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00a0_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00a0();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00a0_opaque(|w| {
                w.set_bits_24_27_opaque(oer_mmio::Bits::constant::<0x4>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_detection_00a0");
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00a0_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00a0();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00a0_opaque(|w| {
                w.set_bits_24_27_opaque(oer_mmio::Bits::constant::<0x4>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_detection_00a0");
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00a8_low_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00a8_low();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00a8_opaque(|w| {
                w.set_bits_0_3_opaque(oer_mmio::Bits::constant::<0x7>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_detection_00a8_low");
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00a8_low_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00a8_low();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00a8_opaque(|w| {
                w.set_bits_0_3_opaque(oer_mmio::Bits::constant::<0x7>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_detection_00a8_low");
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00a8_middle_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00a8_middle();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00a8_opaque(|w| {
                w.set_bits_12_16_opaque(oer_mmio::Bits::constant::<0x4>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_detection_00a8_middle"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00a8_middle_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00a8_middle();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00a8_opaque(|w| {
                w.set_bits_12_16_opaque(oer_mmio::Bits::constant::<0x4>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_detection_00a8_middle"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00b8_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00b8();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00b8_opaque(|w| {
                w.set_bits_8_11_opaque(oer_mmio::Bits::constant::<0x7>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_detection_00b8");
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00b8_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00b8();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00b8_opaque(|w| {
                w.set_bits_8_11_opaque(oer_mmio::Bits::constant::<0x7>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_detection_00b8");
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00c0_bits_10_14_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00c0_bits_10_14();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00c0_opaque(|w| {
                w.set_bits_10_14_opaque(oer_mmio::Bits::constant::<0xa>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_detection_00c0_bits_10_14"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00c0_bits_10_14_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00c0_bits_10_14();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00c0_opaque(|w| {
                w.set_bits_10_14_opaque(oer_mmio::Bits::constant::<0xa>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_detection_00c0_bits_10_14"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00c0_bits_15_19_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00c0_bits_15_19();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00c0_opaque(|w| {
                w.set_bits_15_19_opaque(oer_mmio::Bits::constant::<0xa>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_detection_00c0_bits_15_19"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00c0_bits_15_19_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00c0_bits_15_19();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00c0_opaque(|w| {
                w.set_bits_15_19_opaque(oer_mmio::Bits::constant::<0xa>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_detection_00c0_bits_15_19"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00c0_bits_20_23_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00c0_bits_20_23();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00c0_opaque(|w| {
                w.set_bits_20_23_opaque(oer_mmio::Bits::constant::<0x7>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_detection_00c0_bits_20_23"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00c0_bits_20_23_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00c0_bits_20_23();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00c0_opaque(|w| {
                w.set_bits_20_23_opaque(oer_mmio::Bits::constant::<0x7>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_detection_00c0_bits_20_23"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00c0_bits_24_31_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00c0_bits_24_31();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00c0_opaque(|w| {
                w.set_bits_24_31_opaque(0x9c);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_detection_00c0_bits_24_31"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00c0_bits_24_31_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00c0_bits_24_31();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00c0_opaque(|w| {
                w.set_bits_24_31_opaque(0x9c);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_detection_00c0_bits_24_31"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00c0_bits_5_9_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00c0_bits_5_9();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00c0_opaque(|w| {
                w.set_bits_5_9_opaque(oer_mmio::Bits::constant::<0xa>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_detection_00c0_bits_5_9"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00c0_bits_5_9_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00c0_bits_5_9();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00c0_opaque(|w| {
                w.set_bits_5_9_opaque(oer_mmio::Bits::constant::<0xa>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_detection_00c0_bits_5_9"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00c4_bits_10_14_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00c4_bits_10_14();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00c4_opaque(|w| {
                w.set_bits_10_14_opaque(oer_mmio::Bits::constant::<0xf>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_detection_00c4_bits_10_14"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00c4_bits_10_14_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00c4_bits_10_14();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00c4_opaque(|w| {
                w.set_bits_10_14_opaque(oer_mmio::Bits::constant::<0xf>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_detection_00c4_bits_10_14"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00c4_bits_15_19_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00c4_bits_15_19();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00c4_opaque(|w| {
                w.set_bits_15_19_opaque(oer_mmio::Bits::constant::<0xf>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_detection_00c4_bits_15_19"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00c4_bits_15_19_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00c4_bits_15_19();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00c4_opaque(|w| {
                w.set_bits_15_19_opaque(oer_mmio::Bits::constant::<0xf>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_detection_00c4_bits_15_19"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00c4_bits_20_23_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00c4_bits_20_23();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00c4_opaque(|w| {
                w.set_bits_20_23_opaque(oer_mmio::Bits::constant::<0xa>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_detection_00c4_bits_20_23"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00c4_bits_20_23_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00c4_bits_20_23();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00c4_opaque(|w| {
                w.set_bits_20_23_opaque(oer_mmio::Bits::constant::<0xa>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_detection_00c4_bits_20_23"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00c4_bits_24_31_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00c4_bits_24_31();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00c4_opaque(|w| {
                w.set_bits_24_31_opaque(0x9c);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_detection_00c4_bits_24_31"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_detection_00c4_bits_24_31_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_detection_00c4_bits_24_31();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00c4_opaque(|w| {
                w.set_bits_24_31_opaque(0x9c);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_detection_00c4_bits_24_31"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_dpo_bit_19_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_dpo_bit_19();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_dpo_control(|w| {
                w.set_bit_19_opaque(false);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_dpo_bit_19");
    }
}

#[test]
fn initialize_bluetooth_receive_dpo_bit_19_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_dpo_bit_19();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_dpo_control(|w| {
                w.set_bit_19_opaque(false);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_dpo_bit_19");
    }
}

#[test]
fn initialize_bluetooth_receive_dpo_value_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_dpo_value();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_dpo_control(|w| {
                w.set_config_value(oer_mmio::Bits::constant::<0x6>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_dpo_value");
    }
}

#[test]
fn initialize_bluetooth_receive_dpo_value_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_dpo_value();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_dpo_control(|w| {
                w.set_config_value(oer_mmio::Bits::constant::<0x6>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_dpo_value");
    }
}

#[test]
fn initialize_bluetooth_receive_filter_0_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_filter_0();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_filter_select(|w| {
                w.set_select_0(oer_mmio::Bits::constant::<0x2>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_filter_0");
    }
}

#[test]
fn initialize_bluetooth_receive_filter_0_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_filter_0();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_filter_select(|w| {
                w.set_select_0(oer_mmio::Bits::constant::<0x2>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_filter_0");
    }
}

#[test]
fn initialize_bluetooth_receive_filter_1_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_filter_1();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_filter_select(|w| {
                w.set_select_1(oer_mmio::Bits::constant::<0x2>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_filter_1");
    }
}

#[test]
fn initialize_bluetooth_receive_filter_1_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_filter_1();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_filter_select(|w| {
                w.set_select_1(oer_mmio::Bits::constant::<0x2>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_filter_1");
    }
}

#[test]
fn initialize_bluetooth_receive_filter_2_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_filter_2();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_filter_select(|w| {
                w.set_select_2(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_filter_2");
    }
}

#[test]
fn initialize_bluetooth_receive_filter_2_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_filter_2();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_filter_select(|w| {
                w.set_select_2(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_filter_2");
    }
}

#[test]
fn initialize_bluetooth_receive_filter_3_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_filter_3();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_filter_select(|w| {
                w.set_select_3(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_filter_3");
    }
}

#[test]
fn initialize_bluetooth_receive_filter_3_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_filter_3();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_filter_select(|w| {
                w.set_select_3(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_filter_3");
    }
}

#[test]
fn initialize_bluetooth_receive_filter_4_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_filter_4();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_filter_select(|w| {
                w.set_select_4(oer_mmio::Bits::constant::<0x2>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_filter_4");
    }
}

#[test]
fn initialize_bluetooth_receive_filter_4_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_filter_4();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_filter_select(|w| {
                w.set_select_4(oer_mmio::Bits::constant::<0x2>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_filter_4");
    }
}

#[test]
fn initialize_bluetooth_receive_filter_5_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_filter_5();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_filter_select(|w| {
                w.set_select_5(oer_mmio::Bits::constant::<0x1>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_filter_5");
    }
}

#[test]
fn initialize_bluetooth_receive_filter_5_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_filter_5();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_filter_select(|w| {
                w.set_select_5(oer_mmio::Bits::constant::<0x1>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_filter_5");
    }
}

#[test]
fn initialize_bluetooth_receive_gain_baseband_0() {
    for fill in fills(oer_esp32s31_pac::phy_baseband_config_oracle::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_baseband_config_oracle::PhyBasebandConfigOracle::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_gain_baseband();
        oer_esp32s31_pac::phy_baseband_config_oracle::PhyBasebandConfigOracle::host(&mut new)
            .expect("block")
            .modify_word_7cd0_opaque(|w| {
                w.set_bit_0_opaque(true);
                w.set_bit_1_opaque(true);
                w.set_bit_2_opaque(true);
                w.set_bit_3_opaque(true);
                w.set_bits_16_19_opaque(oer_mmio::Bits::constant::<0xf>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_gain_baseband");
    }
}

#[test]
fn initialize_bluetooth_receive_gain_baseband_1() {
    for fill in fills(oer_esp32s31_pac::phy_baseband_config_oracle::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_baseband_config_oracle::PhyBasebandConfigOracle::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_gain_baseband();
        oer_esp32s31_pac::phy_baseband_config_oracle::PhyBasebandConfigOracle::host(&mut new)
            .expect("block")
            .modify_word_7cd0_opaque(|w| {
                w.set_bit_0_opaque(true);
                w.set_bit_1_opaque(true);
                w.set_bit_2_opaque(true);
                w.set_bit_3_opaque(true);
                w.set_bits_16_19_opaque(oer_mmio::Bits::constant::<0xf>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_gain_baseband");
    }
}

#[test]
fn initialize_bluetooth_receive_gain_offset_0_bits_0_4_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_gain_offset_0_bits_0_4();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0048_opaque(|w| {
                w.set_bits_0_4_opaque(oer_mmio::Bits::constant::<0x4>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_gain_offset_0_bits_0_4"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_gain_offset_0_bits_0_4_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_gain_offset_0_bits_0_4();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0048_opaque(|w| {
                w.set_bits_0_4_opaque(oer_mmio::Bits::constant::<0x4>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_gain_offset_0_bits_0_4"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_gain_offset_0_bits_16_23_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_gain_offset_0_bits_16_23();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0048_opaque(|w| {
                w.set_gain_offset_0(0x50);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_gain_offset_0_bits_16_23"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_gain_offset_0_bits_16_23_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_gain_offset_0_bits_16_23();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0048_opaque(|w| {
                w.set_gain_offset_0(0x50);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_gain_offset_0_bits_16_23"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_gain_offset_0_bits_24_31_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_gain_offset_0_bits_24_31();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0048_opaque(|w| {
                w.set_gain_offset_1(0x50);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_gain_offset_0_bits_24_31"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_gain_offset_0_bits_24_31_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_gain_offset_0_bits_24_31();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0048_opaque(|w| {
                w.set_gain_offset_1(0x50);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_gain_offset_0_bits_24_31"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_gain_offset_0_bits_5_9_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_gain_offset_0_bits_5_9();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0048_opaque(|w| {
                w.set_bits_5_9_opaque(oer_mmio::Bits::constant::<0x4>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_gain_offset_0_bits_5_9"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_gain_offset_0_bits_5_9_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_gain_offset_0_bits_5_9();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0048_opaque(|w| {
                w.set_bits_5_9_opaque(oer_mmio::Bits::constant::<0x4>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_gain_offset_0_bits_5_9"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_gain_offset_1_bits_16_23_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_gain_offset_1_bits_16_23();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0068_opaque(|w| {
                w.set_gain_offset_0(0x50);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_gain_offset_1_bits_16_23"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_gain_offset_1_bits_16_23_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_gain_offset_1_bits_16_23();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0068_opaque(|w| {
                w.set_gain_offset_0(0x50);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_gain_offset_1_bits_16_23"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_gain_offset_1_bits_24_31_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_gain_offset_1_bits_24_31();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0068_opaque(|w| {
                w.set_gain_offset_1(0x50);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_gain_offset_1_bits_24_31"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_gain_offset_1_bits_24_31_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_gain_offset_1_bits_24_31();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0068_opaque(|w| {
                w.set_gain_offset_1(0x50);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_gain_offset_1_bits_24_31"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_recorrection_00b0_bits_13_17_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_recorrection_00b0_bits_13_17();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00b0_opaque(|w| {
                w.set_recorrect_bits_13_17(oer_mmio::Bits::constant::<0x18>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_recorrection_00b0_bits_13_17"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_recorrection_00b0_bits_13_17_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_recorrection_00b0_bits_13_17();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00b0_opaque(|w| {
                w.set_recorrect_bits_13_17(oer_mmio::Bits::constant::<0x18>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_recorrection_00b0_bits_13_17"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_recorrection_00b0_bits_18_22_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_recorrection_00b0_bits_18_22();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00b0_opaque(|w| {
                w.set_recorrect_bits_18_22(oer_mmio::Bits::constant::<0x18>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_recorrection_00b0_bits_18_22"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_recorrection_00b0_bits_18_22_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_recorrection_00b0_bits_18_22();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00b0_opaque(|w| {
                w.set_recorrect_bits_18_22(oer_mmio::Bits::constant::<0x18>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_recorrection_00b0_bits_18_22"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_recorrection_00b4_bit_8_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_recorrection_00b4_bit_8();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00b4_opaque(|w| {
                w.set_recorrect_bit_8(true);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_recorrection_00b4_bit_8"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_recorrection_00b4_bit_8_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_recorrection_00b4_bit_8();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00b4_opaque(|w| {
                w.set_recorrect_bit_8(true);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_recorrection_00b4_bit_8"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_recorrection_00b4_bits_9_13_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_recorrection_00b4_bits_9_13();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00b4_opaque(|w| {
                w.set_recorrect_bits_9_13(oer_mmio::Bits::constant::<0xa>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_recorrection_00b4_bits_9_13"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_recorrection_00b4_bits_9_13_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_recorrection_00b4_bits_9_13();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00b4_opaque(|w| {
                w.set_recorrect_bits_9_13(oer_mmio::Bits::constant::<0xa>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_recorrection_00b4_bits_9_13"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_recorrection_00bc_clear_pair_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_recorrection_00bc_clear_pair();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00bc_opaque(|w| {
                w.set_recorrect_bit_6(false);
                w.set_recorrect_bit_8(false);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_recorrection_00bc_clear_pair"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_recorrection_00bc_clear_pair_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_recorrection_00bc_clear_pair();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00bc_opaque(|w| {
                w.set_recorrect_bit_6(false);
                w.set_recorrect_bit_8(false);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_recorrection_00bc_clear_pair"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_recorrection_00bc_set_pair_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_recorrection_00bc_set_pair();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00bc_opaque(|w| {
                w.set_recorrect_bit_5(true);
                w.set_recorrect_bit_7(true);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_recorrection_00bc_set_pair"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_recorrection_00bc_set_pair_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_recorrection_00bc_set_pair();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00bc_opaque(|w| {
                w.set_recorrect_bit_5(true);
                w.set_recorrect_bit_7(true);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_recorrection_00bc_set_pair"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_recorrection_control_006c_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_recorrection_control_006c();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_006c_opaque(|w| {
                w.set_finite_bits_24_31(0xf);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_recorrection_control_006c"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_recorrection_control_006c_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_recorrection_control_006c();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_006c_opaque(|w| {
                w.set_finite_bits_24_31(0xf);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_recorrection_control_006c"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_recorrection_gain_bit_13_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_recorrection_gain_bit_13();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0068_opaque(|w| {
                w.set_bit_13_opaque(true);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_recorrection_gain_bit_13"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_recorrection_gain_bit_13_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_recorrection_gain_bit_13();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0068_opaque(|w| {
                w.set_bit_13_opaque(true);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_recorrection_gain_bit_13"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_recorrection_gain_bit_5_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_recorrection_gain_bit_5();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0068_opaque(|w| {
                w.set_bit_5_opaque(true);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_recorrection_gain_bit_5"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_recorrection_gain_bit_5_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_recorrection_gain_bit_5();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0068_opaque(|w| {
                w.set_bit_5_opaque(true);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_recorrection_gain_bit_5"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_recorrection_gain_bits_0_4_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_recorrection_gain_bits_0_4();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0068_opaque(|w| {
                w.set_bits_0_4_opaque(oer_mmio::Bits::constant::<0x14>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_recorrection_gain_bits_0_4"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_recorrection_gain_bits_0_4_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_recorrection_gain_bits_0_4();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0068_opaque(|w| {
                w.set_bits_0_4_opaque(oer_mmio::Bits::constant::<0x14>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_recorrection_gain_bits_0_4"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_recorrection_gain_bits_11_12_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_recorrection_gain_bits_11_12();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0068_opaque(|w| {
                w.set_bits_11_12_opaque(oer_mmio::Bits::constant::<0x3>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_recorrection_gain_bits_11_12"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_recorrection_gain_bits_11_12_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_recorrection_gain_bits_11_12();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0068_opaque(|w| {
                w.set_bits_11_12_opaque(oer_mmio::Bits::constant::<0x3>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_recorrection_gain_bits_11_12"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_recorrection_gain_bits_6_10_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_recorrection_gain_bits_6_10();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0068_opaque(|w| {
                w.set_bits_6_10_opaque(oer_mmio::Bits::constant::<0x14>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_recorrection_gain_bits_6_10"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_recorrection_gain_bits_6_10_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_recorrection_gain_bits_6_10();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0068_opaque(|w| {
                w.set_bits_6_10_opaque(oer_mmio::Bits::constant::<0x14>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_recorrection_gain_bits_6_10"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_restart_100_12_15_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_100_12_15();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0100_opaque(|w| {
                w.set_bits_12_15_opaque(oer_mmio::Bits::constant::<0xa>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_100_12_15");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_100_12_15_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_100_12_15();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0100_opaque(|w| {
                w.set_bits_12_15_opaque(oer_mmio::Bits::constant::<0xa>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_100_12_15");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_100_16_19_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_100_16_19();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0100_opaque(|w| {
                w.set_bits_16_19_opaque(oer_mmio::Bits::constant::<0x8>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_100_16_19");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_100_16_19_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_100_16_19();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0100_opaque(|w| {
                w.set_bits_16_19_opaque(oer_mmio::Bits::constant::<0x8>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_100_16_19");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_100_20_23_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_100_20_23();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0100_opaque(|w| {
                w.set_bits_20_23_opaque(oer_mmio::Bits::constant::<0x6>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_100_20_23");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_100_20_23_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_100_20_23();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0100_opaque(|w| {
                w.set_bits_20_23_opaque(oer_mmio::Bits::constant::<0x6>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_100_20_23");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_100_24_27_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_100_24_27();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0100_opaque(|w| {
                w.set_bits_24_27_opaque(oer_mmio::Bits::constant::<0x6>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_100_24_27");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_100_24_27_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_100_24_27();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0100_opaque(|w| {
                w.set_bits_24_27_opaque(oer_mmio::Bits::constant::<0x6>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_100_24_27");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_100_28_31_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_100_28_31();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0100_opaque(|w| {
                w.set_bits_28_31_opaque(oer_mmio::Bits::constant::<0x6>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_100_28_31");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_100_28_31_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_100_28_31();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0100_opaque(|w| {
                w.set_bits_28_31_opaque(oer_mmio::Bits::constant::<0x6>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_100_28_31");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_baseband_10_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_restart_baseband_10();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_agc_restart_control(|w| {
                w.set_bit_10_opaque(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_baseband_10");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_baseband_10_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_restart_baseband_10();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_agc_restart_control(|w| {
                w.set_bit_10_opaque(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_baseband_10");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_baseband_19_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_restart_baseband_19();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_agc_restart_control(|w| {
                w.set_bit_19_opaque(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_baseband_19");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_baseband_19_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_restart_baseband_19();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_agc_restart_control(|w| {
                w.set_bit_19_opaque(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_baseband_19");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_baseband_20_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_restart_baseband_20();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_agc_restart_control(|w| {
                w.set_bit_20_opaque(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_baseband_20");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_baseband_20_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_restart_baseband_20();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_agc_restart_control(|w| {
                w.set_bit_20_opaque(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_baseband_20");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_baseband_21_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_restart_baseband_21();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_agc_restart_control(|w| {
                w.set_bit_21_opaque(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_baseband_21");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_baseband_21_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_restart_baseband_21();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_agc_restart_control(|w| {
                w.set_bit_21_opaque(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_baseband_21");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_baseband_6_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_restart_baseband_6();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_agc_restart_control(|w| {
                w.set_bit_6_opaque(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_baseband_6");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_baseband_6_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_restart_baseband_6();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_agc_restart_control(|w| {
                w.set_bit_6_opaque(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_baseband_6");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_baseband_7_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_restart_baseband_7();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_agc_restart_control(|w| {
                w.set_bit_7_opaque(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_baseband_7");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_baseband_7_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_restart_baseband_7();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_agc_restart_control(|w| {
                w.set_bit_7_opaque(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_baseband_7");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_baseband_8_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_restart_baseband_8();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_agc_restart_control(|w| {
                w.set_bit_8_opaque(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_baseband_8");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_baseband_8_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_restart_baseband_8();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_agc_restart_control(|w| {
                w.set_bit_8_opaque(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_baseband_8");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_baseband_9_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_restart_baseband_9();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_agc_restart_control(|w| {
                w.set_bit_9_opaque(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_baseband_9");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_baseband_9_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_restart_baseband_9();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_agc_restart_control(|w| {
                w.set_bit_9_opaque(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_baseband_9");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_btagc_27_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_btagc_27();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00dc_opaque(|w| {
                w.set_restart_bit_27(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_btagc_27");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_btagc_27_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_btagc_27();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00dc_opaque(|w| {
                w.set_restart_bit_27(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_btagc_27");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_btagc_28_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_btagc_28();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00dc_opaque(|w| {
                w.set_restart_bit_28(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_btagc_28");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_btagc_28_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_btagc_28();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00dc_opaque(|w| {
                w.set_restart_bit_28(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_btagc_28");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_btagc_29_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_btagc_29();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00dc_opaque(|w| {
                w.set_restart_bit_29(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_btagc_29");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_btagc_29_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_btagc_29();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00dc_opaque(|w| {
                w.set_restart_bit_29(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_btagc_29");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_btagc_30_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_btagc_30();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00dc_opaque(|w| {
                w.set_restart_bit_30(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_btagc_30");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_btagc_30_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_btagc_30();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00dc_opaque(|w| {
                w.set_restart_bit_30(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_btagc_30");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_btagc_31_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_btagc_31();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00dc_opaque(|w| {
                w.set_restart_bit_31(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_btagc_31");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_btagc_31_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_btagc_31();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00dc_opaque(|w| {
                w.set_restart_bit_31(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_btagc_31");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_config_0084_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_config_0084();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0084_opaque(|w| {
                w.set_finite_bits_24_31(0xf4);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_config_0084");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_config_0084_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_config_0084();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0084_opaque(|w| {
                w.set_finite_bits_24_31(0xf4);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_config_0084");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_config_0088_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_config_0088();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0088_opaque(|w| {
                w.set_bit_30_opaque(false);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_config_0088");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_config_0088_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_config_0088();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0088_opaque(|w| {
                w.set_bit_30_opaque(false);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_config_0088");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_control_00bc_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_control_00bc();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00bc_opaque(|w| {
                w.set_restart_bit_31(true);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_restart_control_00bc"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_restart_control_00bc_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_control_00bc();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00bc_opaque(|w| {
                w.set_restart_bit_31(true);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_restart_control_00bc"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_restart_correlator_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_restart_correlator();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_correlator_control(|w| {
                w.set_rx_setup_enable(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_correlator");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_correlator_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_restart_correlator();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_correlator_control(|w| {
                w.set_rx_setup_enable(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_correlator");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_e0_0_7_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_e0_0_7();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00e0_opaque(|w| {
                w.set_bits_0_7_opaque(0xf);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_e0_0_7");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_e0_0_7_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_e0_0_7();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00e0_opaque(|w| {
                w.set_bits_0_7_opaque(0xf);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_e0_0_7");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_e0_16_23_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_e0_16_23();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00e0_opaque(|w| {
                w.set_bits_16_23_opaque(0xe);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_e0_16_23");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_e0_16_23_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_e0_16_23();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00e0_opaque(|w| {
                w.set_bits_16_23_opaque(0xe);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_e0_16_23");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_e0_24_31_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_e0_24_31();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00e0_opaque(|w| {
                w.set_bits_24_31_opaque(0x14);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_e0_24_31");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_e0_24_31_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_e0_24_31();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00e0_opaque(|w| {
                w.set_bits_24_31_opaque(0x14);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_e0_24_31");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_e0_8_15_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_e0_8_15();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00e0_opaque(|w| {
                w.set_bits_8_15_opaque(0xe);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_e0_8_15");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_e0_8_15_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_e0_8_15();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00e0_opaque(|w| {
                w.set_bits_8_15_opaque(0xe);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_e0_8_15");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_e4_0_7_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_e4_0_7();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00e4_opaque(|w| {
                w.set_bits_0_7_opaque(0xf2);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_e4_0_7");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_e4_0_7_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_e4_0_7();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00e4_opaque(|w| {
                w.set_bits_0_7_opaque(0xf2);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_e4_0_7");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_e4_16_23_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_e4_16_23();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00e4_opaque(|w| {
                w.set_bits_16_23_opaque(0xec);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_e4_16_23");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_e4_16_23_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_e4_16_23();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00e4_opaque(|w| {
                w.set_bits_16_23_opaque(0xec);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_e4_16_23");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_e4_24_31_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_e4_24_31();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00e4_opaque(|w| {
                w.set_bits_24_31_opaque(0x1e);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_e4_24_31");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_e4_24_31_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_e4_24_31();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00e4_opaque(|w| {
                w.set_bits_24_31_opaque(0x1e);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_e4_24_31");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_e4_8_15_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_e4_8_15();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00e4_opaque(|w| {
                w.set_bits_8_15_opaque(0xf2);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_e4_8_15");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_e4_8_15_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_e4_8_15();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00e4_opaque(|w| {
                w.set_bits_8_15_opaque(0xf2);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_e4_8_15");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_e8_10_15_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_e8_10_15();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00e8_opaque(|w| {
                w.set_bits_10_15_opaque(oer_mmio::Bits::constant::<0x3>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_e8_10_15");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_e8_10_15_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_e8_10_15();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00e8_opaque(|w| {
                w.set_bits_10_15_opaque(oer_mmio::Bits::constant::<0x3>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_e8_10_15");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_e8_16_23_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_e8_16_23();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00e8_opaque(|w| {
                w.set_bits_16_23_opaque(0xe2);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_e8_16_23");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_e8_16_23_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_e8_16_23();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00e8_opaque(|w| {
                w.set_bits_16_23_opaque(0xe2);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_e8_16_23");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_e8_24_31_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_e8_24_31();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00e8_opaque(|w| {
                w.set_bits_24_31_opaque(0xf1);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_e8_24_31");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_e8_24_31_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_e8_24_31();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00e8_opaque(|w| {
                w.set_bits_24_31_opaque(0xf1);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_e8_24_31");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_ec_0_7_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_ec_0_7();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00ec_opaque(|w| {
                w.set_bits_0_7_opaque(0xd8);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_ec_0_7");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_ec_0_7_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_ec_0_7();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00ec_opaque(|w| {
                w.set_bits_0_7_opaque(0xd8);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_ec_0_7");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_ec_16_23_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_ec_16_23();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00ec_opaque(|w| {
                w.set_bits_16_23_opaque(0x28);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_ec_16_23");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_ec_16_23_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_ec_16_23();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00ec_opaque(|w| {
                w.set_bits_16_23_opaque(0x28);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_ec_16_23");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_ec_24_31_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_ec_24_31();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00ec_opaque(|w| {
                w.set_bits_24_31_opaque(0x2d);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_ec_24_31");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_ec_24_31_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_ec_24_31();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00ec_opaque(|w| {
                w.set_bits_24_31_opaque(0x2d);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_ec_24_31");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_ec_8_15_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_ec_8_15();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00ec_opaque(|w| {
                w.set_bits_8_15_opaque(0xd3);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_ec_8_15");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_ec_8_15_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_ec_8_15();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00ec_opaque(|w| {
                w.set_bits_8_15_opaque(0xd3);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_ec_8_15");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_f0_16_23_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_f0_16_23();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00f0_opaque(|w| {
                w.set_bits_16_23_opaque(0xa6);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_f0_16_23");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_f0_16_23_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_f0_16_23();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00f0_opaque(|w| {
                w.set_bits_16_23_opaque(0xa6);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_f0_16_23");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_f0_24_31_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_f0_24_31();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00f0_opaque(|w| {
                w.set_bits_24_31_opaque(0xa6);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_f0_24_31");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_f0_24_31_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_f0_24_31();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00f0_opaque(|w| {
                w.set_bits_24_31_opaque(0xa6);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_f0_24_31");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_f8_14_19_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_f8_14_19();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00f8_opaque(|w| {
                w.set_bits_14_19_opaque(oer_mmio::Bits::constant::<0x20>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_f8_14_19");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_f8_14_19_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_f8_14_19();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00f8_opaque(|w| {
                w.set_bits_14_19_opaque(oer_mmio::Bits::constant::<0x20>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_f8_14_19");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_f8_20_25_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_f8_20_25();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00f8_opaque(|w| {
                w.set_bits_20_25_opaque(oer_mmio::Bits::constant::<0x5>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_f8_20_25");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_f8_20_25_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_f8_20_25();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00f8_opaque(|w| {
                w.set_bits_20_25_opaque(oer_mmio::Bits::constant::<0x5>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_f8_20_25");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_f8_26_31_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_f8_26_31();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00f8_opaque(|w| {
                w.set_bits_26_31_opaque(oer_mmio::Bits::constant::<0xc>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_f8_26_31");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_f8_26_31_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_f8_26_31();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00f8_opaque(|w| {
                w.set_bits_26_31_opaque(oer_mmio::Bits::constant::<0xc>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_f8_26_31");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_f8_2_7_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_f8_2_7();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00f8_opaque(|w| {
                w.set_bits_2_7_opaque(oer_mmio::Bits::constant::<0xc>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_f8_2_7");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_f8_2_7_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_f8_2_7();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00f8_opaque(|w| {
                w.set_bits_2_7_opaque(oer_mmio::Bits::constant::<0xc>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_f8_2_7");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_f8_8_13_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_f8_8_13();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00f8_opaque(|w| {
                w.set_bits_8_13_opaque(oer_mmio::Bits::constant::<0xc>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_f8_8_13");
    }
}

#[test]
fn initialize_bluetooth_receive_restart_f8_8_13_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_restart_f8_8_13();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00f8_opaque(|w| {
                w.set_bits_8_13_opaque(oer_mmio::Bits::constant::<0xc>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_restart_f8_8_13");
    }
}

#[test]
fn initialize_bluetooth_receive_rssi_threshold_00a0_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_rssi_threshold_00a0();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00a0_opaque(|w| {
                w.set_bits_16_23_opaque(0x9c);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_rssi_threshold_00a0");
    }
}

#[test]
fn initialize_bluetooth_receive_rssi_threshold_00a0_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_rssi_threshold_00a0();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00a0_opaque(|w| {
                w.set_bits_16_23_opaque(0x9c);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_rssi_threshold_00a0");
    }
}

#[test]
fn initialize_bluetooth_receive_rssi_threshold_00a8_high_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_rssi_threshold_00a8_high();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00a8_opaque(|w| {
                w.set_bits_17_24_opaque(0x9c);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_rssi_threshold_00a8_high"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_rssi_threshold_00a8_high_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_rssi_threshold_00a8_high();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00a8_opaque(|w| {
                w.set_bits_17_24_opaque(0x9c);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_rssi_threshold_00a8_high"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_rssi_threshold_00a8_low_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_rssi_threshold_00a8_low();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00a8_opaque(|w| {
                w.set_bits_4_11_opaque(0x88);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_rssi_threshold_00a8_low"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_rssi_threshold_00a8_low_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_rssi_threshold_00a8_low();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00a8_opaque(|w| {
                w.set_bits_4_11_opaque(0x88);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_rssi_threshold_00a8_low"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_rssi_threshold_00b8_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_rssi_threshold_00b8();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00b8_opaque(|w| {
                w.set_bits_12_19_opaque(0x88);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_rssi_threshold_00b8");
    }
}

#[test]
fn initialize_bluetooth_receive_rssi_threshold_00b8_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_rssi_threshold_00b8();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00b8_opaque(|w| {
                w.set_bits_12_19_opaque(0x88);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_rssi_threshold_00b8");
    }
}

#[test]
fn initialize_bluetooth_receive_saturation_gain_004c_low_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_saturation_gain_004c_low();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_004c_opaque(|w| {
                w.set_sat_gain_bits_0_18(oer_mmio::Bits::constant::<0x1770>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_saturation_gain_004c_low"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_saturation_gain_004c_low_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_saturation_gain_004c_low();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_004c_opaque(|w| {
                w.set_sat_gain_bits_0_18(oer_mmio::Bits::constant::<0x1770>());
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_saturation_gain_004c_low"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_setup_004c_bit_26_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_004c_bit_26();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_004c_opaque(|w| {
                w.set_bit_26_opaque(false);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_004c_bit_26");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_004c_bit_26_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_004c_bit_26();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_004c_opaque(|w| {
                w.set_bit_26_opaque(false);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_004c_bit_26");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_0088_bit_18_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_0088_bit_18();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0088_opaque(|w| {
                w.set_bit_18_opaque(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_0088_bit_18");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_0088_bit_18_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_0088_bit_18();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0088_opaque(|w| {
                w.set_bit_18_opaque(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_0088_bit_18");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_0088_bit_29_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_0088_bit_29();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0088_opaque(|w| {
                w.set_bit_29_opaque(false);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_0088_bit_29");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_0088_bit_29_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_0088_bit_29();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0088_opaque(|w| {
                w.set_bit_29_opaque(false);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_0088_bit_29");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_008c_bit_29_final_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_008c_bit_29_final();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_008c_opaque(|w| {
                w.set_bit_29_opaque(false);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_setup_008c_bit_29_final"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_setup_008c_bit_29_final_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_008c_bit_29_final();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_008c_opaque(|w| {
                w.set_bit_29_opaque(false);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_setup_008c_bit_29_final"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_setup_008c_bit_29_initial_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_008c_bit_29_initial();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_008c_opaque(|w| {
                w.set_bit_29_opaque(false);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_setup_008c_bit_29_initial"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_setup_008c_bit_29_initial_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_008c_bit_29_initial();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_008c_opaque(|w| {
                w.set_bit_29_opaque(false);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_setup_008c_bit_29_initial"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_setup_agc_00d0_high_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_agc_00d0_high();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00d0_opaque(|w| {
                w.set_config_value_high(oer_mmio::Bits::constant::<0x14>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_agc_00d0_high");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_agc_00d0_high_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_agc_00d0_high();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00d0_opaque(|w| {
                w.set_config_value_high(oer_mmio::Bits::constant::<0x14>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_agc_00d0_high");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_agc_00d0_low_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_agc_00d0_low();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00d0_opaque(|w| {
                w.set_config_value_low(oer_mmio::Bits::constant::<0x14>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_agc_00d0_low");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_agc_00d0_low_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_agc_00d0_low();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00d0_opaque(|w| {
                w.set_config_value_low(oer_mmio::Bits::constant::<0x14>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_agc_00d0_low");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_agc_00d4_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_agc_00d4();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00d4_opaque(|w| {
                w.set_config_value(oer_mmio::Bits::constant::<0x3c0>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_agc_00d4");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_agc_00d4_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_agc_00d4();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00d4_opaque(|w| {
                w.set_config_value(oer_mmio::Bits::constant::<0x3c0>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_agc_00d4");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_argument_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_setup_argument();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_setup_argument(|w| {
                w.set_argument_0(oer_mmio::Bits::constant::<0x4>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_argument");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_argument_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_setup_argument();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_setup_argument(|w| {
                w.set_argument_0(oer_mmio::Bits::constant::<0x4>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_argument");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_cca_1_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_setup_cca_1();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_tx_cca_control_1(|w| {
                w.set_rx_setup_enable(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_cca_1");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_cca_1_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_setup_cca_1();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_tx_cca_control_1(|w| {
                w.set_rx_setup_enable(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_cca_1");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_cca_2_disable_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_setup_cca_2_disable();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_tx_cca_control_2(|w| {
                w.set_rx_setup_disable(false);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_cca_2_disable");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_cca_2_disable_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_setup_cca_2_disable();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_tx_cca_control_2(|w| {
                w.set_rx_setup_disable(false);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_cca_2_disable");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_cca_2_enable_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_setup_cca_2_enable();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_tx_cca_control_2(|w| {
                w.set_rx_setup_enable(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_cca_2_enable");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_cca_2_enable_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_setup_cca_2_enable();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_tx_cca_control_2(|w| {
                w.set_rx_setup_enable(true);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_cca_2_enable");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_cte_max_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_cte_max();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_cte_dc_shift(|w| {
                w.set_dc_shift_max(oer_mmio::Bits::constant::<0x7>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_cte_max");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_cte_max_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_cte_max();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_cte_dc_shift(|w| {
                w.set_dc_shift_max(oer_mmio::Bits::constant::<0x7>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_cte_max");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_cte_value_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_cte_value();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_cte_dc_shift(|w| {
                w.set_rx_config_value(oer_mmio::Bits::constant::<0x7>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_cte_value");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_cte_value_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_cte_value();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_cte_dc_shift(|w| {
                w.set_rx_config_value(oer_mmio::Bits::constant::<0x7>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_cte_value");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_image_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_setup_image();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_setup_image_0(|w| {
                w.set_config_image(oer_mmio::Bits::constant::<0x199a>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_image");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_image_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_receive_setup_image();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_setup_image_0(|w| {
                w.set_config_image(oer_mmio::Bits::constant::<0x199a>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_image");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_shared_control_0() {
    for fill in fills(oer_esp32s31_pac::shared_radio_init_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::shared_radio_init_control::SharedRadioInitControl::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_shared_control();
        oer_esp32s31_pac::shared_radio_init_control::SharedRadioInitControl::host(&mut new)
            .expect("block")
            .modify_control(|w| {
                w.set_bit_31_opaque(false);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_setup_shared_control"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_setup_shared_control_1() {
    for fill in fills(oer_esp32s31_pac::shared_radio_init_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::shared_radio_init_control::SharedRadioInitControl::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_shared_control();
        oer_esp32s31_pac::shared_radio_init_control::SharedRadioInitControl::host(&mut new)
            .expect("block")
            .modify_control(|w| {
                w.set_bit_31_opaque(false);
            });
        assert_eq!(
            old, new,
            "initialize_bluetooth_receive_setup_shared_control"
        );
    }
}

#[test]
fn initialize_bluetooth_receive_setup_zbbb_bit_8_0() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_zbbb_bit_8();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_rx_setup_control(|w| {
                w.set_bit_8_opaque(false);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_zbbb_bit_8");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_zbbb_bit_8_1() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_zbbb_bit_8();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_rx_setup_control(|w| {
                w.set_bit_8_opaque(false);
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_zbbb_bit_8");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_zbbb_image_0() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_zbbb_image();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_rx_setup_image(|w| {
                w.set_bits_12_31_opaque(oer_mmio::Bits::constant::<0x1cb>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_zbbb_image");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_zbbb_image_1() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_zbbb_image();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_rx_setup_image(|w| {
                w.set_bits_12_31_opaque(oer_mmio::Bits::constant::<0x1cb>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_zbbb_image");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_zbbb_low_0() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_zbbb_low();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_rx_setup_control(|w| {
                w.set_bits_0_1_opaque(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_zbbb_low");
    }
}

#[test]
fn initialize_bluetooth_receive_setup_zbbb_low_1() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_setup_zbbb_low();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_rx_setup_control(|w| {
                w.set_bits_0_1_opaque(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_setup_zbbb_low");
    }
}

#[test]
fn initialize_bluetooth_receive_target_008c_high_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_target_008c_high();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_008c_opaque(|w| {
                w.set_target_bits_9_17(oer_mmio::Bits::constant::<0x1dc>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_target_008c_high");
    }
}

#[test]
fn initialize_bluetooth_receive_target_008c_high_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_target_008c_high();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_008c_opaque(|w| {
                w.set_target_bits_9_17(oer_mmio::Bits::constant::<0x1dc>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_target_008c_high");
    }
}

#[test]
fn initialize_bluetooth_receive_target_008c_low_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_target_008c_low();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_008c_opaque(|w| {
                w.set_target_bits_0_8(oer_mmio::Bits::constant::<0x1d4>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_target_008c_low");
    }
}

#[test]
fn initialize_bluetooth_receive_target_008c_low_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_target_008c_low();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_008c_opaque(|w| {
                w.set_target_bits_0_8(oer_mmio::Bits::constant::<0x1d4>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_target_008c_low");
    }
}

#[test]
fn initialize_bluetooth_receive_target_00b0_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_target_00b0();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00b0_opaque(|w| {
                w.set_target_bits_23_31(oer_mmio::Bits::constant::<0x1d4>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_target_00b0");
    }
}

#[test]
fn initialize_bluetooth_receive_target_00b0_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_target_00b0();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00b0_opaque(|w| {
                w.set_target_bits_23_31(oer_mmio::Bits::constant::<0x1d4>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_target_00b0");
    }
}

#[test]
fn initialize_bluetooth_receive_target_00b4_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_target_00b4();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00b4_opaque(|w| {
                w.set_target_bits_23_31(oer_mmio::Bits::constant::<0x1ce>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_target_00b4");
    }
}

#[test]
fn initialize_bluetooth_receive_target_00b4_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_target_00b4();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_00b4_opaque(|w| {
                w.set_target_bits_23_31(oer_mmio::Bits::constant::<0x1ce>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_target_00b4");
    }
}

#[test]
fn initialize_bluetooth_receive_target_cca_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_target_cca();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_cca_config(|w| {
                w.set_target_bits_14_22(oer_mmio::Bits::constant::<0x1ce>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_target_cca");
    }
}

#[test]
fn initialize_bluetooth_receive_target_cca_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_target_cca();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_cca_config(|w| {
                w.set_target_bits_14_22(oer_mmio::Bits::constant::<0x1ce>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_target_cca");
    }
}

#[test]
fn initialize_bluetooth_receive_target_cte_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_target_cte();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_cte_agc_target(|w| {
                w.set_baseband_target_value(oer_mmio::Bits::constant::<0x1d4>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_target_cte");
    }
}

#[test]
fn initialize_bluetooth_receive_target_cte_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_target_cte();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_cte_agc_target(|w| {
                w.set_baseband_target_value(oer_mmio::Bits::constant::<0x1d4>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_target_cte");
    }
}

#[test]
fn initialize_bluetooth_receive_target_gain_force_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_target_gain_force();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_rx_gain_force(|w| {
                w.set_agc_target(oer_mmio::Bits::constant::<0x1d4>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_target_gain_force");
    }
}

#[test]
fn initialize_bluetooth_receive_target_gain_force_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .initialize_bluetooth_receive_target_gain_force();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_rx_gain_force(|w| {
                w.set_agc_target(oer_mmio::Bits::constant::<0x1d4>());
            });
        assert_eq!(old, new, "initialize_bluetooth_receive_target_gain_force");
    }
}

#[test]
fn initialize_bluetooth_shared_receive_final_image_0() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_bluetooth_shared_receive_final_image();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_rx_setup_image(|w| {
                w.set_bits_12_31_opaque(oer_mmio::Bits::constant::<0x19f>());
            });
        assert_eq!(old, new, "initialize_bluetooth_shared_receive_final_image");
    }
}

#[test]
fn initialize_bluetooth_shared_receive_final_image_1() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_bluetooth_shared_receive_final_image();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_rx_setup_image(|w| {
                w.set_bits_12_31_opaque(oer_mmio::Bits::constant::<0x19f>());
            });
        assert_eq!(old, new, "initialize_bluetooth_shared_receive_final_image");
    }
}

#[test]
fn initialize_bluetooth_shared_tx_delay_0() {
    for fill in fills(oer_esp32s31_pac::shared_baseband_tx_timing::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::shared_baseband_tx_timing::SharedBasebandTxTiming::host(&mut old)
            .expect("block")
            .initialize_bluetooth_shared_tx_delay();
        oer_esp32s31_pac::shared_baseband_tx_timing::SharedBasebandTxTiming::host(&mut new)
            .expect("block")
            .modify_auxiliary_tx_on_delay(|w| {
                w.set_encoded_image(oer_mmio::Bits::constant::<0x190>());
            });
        assert_eq!(old, new, "initialize_bluetooth_shared_tx_delay");
    }
}

#[test]
fn initialize_bluetooth_shared_tx_delay_1() {
    for fill in fills(oer_esp32s31_pac::shared_baseband_tx_timing::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::shared_baseband_tx_timing::SharedBasebandTxTiming::host(&mut old)
            .expect("block")
            .initialize_bluetooth_shared_tx_delay();
        oer_esp32s31_pac::shared_baseband_tx_timing::SharedBasebandTxTiming::host(&mut new)
            .expect("block")
            .modify_auxiliary_tx_on_delay(|w| {
                w.set_encoded_image(oer_mmio::Bits::constant::<0x190>());
            });
        assert_eq!(old, new, "initialize_bluetooth_shared_tx_delay");
    }
}

#[test]
fn initialize_bluetooth_tx_cca_period_argument_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_tx_cca_period_argument();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_tx_cca_control_0(|w| {
                w.set_period_argument_0_image(oer_mmio::Bits::constant::<0x157>());
            });
        assert_eq!(old, new, "initialize_bluetooth_tx_cca_period_argument");
    }
}

#[test]
fn initialize_bluetooth_tx_cca_period_argument_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_tx_cca_period_argument();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_tx_cca_control_0(|w| {
                w.set_period_argument_0_image(oer_mmio::Bits::constant::<0x157>());
            });
        assert_eq!(old, new, "initialize_bluetooth_tx_cca_period_argument");
    }
}

#[test]
fn initialize_bluetooth_tx_cca_period_difference_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_tx_cca_period_difference();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_tx_cca_control_0(|w| {
                w.set_bits_20_22_opaque(oer_mmio::Bits::constant::<0x0>());
                w.set_period_argument_0_minus_argument_1_image(0x17);
            });
        assert_eq!(old, new, "initialize_bluetooth_tx_cca_period_difference");
    }
}

#[test]
fn initialize_bluetooth_tx_cca_period_difference_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .initialize_bluetooth_tx_cca_period_difference();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_tx_cca_control_0(|w| {
                w.set_bits_20_22_opaque(oer_mmio::Bits::constant::<0x0>());
                w.set_period_argument_0_minus_argument_1_image(0x17);
            });
        assert_eq!(old, new, "initialize_bluetooth_tx_cca_period_difference");
    }
}

#[test]
fn initialize_bluetooth_tx_pa_delay_0() {
    for fill in fills(oer_esp32s31_pac::phy_baseband_config_oracle::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_baseband_config_oracle::PhyBasebandConfigOracle::host(&mut old)
            .expect("block")
            .initialize_bluetooth_tx_pa_delay();
        oer_esp32s31_pac::phy_baseband_config_oracle::PhyBasebandConfigOracle::host(&mut new)
            .expect("block")
            .modify_tx_pa_control_1(|w| {
                w.set_pa_on_bt_delay(0x96);
            });
        assert_eq!(old, new, "initialize_bluetooth_tx_pa_delay");
    }
}

#[test]
fn initialize_bluetooth_tx_pa_delay_1() {
    for fill in fills(oer_esp32s31_pac::phy_baseband_config_oracle::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_baseband_config_oracle::PhyBasebandConfigOracle::host(&mut old)
            .expect("block")
            .initialize_bluetooth_tx_pa_delay();
        oer_esp32s31_pac::phy_baseband_config_oracle::PhyBasebandConfigOracle::host(&mut new)
            .expect("block")
            .modify_tx_pa_control_1(|w| {
                w.set_pa_on_bt_delay(0x96);
            });
        assert_eq!(old, new, "initialize_bluetooth_tx_pa_delay");
    }
}

#[test]
fn initialize_shared_receive_control_0_0() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_shared_receive_control_0();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_shared_rx_init_control_0(|w| {
                w.set_bit_1_opaque(false);
            });
        assert_eq!(old, new, "initialize_shared_receive_control_0");
    }
}

#[test]
fn initialize_shared_receive_control_0_1() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_shared_receive_control_0();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_shared_rx_init_control_0(|w| {
                w.set_bit_1_opaque(false);
            });
        assert_eq!(old, new, "initialize_shared_receive_control_0");
    }
}

#[test]
fn initialize_shared_receive_control_1_0() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_shared_receive_control_1();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_shared_rx_init_control_1(|w| {
                w.set_bit_30_opaque(true);
            });
        assert_eq!(old, new, "initialize_shared_receive_control_1");
    }
}

#[test]
fn initialize_shared_receive_control_1_1() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_shared_receive_control_1();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_shared_rx_init_control_1(|w| {
                w.set_bit_30_opaque(true);
            });
        assert_eq!(old, new, "initialize_shared_receive_control_1");
    }
}

#[test]
fn initialize_shared_receive_control_image_0() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_shared_receive_control_image();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_rx_setup_control(|w| {
                w.set_bits_10_20_opaque(oer_mmio::Bits::constant::<0x79c>());
            });
        assert_eq!(old, new, "initialize_shared_receive_control_image");
    }
}

#[test]
fn initialize_shared_receive_control_image_1() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_shared_receive_control_image();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_rx_setup_control(|w| {
                w.set_bits_10_20_opaque(oer_mmio::Bits::constant::<0x79c>());
            });
        assert_eq!(old, new, "initialize_shared_receive_control_image");
    }
}

#[test]
fn initialize_shared_receive_image_0_high_0() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_shared_receive_image_0_high();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_shared_rx_init_image_0(|w| {
                w.set_bits_21_31_opaque(oer_mmio::Bits::constant::<0x792>());
            });
        assert_eq!(old, new, "initialize_shared_receive_image_0_high");
    }
}

#[test]
fn initialize_shared_receive_image_0_high_1() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_shared_receive_image_0_high();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_shared_rx_init_image_0(|w| {
                w.set_bits_21_31_opaque(oer_mmio::Bits::constant::<0x792>());
            });
        assert_eq!(old, new, "initialize_shared_receive_image_0_high");
    }
}

#[test]
fn initialize_shared_receive_image_0_middle_0() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_shared_receive_image_0_middle();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_shared_rx_init_image_0(|w| {
                w.set_bits_13_20_opaque(0xa6);
            });
        assert_eq!(old, new, "initialize_shared_receive_image_0_middle");
    }
}

#[test]
fn initialize_shared_receive_image_0_middle_1() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_shared_receive_image_0_middle();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_shared_rx_init_image_0(|w| {
                w.set_bits_13_20_opaque(0xa6);
            });
        assert_eq!(old, new, "initialize_shared_receive_image_0_middle");
    }
}

#[test]
fn initialize_shared_receive_image_1_low_0() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_shared_receive_image_1_low();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_shared_rx_init_image_1(|w| {
                w.set_bits_0_10_opaque(oer_mmio::Bits::constant::<0x7a6>());
            });
        assert_eq!(old, new, "initialize_shared_receive_image_1_low");
    }
}

#[test]
fn initialize_shared_receive_image_1_low_1() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_shared_receive_image_1_low();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_shared_rx_init_image_1(|w| {
                w.set_bits_0_10_opaque(oer_mmio::Bits::constant::<0x7a6>());
            });
        assert_eq!(old, new, "initialize_shared_receive_image_1_low");
    }
}

#[test]
fn initialize_shared_receive_image_2_high_0() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_shared_receive_image_2_high();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_shared_rx_init_image_2(|w| {
                w.set_bits_8_31_opaque(oer_mmio::Bits::constant::<0x7a120>());
            });
        assert_eq!(old, new, "initialize_shared_receive_image_2_high");
    }
}

#[test]
fn initialize_shared_receive_image_2_high_1() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_shared_receive_image_2_high();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_shared_rx_init_image_2(|w| {
                w.set_bits_8_31_opaque(oer_mmio::Bits::constant::<0x7a120>());
            });
        assert_eq!(old, new, "initialize_shared_receive_image_2_high");
    }
}

#[test]
fn initialize_shared_receive_image_2_low_0() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_shared_receive_image_2_low();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_shared_rx_init_image_2(|w| {
                w.set_bits_0_7_opaque(0xf1);
            });
        assert_eq!(old, new, "initialize_shared_receive_image_2_low");
    }
}

#[test]
fn initialize_shared_receive_image_2_low_1() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_shared_receive_image_2_low();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_shared_rx_init_image_2(|w| {
                w.set_bits_0_7_opaque(0xf1);
            });
        assert_eq!(old, new, "initialize_shared_receive_image_2_low");
    }
}

#[test]
fn initialize_shared_receive_image_3_high_0() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_shared_receive_image_3_high();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_shared_rx_init_image_3(|w| {
                w.set_bits_8_31_opaque(oer_mmio::Bits::constant::<0xf85edf>());
            });
        assert_eq!(old, new, "initialize_shared_receive_image_3_high");
    }
}

#[test]
fn initialize_shared_receive_image_3_high_1() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_shared_receive_image_3_high();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_shared_rx_init_image_3(|w| {
                w.set_bits_8_31_opaque(oer_mmio::Bits::constant::<0xf85edf>());
            });
        assert_eq!(old, new, "initialize_shared_receive_image_3_high");
    }
}

#[test]
fn initialize_shared_receive_image_4_high_0() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_shared_receive_image_4_high();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_shared_rx_init_image_4(|w| {
                w.set_bits_20_30_opaque(oer_mmio::Bits::constant::<0x7e1>());
            });
        assert_eq!(old, new, "initialize_shared_receive_image_4_high");
    }
}

#[test]
fn initialize_shared_receive_image_4_high_1() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_shared_receive_image_4_high();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_shared_rx_init_image_4(|w| {
                w.set_bits_20_30_opaque(oer_mmio::Bits::constant::<0x7e1>());
            });
        assert_eq!(old, new, "initialize_shared_receive_image_4_high");
    }
}

#[test]
fn initialize_shared_receive_image_4_low_0() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_shared_receive_image_4_low();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_shared_rx_init_image_4(|w| {
                w.set_bit_0_opaque(true);
            });
        assert_eq!(old, new, "initialize_shared_receive_image_4_low");
    }
}

#[test]
fn initialize_shared_receive_image_4_low_1() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_shared_receive_image_4_low();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_shared_rx_init_image_4(|w| {
                w.set_bit_0_opaque(true);
            });
        assert_eq!(old, new, "initialize_shared_receive_image_4_low");
    }
}

#[test]
fn initialize_shared_receive_image_4_middle_0() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_shared_receive_image_4_middle();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_shared_rx_init_image_4(|w| {
                w.set_bits_9_19_opaque(oer_mmio::Bits::constant::<0x7ed>());
            });
        assert_eq!(old, new, "initialize_shared_receive_image_4_middle");
    }
}

#[test]
fn initialize_shared_receive_image_4_middle_1() {
    for fill in fills(oer_esp32s31_pac::zbbb_radio_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut old)
            .expect("block")
            .initialize_shared_receive_image_4_middle();
        oer_esp32s31_pac::zbbb_radio_control::ZbbbRadioControl::host(&mut new)
            .expect("block")
            .modify_shared_rx_init_image_4(|w| {
                w.set_bits_9_19_opaque(oer_mmio::Bits::constant::<0x7ed>());
            });
        assert_eq!(old, new, "initialize_shared_receive_image_4_middle");
    }
}

#[test]
fn mask_bluetooth_primary_baseline_bank_0_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_interrupt_bank::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut old)
            .expect("block")
            .mask_bluetooth_primary_baseline_bank_0();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut new)
            .expect("block")
            .modify_irq_enable_0(|w| {
                w.set_source_15(false);
            });
        assert_eq!(old, new, "mask_bluetooth_primary_baseline_bank_0");
    }
}

#[test]
fn mask_bluetooth_primary_baseline_bank_0_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_interrupt_bank::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut old)
            .expect("block")
            .mask_bluetooth_primary_baseline_bank_0();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut new)
            .expect("block")
            .modify_irq_enable_0(|w| {
                w.set_source_15(false);
            });
        assert_eq!(old, new, "mask_bluetooth_primary_baseline_bank_0");
    }
}

#[test]
fn mask_bluetooth_primary_baseline_bank_1_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_interrupt_bank::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut old)
            .expect("block")
            .mask_bluetooth_primary_baseline_bank_1();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut new)
            .expect("block")
            .modify_irq_enable_1(|w| {
                w.set_source_8(false);
                w.set_source_9(false);
                w.set_source_12(false);
            });
        assert_eq!(old, new, "mask_bluetooth_primary_baseline_bank_1");
    }
}

#[test]
fn mask_bluetooth_primary_baseline_bank_1_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_interrupt_bank::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut old)
            .expect("block")
            .mask_bluetooth_primary_baseline_bank_1();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut new)
            .expect("block")
            .modify_irq_enable_1(|w| {
                w.set_source_8(false);
                w.set_source_9(false);
                w.set_source_12(false);
            });
        assert_eq!(old, new, "mask_bluetooth_primary_baseline_bank_1");
    }
}

#[test]
fn mask_bluetooth_scheduler_run_interrupts_bank_0_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_interrupt_bank::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut old)
            .expect("block")
            .mask_bluetooth_scheduler_run_interrupts_bank_0();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut new)
            .expect("block")
            .modify_irq_enable_0(|w| {
                w.set_source_21(false);
                w.set_sources_27_28(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "mask_bluetooth_scheduler_run_interrupts_bank_0");
    }
}

#[test]
fn mask_bluetooth_scheduler_run_interrupts_bank_0_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_interrupt_bank::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut old)
            .expect("block")
            .mask_bluetooth_scheduler_run_interrupts_bank_0();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut new)
            .expect("block")
            .modify_irq_enable_0(|w| {
                w.set_source_21(false);
                w.set_sources_27_28(oer_mmio::Bits::constant::<0x0>());
            });
        assert_eq!(old, new, "mask_bluetooth_scheduler_run_interrupts_bank_0");
    }
}

#[test]
fn mask_bluetooth_scheduler_run_interrupts_bank_1_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_interrupt_bank::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut old)
            .expect("block")
            .mask_bluetooth_scheduler_run_interrupts_bank_1();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut new)
            .expect("block")
            .modify_irq_enable_1(|w| {
                w.set_source_3(false);
            });
        assert_eq!(old, new, "mask_bluetooth_scheduler_run_interrupts_bank_1");
    }
}

#[test]
fn mask_bluetooth_scheduler_run_interrupts_bank_1_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_interrupt_bank::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut old)
            .expect("block")
            .mask_bluetooth_scheduler_run_interrupts_bank_1();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut new)
            .expect("block")
            .modify_irq_enable_1(|w| {
                w.set_source_3(false);
            });
        assert_eq!(old, new, "mask_bluetooth_scheduler_run_interrupts_bank_1");
    }
}

#[test]
fn or_ble_phy_init_byte_2_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .or_ble_phy_init_byte_2();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0254_opaque(|w| {
                w.set_init_byte_2_low_7(oer_mmio::Bits::truncate(
                    w.init_byte_2_low_7().get() | 0x32,
                ));
                w.set_init_byte_2_high(true);
            });
        assert_eq!(old, new, "or_ble_phy_init_byte_2");
    }
}

#[test]
fn or_ble_phy_init_byte_2_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .or_ble_phy_init_byte_2();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0254_opaque(|w| {
                w.set_init_byte_2_low_7(oer_mmio::Bits::truncate(
                    w.init_byte_2_low_7().get() | 0x32,
                ));
                w.set_init_byte_2_high(true);
            });
        assert_eq!(old, new, "or_ble_phy_init_byte_2");
    }
}

#[test]
fn or_ble_phy_init_low_byte_pair_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .or_ble_phy_init_low_byte_pair();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0254_opaque(|w| {
                w.set_init_byte_0(w.init_byte_0() | 0x1);
                w.set_init_byte_1(w.init_byte_1() | 0x1);
            });
        assert_eq!(old, new, "or_ble_phy_init_low_byte_pair");
    }
}

#[test]
fn or_ble_phy_init_low_byte_pair_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .or_ble_phy_init_low_byte_pair();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0254_opaque(|w| {
                w.set_init_byte_0(w.init_byte_0() | 0x1);
                w.set_init_byte_1(w.init_byte_1() | 0x1);
            });
        assert_eq!(old, new, "or_ble_phy_init_low_byte_pair");
    }
}

#[test]
fn prepare_bluetooth_interrupt_output_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_interrupt_bank::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut old)
            .expect("block")
            .prepare_bluetooth_interrupt_output();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut new)
            .expect("block")
            .write_irq_control_0(|w| {
                w.set_command(0x1);
            });
        assert_eq!(old, new, "prepare_bluetooth_interrupt_output");
    }
}

#[test]
fn prepare_bluetooth_interrupt_output_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_interrupt_bank::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut old)
            .expect("block")
            .prepare_bluetooth_interrupt_output();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut new)
            .expect("block")
            .write_irq_control_0(|w| {
                w.set_command(0x1);
            });
        assert_eq!(old, new, "prepare_bluetooth_interrupt_output");
    }
}

#[test]
fn prepare_bluetooth_modem_lp_timer_command_0010_0() {
    for fill in fills(oer_esp32s31_pac::btdm_runtime_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btdm_runtime_control::BtdmRuntimeControl::host(&mut old)
            .expect("block")
            .prepare_bluetooth_modem_lp_timer_command_0010();
        oer_esp32s31_pac::btdm_runtime_control::BtdmRuntimeControl::host(&mut new)
            .expect("block")
            .write_word_0010_opaque(|w| {
                w.set_bit_1_opaque(true);
            });
        assert_eq!(old, new, "prepare_bluetooth_modem_lp_timer_command_0010");
    }
}

#[test]
fn prepare_bluetooth_modem_lp_timer_command_0010_1() {
    for fill in fills(oer_esp32s31_pac::btdm_runtime_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btdm_runtime_control::BtdmRuntimeControl::host(&mut old)
            .expect("block")
            .prepare_bluetooth_modem_lp_timer_command_0010();
        oer_esp32s31_pac::btdm_runtime_control::BtdmRuntimeControl::host(&mut new)
            .expect("block")
            .write_word_0010_opaque(|w| {
                w.set_bit_1_opaque(true);
            });
        assert_eq!(old, new, "prepare_bluetooth_modem_lp_timer_command_0010");
    }
}

#[test]
fn prepare_bluetooth_modem_lp_timer_control_25_0() {
    for fill in fills(oer_esp32s31_pac::btdm_runtime_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btdm_runtime_control::BtdmRuntimeControl::host(&mut old)
            .expect("block")
            .prepare_bluetooth_modem_lp_timer_control_25();
        oer_esp32s31_pac::btdm_runtime_control::BtdmRuntimeControl::host(&mut new)
            .expect("block")
            .modify_word_0078_opaque(|w| {
                w.set_control_25(true);
            });
        assert_eq!(old, new, "prepare_bluetooth_modem_lp_timer_control_25");
    }
}

#[test]
fn prepare_bluetooth_modem_lp_timer_control_25_1() {
    for fill in fills(oer_esp32s31_pac::btdm_runtime_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btdm_runtime_control::BtdmRuntimeControl::host(&mut old)
            .expect("block")
            .prepare_bluetooth_modem_lp_timer_control_25();
        oer_esp32s31_pac::btdm_runtime_control::BtdmRuntimeControl::host(&mut new)
            .expect("block")
            .modify_word_0078_opaque(|w| {
                w.set_control_25(true);
            });
        assert_eq!(old, new, "prepare_bluetooth_modem_lp_timer_control_25");
    }
}

#[test]
fn preserve_bluetooth_memory_list_1_initial_control_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .preserve_bluetooth_memory_list_1_initial_control();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_global_rxlink_1_current(|_| {});
        assert_eq!(old, new, "preserve_bluetooth_memory_list_1_initial_control");
    }
}

#[test]
fn preserve_bluetooth_memory_list_1_initial_control_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .preserve_bluetooth_memory_list_1_initial_control();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_global_rxlink_1_current(|_| {});
        assert_eq!(old, new, "preserve_bluetooth_memory_list_1_initial_control");
    }
}

#[test]
fn preserve_bluetooth_memory_list_2_initial_control_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .preserve_bluetooth_memory_list_2_initial_control();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_global_rxlink_2_current(|_| {});
        assert_eq!(old, new, "preserve_bluetooth_memory_list_2_initial_control");
    }
}

#[test]
fn preserve_bluetooth_memory_list_2_initial_control_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .preserve_bluetooth_memory_list_2_initial_control();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_global_rxlink_2_current(|_| {});
        assert_eq!(old, new, "preserve_bluetooth_memory_list_2_initial_control");
    }
}

#[test]
fn publish_ble_phy_accelerator_config_0() {
    for fill in fills(oer_esp32s31_pac::ble_hw_accelerator::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_accelerator::BleHwAccelerator::host(&mut old)
            .expect("block")
            .publish_ble_phy_accelerator_config();
        oer_esp32s31_pac::ble_hw_accelerator::BleHwAccelerator::host(&mut new)
            .expect("block")
            .modify_init_config(|w| {
                w.set_value(0x2f0);
            });
        assert_eq!(old, new, "publish_ble_phy_accelerator_config");
    }
}

#[test]
fn publish_ble_phy_accelerator_config_1() {
    for fill in fills(oer_esp32s31_pac::ble_hw_accelerator::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_accelerator::BleHwAccelerator::host(&mut old)
            .expect("block")
            .publish_ble_phy_accelerator_config();
        oer_esp32s31_pac::ble_hw_accelerator::BleHwAccelerator::host(&mut new)
            .expect("block")
            .modify_init_config(|w| {
                w.set_value(0x2f0);
            });
        assert_eq!(old, new, "publish_ble_phy_accelerator_config");
    }
}

#[test]
fn publish_ble_phy_accelerator_sram_region_0_0() {
    for fill in fills(oer_esp32s31_pac::ble_hw_accelerator::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_accelerator::BleHwAccelerator::host(&mut old)
            .expect("block")
            .publish_ble_phy_accelerator_sram_region_0();
        oer_esp32s31_pac::ble_hw_accelerator::BleHwAccelerator::host(&mut new)
            .expect("block")
            .modify_init_sram_region_0(|w| {
                w.set_value(0x2f080000);
            });
        assert_eq!(old, new, "publish_ble_phy_accelerator_sram_region_0");
    }
}

#[test]
fn publish_ble_phy_accelerator_sram_region_0_1() {
    for fill in fills(oer_esp32s31_pac::ble_hw_accelerator::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_accelerator::BleHwAccelerator::host(&mut old)
            .expect("block")
            .publish_ble_phy_accelerator_sram_region_0();
        oer_esp32s31_pac::ble_hw_accelerator::BleHwAccelerator::host(&mut new)
            .expect("block")
            .modify_init_sram_region_0(|w| {
                w.set_value(0x2f080000);
            });
        assert_eq!(old, new, "publish_ble_phy_accelerator_sram_region_0");
    }
}

#[test]
fn publish_ble_phy_accelerator_sram_region_1_0() {
    for fill in fills(oer_esp32s31_pac::ble_hw_accelerator::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_accelerator::BleHwAccelerator::host(&mut old)
            .expect("block")
            .publish_ble_phy_accelerator_sram_region_1();
        oer_esp32s31_pac::ble_hw_accelerator::BleHwAccelerator::host(&mut new)
            .expect("block")
            .modify_init_sram_region_1(|w| {
                w.set_value(0x2f000000);
            });
        assert_eq!(old, new, "publish_ble_phy_accelerator_sram_region_1");
    }
}

#[test]
fn publish_ble_phy_accelerator_sram_region_1_1() {
    for fill in fills(oer_esp32s31_pac::ble_hw_accelerator::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_accelerator::BleHwAccelerator::host(&mut old)
            .expect("block")
            .publish_ble_phy_accelerator_sram_region_1();
        oer_esp32s31_pac::ble_hw_accelerator::BleHwAccelerator::host(&mut new)
            .expect("block")
            .modify_init_sram_region_1(|w| {
                w.set_value(0x2f000000);
            });
        assert_eq!(old, new, "publish_ble_phy_accelerator_sram_region_1");
    }
}

#[test]
fn publish_ble_phy_controller_environment_tail_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old).expect("block").publish_ble_phy_controller_environment_tail(oer_esp32s31_pac::bluetooth_controller_core::phy_environment_tail_pointer::AddressImage::new(reference(0x2f00_1230)));
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new).expect("block").write_phy_environment_tail_pointer(|w| { w.set_address(oer_esp32s31_pac::bluetooth_controller_core::phy_environment_tail_pointer::AddressImage::new(reference(0x2f00_1230))); });
        assert_eq!(old, new, "publish_ble_phy_controller_environment_tail");
    }
}

#[test]
fn publish_ble_phy_controller_environment_tail_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old).expect("block").publish_ble_phy_controller_environment_tail(oer_esp32s31_pac::bluetooth_controller_core::phy_environment_tail_pointer::AddressImage::new(reference(0x2f00_1270)));
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new).expect("block").write_phy_environment_tail_pointer(|w| { w.set_address(oer_esp32s31_pac::bluetooth_controller_core::phy_environment_tail_pointer::AddressImage::new(reference(0x2f00_1270))); });
        assert_eq!(old, new, "publish_ble_phy_controller_environment_tail");
    }
}

#[test]
fn publish_ble_phy_controller_value_01f0_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_ble_phy_controller_value_01f0();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .write_gpio_diag_argument_1(|w| {
                w.set_value(0x55);
            });
        assert_eq!(old, new, "publish_ble_phy_controller_value_01f0");
    }
}

#[test]
fn publish_ble_phy_controller_value_01f0_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_ble_phy_controller_value_01f0();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .write_gpio_diag_argument_1(|w| {
                w.set_value(0x55);
            });
        assert_eq!(old, new, "publish_ble_phy_controller_value_01f0");
    }
}

#[test]
fn publish_ble_phy_controller_value_0248_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_ble_phy_controller_value_0248();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .write_gpio_diag_control(|w| {
                w.set_value(0xfff);
            });
        assert_eq!(old, new, "publish_ble_phy_controller_value_0248");
    }
}

#[test]
fn publish_ble_phy_controller_value_0248_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_ble_phy_controller_value_0248();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .write_gpio_diag_control(|w| {
                w.set_value(0xfff);
            });
        assert_eq!(old, new, "publish_ble_phy_controller_value_0248");
    }
}

#[test]
fn publish_ble_phy_init_0550_byte_0_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_0550_byte_0();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0550_opaque(|w| {
                w.set_init_byte_0(0x3);
            });
        assert_eq!(old, new, "publish_ble_phy_init_0550_byte_0");
    }
}

#[test]
fn publish_ble_phy_init_0550_byte_0_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_0550_byte_0();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0550_opaque(|w| {
                w.set_init_byte_0(0x3);
            });
        assert_eq!(old, new, "publish_ble_phy_init_0550_byte_0");
    }
}

#[test]
fn publish_ble_phy_init_0550_byte_1_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_0550_byte_1();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0550_opaque(|w| {
                w.set_init_byte_1(0x3);
            });
        assert_eq!(old, new, "publish_ble_phy_init_0550_byte_1");
    }
}

#[test]
fn publish_ble_phy_init_0550_byte_1_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_0550_byte_1();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0550_opaque(|w| {
                w.set_init_byte_1(0x3);
            });
        assert_eq!(old, new, "publish_ble_phy_init_0550_byte_1");
    }
}

#[test]
fn publish_ble_phy_init_0550_byte_2_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_0550_byte_2();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0550_opaque(|w| {
                w.set_init_byte_2(0x44);
            });
        assert_eq!(old, new, "publish_ble_phy_init_0550_byte_2");
    }
}

#[test]
fn publish_ble_phy_init_0550_byte_2_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_0550_byte_2();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0550_opaque(|w| {
                w.set_init_byte_2(0x44);
            });
        assert_eq!(old, new, "publish_ble_phy_init_0550_byte_2");
    }
}

#[test]
fn publish_ble_phy_init_0554_byte_0_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_0554_byte_0();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0554_opaque(|w| {
                w.set_init_byte_0(0x10);
            });
        assert_eq!(old, new, "publish_ble_phy_init_0554_byte_0");
    }
}

#[test]
fn publish_ble_phy_init_0554_byte_0_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_0554_byte_0();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0554_opaque(|w| {
                w.set_init_byte_0(0x10);
            });
        assert_eq!(old, new, "publish_ble_phy_init_0554_byte_0");
    }
}

#[test]
fn publish_ble_phy_init_0554_byte_1_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_0554_byte_1();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0554_opaque(|w| {
                w.set_init_byte_1(0x10);
            });
        assert_eq!(old, new, "publish_ble_phy_init_0554_byte_1");
    }
}

#[test]
fn publish_ble_phy_init_0554_byte_1_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_0554_byte_1();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0554_opaque(|w| {
                w.set_init_byte_1(0x10);
            });
        assert_eq!(old, new, "publish_ble_phy_init_0554_byte_1");
    }
}

#[test]
fn publish_ble_phy_init_0554_byte_2_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_0554_byte_2();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0554_opaque(|w| {
                w.set_init_byte_2(0x3c);
            });
        assert_eq!(old, new, "publish_ble_phy_init_0554_byte_2");
    }
}

#[test]
fn publish_ble_phy_init_0554_byte_2_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_0554_byte_2();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0554_opaque(|w| {
                w.set_init_byte_2(0x3c);
            });
        assert_eq!(old, new, "publish_ble_phy_init_0554_byte_2");
    }
}

#[test]
fn publish_ble_phy_init_0554_byte_3_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_0554_byte_3();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0554_opaque(|w| {
                w.set_init_byte_3(0x28);
            });
        assert_eq!(old, new, "publish_ble_phy_init_0554_byte_3");
    }
}

#[test]
fn publish_ble_phy_init_0554_byte_3_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_0554_byte_3();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0554_opaque(|w| {
                w.set_init_byte_3(0x28);
            });
        assert_eq!(old, new, "publish_ble_phy_init_0554_byte_3");
    }
}

#[test]
fn publish_ble_phy_init_0558_byte_0_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_0558_byte_0();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0558_opaque(|w| {
                w.set_init_byte_0(0xc);
            });
        assert_eq!(old, new, "publish_ble_phy_init_0558_byte_0");
    }
}

#[test]
fn publish_ble_phy_init_0558_byte_0_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_0558_byte_0();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0558_opaque(|w| {
                w.set_init_byte_0(0xc);
            });
        assert_eq!(old, new, "publish_ble_phy_init_0558_byte_0");
    }
}

#[test]
fn publish_ble_phy_init_0558_byte_1_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_0558_byte_1();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0558_opaque(|w| {
                w.set_init_byte_1(0x8);
            });
        assert_eq!(old, new, "publish_ble_phy_init_0558_byte_1");
    }
}

#[test]
fn publish_ble_phy_init_0558_byte_1_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_0558_byte_1();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0558_opaque(|w| {
                w.set_init_byte_1(0x8);
            });
        assert_eq!(old, new, "publish_ble_phy_init_0558_byte_1");
    }
}

#[test]
fn publish_ble_phy_init_0558_byte_2_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_0558_byte_2();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0558_opaque(|w| {
                w.set_init_byte_2(0xc);
            });
        assert_eq!(old, new, "publish_ble_phy_init_0558_byte_2");
    }
}

#[test]
fn publish_ble_phy_init_0558_byte_2_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_0558_byte_2();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0558_opaque(|w| {
                w.set_init_byte_2(0xc);
            });
        assert_eq!(old, new, "publish_ble_phy_init_0558_byte_2");
    }
}

#[test]
fn publish_ble_phy_init_0558_byte_3_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_0558_byte_3();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0558_opaque(|w| {
                w.set_init_byte_3(0xc);
            });
        assert_eq!(old, new, "publish_ble_phy_init_0558_byte_3");
    }
}

#[test]
fn publish_ble_phy_init_0558_byte_3_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_0558_byte_3();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0558_opaque(|w| {
                w.set_init_byte_3(0xc);
            });
        assert_eq!(old, new, "publish_ble_phy_init_0558_byte_3");
    }
}

#[test]
fn publish_ble_phy_init_055c_byte_0_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_055c_byte_0();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_055c_opaque(|w| {
                w.set_init_byte_0(0x8);
            });
        assert_eq!(old, new, "publish_ble_phy_init_055c_byte_0");
    }
}

#[test]
fn publish_ble_phy_init_055c_byte_0_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_055c_byte_0();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_055c_opaque(|w| {
                w.set_init_byte_0(0x8);
            });
        assert_eq!(old, new, "publish_ble_phy_init_055c_byte_0");
    }
}

#[test]
fn publish_ble_phy_init_055c_byte_1_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_055c_byte_1();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_055c_opaque(|w| {
                w.set_init_byte_1(0x8);
            });
        assert_eq!(old, new, "publish_ble_phy_init_055c_byte_1");
    }
}

#[test]
fn publish_ble_phy_init_055c_byte_1_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_055c_byte_1();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_055c_opaque(|w| {
                w.set_init_byte_1(0x8);
            });
        assert_eq!(old, new, "publish_ble_phy_init_055c_byte_1");
    }
}

#[test]
fn publish_ble_phy_init_055c_byte_2_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_055c_byte_2();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_055c_opaque(|w| {
                w.set_init_byte_2(0x8);
            });
        assert_eq!(old, new, "publish_ble_phy_init_055c_byte_2");
    }
}

#[test]
fn publish_ble_phy_init_055c_byte_2_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_055c_byte_2();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_055c_opaque(|w| {
                w.set_init_byte_2(0x8);
            });
        assert_eq!(old, new, "publish_ble_phy_init_055c_byte_2");
    }
}

#[test]
fn publish_ble_phy_init_055c_byte_3_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_055c_byte_3();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_055c_opaque(|w| {
                w.set_init_byte_3(0x8);
            });
        assert_eq!(old, new, "publish_ble_phy_init_055c_byte_3");
    }
}

#[test]
fn publish_ble_phy_init_055c_byte_3_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_055c_byte_3();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_055c_opaque(|w| {
                w.set_init_byte_3(0x8);
            });
        assert_eq!(old, new, "publish_ble_phy_init_055c_byte_3");
    }
}

#[test]
fn publish_ble_phy_init_bytes_04a4_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_bytes_04a4();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_04a4_opaque(|w| {
                w.set_bits_0_7_opaque(0x2);
                w.set_bits_8_15_opaque(0x2);
                w.set_bits_16_23_opaque(0x2);
                w.set_bits_24_31_opaque(0x2);
            });
        assert_eq!(old, new, "publish_ble_phy_init_bytes_04a4");
    }
}

#[test]
fn publish_ble_phy_init_bytes_04a4_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_bytes_04a4();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_04a4_opaque(|w| {
                w.set_bits_0_7_opaque(0x2);
                w.set_bits_8_15_opaque(0x2);
                w.set_bits_16_23_opaque(0x2);
                w.set_bits_24_31_opaque(0x2);
            });
        assert_eq!(old, new, "publish_ble_phy_init_bytes_04a4");
    }
}

#[test]
fn publish_ble_phy_init_bytes_04a8_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_bytes_04a8();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_04a8_opaque(|w| {
                w.set_bits_0_7_opaque(0x2);
                w.set_bits_8_15_opaque(0x2);
                w.set_bits_16_23_opaque(0x2);
                w.set_bits_24_31_opaque(0x2);
            });
        assert_eq!(old, new, "publish_ble_phy_init_bytes_04a8");
    }
}

#[test]
fn publish_ble_phy_init_bytes_04a8_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_bytes_04a8();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_04a8_opaque(|w| {
                w.set_bits_0_7_opaque(0x2);
                w.set_bits_8_15_opaque(0x2);
                w.set_bits_16_23_opaque(0x2);
                w.set_bits_24_31_opaque(0x2);
            });
        assert_eq!(old, new, "publish_ble_phy_init_bytes_04a8");
    }
}

#[test]
fn publish_ble_phy_init_environment_member_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old).expect("block").publish_ble_phy_init_environment_member(oer_esp32s31_pac::btmac_ble_phy_init::phy_environment_member_pointer::AddressImage::new(reference(0x2f00_1230)));
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new).expect("block").modify_phy_environment_member_pointer(|w| { w.set_address(oer_esp32s31_pac::btmac_ble_phy_init::phy_environment_member_pointer::AddressImage::new(reference(0x2f00_1230))); });
        assert_eq!(old, new, "publish_ble_phy_init_environment_member");
    }
}

#[test]
fn publish_ble_phy_init_environment_member_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old).expect("block").publish_ble_phy_init_environment_member(oer_esp32s31_pac::btmac_ble_phy_init::phy_environment_member_pointer::AddressImage::new(reference(0x2f00_1270)));
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new).expect("block").modify_phy_environment_member_pointer(|w| { w.set_address(oer_esp32s31_pac::btmac_ble_phy_init::phy_environment_member_pointer::AddressImage::new(reference(0x2f00_1270))); });
        assert_eq!(old, new, "publish_ble_phy_init_environment_member");
    }
}

#[test]
fn publish_ble_phy_init_followup_image_1_0() {
    for fill in fills(oer_esp32s31_pac::ble_hw_runtime_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_runtime_control::BleHwRuntimeControl::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_followup_image_1();
        oer_esp32s31_pac::ble_hw_runtime_control::BleHwRuntimeControl::host(&mut new)
            .expect("block")
            .write_phy_init_configuration_followup(|w| {
                w.set_value(0x1);
            });
        assert_eq!(old, new, "publish_ble_phy_init_followup_image_1");
    }
}

#[test]
fn publish_ble_phy_init_followup_image_1_1() {
    for fill in fills(oer_esp32s31_pac::ble_hw_runtime_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_runtime_control::BleHwRuntimeControl::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_followup_image_1();
        oer_esp32s31_pac::ble_hw_runtime_control::BleHwRuntimeControl::host(&mut new)
            .expect("block")
            .write_phy_init_configuration_followup(|w| {
                w.set_value(0x1);
            });
        assert_eq!(old, new, "publish_ble_phy_init_followup_image_1");
    }
}

#[test]
fn publish_ble_phy_init_high_half_0458_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_high_half_0458();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0458_opaque(|w| {
                w.set_init_high_half(0xf);
            });
        assert_eq!(old, new, "publish_ble_phy_init_high_half_0458");
    }
}

#[test]
fn publish_ble_phy_init_high_half_0458_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_high_half_0458();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0458_opaque(|w| {
                w.set_init_high_half(0xf);
            });
        assert_eq!(old, new, "publish_ble_phy_init_high_half_0458");
    }
}

#[test]
fn publish_ble_phy_init_low_5_054c_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_low_5_054c();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_054c_opaque(|w| {
                w.set_init_low_5(oer_mmio::Bits::constant::<0x12>());
            });
        assert_eq!(old, new, "publish_ble_phy_init_low_5_054c");
    }
}

#[test]
fn publish_ble_phy_init_low_5_054c_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_low_5_054c();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_054c_opaque(|w| {
                w.set_init_low_5(oer_mmio::Bits::constant::<0x12>());
            });
        assert_eq!(old, new, "publish_ble_phy_init_low_5_054c");
    }
}

#[test]
fn publish_ble_phy_init_value_0138_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_value_0138();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0138_opaque(|w| {
                w.set_bits_0_31_opaque(0x65b);
            });
        assert_eq!(old, new, "publish_ble_phy_init_value_0138");
    }
}

#[test]
fn publish_ble_phy_init_value_0138_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_value_0138();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_0138_opaque(|w| {
                w.set_bits_0_31_opaque(0x65b);
            });
        assert_eq!(old, new, "publish_ble_phy_init_value_0138");
    }
}

#[test]
fn publish_ble_phy_init_value_045c_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_value_045c();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_045c_opaque(|w| {
                w.set_bits_0_31_opaque(0x8);
            });
        assert_eq!(old, new, "publish_ble_phy_init_value_045c");
    }
}

#[test]
fn publish_ble_phy_init_value_045c_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .publish_ble_phy_init_value_045c();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_word_045c_opaque(|w| {
                w.set_bits_0_31_opaque(0x8);
            });
        assert_eq!(old, new, "publish_ble_phy_init_value_045c");
    }
}

#[test]
fn publish_ble_phy_resolving_list_base_pointer_0() {
    for fill in fills(oer_esp32s31_pac::ble_hw_resolving_list::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_resolving_list::BleHwResolvingList::host(&mut old).expect("block").publish_ble_phy_resolving_list_base_pointer(oer_esp32s31_pac::ble_hw_resolving_list::base_pointer::CompressedSramPointerImage::new(reference(0x2f00_1230)).expect("encodable"));
        oer_esp32s31_pac::ble_hw_resolving_list::BleHwResolvingList::host(&mut new).expect("block").write_base_pointer(|w| { w.set_compressed_sram_pointer(oer_esp32s31_pac::ble_hw_resolving_list::base_pointer::CompressedSramPointerImage::new(reference(0x2f00_1230)).expect("encodable")); });
        assert_eq!(old, new, "publish_ble_phy_resolving_list_base_pointer");
    }
}

#[test]
fn publish_ble_phy_resolving_list_base_pointer_1() {
    for fill in fills(oer_esp32s31_pac::ble_hw_resolving_list::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_resolving_list::BleHwResolvingList::host(&mut old).expect("block").publish_ble_phy_resolving_list_base_pointer(oer_esp32s31_pac::ble_hw_resolving_list::base_pointer::CompressedSramPointerImage::new(reference(0x2f00_1270)).expect("encodable"));
        oer_esp32s31_pac::ble_hw_resolving_list::BleHwResolvingList::host(&mut new).expect("block").write_base_pointer(|w| { w.set_compressed_sram_pointer(oer_esp32s31_pac::ble_hw_resolving_list::base_pointer::CompressedSramPointerImage::new(reference(0x2f00_1270)).expect("encodable")); });
        assert_eq!(old, new, "publish_ble_phy_resolving_list_base_pointer");
    }
}

#[test]
fn publish_ble_phy_runtime_configuration_0() {
    for fill in fills(oer_esp32s31_pac::ble_hw_runtime_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_runtime_control::BleHwRuntimeControl::host(&mut old)
            .expect("block")
            .publish_ble_phy_runtime_configuration(0xa5, false);
        oer_esp32s31_pac::ble_hw_runtime_control::BleHwRuntimeControl::host(&mut new)
            .expect("block")
            .write_phy_init_configuration(|w| {
                w.set_value_low_8(0xa5);
                w.set_config_8(false);
            });
        assert_eq!(old, new, "publish_ble_phy_runtime_configuration");
    }
}

#[test]
fn publish_ble_phy_runtime_configuration_1() {
    for fill in fills(oer_esp32s31_pac::ble_hw_runtime_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_runtime_control::BleHwRuntimeControl::host(&mut old)
            .expect("block")
            .publish_ble_phy_runtime_configuration(0x5a, true);
        oer_esp32s31_pac::ble_hw_runtime_control::BleHwRuntimeControl::host(&mut new)
            .expect("block")
            .write_phy_init_configuration(|w| {
                w.set_value_low_8(0x5a);
                w.set_config_8(true);
            });
        assert_eq!(old, new, "publish_ble_phy_runtime_configuration");
    }
}

#[test]
fn publish_ble_positional_word_1ff0_image_0_0() {
    for fill in fills(oer_esp32s31_pac::ble_hw_positional_word_1ff0::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_positional_word_1ff0::BleHwPositionalWord1ff0::host(&mut old)
            .expect("block")
            .publish_ble_positional_word_1ff0_image_0();
        oer_esp32s31_pac::ble_hw_positional_word_1ff0::BleHwPositionalWord1ff0::host(&mut new)
            .expect("block")
            .write_value(|_| {});
        assert_eq!(old, new, "publish_ble_positional_word_1ff0_image_0");
    }
}

#[test]
fn publish_ble_positional_word_1ff0_image_0_1() {
    for fill in fills(oer_esp32s31_pac::ble_hw_positional_word_1ff0::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_positional_word_1ff0::BleHwPositionalWord1ff0::host(&mut old)
            .expect("block")
            .publish_ble_positional_word_1ff0_image_0();
        oer_esp32s31_pac::ble_hw_positional_word_1ff0::BleHwPositionalWord1ff0::host(&mut new)
            .expect("block")
            .write_value(|_| {});
        assert_eq!(old, new, "publish_ble_positional_word_1ff0_image_0");
    }
}

#[test]
fn publish_ble_positional_word_1ff0_image_1_0() {
    for fill in fills(oer_esp32s31_pac::ble_hw_positional_word_1ff0::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_positional_word_1ff0::BleHwPositionalWord1ff0::host(&mut old)
            .expect("block")
            .publish_ble_positional_word_1ff0_image_1();
        oer_esp32s31_pac::ble_hw_positional_word_1ff0::BleHwPositionalWord1ff0::host(&mut new)
            .expect("block")
            .write_value(|w| {
                w.set_bit_0_opaque(true);
            });
        assert_eq!(old, new, "publish_ble_positional_word_1ff0_image_1");
    }
}

#[test]
fn publish_ble_positional_word_1ff0_image_1_1() {
    for fill in fills(oer_esp32s31_pac::ble_hw_positional_word_1ff0::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_positional_word_1ff0::BleHwPositionalWord1ff0::host(&mut old)
            .expect("block")
            .publish_ble_positional_word_1ff0_image_1();
        oer_esp32s31_pac::ble_hw_positional_word_1ff0::BleHwPositionalWord1ff0::host(&mut new)
            .expect("block")
            .write_value(|w| {
                w.set_bit_0_opaque(true);
            });
        assert_eq!(old, new, "publish_ble_positional_word_1ff0_image_1");
    }
}

#[test]
fn publish_ble_positional_word_891c_image_20_0() {
    for fill in fills(oer_esp32s31_pac::ble_hw_positional_word_891c::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_positional_word_891c::BleHwPositionalWord891c::host(&mut old)
            .expect("block")
            .publish_ble_positional_word_891c_image_20();
        oer_esp32s31_pac::ble_hw_positional_word_891c::BleHwPositionalWord891c::host(&mut new)
            .expect("block")
            .write_value(|w| {
                w.set_bits_5_6_opaque(oer_mmio::Bits::constant::<0x1>());
            });
        assert_eq!(old, new, "publish_ble_positional_word_891c_image_20");
    }
}

#[test]
fn publish_ble_positional_word_891c_image_20_1() {
    for fill in fills(oer_esp32s31_pac::ble_hw_positional_word_891c::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_positional_word_891c::BleHwPositionalWord891c::host(&mut old)
            .expect("block")
            .publish_ble_positional_word_891c_image_20();
        oer_esp32s31_pac::ble_hw_positional_word_891c::BleHwPositionalWord891c::host(&mut new)
            .expect("block")
            .write_value(|w| {
                w.set_bits_5_6_opaque(oer_mmio::Bits::constant::<0x1>());
            });
        assert_eq!(old, new, "publish_ble_positional_word_891c_image_20");
    }
}

#[test]
fn publish_ble_positional_word_891c_image_40_0() {
    for fill in fills(oer_esp32s31_pac::ble_hw_positional_word_891c::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_positional_word_891c::BleHwPositionalWord891c::host(&mut old)
            .expect("block")
            .publish_ble_positional_word_891c_image_40();
        oer_esp32s31_pac::ble_hw_positional_word_891c::BleHwPositionalWord891c::host(&mut new)
            .expect("block")
            .write_value(|w| {
                w.set_bits_5_6_opaque(oer_mmio::Bits::constant::<0x2>());
            });
        assert_eq!(old, new, "publish_ble_positional_word_891c_image_40");
    }
}

#[test]
fn publish_ble_positional_word_891c_image_40_1() {
    for fill in fills(oer_esp32s31_pac::ble_hw_positional_word_891c::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_positional_word_891c::BleHwPositionalWord891c::host(&mut old)
            .expect("block")
            .publish_ble_positional_word_891c_image_40();
        oer_esp32s31_pac::ble_hw_positional_word_891c::BleHwPositionalWord891c::host(&mut new)
            .expect("block")
            .write_value(|w| {
                w.set_bits_5_6_opaque(oer_mmio::Bits::constant::<0x2>());
            });
        assert_eq!(old, new, "publish_ble_positional_word_891c_image_40");
    }
}

#[test]
fn publish_bluetooth_cte_sample_descriptor_pointer_0() {
    for fill in fills(oer_esp32s31_pac::ble_hw_cte_ring_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_cte_ring_control::BleHwCteRingControl::host(&mut old).expect("block").publish_bluetooth_cte_sample_descriptor_pointer(oer_mmio::Index::constant::<5>(), oer_esp32s31_pac::ble_hw_cte_ring_control::buffer_pointer::CompressedDescriptorPointerImage::new(reference(0x2f00_1270)).expect("encodable"));
        oer_esp32s31_pac::ble_hw_cte_ring_control::BleHwCteRingControl::host(&mut new).expect("block").modify_buffer_pointer(oer_mmio::Index::constant::<5>(), |w| { w.set_compressed_descriptor_pointer(oer_esp32s31_pac::ble_hw_cte_ring_control::buffer_pointer::CompressedDescriptorPointerImage::new(reference(0x2f00_1270)).expect("encodable")); });
        assert_eq!(old, new, "publish_bluetooth_cte_sample_descriptor_pointer");
    }
}

#[test]
fn publish_bluetooth_cte_sample_descriptor_pointer_1() {
    for fill in fills(oer_esp32s31_pac::ble_hw_cte_ring_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_hw_cte_ring_control::BleHwCteRingControl::host(&mut old).expect("block").publish_bluetooth_cte_sample_descriptor_pointer(oer_mmio::Index::constant::<5>(), oer_esp32s31_pac::ble_hw_cte_ring_control::buffer_pointer::CompressedDescriptorPointerImage::new(reference(0x2f00_12b0)).expect("encodable"));
        oer_esp32s31_pac::ble_hw_cte_ring_control::BleHwCteRingControl::host(&mut new).expect("block").modify_buffer_pointer(oer_mmio::Index::constant::<5>(), |w| { w.set_compressed_descriptor_pointer(oer_esp32s31_pac::ble_hw_cte_ring_control::buffer_pointer::CompressedDescriptorPointerImage::new(reference(0x2f00_12b0)).expect("encodable")); });
        assert_eq!(old, new, "publish_bluetooth_cte_sample_descriptor_pointer");
    }
}

#[test]
fn publish_bluetooth_device_table_first_entry_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old).expect("block").publish_bluetooth_device_table_first_entry(oer_esp32s31_pac::btmac_ble_phy_init::device_table_first_entry::CompressedAddressImage::new(reference(0x2f00_1230)).expect("encodable"));
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new).expect("block").write_device_table_first_entry(|w| { w.set_compressed_address(oer_esp32s31_pac::btmac_ble_phy_init::device_table_first_entry::CompressedAddressImage::new(reference(0x2f00_1230)).expect("encodable")); });
        assert_eq!(old, new, "publish_bluetooth_device_table_first_entry");
    }
}

#[test]
fn publish_bluetooth_device_table_first_entry_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old).expect("block").publish_bluetooth_device_table_first_entry(oer_esp32s31_pac::btmac_ble_phy_init::device_table_first_entry::CompressedAddressImage::new(reference(0x2f00_1270)).expect("encodable"));
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new).expect("block").write_device_table_first_entry(|w| { w.set_compressed_address(oer_esp32s31_pac::btmac_ble_phy_init::device_table_first_entry::CompressedAddressImage::new(reference(0x2f00_1270)).expect("encodable")); });
        assert_eq!(old, new, "publish_bluetooth_device_table_first_entry");
    }
}

#[test]
fn publish_bluetooth_hal_scheduler_byte_1_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_bluetooth_hal_scheduler_byte_1();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_scheduler_control(|w| {
                w.set_config_byte_1(0x20);
            });
        assert_eq!(old, new, "publish_bluetooth_hal_scheduler_byte_1");
    }
}

#[test]
fn publish_bluetooth_hal_scheduler_byte_1_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_bluetooth_hal_scheduler_byte_1();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_scheduler_control(|w| {
                w.set_config_byte_1(0x20);
            });
        assert_eq!(old, new, "publish_bluetooth_hal_scheduler_byte_1");
    }
}

#[test]
fn publish_bluetooth_hal_scheduler_config_16_20_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_bluetooth_hal_scheduler_config_16_20();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_scheduler_control(|w| {
                w.set_config_16_20(oer_mmio::Bits::constant::<0x10>());
            });
        assert_eq!(old, new, "publish_bluetooth_hal_scheduler_config_16_20");
    }
}

#[test]
fn publish_bluetooth_hal_scheduler_config_16_20_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_bluetooth_hal_scheduler_config_16_20();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_scheduler_control(|w| {
                w.set_config_16_20(oer_mmio::Bits::constant::<0x10>());
            });
        assert_eq!(old, new, "publish_bluetooth_hal_scheduler_config_16_20");
    }
}

#[test]
fn publish_bluetooth_hal_scheduler_sram_prefix_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_bluetooth_hal_scheduler_sram_prefix();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_scheduler_sram_pointer_prefix(|w| {
                w.set_bits_0_21_opaque(oer_mmio::Bits::constant::<0x0>());
                w.set_address_prefix(oer_mmio::Bits::constant::<0xbc>());
            });
        assert_eq!(old, new, "publish_bluetooth_hal_scheduler_sram_prefix");
    }
}

#[test]
fn publish_bluetooth_hal_scheduler_sram_prefix_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_bluetooth_hal_scheduler_sram_prefix();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_scheduler_sram_pointer_prefix(|w| {
                w.set_bits_0_21_opaque(oer_mmio::Bits::constant::<0x0>());
                w.set_address_prefix(oer_mmio::Bits::constant::<0xbc>());
            });
        assert_eq!(old, new, "publish_bluetooth_hal_scheduler_sram_prefix");
    }
}

#[test]
fn publish_bluetooth_hal_slot_lane_0_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_bluetooth_hal_slot_lane_0(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_0_enable(true);
                w.set_lane_0_index_high(false);
            });
        assert_eq!(old, new, "publish_bluetooth_hal_slot_lane_0");
    }
}

#[test]
fn publish_bluetooth_hal_slot_lane_0_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_bluetooth_hal_slot_lane_0(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_0_enable(true);
                w.set_lane_0_index_high(false);
            });
        assert_eq!(old, new, "publish_bluetooth_hal_slot_lane_0");
    }
}

#[test]
fn publish_bluetooth_hal_slot_lane_1_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_bluetooth_hal_slot_lane_1(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_1_enable(true);
                w.set_lane_1_retained_index_low(true);
                w.set_lane_1_index_high(false);
            });
        assert_eq!(old, new, "publish_bluetooth_hal_slot_lane_1");
    }
}

#[test]
fn publish_bluetooth_hal_slot_lane_1_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_bluetooth_hal_slot_lane_1(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_1_enable(true);
                w.set_lane_1_retained_index_low(true);
                w.set_lane_1_index_high(false);
            });
        assert_eq!(old, new, "publish_bluetooth_hal_slot_lane_1");
    }
}

#[test]
fn publish_bluetooth_hal_slot_lane_2_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_bluetooth_hal_slot_lane_2(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_2_enable(true);
                w.set_lane_2_index_high(true);
            });
        assert_eq!(old, new, "publish_bluetooth_hal_slot_lane_2");
    }
}

#[test]
fn publish_bluetooth_hal_slot_lane_2_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_bluetooth_hal_slot_lane_2(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_2_enable(true);
                w.set_lane_2_index_high(true);
            });
        assert_eq!(old, new, "publish_bluetooth_hal_slot_lane_2");
    }
}

#[test]
fn publish_bluetooth_hal_slot_lane_3_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_bluetooth_hal_slot_lane_3(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_3_enable(true);
                w.set_lane_3_retained_index_low(true);
                w.set_lane_3_index_high(true);
            });
        assert_eq!(old, new, "publish_bluetooth_hal_slot_lane_3");
    }
}

#[test]
fn publish_bluetooth_hal_slot_lane_3_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_bluetooth_hal_slot_lane_3(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_3_enable(true);
                w.set_lane_3_retained_index_low(true);
                w.set_lane_3_index_high(true);
            });
        assert_eq!(old, new, "publish_bluetooth_hal_slot_lane_3");
    }
}

#[test]
fn publish_bluetooth_hal_slot_lane_4_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_bluetooth_hal_slot_lane_4(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_4_enable(true);
                w.set_lane_4_index_high(false);
            });
        assert_eq!(old, new, "publish_bluetooth_hal_slot_lane_4");
    }
}

#[test]
fn publish_bluetooth_hal_slot_lane_4_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_bluetooth_hal_slot_lane_4(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_4_enable(true);
                w.set_lane_4_index_high(false);
            });
        assert_eq!(old, new, "publish_bluetooth_hal_slot_lane_4");
    }
}

#[test]
fn publish_bluetooth_hal_slot_lane_5_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_bluetooth_hal_slot_lane_5(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_5_enable(true);
                w.set_lane_5_retained_index_low(true);
                w.set_lane_5_index_high(false);
            });
        assert_eq!(old, new, "publish_bluetooth_hal_slot_lane_5");
    }
}

#[test]
fn publish_bluetooth_hal_slot_lane_5_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_bluetooth_hal_slot_lane_5(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_5_enable(true);
                w.set_lane_5_retained_index_low(true);
                w.set_lane_5_index_high(false);
            });
        assert_eq!(old, new, "publish_bluetooth_hal_slot_lane_5");
    }
}

#[test]
fn publish_bluetooth_hal_slot_lane_6_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_bluetooth_hal_slot_lane_6(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_6_enable(true);
                w.set_lane_6_index_high(true);
            });
        assert_eq!(old, new, "publish_bluetooth_hal_slot_lane_6");
    }
}

#[test]
fn publish_bluetooth_hal_slot_lane_6_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_bluetooth_hal_slot_lane_6(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_6_enable(true);
                w.set_lane_6_index_high(true);
            });
        assert_eq!(old, new, "publish_bluetooth_hal_slot_lane_6");
    }
}

#[test]
fn publish_bluetooth_hal_slot_lane_7_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_bluetooth_hal_slot_lane_7(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_7_enable(true);
                w.set_lane_7_retained_index_low(true);
                w.set_lane_7_index_high(true);
            });
        assert_eq!(old, new, "publish_bluetooth_hal_slot_lane_7");
    }
}

#[test]
fn publish_bluetooth_hal_slot_lane_7_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_bluetooth_hal_slot_lane_7(oer_mmio::Index::constant::<1>());
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_hal_init_slot_map(oer_mmio::Index::constant::<1>(), |w| {
                w.set_lane_7_enable(true);
                w.set_lane_7_retained_index_low(true);
                w.set_lane_7_index_high(true);
            });
        assert_eq!(old, new, "publish_bluetooth_hal_slot_lane_7");
    }
}

#[test]
fn publish_bluetooth_modem_lp_timer_control_1_0() {
    for fill in fills(oer_esp32s31_pac::btdm_runtime_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btdm_runtime_control::BtdmRuntimeControl::host(&mut old)
            .expect("block")
            .publish_bluetooth_modem_lp_timer_control_1();
        oer_esp32s31_pac::btdm_runtime_control::BtdmRuntimeControl::host(&mut new)
            .expect("block")
            .modify_word_0058_opaque(|w| {
                w.set_control_1(true);
            });
        assert_eq!(old, new, "publish_bluetooth_modem_lp_timer_control_1");
    }
}

#[test]
fn publish_bluetooth_modem_lp_timer_control_1_1() {
    for fill in fills(oer_esp32s31_pac::btdm_runtime_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btdm_runtime_control::BtdmRuntimeControl::host(&mut old)
            .expect("block")
            .publish_bluetooth_modem_lp_timer_control_1();
        oer_esp32s31_pac::btdm_runtime_control::BtdmRuntimeControl::host(&mut new)
            .expect("block")
            .modify_word_0058_opaque(|w| {
                w.set_control_1(true);
            });
        assert_eq!(old, new, "publish_bluetooth_modem_lp_timer_control_1");
    }
}

#[test]
fn publish_bluetooth_scan_standard_upper_limit_max_0() {
    for fill in fills(oer_esp32s31_pac::ble_scan_backoff::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_scan_backoff::BleScanBackoff::host(&mut old)
            .expect("block")
            .publish_bluetooth_scan_standard_upper_limit_max();
        oer_esp32s31_pac::ble_scan_backoff::BleScanBackoff::host(&mut new)
            .expect("block")
            .write_upper_limit_max(|w| {
                w.set_value(oer_mmio::Bits::constant::<0x20>());
            });
        assert_eq!(old, new, "publish_bluetooth_scan_standard_upper_limit_max");
    }
}

#[test]
fn publish_bluetooth_scan_standard_upper_limit_max_1() {
    for fill in fills(oer_esp32s31_pac::ble_scan_backoff::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::ble_scan_backoff::BleScanBackoff::host(&mut old)
            .expect("block")
            .publish_bluetooth_scan_standard_upper_limit_max();
        oer_esp32s31_pac::ble_scan_backoff::BleScanBackoff::host(&mut new)
            .expect("block")
            .write_upper_limit_max(|w| {
                w.set_value(oer_mmio::Bits::constant::<0x20>());
            });
        assert_eq!(old, new, "publish_bluetooth_scan_standard_upper_limit_max");
    }
}

#[test]
fn publish_bluetooth_scheduler_hardware_list_head_0() {
    for fill in fills(oer_esp32s31_pac::btdm_scheduler_table::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btdm_scheduler_table::BtdmSchedulerTable::host(&mut old)
            .expect("block")
            .publish_bluetooth_scheduler_hardware_list_head(
                oer_mmio::Index::constant::<15>(),
                oer_esp32s31_pac::btdm_scheduler_table::entry::HeadPointerImage::new(reference(
                    0x2f00_1270,
                ))
                .expect("encodable"),
            );
        oer_esp32s31_pac::btdm_scheduler_table::BtdmSchedulerTable::host(&mut new)
            .expect("block")
            .modify_entry(oer_mmio::Index::constant::<15>(), |w| {
                w.set_head_pointer(
                    oer_esp32s31_pac::btdm_scheduler_table::entry::HeadPointerImage::new(
                        reference(0x2f00_1270),
                    )
                    .expect("encodable"),
                );
            });
        assert_eq!(old, new, "publish_bluetooth_scheduler_hardware_list_head");
    }
}

#[test]
fn publish_bluetooth_scheduler_hardware_list_head_1() {
    for fill in fills(oer_esp32s31_pac::btdm_scheduler_table::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btdm_scheduler_table::BtdmSchedulerTable::host(&mut old)
            .expect("block")
            .publish_bluetooth_scheduler_hardware_list_head(
                oer_mmio::Index::constant::<15>(),
                oer_esp32s31_pac::btdm_scheduler_table::entry::HeadPointerImage::new(reference(
                    0x2f00_12b0,
                ))
                .expect("encodable"),
            );
        oer_esp32s31_pac::btdm_scheduler_table::BtdmSchedulerTable::host(&mut new)
            .expect("block")
            .modify_entry(oer_mmio::Index::constant::<15>(), |w| {
                w.set_head_pointer(
                    oer_esp32s31_pac::btdm_scheduler_table::entry::HeadPointerImage::new(
                        reference(0x2f00_12b0),
                    )
                    .expect("encodable"),
                );
            });
        assert_eq!(old, new, "publish_bluetooth_scheduler_hardware_list_head");
    }
}

#[test]
fn publish_bluetooth_scheduler_lifecycle_request_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_bluetooth_scheduler_lifecycle_request();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .write_scheduler_lifecycle_request(|w| {
                w.set_value(0x1);
            });
        assert_eq!(old, new, "publish_bluetooth_scheduler_lifecycle_request");
    }
}

#[test]
fn publish_bluetooth_scheduler_lifecycle_request_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .publish_bluetooth_scheduler_lifecycle_request();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .write_scheduler_lifecycle_request(|w| {
                w.set_value(0x1);
            });
        assert_eq!(old, new, "publish_bluetooth_scheduler_lifecycle_request");
    }
}

#[test]
fn release_bluetooth_interrupt_output_0_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_interrupt_bank::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut old)
            .expect("block")
            .release_bluetooth_interrupt_output_0();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut new)
            .expect("block")
            .write_irq_control_1(|w| {
                w.set_command(0x1);
            });
        assert_eq!(old, new, "release_bluetooth_interrupt_output_0");
    }
}

#[test]
fn release_bluetooth_interrupt_output_0_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_interrupt_bank::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut old)
            .expect("block")
            .release_bluetooth_interrupt_output_0();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut new)
            .expect("block")
            .write_irq_control_1(|w| {
                w.set_command(0x1);
            });
        assert_eq!(old, new, "release_bluetooth_interrupt_output_0");
    }
}

#[test]
fn release_bluetooth_interrupt_output_1_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_interrupt_bank::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut old)
            .expect("block")
            .release_bluetooth_interrupt_output_1();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut new)
            .expect("block")
            .write_irq_control_2(|w| {
                w.set_command(0x1);
            });
        assert_eq!(old, new, "release_bluetooth_interrupt_output_1");
    }
}

#[test]
fn release_bluetooth_interrupt_output_1_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_interrupt_bank::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut old)
            .expect("block")
            .release_bluetooth_interrupt_output_1();
        oer_esp32s31_pac::bluetooth_interrupt_bank::BluetoothInterruptBank::host(&mut new)
            .expect("block")
            .write_irq_control_2(|w| {
                w.set_command(0x1);
            });
        assert_eq!(old, new, "release_bluetooth_interrupt_output_1");
    }
}

#[test]
fn request_bluetooth_controller_time_latch_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .request_bluetooth_controller_time_latch();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_btlc_timer_control(|w| {
                w.set_latch_request(true);
            });
        assert_eq!(old, new, "request_bluetooth_controller_time_latch");
    }
}

#[test]
fn request_bluetooth_controller_time_latch_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .request_bluetooth_controller_time_latch();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_btlc_timer_control(|w| {
                w.set_latch_request(true);
            });
        assert_eq!(old, new, "request_bluetooth_controller_time_latch");
    }
}

#[test]
fn set_ble_connection_link_state_control_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .set_ble_connection_link_state_control();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_connection_abort_control(|w| {
                w.set_link_state_control(true);
            });
        assert_eq!(old, new, "set_ble_connection_link_state_control");
    }
}

#[test]
fn set_ble_connection_link_state_control_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .set_ble_connection_link_state_control();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_connection_abort_control(|w| {
                w.set_link_state_control(true);
            });
        assert_eq!(old, new, "set_ble_connection_link_state_control");
    }
}

#[test]
fn set_ble_phy_device_table_control_bit_18_0() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .set_ble_phy_device_table_control_bit_18();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_device_table_control(|w| {
                w.set_bit_18_opaque(true);
            });
        assert_eq!(old, new, "set_ble_phy_device_table_control_bit_18");
    }
}

#[test]
fn set_ble_phy_device_table_control_bit_18_1() {
    for fill in fills(oer_esp32s31_pac::btmac_ble_phy_init::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
            .expect("block")
            .set_ble_phy_device_table_control_bit_18();
        oer_esp32s31_pac::btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
            .expect("block")
            .modify_device_table_control(|w| {
                w.set_bit_18_opaque(true);
            });
        assert_eq!(old, new, "set_ble_phy_device_table_control_bit_18");
    }
}

#[test]
fn set_bluetooth_receive_saturation_gain_004c_bits_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .set_bluetooth_receive_saturation_gain_004c_bits();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_004c_opaque(|w| {
                w.set_bit_19_opaque(true);
                w.set_bit_24_opaque(true);
                w.set_bit_26_opaque(true);
                w.set_bit_28_opaque(true);
                w.set_bit_30_opaque(true);
                w.set_bit_31_opaque(true);
            });
        assert_eq!(old, new, "set_bluetooth_receive_saturation_gain_004c_bits");
    }
}

#[test]
fn set_bluetooth_receive_saturation_gain_004c_bits_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .set_bluetooth_receive_saturation_gain_004c_bits();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_004c_opaque(|w| {
                w.set_bit_19_opaque(true);
                w.set_bit_24_opaque(true);
                w.set_bit_26_opaque(true);
                w.set_bit_28_opaque(true);
                w.set_bit_30_opaque(true);
                w.set_bit_31_opaque(true);
            });
        assert_eq!(old, new, "set_bluetooth_receive_saturation_gain_004c_bits");
    }
}

#[test]
fn set_bluetooth_receive_saturation_gain_0088_bit_30_0() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .set_bluetooth_receive_saturation_gain_0088_bit_30();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0088_opaque(|w| {
                w.set_bit_30_opaque(true);
            });
        assert_eq!(
            old, new,
            "set_bluetooth_receive_saturation_gain_0088_bit_30"
        );
    }
}

#[test]
fn set_bluetooth_receive_saturation_gain_0088_bit_30_1() {
    for fill in fills(oer_esp32s31_pac::phy_btagc_recovered::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut old)
            .expect("block")
            .set_bluetooth_receive_saturation_gain_0088_bit_30();
        oer_esp32s31_pac::phy_btagc_recovered::PhyBtagcRecovered::host(&mut new)
            .expect("block")
            .modify_word_0088_opaque(|w| {
                w.set_bit_30_opaque(true);
            });
        assert_eq!(
            old, new,
            "set_bluetooth_receive_saturation_gain_0088_bit_30"
        );
    }
}

#[test]
fn set_bluetooth_receive_setup_control_0_bit_0_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .set_bluetooth_receive_setup_control_0_bit_0();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_setup_control_0(|w| {
                w.set_bit_0_opaque(true);
            });
        assert_eq!(old, new, "set_bluetooth_receive_setup_control_0_bit_0");
    }
}

#[test]
fn set_bluetooth_receive_setup_control_0_bit_0_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .set_bluetooth_receive_setup_control_0_bit_0();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_setup_control_0(|w| {
                w.set_bit_0_opaque(true);
            });
        assert_eq!(old, new, "set_bluetooth_receive_setup_control_0_bit_0");
    }
}

#[test]
fn set_bluetooth_receive_setup_control_1_bit_2_0() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .set_bluetooth_receive_setup_control_1_bit_2();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_setup_control_1(|w| {
                w.set_bit_2_opaque(true);
            });
        assert_eq!(old, new, "set_bluetooth_receive_setup_control_1_bit_2");
    }
}

#[test]
fn set_bluetooth_receive_setup_control_1_bit_2_1() {
    for fill in fills(oer_esp32s31_pac::bt_v3_2_baseband::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
            .expect("block")
            .split_rx_info()
            .0
            .set_bluetooth_receive_setup_control_1_bit_2();
        oer_esp32s31_pac::bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
            .expect("block")
            .split_rx_info()
            .0
            .modify_rx_setup_control_1(|w| {
                w.set_bit_2_opaque(true);
            });
        assert_eq!(old, new, "set_bluetooth_receive_setup_control_1_bit_2");
    }
}

#[test]
fn set_bluetooth_scheduler_cancellation_control_0() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .set_bluetooth_scheduler_cancellation_control();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_word_0204_opaque(|w| {
                w.set_control_0(true);
            });
        assert_eq!(old, new, "set_bluetooth_scheduler_cancellation_control");
    }
}

#[test]
fn set_bluetooth_scheduler_cancellation_control_1() {
    for fill in fills(oer_esp32s31_pac::bluetooth_controller_core::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
            .expect("block")
            .set_bluetooth_scheduler_cancellation_control();
        oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
            .expect("block")
            .modify_word_0204_opaque(|w| {
                w.set_control_0(true);
            });
        assert_eq!(old, new, "set_bluetooth_scheduler_cancellation_control");
    }
}

#[test]
fn trigger_bluetooth_modem_lp_timer_command_0() {
    for fill in fills(oer_esp32s31_pac::btdm_runtime_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btdm_runtime_control::BtdmRuntimeControl::host(&mut old)
            .expect("block")
            .trigger_bluetooth_modem_lp_timer_command();
        oer_esp32s31_pac::btdm_runtime_control::BtdmRuntimeControl::host(&mut new)
            .expect("block")
            .write_word_0010_opaque(|w| {
                w.set_bit_18_opaque(true);
            });
        assert_eq!(old, new, "trigger_bluetooth_modem_lp_timer_command");
    }
}

#[test]
fn trigger_bluetooth_modem_lp_timer_command_1() {
    for fill in fills(oer_esp32s31_pac::btdm_runtime_control::SIZE.div_ceil(4)) {
        let mut old = fill.clone();
        let mut new = fill.clone();
        oer_esp32s31_pac::btdm_runtime_control::BtdmRuntimeControl::host(&mut old)
            .expect("block")
            .trigger_bluetooth_modem_lp_timer_command();
        oer_esp32s31_pac::btdm_runtime_control::BtdmRuntimeControl::host(&mut new)
            .expect("block")
            .write_word_0010_opaque(|w| {
                w.set_bit_18_opaque(true);
            });
        assert_eq!(old, new, "trigger_bluetooth_modem_lp_timer_command");
    }
}

// ---- hand-converted transactions with typed arguments ----

#[test]
fn domain_typed_transactions() {
    use oer_esp32s31_pac::*;
    for fill in fills(bluetooth_controller_core::SIZE.div_ceil(4)) {
        for v in [0u8, 1, 0x5a, 0xff] {
            let (mut old, mut new) = (fill.clone(), fill.clone());
            bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
                .unwrap()
                .publish_bluetooth_hal_value_0(BluetoothHalInitByte::new(u32::from(v)).unwrap());
            bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
                .unwrap()
                .modify_hal_init_bytes(|w| {
                    w.set_value_0(v);
                });
            assert_eq!(old, new);
            let (mut old, mut new) = (fill.clone(), fill.clone());
            bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
                .unwrap()
                .publish_bluetooth_hal_value_1(BluetoothHalInitByte::new(u32::from(v)).unwrap());
            bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
                .unwrap()
                .modify_hal_init_bytes(|w| {
                    w.set_value_1(v);
                });
            assert_eq!(old, new);
        }
        for (shift, domain) in [
            (2u8, BluetoothHalSleepTimerShift::Two),
            (3, BluetoothHalSleepTimerShift::Three),
            (4, BluetoothHalSleepTimerShift::Four),
            (5, BluetoothHalSleepTimerShift::Five),
        ] {
            let (mut old, mut new) = (fill.clone(), fill.clone());
            bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
                .unwrap()
                .publish_bluetooth_hal_sleep_timer_shift(domain);
            bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
                .unwrap()
                .modify_btlc_timer_control(|w| {
                    w.set_clock_shift(oer_mmio::Bits::truncate(u32::from(shift)));
                });
            assert_eq!(old, new);
        }
        for index in 0..16u8 {
            let typed =
                crate::bluetooth::registers::BluetoothSchedulerHardwareListIndex::new(index)
                    .unwrap();
            let (mut old, mut new) = (fill.clone(), fill.clone());
            bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
                .unwrap()
                .publish_bluetooth_scheduler_lock_modify_hardware_list_index(
                    BluetoothSchedulerLockModifyHardwareListIndex::new(u32::from(index)).unwrap(),
                );
            bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
                .unwrap()
                .modify_word_036c_opaque(|w| {
                    w.set_lock_modify_hardware_list_index(typed.field_bits());
                });
            assert_eq!(old, new);
            let (mut old, mut new) = (fill.clone(), fill.clone());
            bluetooth_controller_core::BluetoothControllerCore::host(&mut old)
                .unwrap()
                .publish_bluetooth_scheduler_cancellation_hardware_list_index(
                    BluetoothSchedulerCancellationHardwareListIndex::new(u32::from(index)).unwrap(),
                );
            bluetooth_controller_core::BluetoothControllerCore::host(&mut new)
                .unwrap()
                .modify_word_036c_opaque(|w| {
                    w.set_cancellation_hardware_list_index(typed.field_bits());
                });
            assert_eq!(old, new);
        }
    }
    for fill in fills(btmac_ble_phy_init::SIZE.div_ceil(4)) {
        for v in [0u8, 1, 0x5a, 0xff] {
            let (mut old, mut new) = (fill.clone(), fill.clone());
            btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
                .unwrap()
                .publish_bluetooth_device_table_entry_count(
                    BluetoothDeviceTableEntryCount::new(u32::from(v)).unwrap(),
                );
            btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
                .unwrap()
                .modify_device_table_control(|w| {
                    w.set_entry_count(v);
                });
            assert_eq!(old, new);
            let (mut old, mut new) = (fill.clone(), fill.clone());
            btmac_ble_phy_init::BtmacBlePhyInit::host(&mut old)
                .unwrap()
                .or_ble_phy_init_tx_on_delay(
                    BluetoothPhyInitTimingByte::new(u32::from(v)).unwrap(),
                );
            btmac_ble_phy_init::BtmacBlePhyInit::host(&mut new)
                .unwrap()
                .modify_lc_tx_on_delay_config(|w| {
                    w.set_lc_tx_on_delay(w.lc_tx_on_delay() | v);
                    w.set_init_duplicate_byte(w.init_duplicate_byte() | v);
                });
            assert_eq!(old, new);
        }
    }
    for fill in fills(btdm_runtime_control::SIZE.div_ceil(4)) {
        for v in [0u32, 1, 0x00ff_ffff, 0xa55a_3cc3, u32::MAX] {
            let (mut old, mut new) = (fill.clone(), fill.clone());
            btdm_runtime_control::BtdmRuntimeControl::host(&mut old)
                .unwrap()
                .publish_bluetooth_modem_lp_timer_compare(BluetoothModemLpTimerCompareImage::new(
                    v,
                ));
            btdm_runtime_control::BtdmRuntimeControl::host(&mut new)
                .unwrap()
                .modify_word_006c_opaque(|w| {
                    w.set_bits_0_31_opaque(v);
                });
            assert_eq!(old, new);
        }
    }
    for fill in fills(phy_btagc_recovered::SIZE.div_ceil(4)) {
        for v in [0u8, 1, 4, 5, 6, 0x5a, 0x7f, 0x80, 0xff] {
            let (mut old, mut new) = (fill.clone(), fill.clone());
            let p = BluetoothBasebandGainParameterByte::new(u32::from(v)).unwrap();
            {
                let mut b = phy_btagc_recovered::PhyBtagcRecovered::host(&mut old).unwrap();
                b.initialize_bluetooth_receive_gain_force(p);
                b.initialize_bluetooth_receive_gain_image(p);
            }
            {
                let mut b = phy_btagc_recovered::PhyBtagcRecovered::host(&mut new).unwrap();
                let parameter = u32::from(v);
                b.modify_rx_gain_force(|w| {
                    w.set_dynamic_bits_0_6(oer_mmio::Bits::truncate(parameter));
                });
                b.modify_agc_gain_image(|w| {
                    w.set_dynamic_bits_2_8(oer_mmio::Bits::truncate(parameter.wrapping_sub(5)));
                });
            }
            assert_eq!(old, new);
        }
    }
    for fill in fills(bt_v3_2_baseband::SIZE.div_ceil(4)) {
        for v in [0u8, 1, 9, 10, 11, 0x5a, 0xff] {
            let (mut old, mut new) = (fill.clone(), fill.clone());
            let (rest, _) = bt_v3_2_baseband::BtV3_2Baseband::host(&mut old)
                .unwrap()
                .split_rx_info();
            let mut rest = rest;
            rest.publish_ble_phy_le_tx_on_delay(
                BluetoothPhyInitTimingByte::new(u32::from(v.wrapping_sub(10))).unwrap(),
            );
            let (mut rest2, _) = bt_v3_2_baseband::BtV3_2Baseband::host(&mut new)
                .unwrap()
                .split_rx_info();
            rest2.modify_le_tx_on_delay(|w| {
                w.set_encoded_value_minus_10(v.wrapping_sub(10));
                w.set_bits_16_18_opaque(oer_mmio::Bits::constant::<0>());
            });
            drop(rest);
            drop(rest2);
            assert_eq!(old, new);
        }
    }
}
