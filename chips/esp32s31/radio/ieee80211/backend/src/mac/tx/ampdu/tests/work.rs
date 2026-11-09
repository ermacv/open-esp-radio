use super::*;

#[test]
fn rejected_publication_costs_nothing_and_abort_keeps_submitted_work() {
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<2>::new());
    let pool: PlacedTxPool<256, 0, 0, 1> = host_pool(std::boxed::Box::leak(std::boxed::Box::new(
        PinnedDmaTxPool::new(),
    )));
    let (index, ()) = pool.claim_network(0).unwrap().publish(
        oer_portable_memory::FrameLen::new(TX_AMPDU_METADATA_SIZE + 32).expect("test frame length"),
        |_| {},
    );
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    let cookie = owner.begin().unwrap();
    let rate = HtRate::new(
        crate::mac::tx::HtMcs::Mcs0,
        crate::mac::tx::HtGuardInterval::Long800Ns,
        crate::mac::tx::HtChannelWidth::Mhz20,
    );
    commit(
        &mut owner,
        cookie,
        pool.claim_radio(index).expect("ready placed slot"),
        32,
        8,
    )
    .unwrap();
    let aggregate = prepared(&mut owner, cookie).unwrap();
    let mut config = HtAmpduTxConfig::new(rate, aggregate.bytes, aggregate.subframes).unwrap();
    config.control = crate::mac::tx::TxControlFrame {
        protection: crate::mac::tx::protection::TxProtection::RtsCts {
            rate: oer_chips_esp32s31_hal::types::MacLegacyRate::Ofdm12M,
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
        Some(oer_chips_esp32s31_hal::types::MacTxControlFrame {
            protection: oer_chips_esp32s31_hal::types::MacTxProtection::RtsCts,
            rate: oer_chips_esp32s31_hal::types::MacLegacyRate::Ofdm12M,
            power_primary: 3,
            power_alternate: 4,
        })
    );
    assert_eq!(submitted.publications, 1);
    assert_eq!(submitted.mpdus, 1);
    assert_eq!(submitted.psdu_bytes, u32::from(aggregate.bytes));
    assert!(
        owner
            .begin_timeout_abort(
                &mut hardware,
                cookie,
                oer_chips_esp32s31_hal::types::MacTxAbortCause::TimeoutEdge
            )
            .unwrap()
    );
    owner
        .finish_timeout_abort(&mut hardware, cookie, drop)
        .unwrap();
    assert_eq!(owner.work(), submitted);
    assert_eq!(pool.claimed_slots(), 0);
}

/// Publish one 32-byte MPDU as an HE A-MPDU at `rate`; return its work.
fn he_publication_work(rate: HeRate) -> (HtAmpduLength, MacTxWork) {
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<2>::new());
    let pool = frame_pool::<256, 1>();
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    let cookie = owner.begin().unwrap();
    let density = HtAmpduDensity::NoRestriction;
    commit(&mut owner, cookie, frame(&pool, 0, 32, |_| {}), 32, 8).unwrap();
    let aggregate = prepared_he(&mut owner, cookie, rate, density).unwrap();
    let mut hardware = DetachingCompletionHardware::successful();
    owner
        .submit_he(
            &mut hardware,
            cookie,
            LegacyTxQueue::BestEffort,
            HeAmpduTxConfig::new(rate, 0, aggregate.bytes, aggregate.subframes, density).unwrap(),
        )
        .unwrap();
    let work = owner.work();
    owner.acknowledge_completion(&mut hardware).unwrap();
    owner.detach_completed(&mut hardware, cookie).unwrap();
    (aggregate, work)
}

#[test]
fn he_publication_records_its_su_ppdu_duration_and_dcm_stays_unestimated() {
    let gi = crate::mac::rx::HeGuardIntervalAndLtf::TwoLtf1600Ns;
    let (aggregate, work) = he_publication_work(HeRate::ldpc(crate::mac::tx::HeMcs::Mcs0, gi));
    assert_eq!(aggregate.bytes, 48);
    // 48-byte APEP, LDPC MCS0: 400 bits = 3 x 117 + 49, a_init = 2. N_pld
    // 411, N_avbits 822 < 411 + 1464/2, so one 1296-bit codeword: N_shrt
    // 237, N_punc 237 > 64.8 and 237 < 1.2 x 237: extra segment, a = 3.
    // 44 + 4 x 14.4 + 12 + 6 = 119.6 us.
    assert_eq!(work.ppdu_micros, 120);
    assert_eq!(work.unestimated_ppdus, 0);
    assert_eq!(work.psdu_bytes, 48);

    let dcm = HeRate::ldpc_dcm(crate::mac::tx::HeLdpcDcmMcs::Mcs0, gi);
    let (_, work) = he_publication_work(dcm);
    assert_eq!(work.ppdu_micros, 0);
    assert_eq!(work.unestimated_ppdus, 1);
}
