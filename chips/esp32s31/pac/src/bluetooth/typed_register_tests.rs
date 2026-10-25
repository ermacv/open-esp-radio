//! Behaviour of the generated typed Bluetooth LE blocks over host memory.
//!
//! Each test fills a block's memory, performs the typed operation the
//! handwritten transactions use and compares the memory before and after, so
//! no test names a register address, mask or shift: only what changed and
//! what the typed readers see.

use crate::generated::{
    bluetooth_controller_core::{self as core, BluetoothControllerCore},
    bluetooth_interrupt_bank::{self as bank, BluetoothInterruptBank},
    bluetooth_scheduler_interrupt_runtime::{self as runtime, BluetoothSchedulerInterruptRuntime},
    btdm_runtime_control::{self as timer, BtdmRuntimeControl},
    btdm_scheduler_table::{self as table, BtdmSchedulerTable},
    btmac_ble_phy_init::{self as btmac, BtmacBlePhyInit},
};
use crate::soc::Interrupt;
use oer_mmio::{Host, Index, InterruptSource};
use std::{vec, vec::Vec};

fn memory(size: usize, fill: u32) -> Vec<u32> {
    vec![fill; size / 4]
}

/// Each word that differs between two images of a block, as its index and
/// the bits that changed.
fn changed(before: &[u32], after: &[u32]) -> Vec<(usize, u32)> {
    before
        .iter()
        .zip(after)
        .enumerate()
        .map(|(index, (before, after))| (index, before ^ after))
        .filter(|(_, difference)| *difference != 0)
        .collect()
}

#[test]
fn the_interrupt_owners_name_the_primary_and_low_power_timer_sources() {
    assert_eq!(
        <BluetoothInterruptBank<Host<'_>> as InterruptSource>::SOURCE,
        Interrupt::MODEM_BT_MAC.source()
    );
    assert_eq!(
        <BtdmRuntimeControl<Host<'_>> as InterruptSource>::SOURCE,
        Interrupt::MODEM_LP_TIMER.source()
    );
}

#[test]
fn a_whole_word_vendor_image_also_writes_the_bits_the_model_leaves_undeclared() {
    let before = memory(btmac::SIZE, 0);
    let mut after = before.clone();
    let mut block = BtmacBlePhyInit::host(&mut after).expect("memory holds the block");
    block.publish_ble_phy_init_value_04ac();
    let control = block.read_connection_abort_control();
    assert!(control.link_state_control() && !control.custom_opcode_enabled());
    drop(block);
    let difference = changed(&before, &after);
    assert_eq!(difference.len(), 1, "one word changed");
    assert_eq!(
        difference[0].1.count_ones(),
        9,
        "the link-state bit and the eight undeclared bits of the vendor word"
    );
}

#[test]
fn a_memory_list_pointer_is_cleared_then_ored_and_keeps_its_other_fields() {
    let mut words = memory(core::SIZE, u32::MAX);
    let mut block = BluetoothControllerCore::host(&mut words).expect("memory holds the block");
    block.clear_bluetooth_memory_list_1_pointer_a();
    let cleared = block.read_global_rxlink_1_current();
    assert_eq!(cleared.compressed_sram_pointer().get(), 0);
    assert!(cleared.rx_valid(), "the pointer clear keeps RX_VALID");
    let image = core::global_rxlink_1_current::CompressedSramPointerImage::new(
        crate::test_memory::reference(0x2f00_0040),
    )
    .expect("a placed header is encodable");
    block.or_bluetooth_memory_list_1_pointer_a(image);
    let published = block.read_global_rxlink_1_current();
    assert_eq!(published.compressed_sram_pointer().get(), image.bits());
    assert!(published.rx_valid());
}

#[test]
fn a_scheduler_head_replaces_only_the_pointer_of_its_entry() {
    let before = memory(table::SIZE, u32::MAX);
    let mut after = before.clone();
    let mut block = BtdmSchedulerTable::host(&mut after).expect("memory holds the block");
    let image = table::entry::HeadPointerImage::new(crate::test_memory::reference(0x2f00_0100))
        .expect("a placed item is encodable");
    let index = Index::constant::<3>();
    block.publish_bluetooth_scheduler_hardware_list_head(index, image);
    assert_eq!(block.read_entry(index).head_pointer().get(), image.bits());
    assert_eq!(
        block
            .read_entry(Index::constant::<2>())
            .head_pointer()
            .get(),
        oer_mmio::Bits::<20>::truncate(u32::MAX).get(),
        "another entry keeps the head the memory held"
    );
    drop(block);
    assert_eq!(changed(&before, &after).len(), 1, "one entry changed");
}

