//! A descriptor publishes the typed addresses it is given. Kept outside
//! `descriptor`, which forbids `unsafe`: a host test makes its typed
//! addresses through the HAL's placed source.

use crate::dma_address;

use crate::ieee80211::dma::descriptor::{RxDescriptor, TxDescriptor, rx_armed_word, tx_owned_word};

#[test]
fn a_descriptor_publishes_its_three_words() {
    let descriptor = RxDescriptor::new();
    let armed = rx_armed_word(1664).expect("fits");
    // SAFETY: a host descriptor no hardware reads; the addresses only round-trip.
    #[allow(unsafe_code, reason = "host-only typed addresses")]
    let (buffer, next) = unsafe {
        (
            dma_address::placed(0x2f00_1000),
            dma_address::placed_link(0x2f00_2000),
        )
    };
    descriptor.publish_owned(armed, buffer, next);
    assert_eq!(
        (
            descriptor.word0(),
            descriptor.buffer_address(),
            descriptor.next_address()
        ),
        (armed, 0x2f00_1000, 0x2f00_2000)
    );
    let tx = TxDescriptor::new();
    let owned = tx_owned_word(512, 123).expect("fits");
    // SAFETY: as above.
    #[allow(unsafe_code, reason = "host-only typed addresses")]
    let buffer = unsafe { dma_address::placed(0x2f00_3000) };
    tx.publish_owned(owned, buffer, None);
    assert_eq!(
        (tx.word0(), tx.buffer_address(), tx.next_address()),
        (owned, 0x2f00_3000, 0)
    );
    tx.clear();
    assert_eq!(
        (tx.word0().bits(), tx.buffer_address(), tx.next_address()),
        (0, 0, 0)
    );
}
