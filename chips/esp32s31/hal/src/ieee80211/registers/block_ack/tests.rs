use super::{MacExtraSoftApRxBlockAckEntryIndex, ordinary_entry_update_bit, rx_block_ack_bank};
use oer_esp32s31_pac::MacRxBlockAckEntryIndex;

#[test]
fn extra_softap_entry_domain_matches_the_vendor_allocator() {
    assert!(MacExtraSoftApRxBlockAckEntryIndex::new(0).is_some());
    assert!(MacExtraSoftApRxBlockAckEntryIndex::new(7).is_some());
    assert!(MacExtraSoftApRxBlockAckEntryIndex::new(8).is_none());
}

#[test]
fn ordinary_entries_are_reverse_addressed_but_update_by_logical_index() {
    for logical in 0..8 {
        let index = MacRxBlockAckEntryIndex::new(logical).expect("an ordinary entry");
        assert_eq!(
            rx_block_ack_bank(index).map(|bank| bank.get()),
            Some(7 - logical as usize)
        );
        assert_eq!(ordinary_entry_update_bit(index), 1 << logical);
    }
}
