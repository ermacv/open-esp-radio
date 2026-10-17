use crate::*;

#[test]
fn descriptor_words_preserve_the_recovered_geometry() {
    assert_eq!(
        core::mem::size_of::<Descriptor>(),
        DESCRIPTOR_BYTES as usize
    );
    assert!(descriptor_address_valid(0x2f00_0000));
    assert!(!descriptor_address_valid(0x2f00_0002));
    assert!(dma_range_valid(0x2f00_0100, 0x100));
    assert!(!dma_range_valid(0x2f07_fff0, 0x20));

    let rx = rx_armed_word(1700).unwrap();
    assert_eq!(size(rx), 1700);
    assert_eq!(length(rx), 1700);
    assert_ne!(rx & BIT_31, 0);
    assert_eq!(rx & BIT_30, 0);

    let completed = 1700 | (96 << LENGTH_SHIFT) | BIT_30 | BIT_31;
    let recycled = rx_rearm_word(completed).unwrap();
    assert_eq!(size(recycled), 1700);
    assert_eq!(length(recycled), 1700);
    assert_ne!(recycled & BIT_31, 0);
    assert_eq!(recycled & BIT_30, 0);

    let tx = tx_owned_word(512, 123).unwrap();
    assert_eq!(size(tx), 512);
    assert_eq!(length(tx), 123);
    assert_eq!(tx & (BIT_30 | BIT_31), BIT_30 | BIT_31);
    assert_eq!(tx_owned_word(64, 65), None);
}
