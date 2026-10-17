use super::MacExtraSoftApRxBlockAckEntryIndex;

#[test]
fn extra_softap_entry_domain_matches_the_vendor_allocator() {
    assert!(MacExtraSoftApRxBlockAckEntryIndex::new(0).is_some());
    assert!(MacExtraSoftApRxBlockAckEntryIndex::new(7).is_some());
    assert!(MacExtraSoftApRxBlockAckEntryIndex::new(8).is_none());
}
