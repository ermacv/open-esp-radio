use super::*;

#[test]
fn prefix_budget_matches_commit_with_final_padding_and_delimiters() {
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<4, 256>::new());
    storage.as_mut().configure_max_aggregate_bytes(350).unwrap();
    let cookie = storage.as_mut().begin().unwrap();
    let rate = HtRate::new(
        crate::tx::HtMcs::Mcs7,
        crate::tx::HtGuardInterval::Long800Ns,
        crate::tx::HtChannelWidth::Mhz40,
    );
    storage.as_mut().next_frame_buffer(cookie).unwrap()[..101].fill(0x42);
    storage.as_mut().commit_frame(cookie, 101, 8, 2).unwrap();
    let before = storage.prepared_aggregate(cookie).unwrap();
    let mut budget = storage.ht_length_budget(rate).unwrap();
    budget.push(103 + 8 + 4, 1).unwrap();
    assert_eq!(storage.prepared_aggregate(cookie).unwrap(), before);
    storage.as_mut().next_frame_buffer(cookie).unwrap()[..103].fill(0x43);
    storage.as_mut().commit_frame(cookie, 103, 8, 1).unwrap();
    assert_eq!(
        budget.finish().unwrap(),
        storage.prepared_aggregate(cookie).unwrap()
    );
    assert!(matches!(
        budget.push(200, 0),
        Err(HtAmpduLengthError::AggregateTooLong(_))
    ));
    assert_eq!(
        budget.finish().unwrap(),
        storage.prepared_aggregate(cookie).unwrap()
    );
    budget.push(32 + 8 + 4, 0).unwrap();
    storage.as_mut().next_frame_buffer(cookie).unwrap()[..32].fill(0x44);
    storage.as_mut().commit_frame(cookie, 32, 8, 0).unwrap();
    assert_eq!(
        budget.finish().unwrap(),
        storage.prepared_aggregate(cookie).unwrap()
    );
}

#[test]
fn fresh_budget_intersects_rate_limit_without_reserving_storage() {
    let storage = HtAmpduTxStorage::<32, 0>::new();
    let rate = HtRate::new(
        crate::tx::HtMcs::Mcs0,
        crate::tx::HtGuardInterval::Short400Ns,
        crate::tx::HtChannelWidth::Mhz20,
    );
    let mut budget = storage.ht_length_budget(rate).unwrap();
    let mut admitted = 0;
    while budget.push(1500, 0).is_ok() {
        admitted += 1;
    }
    assert!(admitted > 0 && admitted < 32);
    assert_eq!(storage.state(), crate::tx::TxSlotState::Free);
    assert_eq!(storage.frame_count(), 0);
}

#[test]
fn unsupported_slot_capacity_cannot_wrap_into_a_valid_budget() {
    let storage = HtAmpduTxStorage::<257, 0>::new();
    let rate = HtRate::new(
        crate::tx::HtMcs::Mcs7,
        crate::tx::HtGuardInterval::Long800Ns,
        crate::tx::HtChannelWidth::Mhz40,
    );
    assert!(matches!(
        storage.ht_length_budget(rate),
        Err(HtAmpduTxError::Length(HtAmpduLengthError::InvalidLimits))
    ));
}
