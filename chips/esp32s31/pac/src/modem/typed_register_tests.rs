//! Behaviour of the generated typed shared-radio blocks over host memory:
//! the modem ETM division, the baseband's receive-information read and FIFO
//! port, and a vendor whole-word image.
//!
//! Each test fills a block's memory, performs the typed operations the
//! handwritten transactions use and compares the memory before and after, so
//! no test names a register address, mask or shift.

use crate::generated::{
    bt_v3_2_baseband::{self as baseband, BtV3_2Baseband},
    modem_etm::{self as etm, ModemEtm},
    phy_agc_oracle::{self as agc, PhyAgcOracle},
};
use oer_mmio::Index;
use std::{vec, vec::Vec};

/// Each word that differs between two images of a block, as its index, its
/// new value and the bits that changed.
fn changed(before: &[u32], after: &[u32]) -> Vec<(usize, u32, u32)> {
    before
        .iter()
        .zip(after)
        .enumerate()
        .filter(|(_, (before, after))| before != after)
        .map(|(index, (before, after))| (index, *after, before ^ after))
        .collect()
}

/// The one word a strobe of one ETM part changes in memory of all ones,
/// and the value it stores there.
fn strobe(
    operation: impl FnOnce(
        &mut etm::ModemEtmIeee802154<oer_mmio::Host<'_>>,
        &mut etm::ModemEtmBluetoothPhy<oer_mmio::Host<'_>>,
        &mut etm::ModemEtmBluetooth<oer_mmio::Host<'_>>,
    ),
) -> u32 {
    let before = vec![u32::MAX; etm::SIZE / 4];
    let mut after = before.clone();
    {
        let (mut ieee802154, mut phy, mut bluetooth) = ModemEtm::host(&mut after)
            .expect("memory holds the block")
            .divide();
        operation(&mut ieee802154, &mut phy, &mut bluetooth);
    }
    let difference = changed(&before, &after);
    assert_eq!(difference.len(), 1, "one trigger word");
    difference[0].1
}

#[test]
fn each_etm_part_strobes_only_its_own_channel_bits() {
    assert_eq!(
        strobe(|ieee802154, _, _| ieee802154.trigger_channel_enable_set_ch1()),
        1 << 1
    );
    assert_eq!(
        strobe(|_, phy, _| phy.enable_bluetooth_phy_etm_channel2()),
        1 << 2
    );
    assert_eq!(
        strobe(|_, phy, _| phy.disable_bluetooth_phy_etm_channel2()),
        1 << 2
    );
    assert_eq!(
        strobe(|_, _, bluetooth| bluetooth.trigger_channel_enable_clear_ch7()),
        1 << 7
    );
}

#[test]
fn an_etm_enable_and_its_disable_strobe_different_words() {
    let before = vec![0; etm::SIZE / 4];
    let mut after = before.clone();
    {
        let (mut ieee802154, _, _) = ModemEtm::host(&mut after)
            .expect("memory holds the block")
            .divide();
        ieee802154.trigger_channel_enable_set_ch0();
        ieee802154.trigger_channel_enable_clear_ch1();
    }
    let difference = changed(&before, &after);
    assert_eq!(difference.len(), 2);
    assert_eq!(difference[0].1 | difference[1].1, 0b11);
    assert_ne!(difference[0].0, difference[1].0);
}

#[test]
fn the_bluetooth_low_power_routes_reach_channels_four_to_seven_only() {
    let mut memory = vec![0; etm::SIZE / 4];
    let (ieee802154, phy, mut bluetooth) = ModemEtm::host(&mut memory)
        .expect("memory holds the block")
        .divide();
    bluetooth.initialize_bluetooth_low_power_config();
    // The part indexes its own channels from zero: channel four is index 0.
    let routes = [(0x45, 0x11), (0x43, 0x12), (0x46, 0x11), (0x46, 0x0f)];
    for (index, (event, task)) in routes.into_iter().enumerate() {
        let index = Index::<4>::new(index).expect("four channels");
        assert_eq!(bluetooth.read_ch_event(index).id(), event);
        assert_eq!(bluetooth.read_ch_task(index).id(), task);
    }
    for index in 0..2 {
        let index = Index::<2>::new(index).expect("two channels");
        assert_eq!(ieee802154.read_ch_event(index).id(), 0);
        assert_eq!(ieee802154.read_ch_task(index).id(), 0);
    }
    assert_eq!(phy.read_ch_event(Index::constant::<0>()).id(), 0);
}

#[test]
fn the_receive_information_read_returns_the_signed_rssi_byte() {
    let mut memory = vec![0xffff_ff9c; baseband::SIZE / 4];
    let (_rest, rx_info) = BtV3_2Baseband::host(&mut memory)
        .expect("memory holds the block")
        .split_rx_info();
    assert_eq!(rx_info.read_current_rx_info().rssi().cast_signed(), -100);
}

#[test]
fn the_tx_cca_fifo_pops_its_whole_word_and_writes_nothing() {
    let before = vec![0x1234_5678; baseband::SIZE / 4];
    let mut after = before.clone();
    let mut block = BtV3_2Baseband::host(&mut after).expect("memory holds the block");
    assert_eq!(block.pop_tx_cca_fifo_data(), 0x1234_5678);
    drop(block);
    assert!(changed(&before, &after).is_empty());
}

#[test]
fn a_whole_word_agc_image_also_writes_the_bits_the_model_leaves_undeclared() {
    let before = vec![u32::MAX; agc::SIZE / 4];
    let mut after = before.clone();
    let mut block = PhyAgcOracle::host(&mut after).expect("memory holds the block");
    block.initialize_agc_update_8070();
    drop(block);
    let difference = changed(&before, &after);
    assert_eq!(difference.len(), 1, "one word changed");
    // The vendor stores the whole word: every bit outside its image is zero.
    assert_eq!(difference[0].1, 0x0000_08c7);
}
