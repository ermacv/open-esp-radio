//! Behaviour of the generated typed IEEE 802.15.4 blocks over host memory.
//!
//! Each test fills the block's memory, performs one typed operation and
//! compares the whole memory before and after, so no test names a register
//! address, mask or shift: only what changed and what the typed readers see.

use crate::generated::ieee802154_mac::{self as mac, Ieee802154Mac};
use oer_mmio::{Index, NotOneSplit};
use std::{vec, vec::Vec};

fn memory(fill: u32) -> Vec<u32> {
    vec![fill; mac::SIZE / 4]
}

/// The bits that differ between two images of the block, per word.
fn changed(before: &[u32], after: &[u32]) -> Vec<u32> {
    before
        .iter()
        .zip(after)
        .map(|(before, after)| before ^ after)
        .filter(|difference| *difference != 0)
        .collect()
}

#[test]
fn a_field_change_keeps_every_other_bit_of_the_block() {
    let before = memory(u32::MAX);
    let mut after = before.clone();
    let mut block = Ieee802154Mac::host(&mut after).expect("memory holds the block");
    block.modify_control(|writer| {
        writer.set_auto_ack_tx(false);
    });
    let control = block.read_control();
    assert!(!control.auto_ack_tx());
    assert!(control.auto_ack_rx() && control.multipan3_enabled());
    drop(block);
    let difference = changed(&before, &after);
    assert_eq!(difference.len(), 1, "one word changed");
    assert_eq!(difference[0].count_ones(), 1, "one bit of it changed");
}

#[test]
fn an_enumerated_field_takes_its_declared_value_and_reads_it_back() {
    let before = memory(u32::MAX);
    let mut after = before.clone();
    let mut block = Ieee802154Mac::host(&mut after).expect("memory holds the block");
    block.modify_ed_config(|writer| {
        writer.set_cca_mode(mac::ed_config::CcaMode::Carrier);
    });
    let config = block.read_ed_config();
    assert_eq!(config.cca_mode_variant(), mac::ed_config::CcaMode::Carrier);
    assert_eq!(
        config.ed_sample_rate_variant(),
        mac::ed_config::EdSampleRate::EightPerUs
    );
    drop(block);
    let difference = changed(&before, &after);
    assert_eq!(difference.len(), 1, "one word changed");
    assert_eq!(difference[0].count_ones(), 2, "the two-bit field changed");
}

#[test]
fn an_acknowledge_writes_exactly_the_retained_sample() {
    let before = memory(u32::MAX);
    let mut after = before.clone();
    let block = Ieee802154Mac::host(&mut after).expect("memory holds the block");
    let (task, mut interrupt) = block.split();
    let snapshot = interrupt.sample_event_status();
    assert!(snapshot.tx_done() && snapshot.rx_sfd_done());
    let snapshot = snapshot
        .retain_rx_done(false)
        .retain_ack_tx_done(false)
        .retain_ack_rx_done(false)
        .retain_rx_abort(false)
        .retain_tx_abort(false)
        .retain_ed_done(false)
        .retain_events_7(false)
        .retain_timer0_overflow(false)
        .retain_timer1_overflow(false)
        .retain_clock_count_match(false)
        .retain_tx_sfd_done(false)
        .retain_rx_sfd_done(false)
        .retain_events_13(false);
    interrupt.acknowledge_event_status(snapshot);
    // Host memory keeps what was written: a read-modify-write would have
    // written every asserted event back; the acknowledge wrote one.
    let status = interrupt.read_event_status();
    assert!(status.tx_done());
    assert!(!status.rx_done() && !status.rx_sfd_done());
    drop(Ieee802154Mac::try_join(task, interrupt).expect("owners of one split"));
    let difference = changed(&before, &after);
    assert_eq!(difference.len(), 1, "one word written");
    assert_eq!(
        difference[0].count_ones(),
        31,
        "only the acknowledged bit stays"
    );
}

#[test]
fn an_empty_acknowledge_writes_zero() {
    let mut words = memory(0);
    let mut block = Ieee802154Mac::host(&mut words).expect("memory holds the block");
    let snapshot = block.sample_event_status();
    assert!(snapshot.is_clear());
    block.acknowledge_event_status(snapshot);
    drop(block);
    assert!(words.iter().all(|word| *word == 0));
}

