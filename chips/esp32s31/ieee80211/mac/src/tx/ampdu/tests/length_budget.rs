use super::*;

#[test]
fn prefix_budget_matches_commit_with_final_padding_and_delimiters() {
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<4>::new());
    let pool = frame_pool::<256, 3>();
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    owner.configure_max_aggregate_bytes(350).unwrap();
    let cookie = owner.begin().unwrap();
    let rate = HtRate::new(
        crate::tx::HtMcs::Mcs7,
        crate::tx::HtGuardInterval::Long800Ns,
        crate::tx::HtChannelWidth::Mhz40,
    );
    let commit = unlimited_ht_rate();
    owner
        .commit_ht(
            cookie,
            frame(&pool, 0, 101, |bytes| bytes.fill(0x42)),
            ht_frame_request(0, 101, 8, 2, commit),
        )
        .unwrap();
    let before = owner.prepared_aggregate(cookie).unwrap();
    let mut budget = owner.ht_length_budget(rate).unwrap();
    budget.push(103 + 8 + 4, 1).unwrap();
    assert_eq!(owner.prepared_aggregate(cookie).unwrap(), before);
    owner
        .commit_ht(
            cookie,
            frame(&pool, 1, 103, |bytes| bytes.fill(0x43)),
            ht_frame_request(0, 103, 8, 1, commit),
        )
        .unwrap();
    assert_eq!(
        budget.finish().unwrap(),
        owner.prepared_aggregate(cookie).unwrap()
    );
    assert!(matches!(
        budget.push(200, 0),
        Err(HtAmpduLengthError::AggregateTooLong(_))
    ));
    assert_eq!(
        budget.finish().unwrap(),
        owner.prepared_aggregate(cookie).unwrap()
    );
    budget.push(32 + 8 + 4, 0).unwrap();
    owner
        .commit_ht(
            cookie,
            frame(&pool, 2, 32, |bytes| bytes.fill(0x44)),
            ht_frame_request(0, 32, 8, 0, commit),
        )
        .unwrap();
    assert_eq!(
        budget.finish().unwrap(),
        owner.prepared_aggregate(cookie).unwrap()
    );
    owner.cancel(cookie).unwrap();
}

#[test]
fn fresh_budget_intersects_rate_limit_without_reserving_storage() {
    let storage = HtAmpduTxStorage::<32>::new();
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
    assert_eq!(storage.state(), crate::tx::TxDmaState::Free);
    assert_eq!(storage.frame_count(), 0);
}

#[test]
fn unsupported_slot_capacity_cannot_wrap_into_a_valid_budget() {
    let storage = HtAmpduTxStorage::<257>::new();
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
