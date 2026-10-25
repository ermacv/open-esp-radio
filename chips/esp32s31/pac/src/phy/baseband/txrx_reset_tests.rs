//! The frontend TX/RX reset edges over host memory.

use crate::generated::phy_fedata_recovered::{self as fedata, PhyFedataRecovered};
use std::{vec, vec::Vec};

/// The reset word as each edge leaves it, from a block whose every word is
/// `initial`.
fn edges(initial: u32) -> (fedata::word_0040_opaque::R, fedata::word_0040_opaque::R) {
    let mut memory: Vec<u32> = vec![initial; fedata::SIZE / 4];
    let mut block = PhyFedataRecovered::host(&mut memory).expect("memory holds the block");
    block.clear_phy_frontend_txrx_reset_state();
    let cleared = block.read_word_0040_opaque();
    block.assert_phy_frontend_txrx_reset_state();
    (cleared, block.read_word_0040_opaque())
}

#[test]
fn the_frontend_reset_lowers_then_raises_both_reset_bits() {
    for initial in [0xffff_ffff, 0x01ff_f000, 0x0600_0000, 0] {
        let (cleared, asserted) = edges(initial);
        assert!(!cleared.tx_rx_reset_bit_25() && !cleared.tx_rx_reset_bit_26());
        assert!(asserted.tx_rx_reset_bit_25() && asserted.tx_rx_reset_bit_26());
    }
}

#[test]
fn each_frontend_reset_edge_keeps_every_other_bit_of_the_word() {
    for initial in [0xffff_ffff_u32, 0x01ff_f000, 0x9155_aa55] {
        let mut memory: Vec<u32> = vec![initial; fedata::SIZE / 4];
        let mut block = PhyFedataRecovered::host(&mut memory).expect("memory holds the block");
        block.clear_phy_frontend_txrx_reset_state();
        drop(block);
        let changed: Vec<u32> = memory.iter().map(|word| word ^ initial).collect();
        // Only the reset bits that were set change, and only in one word.
        let differing: Vec<&u32> = changed.iter().filter(|bits| **bits != 0).collect();
        assert!(differing.len() <= 1);
        for bits in differing {
            assert_eq!(bits & !initial, 0, "the clear edge only lowers bits");
            assert_eq!(bits.count_ones(), 2, "it lowers the two reset bits alone");
        }
    }
}

#[test]
fn the_raise_edge_reads_the_word_afresh() {
    // A hardware update between the edges survives the second edge: the
    // raise reads the word again instead of reusing the cleared image.
    let mut memory: Vec<u32> = vec![0; fedata::SIZE / 4];
    PhyFedataRecovered::host(&mut memory)
        .expect("memory holds the block")
        .clear_phy_frontend_txrx_reset_state();
    memory.fill(0x9155_aa55);
    let mut block = PhyFedataRecovered::host(&mut memory).expect("memory holds the block");
    block.assert_phy_frontend_txrx_reset_state();
    let raised = block.read_word_0040_opaque();
    assert!(raised.tx_rx_reset_bit_25() && raised.tx_rx_reset_bit_26());
    drop(block);
    let changed: Vec<u32> = memory
        .iter()
        .map(|word| word ^ 0x9155_aa55)
        .filter(|bits| *bits != 0)
        .collect();
    assert_eq!(changed.len(), 1, "one word changed");
    assert_eq!(changed[0] & 0x9155_aa55, 0, "every updated bit is kept");
}
