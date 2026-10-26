//! The owners' operations over host memory.
//!
//! Each test runs the real operation on host blocks and checks what it wrote
//! through the generated typed readers, or, where the vendor source names the
//! word and its image, against that absolute address less the block's base,
//! comparing the whole memory.

#![forbid(unsafe_code)]

use std::vec::Vec;

use oer_esp32s31_pac::{bt_bb::BtV32Baseband, instances};
use oer_mmio::Index;

use super::host::{HostMemory, changed};
use super::{
    BluetoothControllerHalInitConfig, BluetoothInterruptSetup, BluetoothMemoryListPointerImage,
    BluetoothMemoryListSelector, BluetoothMemoryListSlot, BluetoothModemLpTimerCompareDisposition,
    BluetoothModemLpTimerEpoch, BluetoothModemLpTimerInstant, BluetoothPhyRegisterInitInputs,
    BluetoothSchedulerHardwareListHead, BluetoothSchedulerHardwareListIndex,
};

/// The word index of the vendor's absolute address in a block at `base`.
fn word(address: usize, base: usize) -> usize {
    (address - base) / 4
}

/// Each changed word of a block as its absolute address and new value.
fn absolute(base: usize, before: &[u32], after: &[u32]) -> Vec<(usize, u32)> {
    changed(before, after)
        .into_iter()
        .map(|(index, value)| (base + index * 4, value))
        .collect()
}

#[test]
fn hal_init_publishes_the_sram_prefix_and_the_reviewed_profile() {
    let before = HostMemory::filled(0);
    let mut memory = before.clone();
    {
        let (mut task, _, _) = memory.owners();
        task.initialize_controller_hal(BluetoothControllerHalInitConfig::reviewed_standalone());
        let core = &task.bluetooth.bluetooth_controller_core;
        // The standalone period doubles each byte: 11 and 33.
        assert_eq!(core.read_hal_init_bytes().value_0(), 22);
        assert_eq!(core.read_hal_init_bytes().value_1(), 66);
        assert_eq!(core.read_btlc_timer_control().clock_shift().get(), 3);
        let latch = core.read_hal_init_latch();
        assert!(latch.config_6() && latch.enable_31());
        let lanes = core.read_hal_init_slot_map(Index::constant::<1>());
        assert!(lanes.lane_3_enable() && lanes.lane_3_retained_index_low());
        assert!(lanes.lane_3_index_high() && !lanes.lane_0_index_high());
    }
    // SOURCE: BLOB_LIBBTDM_HAL_CONTROLLER_INIT: `0x2010_1074 = 0x2f00_0000`.
    let base = instances::bt_mac::BASE;
    assert_eq!(memory.core[word(0x2010_1074, base)], 0x2f00_0000);
    assert_eq!(
        before.phy_init, memory.phy_init,
        "only the core's words change"
    );
    assert_eq!(before.interrupt_bank, memory.interrupt_bank);
}

#[test]
fn hal_init_keeps_the_sleep_timer_control_bits_it_does_not_own() {
    let mut memory = HostMemory::filled(u32::MAX);
    let (mut task, _, _) = memory.owners();
    task.initialize_controller_hal(BluetoothControllerHalInitConfig::reviewed_standalone());
    let control = task
        .bluetooth
        .bluetooth_controller_core
        .read_btlc_timer_control();
    assert_eq!(control.clock_shift().get(), 3);
    assert!(control.config_24() && control.timer_arm());
}

#[test]
fn address_slots_are_written_low_word_then_high_halfword() {
    let before = HostMemory::filled(u32::MAX);
    let mut memory = before.clone();
    {
        let (mut task, _, _) = memory.owners();
        task.program_bluetooth_random_device_address([1, 2, 3, 4, 5, 6]);
    }
    let base = instances::bt_mac::BASE;
    assert_eq!(
        absolute(base, &before.address_slots, &memory.address_slots),
        [(0x2010_1814, 0x0403_0201), (0x2010_1818, 0x0000_0605)],
        "slot one, each word written from zero"
    );
}