#[test]
fn an_array_element_outside_its_array_is_an_error() {
    assert!(Index::<4>::new(4).is_err());
    let before = memory(0);
    let mut after = before.clone();
    let mut block = Ieee802154Mac::host(&mut after).expect("memory holds the block");
    let last = Index::<4>::new(3).expect("context 3 exists");
    block.modify_multipan_pan_id(last, |writer| {
        writer.set_pan_id(0xbeef);
    });
    assert_eq!(block.read_multipan_pan_id(last).pan_id(), 0xbeef);
    let first = Index::<4>::new(0).expect("context 0 exists");
    assert_eq!(block.read_multipan_pan_id(first).pan_id(), 0);
    drop(block);
    assert_eq!(changed(&before, &after).len(), 1, "one element written");
}

#[test]
fn a_write_only_register_is_written_from_zero() {
    let before = memory(u32::MAX);
    let mut after = before.clone();
    let mut block = Ieee802154Mac::host(&mut after).expect("memory holds the block");
    block.write_diagnostic_counter_clear(|writer| {
        writer.set_crc_error(true);
    });
    drop(block);
    let difference = changed(&before, &after);
    assert_eq!(difference.len(), 1, "one word written");
    assert_eq!(
        difference[0].count_ones(),
        31,
        "one bit set, every other zero"
    );
}

#[test]
fn a_split_part_reaches_only_its_words() {
    let before = memory(0);
    let mut after = before.clone();
    let block = Ieee802154Mac::host(&mut after).expect("memory holds the block");
    let (mut task, interrupt) = block.split();
    task.write_command(|writer| {
        writer.set_opcode(mac::command::Opcode::TxStart);
    });
    task.modify_ed_config(|writer| {
        writer.set_cca_mode(mac::ed_config::CcaMode::EnergyDetection);
    });
    // The interrupt owner observes the configuration word the task owns.
    assert_eq!(
        interrupt.read_ed_config().cca_mode_variant(),
        mac::ed_config::CcaMode::EnergyDetection
    );
    drop(Ieee802154Mac::try_join(task, interrupt).expect("owners of one split"));
    assert_eq!(changed(&before, &after).len(), 2);
}

#[test]
fn host_memory_smaller_than_the_block_is_refused() {
    let mut words = vec![0_u32; mac::SIZE / 4 - 1];
    assert!(Ieee802154Mac::host(&mut words).is_err());
}

#[test]
fn owners_of_two_blocks_do_not_join() {
    let mut first = memory(0);
    let mut second = memory(0);
    let (task, interrupt) = Ieee802154Mac::host(&mut first)
        .expect("memory holds the block")
        .split();
    let (mut other_task, other_interrupt) = Ieee802154Mac::host(&mut second)
        .expect("memory holds the block")
        .split();
    let Err(NotOneSplit {
        first: task,
        second: other_interrupt,
    }) = Ieee802154Mac::try_join(task, other_interrupt)
    else {
        panic!("owners of two blocks joined");
    };
    other_task.write_command(|writer| {
        writer.set_opcode(mac::command::Opcode::TxStart);
    });
    drop(Ieee802154Mac::try_join(task, interrupt).expect("owners of one split"));
    drop(Ieee802154Mac::try_join(other_task, other_interrupt).expect("owners of one split"));
    assert!(first.iter().all(|word| *word == 0));
    assert_eq!(changed(&memory(0), &second).len(), 1, "one command written");
}

#[test]
fn a_dma_address_field_takes_its_typed_address_whole() {
    let address =
        crate::test_memory::reference::<crate::layouts::WifiRxDmaDescriptor>(0x4080_1000).address();
    let before = memory(u32::MAX);
    let mut after = before.clone();
    let mut block = Ieee802154Mac::host(&mut after).expect("memory holds the block");
    block.write_tx_dma_address(address);
    drop(block);
    let difference = changed(&before, &after);
    assert_eq!(difference.len(), 1, "one word changed");
    assert_eq!(difference[0], !address.get(), "the word holds the address");
}
