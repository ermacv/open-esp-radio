//! Safe consumers claim zeroed, placed storage only once.
#![forbid(unsafe_code)]

oer_esp32c5_hal::dma_static! {
    static BUFFERS: ([u32; 4], [u8; 16]) = zeroed in ".dma.bss.ieee802154";
}

#[test]
fn placed_storage_is_zeroed_claimed_once_and_split() {
    let (first, second) = BUFFERS.take().unwrap().split();
    assert_eq!(*first.into_inner(), [0; 4]);
    assert_eq!(*second.into_inner(), [0; 16]);
    assert!(BUFFERS.take().is_none());
}