#[test]
fn ble_phy_initialization_points_the_controller_at_its_memory() {
    let environment = crate::dma_address::model_reference(0x2f00_1000);
    let resolving_list = crate::dma_address::model_reference(0x2f00_2000);
    let inputs =
        BluetoothPhyRegisterInitInputs::normal_controller_profile(environment, resolving_list);
    let mut baseband_words = std::vec![0; instances::bt_bb::SIZE.div_ceil(4)];
    let before = HostMemory::filled(0);
    let mut memory = before.clone();
    {
        let mut baseband =
            BtV32Baseband::host(&mut baseband_words).expect("memory holds the instance");
        let (mut task, _, _) = memory.owners();
        task.enable_ble_base_stack_hardware(&mut baseband, inputs);
        let phy = &task.bluetooth.btmac_ble_phy_init;
        // The private timing byte 91 less one, in both lanes.
        let delay = phy.read_lc_tx_on_delay_config();
        assert_eq!(
            (delay.lc_tx_on_delay(), delay.init_duplicate_byte()),
            (90, 90)
        );
        assert!(!phy.read_device_table_control().bit_18_opaque());
        assert!(phy.read_word_0800_opaque().init_enable_22());
        assert_eq!(baseband.read_le_tx_on_delay().encoded_value_minus_10(), 80);
        assert!(baseband.read_le_correlation_threshold_0().aa_lc_enable());
        let route = (
            task.etm.read_ch2_event().id(),
            task.etm.read_ch2_task().id(),
        );
        assert_eq!(route, (8, 20), "event 8 routed to task 20 on channel two");
    }
    // SOURCE: BLOB_LIBBLE_PHY_INIT_REGISTERS: the accelerator's SRAM window.
    let base = instances::ble_accelerator::BASE;
    assert_eq!(memory.accelerator[word(0x2010_98cc, base)], 0x2f08_0000);
    assert_eq!(memory.accelerator[word(0x2010_98d0, base)], 0x2f00_0000);
    assert_eq!(
        memory.resolving_list[word(0x2010_1880, instances::bt_mac::BASE)],
        0x2f00_2000 >> 2 & 0x000f_ffff,
        "the resolving list's compressed address"
    );
}

#[test]
fn the_interrupt_output_enables_the_baseline_and_its_release_masks_it() {
    let mut memory = HostMemory::filled(0);
    let (_, interrupts, _) = memory.owners();
    let setup = BluetoothInterruptSetup {
        peripherals: interrupts.peripherals,
    };
    let prepared = setup.prepare_controller_output();
    let enable_0 = prepared
        .peripherals
        .bluetooth_interrupt_bank
        .read_irq_enable_0();
    let enable_1 = prepared
        .peripherals
        .bluetooth_interrupt_bank
        .read_irq_enable_1();
    assert!(enable_0.source_15());
    assert!(enable_1.source_8() && enable_1.source_9() && enable_1.source_12());
    let released = prepared.release_controller_output();
    let enable_0 = released
        .peripherals
        .bluetooth_interrupt_bank
        .read_irq_enable_0();
    let enable_1 = released
        .peripherals
        .bluetooth_interrupt_bank
        .read_irq_enable_1();
    assert!(!enable_0.source_15());
    assert!(!(enable_1.source_8() || enable_1.source_9() || enable_1.source_12()));
    drop(released);
    // SOURCE: complete `btdm_hal.c` helpers: `IRQ_CONTROL_0..2` take image one.
    let base = instances::bt_mac::BASE;
    for address in [0x2010_100c, 0x2010_1010, 0x2010_1014] {
        assert_eq!(memory.interrupt_bank[word(address, base)], 1);
    }
}

#[test]
fn a_near_deadline_programs_its_low_counter_image() {
    let mut memory = HostMemory::filled(0);
    let (_, _, mut timer) = memory.owners();
    let disposition = timer.program_compare(
        BluetoothModemLpTimerInstant::from_bits(0x0012_3456),
        BluetoothModemLpTimerEpoch::new(),
    );
    assert_eq!(
        disposition,
        BluetoothModemLpTimerCompareDisposition::Deadline
    );
    assert_eq!(
        timer
            .runtime_control
            .read_word_006c_opaque()
            .bits_0_31_opaque(),
        0x0012_3456
    );
    drop(timer);
    // SOURCE: BLOB_LIBBTDM_RUNTIME_CONTROL: the command strobe `0x40000`.
    let base = instances::modem_pwr::BASE;
    assert_eq!(memory.timer[word(0x2010_d010, base)], 0x0004_0000);
}

