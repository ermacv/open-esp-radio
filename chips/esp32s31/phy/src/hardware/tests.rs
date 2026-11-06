use super::{packed_byte, packed_halfword, tx_baseband_gain_row, tx_gain_seed_rows};

#[test]
fn phy_baseband_gain_indices_match_the_rom_leaf() {
    assert_eq!(tx_baseband_gain_row(0x0080).get(), 1);
    assert_eq!(tx_baseband_gain_row(0x0100).get(), 2);
    assert_eq!(tx_baseband_gain_row(0x0020).get(), 3);
    assert_eq!(tx_baseband_gain_row(0x00a0).get(), 4);
    assert_eq!(tx_baseband_gain_row(0).get(), 0);
    assert_eq!(tx_baseband_gain_row(u16::MAX).get(), 0);
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

    let rows = tx_gain_seed_rows(&image);
    assert_eq!(rows[2][2], 0);
    assert_eq!(rows[2][3], 0x0b0a);
    assert_eq!(rows[3][0], 0x0d0c);
    assert_eq!(rows[4][3], 0x1b1a);
    let mut words = [0; 16];
    words[0] = 0x0f0e_0d0c;
    assert_eq!(packed_halfword(&words, super::TxGainEntry::nth(1)), 0x0f0e);
    assert_eq!(
        packed_byte(&image.output_32, super::TxGainEntry::nth(3)),
        0x0f
    );
}
