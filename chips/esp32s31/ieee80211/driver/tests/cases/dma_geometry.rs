use crate::*;

#[test]
fn descriptor_words_preserve_the_recovered_geometry() {
    assert_eq!(
        core::mem::size_of::<RxDescriptor>(),
        DESCRIPTOR_BYTES as usize
    );
    assert_eq!(
        core::mem::size_of::<TxDescriptor>(),
        DESCRIPTOR_BYTES as usize
    );

    let rx = rx_armed_word(1700).unwrap();
    assert_eq!((rx.size(), rx.length()), (1700, 1700));
    assert!(rx.owner() && !rx.rx_done());

    let completed = rx_landed_word(1700, 96, true);
    let recycled = rx_rearm_word(completed).unwrap();
    assert_eq!((recycled.size(), recycled.length()), (1700, 1700));
    assert!(recycled.owner() && !recycled.rx_done());

    let tx = tx_owned_word(512, 123).unwrap();
    assert_eq!((tx.size(), tx.length()), (512, 123));
    assert!(tx.tx_last() && tx.tx_owned());
    assert_eq!(tx_owned_word(64, 65), None);
}