#[test]
fn a_stale_source_clear_stores_its_reviewed_image_beside_the_snapshot() {
    let before = memory(bank::SIZE, 0);
    let mut after = before.clone();
    let mut block = BluetoothInterruptBank::host(&mut after).expect("memory holds the block");
    block.clear_bluetooth_primary_baseline_bank_0();
    drop(block);
    let stale = changed(&before, &after);
    assert_eq!(stale.len(), 1, "one clear word");
    assert_eq!(stale[0].1.count_ones(), 1, "baseline source 15 alone");

    // The snapshot's acknowledgement writes the same clear word with exactly
    // the sampled events.
    let mut status = memory(bank::SIZE, u32::MAX);
    let block = BluetoothInterruptBank::host(&mut status).expect("memory holds the block");
    let snapshot = block.sample_bluetooth_primary_interrupt_bank_0();
    assert!(snapshot.source_15() && snapshot.source_21());
    drop(block);
    let mut acknowledged = memory(bank::SIZE, 0);
    let mut block =
        BluetoothInterruptBank::host(&mut acknowledged).expect("memory holds the block");
    block.acknowledge_bluetooth_primary_interrupt_bank_0(
        snapshot
            .retain_source_21(false)
            .retain_sources_27_28(false)
            .retain_bits_0_14_opaque(false)
            .retain_bits_16_20_opaque(false)
            .retain_bits_22_26_opaque(false)
            .retain_bits_29_31_opaque(false),
    );
    drop(block);
    assert_eq!(
        changed(&before, &acknowledged),
        stale,
        "the retained source 15 alone, in the same word"
    );
}

#[test]
fn a_stale_source_clear_never_acknowledges_an_enabled_event() {
    // Scheduler-run source 21 and baseline source 15 are enabled and every
    // source is pending, as on a repeated scheduler-run preparation.
    let mut words = memory(bank::SIZE, u32::MAX);
    let mut block = BluetoothInterruptBank::host(&mut words).expect("memory holds the block");
    let pending = block.sample_bluetooth_primary_interrupt_bank_0();
    assert!(pending.source_21(), "source 21 is pending");
    block.clear_bluetooth_scheduler_run_interrupts_bank_0();
    let enable = block.read_irq_enable_0();
    assert!(
        !enable.source_21() && enable.sources_27_28().get() == 0,
        "the acknowledged sources are masked first: no enabled event is acknowledged unsampled"
    );
    assert!(
        enable.source_15() && enable.source_0(),
        "the other enabled sources keep their events"
    );
    block.enable_bluetooth_scheduler_run_interrupts_bank_0();
    let enable = block.read_irq_enable_0();
    assert!(enable.source_21() && enable.sources_27_28().get() == 0b11);
}

#[test]
fn a_diagnostic_selector_is_written_whole_from_zero() {
    let before = memory(runtime::SIZE, u32::MAX);
    let mut after = before.clone();
    let mut block =
        BluetoothSchedulerInterruptRuntime::host(&mut after).expect("memory holds the block");
    let selection = runtime::diagnostic_select::Selection::SchedulerStatus;
    block.write_diagnostic_select(|writer| {
        writer.set_selection(selection);
    });
    drop(block);
    let difference = changed(&before, &after);
    assert_eq!(difference.len(), 1, "one word changed");
    assert_eq!(after[difference[0].0], selection.bits());
}

#[test]
fn the_runtime_timer_start_command_is_its_one_variant_from_zero() {
    let before = memory(timer::SIZE, u32::MAX);
    let mut after = before.clone();
    let mut block = BtdmRuntimeControl::host(&mut after).expect("memory holds the block");
    let start = timer::runtime_timer_start_command::Command::Start;
    block.write_runtime_timer_start_command(|writer| {
        writer.set_command(start);
    });
    drop(block);
    let difference = changed(&before, &after);
    assert_eq!(difference.len(), 1, "one word changed");
    assert_eq!(after[difference[0].0], start.bits());
}
