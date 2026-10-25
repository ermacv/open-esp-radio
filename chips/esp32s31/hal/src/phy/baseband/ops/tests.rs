use super::{TonePath0, TxIqToneControlFields, configure_tone_path_0, reset_frontend_txrx};
use crate::phy::registers::BasebandRegisters;
use oer_esp32s31_pac::fe::PhyFedataRecovered;
use oer_mmio::Bits;
use std::{vec, vec::Vec};

fn changed(before: &[u32], after: &[u32]) -> Vec<u32> {
    before
        .iter()
        .zip(after)
        .map(|(before, after)| before ^ after)
        .filter(|bits| *bits != 0)
        .collect()
}

#[test]
fn a_restored_tone_control_capture_reproduces_the_word() {
    for fill in [0, u32::MAX, 0x9a5c_3e71] {
        let before = vec![fill; 31956 / 4];
        let mut after = before.clone();
        let mut block = BasebandRegisters::host(&mut after).expect("memory holds it");
        let captured = TxIqToneControlFields::capture(&block);
        captured.restore(&mut block);
        assert_eq!(TxIqToneControlFields::capture(&block), captured);
        drop(block);
        assert!(changed(&before, &after).is_empty(), "{fill:#x}");
    }
}

#[test]
fn a_tone_path_edge_zeroes_both_reserved_lanes_and_keeps_the_high_nibble() {
    let before = vec![u32::MAX; 31956 / 4];
    let mut after = before.clone();
    let mut block = BasebandRegisters::host(&mut after).expect("memory holds it");
    configure_tone_path_0(
        &mut block,
        TonePath0 {
            selector_high: 0x12,
            negated_step: 0x34,
            enabled: true,
            mismatch_mode: Bits::constant::<5>(),
            polarity: Bits::constant::<4>(),
        },
    );
    let control = block.fe.read_tone_path_0_control();
    assert_eq!(control.selector_high(), 0x12);
    assert_eq!(control.negated_step_or_attenuation(), 0x34);
    assert!(control.tone_enable_or_arm());
    assert_eq!(control.bits_19_21_opaque().get(), 5);
    assert_eq!(control.txiq_polarity_image().get(), 4);
    assert_eq!(control.bits_8_9_opaque().get(), 0);
    assert_eq!(control.bits_22_23_opaque().get(), 0);
    assert_eq!(control.bits_28_31_opaque().get(), 0xf, "the RMW keeps it");
    drop(block);
    assert_eq!(changed(&before, &after).len(), 1);
}

#[test]
fn the_frontend_reset_ends_with_both_reset_bits_raised_and_keeps_every_other_bit() {
    for initial in [0xffff_ffff_u32, 0x01ff_f000, 0x0600_0000, 0x9155_aa55, 0] {
        let mut memory: Vec<u32> = vec![initial; oer_esp32s31_pac::instances::fe::SIZE / 4];
        let mut block = PhyFedataRecovered::host(&mut memory).expect("memory holds the block");
        reset_frontend_txrx(&mut block);
        let raised = block.read_word_0440_opaque();
        assert!(raised.tx_rx_reset_bit_25() && raised.tx_rx_reset_bit_26());
        drop(block);
        let differing: Vec<u32> = changed(
            &[initial; oer_esp32s31_pac::instances::fe::SIZE / 4],
            &memory,
        );
        assert!(differing.len() <= 1, "one word at most");
        for bits in differing {
            assert_eq!(bits & initial, 0, "the edges only raise the reset bits");
            assert!(bits.count_ones() <= 2);
        }
    }
}
