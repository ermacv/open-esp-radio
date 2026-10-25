mod tone_control {
    use super::super::{TonePath0, TxIqToneControlFields, configure_tone_path_0};
    use crate::generated::phy_baseband_config_oracle::{self as bb, PhyBasebandConfigOracle};
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
            let before = vec![fill; bb::SIZE / 4];
            let mut after = before.clone();
            let mut block = PhyBasebandConfigOracle::host(&mut after).expect("memory holds it");
            let captured = TxIqToneControlFields::capture(&block);
            captured.restore(&mut block);
            assert_eq!(TxIqToneControlFields::capture(&block), captured);
            drop(block);
            assert!(changed(&before, &after).is_empty(), "{fill:#x}");
        }
    }

    #[test]
    fn a_tone_path_edge_zeroes_both_reserved_lanes_and_keeps_the_high_nibble() {
        let before = vec![u32::MAX; bb::SIZE / 4];
        let mut after = before.clone();
        let mut block = PhyBasebandConfigOracle::host(&mut after).expect("memory holds it");
        configure_tone_path_0(
            &mut block,
            TonePath0 {
                selector_high: 0x12,
                negated_step: 0x34,
                enabled: true,
                mismatch_mode: oer_mmio::Bits::constant::<5>(),
                polarity: oer_mmio::Bits::constant::<4>(),
            },
        );
        let control = block.read_tone_path_0_control();
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
}