#[test]
fn a_memory_list_pointer_keeps_the_valid_bit_of_its_word() {
    let mut memory = HostMemory::filled(0x0010_0000 | 0x5_5555);
    let (mut task, _, _) = memory.owners();
    let header = crate::dma_address::model_reference(0x2f00_0040);
    task.program_memory_list_pointer(
        BluetoothMemoryListSelector::One,
        BluetoothMemoryListSlot::CurrentRx,
        BluetoothMemoryListPointerImage::Address(header),
    );
    let current = task
        .bluetooth
        .bluetooth_controller_core
        .read_global_rxlink_1_current();
    assert_eq!(
        current.compressed_sram_pointer().get(),
        0x2f00_0040 >> 2 & 0x000f_ffff
    );
    assert!(current.rx_valid());
}

#[test]
fn a_published_head_reads_back_and_the_heads_clear() {
    let mut memory = HostMemory::filled(u32::MAX);
    let (mut task, _, _) = memory.owners();
    let index = BluetoothSchedulerHardwareListIndex::new(3).expect("list three");
    let item = crate::dma_address::model_reference(0x2f00_0100);
    let published = task.publish_scheduler_hardware_list_head(index, Some(item));
    assert_eq!(published.head().address(), Some(0x2f00_0100));
    assert_eq!(
        task.observe_scheduler_hardware_list_head(index),
        BluetoothSchedulerHardwareListHead::from_address(item).expect("encodable")
    );
    let _cleared = task.clear_scheduler_hardware_list_heads();
    assert_eq!(
        task.observe_scheduler_hardware_list_head(index).address(),
        None
    );
    let entry = task
        .bluetooth
        .btdm_scheduler_table
        .read_entry(Index::constant::<15>());
    assert_eq!(entry.head_pointer().get(), 0);
}

#[test]
fn connection_packet_control_toggles_only_the_link_state_bit() {
    let before = HostMemory::filled(0);
    let mut memory = before.clone();
    {
        let (mut task, _, _) = memory.owners();
        task.prepare_software_connection_packet_control();
        assert!(
            !task
                .bluetooth
                .btmac_ble_phy_init
                .read_connection_abort_control()
                .link_state_control()
        );
        task.restore_default_connection_packet_control();
        assert!(
            task.bluetooth
                .btmac_ble_phy_init
                .read_connection_abort_control()
                .link_state_control()
        );
    }
    let base = instances::bt_mac::BASE;
    assert_eq!(
        absolute(base, &before.phy_init, &memory.phy_init),
        [(0x2010_18ac, 0x0000_0002)]
    );
}

#[test]
fn scanner_start_publishes_both_backoff_words_and_the_upper_limit() {
    let before = HostMemory::filled(0);
    let mut memory = before.clone();
    {
        let (mut task, _, _) = memory.owners();
        let _published = task.publish_scan_start();
    }
    // SOURCE: BLOB_LIBBLE_SCAN_BACKOFF: the upper limit 32, each backoff
    // word one.
    let base = instances::bt_mac::BASE;
    assert_eq!(
        absolute(base, &before.scan_backoff, &memory.scan_backoff),
        [(0x2010_1970, 0x20), (0x2010_1974, 1), (0x2010_1978, 1)]
    );
}

