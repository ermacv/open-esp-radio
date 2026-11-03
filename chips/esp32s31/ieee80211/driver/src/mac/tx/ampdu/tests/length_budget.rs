use super::*;

#[test]
fn prefix_budget_matches_the_prepared_batch_with_final_padding_and_delimiters() {
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<4>::new());
    let pool = frame_pool::<256, 3>();
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    owner.configure_max_aggregate_bytes(350).unwrap();
    let cookie = owner.begin().unwrap();
    let rate = HtRate::new(
        crate::mac::tx::HtMcs::Mcs7,
        crate::mac::tx::HtGuardInterval::Long800Ns,
        crate::mac::tx::HtChannelWidth::Mhz40,
    );
    // Each PSDU's empty delimiters, by its length: 113 octets take two,
    // 115 one, 44 none.
    let delimiters = |psdu: u16| match psdu {
        113 => Some(2),
        115 => Some(1),
        _ => Some(0),
    };
    commit(
        &mut owner,
        cookie,
        frame(&pool, 0, 101, |bytes| bytes.fill(0x42)),
        101,
        8,
    )
    .unwrap();
    let before = owner.prepare_format(cookie, delimiters, false).unwrap();
    // The budget starts from the prepared prefix.
    let mut budget = owner.ht_length_budget(rate).unwrap();
    assert_eq!(budget.finish().unwrap(), before);
    budget.push(103 + 8 + 4, 1).unwrap();
    assert_eq!(
        owner.prepare_format(cookie, delimiters, false).unwrap(),
        before
    );
    commit(
        &mut owner,
        cookie,
        frame(&pool, 1, 103, |bytes| bytes.fill(0x43)),
        103,
        8,
    )
    .unwrap();
    assert_eq!(
        budget.finish().unwrap(),
        owner.prepare_format(cookie, delimiters, false).unwrap()
    );
    assert!(matches!(
        budget.push(200, 0),
        Err(HtAmpduLengthError::AggregateTooLong(_))
    ));
    assert_eq!(
        budget.finish().unwrap(),
        owner.prepare_format(cookie, delimiters, false).unwrap()
    );
    budget.push(32 + 8 + 4, 0).unwrap();
    commit(
        &mut owner,
        cookie,
        frame(&pool, 2, 32, |bytes| bytes.fill(0x44)),
        32,
        8,
    )
    .unwrap();
    assert_eq!(
        budget.finish().unwrap(),
        owner.prepare_format(cookie, delimiters, false).unwrap()
    );
    owner.cancel(cookie, drop).unwrap();
}

#[test]
fn fresh_budget_intersects_rate_limit_without_reserving_storage() {
    let storage = HtAmpduTxStorage::<32>::new();
    let rate = HtRate::new(
        crate::mac::tx::HtMcs::Mcs0,
        crate::mac::tx::HtGuardInterval::Short400Ns,
        crate::mac::tx::HtChannelWidth::Mhz20,
    );
    let mut budget = storage.ht_length_budget(rate).unwrap();
    let mut admitted = 0;
    while budget.push(1500, 0).is_ok() {
        admitted += 1;
    }
    assert!(admitted > 0 && admitted < 32);
    assert_eq!(storage.state(), crate::mac::tx::TxDmaState::Free);
    assert_eq!(storage.frame_count(), 0);
}

#[test]
fn unsupported_slot_capacity_cannot_wrap_into_a_valid_budget() {
    let storage = HtAmpduTxStorage::<257>::new();
    let rate = HtRate::new(
        crate::mac::tx::HtMcs::Mcs7,
        crate::mac::tx::HtGuardInterval::Long800Ns,
        crate::mac::tx::HtChannelWidth::Mhz40,
    );
    assert!(matches!(
        storage.ht_length_budget(rate),
        Err(HtAmpduTxError::Length(HtAmpduLengthError::InvalidLimits))
    ));
}
