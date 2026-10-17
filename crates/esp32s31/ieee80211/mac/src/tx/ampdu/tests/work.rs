use super::*;

#[test]
fn rejected_publication_costs_nothing_and_abort_keeps_submitted_work() {
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<2, 0>::new());
    let pool: PinnedDmaTxPoolHandle<256, 0, 0, 1> = PinnedDmaTxPool::pin_static(
        std::boxed::Box::leak(std::boxed::Box::new(PinnedDmaTxPool::new())),
    );
    let (index, ()) = pool
        .claim_network(0)
        .publish(TX_AMPDU_METADATA_SIZE + 32, |_| {});
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    let cookie = owner.begin().unwrap();
    let rate = HtRate::new(
        crate::tx::HtMcs::Mcs0,
        crate::tx::HtGuardInterval::Long800Ns,
        crate::tx::HtChannelWidth::Mhz20,
    );
    owner
        .commit_ht(
            cookie,
            pool.claim_radio(index),
            ht_frame_request(0, 32, 8, 0, rate),
        )
        .unwrap();
    let aggregate = owner.prepared_aggregate(cookie).unwrap();
    let mut config = HtAmpduTxConfig::new(rate, aggregate.bytes, aggregate.subframes).unwrap();
    config.control = crate::tx::TxControlFrame {
        protection: crate::tx::protection::TxProtection::RtsCts {
            rate: crate::tx::LegacyRate::Ofdm12M,
        },
        power_primary: 3,
        power_alternate: 4,
    };
    let mut hardware = DetachingCompletionHardware::successful();
    hardware.reject_publication = true;
    assert_eq!(
        owner.submit(&mut hardware, cookie, LegacyTxQueue::BestEffort, config),
        Err(HtAmpduTxError::QueueActive)
    );
    assert_eq!(owner.work(), Default::default());
    assert_eq!(hardware.control, None);
    assert_eq!(pool.claimed_slots(), 1);
    hardware.reject_publication = false;
    owner
        .submit(&mut hardware, cookie, LegacyTxQueue::BestEffort, config)
        .unwrap();
    let submitted = owner.work();
    assert_eq!(
        hardware.control,
        Some(oer_esp32s31_hal::types::MacTxControlFrame {
            protection: oer_esp32s31_hal::types::MacTxProtection::RtsCts,
            rate: oer_esp32s31_hal::types::MacLegacyRate::Ofdm12M,
            power_primary: 3,
            power_alternate: 4,
        })
    );
    assert_eq!(submitted.publications, 1);
    assert_eq!(submitted.mpdus, 1);
    assert_eq!(submitted.psdu_bytes, u32::from(aggregate.bytes));
    assert!(owner.begin_timeout_abort(&mut hardware, cookie).unwrap());
    owner.finish_timeout_abort(&mut hardware, cookie).unwrap();
    assert_eq!(owner.work(), submitted);
    assert_eq!(pool.claimed_slots(), 0);
}