#[test]
fn scheduler_requests_store_every_field_of_their_words() {
    let item = crate::dma_address::model_reference(0x2f00_0100);
    let index = BluetoothSchedulerHardwareListIndex::new(3).expect("list three");
    // Every bit set before: the requests are the vendor's stores from zero.
    let mut memory = HostMemory::filled(u32::MAX);
    let base = instances::bt_mac::BASE;
    {
        let (mut task, _, _) = memory.owners();
        let skip =
            task.publish_scheduler_skip(super::BluetoothSchedulerSkipRequest::new(item, index));
        let core = &task.bluetooth.bluetooth_controller_core;
        assert!(core.read_scheduler_skip_request().start());
        let _cleared = task.clear_scheduler_skip(skip);
    }
    // The skip word is cleared whole, its pointer included.
    assert_eq!(memory.core[word(0x2010_10ec, base)], 0);
    let mut memory = HostMemory::filled(u32::MAX);
    {
        let (mut task, _, _) = memory.owners();
        let _lock = task.publish_scheduler_execution_lock(
            super::BluetoothSchedulerExecutionLockRequest::new(item, index),
        );
        let _modify = task.publish_scheduler_execution_modify(index);
    }
    // START, list three and the item's word index 0x40; every other field
    // zero.
    assert_eq!(memory.core[word(0x2010_1254, base)], 0x8030_0040);
    assert_eq!(memory.core[word(0x2010_1258, base)], 0x8000_0008);
    let mut memory = HostMemory::filled(u32::MAX);
    {
        let (mut task, _, _) = memory.owners();
        let _modify = task.publish_scheduler_execution_modify_list_deletion(index);
        let _published = task.publish_scheduler_lock_modify(
            super::BluetoothSchedulerLockModifyRequest::new(item, index),
        );
    }
    assert_eq!(memory.core[word(0x2010_1258, base)], 0x8001_0008);
    assert_eq!(memory.core[word(0x2010_1218, base)], 0x8000_0040);
}

#[test]
fn a_memory_list_pointer_is_cleared_then_ored_into_a_fresh_read() {
    let mut memory = HostMemory::filled(0x0010_0000 | 0x5_5555);
    {
        let (mut task, _, _) = memory.owners();
        task.program_memory_list_pointer(
            BluetoothMemoryListSelector::Two,
            BluetoothMemoryListSlot::NextRx,
            BluetoothMemoryListPointerImage::Zero,
        );
    }
    // The zero image clears bits 19:0 and keeps the rest of the word.
    let base = instances::bt_mac::BASE;
    assert_eq!(memory.core[word(0x2010_128c, base)], 0x0010_0000);
}

#[test]
fn ble_phy_initialization_clears_every_source_in_one_write() {
    let environment = crate::dma_address::model_reference(0x2f00_1000);
    let resolving_list = crate::dma_address::model_reference(0x2f00_2000);
    let inputs =
        BluetoothPhyRegisterInitInputs::normal_controller_profile(environment, resolving_list);
    let mut baseband_words = std::vec![0; instances::bt_bb::SIZE.div_ceil(4)];
    let mut memory = HostMemory::filled(0);
    {
        let mut baseband =
            BtV32Baseband::host(&mut baseband_words).expect("memory holds the instance");
        let (mut task, _, _) = memory.owners();
        task.enable_ble_base_stack_hardware(&mut baseband, inputs);
    }
    // SOURCE: BLOB_LIBBLE_PHY_INIT_REGISTERS: `INTERRUPT_CLEAR = 0xffffffff`.
    let base = instances::bt_mac::BASE;
    assert_eq!(memory.phy_init[word(0x2010_14b8, base)], u32::MAX);
}

#[test]
fn the_low_power_component_routes_channels_four_through_seven() {
    let mut memory = HostMemory::filled(0);
    {
        let (_, _, mut timer) = memory.owners();
        let _observation = timer.initialize_low_power_hardware();
    }
    // SOURCE: BLOB_LIBBTDM_RUNTIME_CONTROL: each channel's event and task.
    let base = instances::modem_etm::BASE;
    for (event, task, image) in [
        (0x2010_882c, 0x2010_8830, (0x45, 0x11)),
        (0x2010_8834, 0x2010_8838, (0x43, 0x12)),
        (0x2010_883c, 0x2010_8840, (0x46, 0x11)),
        (0x2010_8844, 0x2010_8848, (0x46, 0x0f)),
    ] {
        assert_eq!(
            (memory.etm[word(event, base)], memory.etm[word(task, base)]),
            image
        );
    }
}
