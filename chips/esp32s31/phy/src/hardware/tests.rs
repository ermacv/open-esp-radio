use super::{packed_byte, packed_halfword, tx_baseband_gain_index, tx_gain_seed_halfword};

#[test]
fn phy_baseband_gain_indices_match_the_rom_leaf() {
    assert_eq!(tx_baseband_gain_index(0x0080), 1);
    assert_eq!(tx_baseband_gain_index(0x0100), 2);
    assert_eq!(tx_baseband_gain_index(0x0020), 3);
    assert_eq!(tx_baseband_gain_index(0x00a0), 4);
    assert_eq!(tx_baseband_gain_index(0), 0);
    assert_eq!(tx_baseband_gain_index(u16::MAX), 0);
}

#[test]
fn tx_gain_seed_view_crosses_the_owned_field_boundary_explicitly() {
    let image = crate::channel::PhyWifiTxGainImage {
        seed: [
            0x0100_0000,
            0x0302_0000,
            0x0504_0000,
            0x0706_0000,
            0x0908_0000,
            0x0b0a_0000,
        ],
        output_32: [
            0x0f0e_0d0c,
            0x1312_1110,
            0x1716_1514,
            0x1b1a_1918,
            0,
            0,
            0,
            0,
        ],
        output_64: [0; 16],
        output_72: [0; 16],
        config: 0,
    };

    assert_eq!(tx_gain_seed_halfword(&image, 10), 0);
    assert_eq!(tx_gain_seed_halfword(&image, 11), 0x0b0a);
    assert_eq!(tx_gain_seed_halfword(&image, 12), 0x0d0c);
    assert_eq!(tx_gain_seed_halfword(&image, 19), 0x1b1a);
    assert_eq!(packed_halfword(&image.output_32, 1), 0x0f0e);
    assert_eq!(packed_byte(&image.output_32, 3), 0x0f);
}
