use crate::*;
use oer_esp32s31_ieee80211_mac::irq::{
    EVENT_COLLISION, EVENT_RX_SUCCESS, EVENT_TX_COMPLETE, EVENT_TX_TIMEOUT,
};

#[test]
fn split_publication_preserves_all_coalesced_facts_in_vendor_order() {
    let mut storage = IrqStorage::new();
    let (publisher, mut consumer) = storage.split();
    publisher.publish(EVENT_COLLISION | EVENT_TX_TIMEOUT | EVENT_TX_COMPLETE | EVENT_RX_SUCCESS);
    publisher.publish(EVENT_RX_SUCCESS);
    let batch = consumer.take_batch();
    assert_eq!(
        batch.collect::<Vec<_>>(),
        [
            IrqWork::RxSuccess,
            IrqWork::TxComplete,
            IrqWork::TxTimeout,
            IrqWork::Collision,
        ]
    );
    assert!(consumer.take_batch().is_empty());
}

#[test]
fn taking_a_finite_batch_does_not_consume_a_later_publication() {
    let mut storage = IrqStorage::new();
    let (publisher, mut consumer) = storage.split();
    publisher.publish(EVENT_RX_SUCCESS | EVENT_TX_COMPLETE);
    let batch = consumer.take_batch();
    publisher.publish(EVENT_RX_SUCCESS | EVENT_TX_TIMEOUT);
    assert_eq!(
        batch.collect::<Vec<_>>(),
        [IrqWork::RxSuccess, IrqWork::TxComplete]
    );
    assert_eq!(
        consumer.take_batch().collect::<Vec<_>>(),
        [IrqWork::RxSuccess, IrqWork::TxTimeout]
    );
}
