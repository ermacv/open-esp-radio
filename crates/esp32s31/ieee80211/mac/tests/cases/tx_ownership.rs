use crate::{support::*, *};
use oer_esp32s31_ieee80211_mac::tx::CompletionObservation;

fn observed<T>(event: CompletionObservation<T>) -> T {
    match event {
        CompletionObservation::Observed(value) => value,
        CompletionObservation::NoEffect => panic!("fixture needs an observed completion"),
    }
}

#[test]
fn each_publication_carries_its_control_frame_across_phy_and_queue_changes() {
    use oer_esp32s31_hal::types::{MacLegacyRate, MacTxControlFrame, MacTxProtection};
    use oer_esp32s31_ieee80211_mac::rx::HeGuardIntervalAndLtf;
    use oer_esp32s31_ieee80211_mac::tx::{
        HeSmpduTxConfig, HtTxConfig, TxControlFrame, protection::TxProtection,
    };

    let mut slot = core::pin::pin!(TxSlot::<512>::new_model().unwrap());
    let mut hardware = MockMmio::default();
    let mut expected = [None; 4];
    for protection in [
        TxProtection::CtsToSelf {
            rate: LegacyRate::Cck11MShort,
        },
        TxProtection::RtsCts {
            rate: LegacyRate::Ofdm12M,
        },
        TxProtection::None,
    ] {
        let control = TxControlFrame {
            protection,
            power_primary: 5,
            power_alternate: 6,
        };
        for queue in [
            LegacyTxQueue::Voice,
            LegacyTxQueue::Video,
            LegacyTxQueue::BestEffort,
            LegacyTxQueue::Background,
        ] {
            for phy in 0..3 {
                let cookie = slot.as_mut().reserve(512, 100).unwrap();
                // Unprotected PPDUs publish the vendor rate of their data rate.
                let unprotected_rate = match phy {
                    0 => {
                        let mut config = LegacyTxConfig::management_1m(100);
                        config.control = control;
                        slot.as_mut()
                            .submit_legacy(&mut hardware, cookie, queue, config)
                            .unwrap();
                        MacLegacyRate::Dsss1MLong
                    }
                    1 => {
                        let mut config = HtTxConfig::single_mpdu(
                            HtRate::new(
                                HtMcs::Mcs7,
                                HtGuardInterval::Long800Ns,
                                HtChannelWidth::Mhz20,
                            ),
                            96,
                            0,
                        )
                        .unwrap();
                        config.control = control;
                        slot.as_mut()
                            .submit_ht(&mut hardware, cookie, queue, config)
                            .unwrap();
                        MacLegacyRate::Ofdm24M
                    }
                    _ => {
                        let mut config = HeSmpduTxConfig::new(
                            HeRate::new(HeMcs::Mcs0, HeGuardIntervalAndLtf::TwoLtf1600Ns),
                            0,
                            96,
                        )
                        .unwrap();
                        config.control = control;
                        slot.as_mut()
                            .submit_he_smpdu(&mut hardware, cookie, queue, config)
                            .unwrap();
                        MacLegacyRate::Ofdm6M
                    }
                };
                let (mechanism, rate) = match protection {
                    TxProtection::None => (MacTxProtection::None, unprotected_rate),
                    TxProtection::CtsToSelf { .. } => {
                        (MacTxProtection::CtsToSelf, MacLegacyRate::Cck11MShort)
                    }
                    TxProtection::RtsCts { .. } => {
                        (MacTxProtection::RtsCts, MacLegacyRate::Ofdm12M)
                    }
                };
                let index = usize::from(queue.hardware_index());
                expected[index] = Some(MacTxControlFrame {
                    protection: mechanism,
                    rate,
                    power_primary: 5,
                    power_alternate: 6,
                });
                assert_eq!(hardware.tx_control, expected);
                assert_eq!(slot.state(), TxSlotState::HardwareOwned);
                hardware.set_tx_completion(
                    queue.hardware_index(),
                    MacTxCompletionObservation::new_model(0, 0),
                );
                slot.as_mut()
                    .acknowledge_completion(&mut hardware)
                    .unwrap()
                    .unwrap();
                slot.as_mut()
                    .detach_completed(&mut hardware, cookie)
                    .unwrap();
                assert_eq!(slot.state(), TxSlotState::Free);
            }
        }
    }
}

#[test]
fn completed_tx_keeps_buffer_until_detach_and_rejects_previous_generation() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model().unwrap());
    let mut hardware = MockMmio::default();
    let mut previous = None;
    for payload in 0..32_u8 {
        slot.as_mut().buffer_mut().unwrap()[0] = payload;
        let cookie = slot.as_mut().reserve(512, 100).unwrap();
        slot.as_mut().mark_hardware_owned(cookie).unwrap();
        assert!(
            slot.as_mut()
                .acknowledge_completion(&mut hardware)
                .unwrap()
                .is_none()
        );
        assert!(matches!(slot.as_mut().buffer_mut(), Err(TxError::Busy)));
        hardware.set_tx_completion(0, MacTxCompletionObservation::new_model(0, 0));
        let completion = slot
            .as_mut()
            .acknowledge_completion(&mut hardware)
            .unwrap()
            .unwrap();
        let completion = observed(completion);
        assert_eq!(completion.cookie(), cookie);
        assert!(matches!(slot.as_mut().buffer_mut(), Err(TxError::Busy)));
        assert!(matches!(
            slot.as_mut().reserve(512, 100),
            Err(TxError::Busy)
        ));
        if let Some(stale) = previous {
            assert_eq!(
                slot.as_mut().detach_completed(&mut hardware, stale),
                Err(TxError::Stale)
            );
            assert_eq!(slot.state(), TxSlotState::Completed);
        }
        slot.as_mut()
            .detach_completed(&mut hardware, cookie)
            .unwrap();
        assert_eq!(slot.as_mut().buffer_mut().unwrap()[0], payload);
        previous = Some(cookie);
    }
}

#[test]
fn failed_completion_detach_quarantines_storage_despite_success_status() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model().unwrap());
    let cookie = slot.as_mut().reserve(512, 100).unwrap();
    slot.as_mut().mark_hardware_owned(cookie).unwrap();
    let mut hardware = MockMmio::default();
    hardware.set_tx_completion(0, MacTxCompletionObservation::new_model(0, 0));
    slot.as_mut()
        .acknowledge_completion(&mut hardware)
        .unwrap()
        .unwrap();
    hardware.tx_detach_fails[0] = true;
    assert_eq!(
        slot.as_mut().detach_completed(&mut hardware, cookie),
        Err(TxError::DetachFailed)
    );
    assert_eq!(slot.state(), TxSlotState::ResetRequired);
    assert!(matches!(slot.as_mut().buffer_mut(), Err(TxError::Busy)));
    hardware.tx_detach_fails[0] = false;
    assert_eq!(
        slot.as_mut().detach_completed(&mut hardware, cookie),
        Err(TxError::Stale)
    );
    assert!(matches!(
        slot.as_mut().reserve(512, 100),
        Err(TxError::Busy)
    ));
}

#[test]
fn tx_slot_rejects_stale_cookie_and_completes_one_generation() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model().unwrap());
    slot.as_mut().buffer_mut().unwrap()[..4].copy_from_slice(&[1, 2, 3, 4]);
    let cookie = slot.as_mut().reserve(512, 100).unwrap();
    assert!(matches!(slot.as_mut().buffer_mut(), Err(TxError::Busy)));
    assert_eq!(size(slot.descriptor_word0()), 512);
    assert_eq!(length(slot.descriptor_word0()), 100);
    assert_eq!(slot.state(), TxSlotState::Reserved);
    assert_eq!(slot.as_mut().mark_hardware_owned(cookie), Ok(()));
    assert_eq!(
        slot.as_mut().mark_hardware_owned(cookie),
        Err(TxError::Stale)
    );

    let mut mmio = MockMmio::default();
    mmio.set_tx_completion(
        0,
        MacTxCompletionObservation::new_model(0, 0).with_trigger_flow_model(true),
    );

    let completion = slot
        .as_mut()
        .acknowledge_q0_completion(&mut mmio)
        .unwrap()
        .unwrap();
    let completion = observed(completion);
    assert_eq!(completion.cookie(), cookie);
    assert_eq!(completion.status(), 0);
    assert!(completion.is_trigger_flow());
    assert!(!completion.used_alternate_record());
    assert_eq!(slot.state(), TxSlotState::Completed);

    mmio.set_tx_queue_attached(0, true);
    slot.as_mut().detach_completed(&mut mmio, cookie).unwrap();
    assert_eq!(slot.state(), TxSlotState::Free);
}

#[test]
fn tx_slot_cancels_only_an_unpublished_reservation() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model().unwrap());
    let cookie = slot.as_mut().reserve(512, 100).unwrap();

    assert_eq!(slot.as_mut().cancel_reservation(cookie), Ok(()));
    assert_eq!(slot.state(), TxSlotState::Free);
    assert_eq!(slot.descriptor_word0(), 0);
    assert!(slot.as_mut().buffer_mut().is_ok());
    assert_eq!(
        slot.as_mut().cancel_reservation(cookie),
        Err(TxError::Stale)
    );
}

#[test]
fn executor_deadline_quarantines_hardware_owned_tx_storage_without_drop_panic() {
    let mut slot = std::boxed::Box::pin(TxSlot::<512>::new_model().unwrap());
    let cookie = slot.as_mut().reserve(512, 100).unwrap();
    slot.as_mut().mark_hardware_owned(cookie).unwrap();

    assert_eq!(slot.as_mut().require_reset(cookie), Ok(()));
    assert_eq!(slot.state(), TxSlotState::ResetRequired);
    assert!(matches!(slot.as_mut().buffer_mut(), Err(TxError::Busy)));
    assert_eq!(slot.as_mut().require_reset(cookie), Err(TxError::Stale));
    drop(slot);
}

#[test]
fn tx_completion_decodes_the_blob_ack_snr_byte() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model().unwrap());
    let cookie = slot.as_mut().reserve(512, 100).unwrap();
    slot.as_mut().mark_hardware_owned(cookie).unwrap();

    let mut mmio = MockMmio::default();
    // Encoded 0x8b plus the pinned 0x60 offset narrows to signed -21.
    mmio.set_tx_completion(
        0,
        MacTxCompletionObservation::new_model(0, 0).with_ack_snr_encoded_model(0x8b),
    );

    let completion = slot
        .as_mut()
        .acknowledge_q0_completion(&mut mmio)
        .unwrap()
        .unwrap();
    let completion = observed(completion);
    assert_eq!(completion.status(), 0);
    assert_eq!(completion.ack_snr_sample(), Some(-21));

    let failed =
        observed(TxCompletion::new_model(cookie, 5, 0).unwrap()).with_ack_snr_encoded_model(0x8b);
    assert_eq!(failed.ack_snr_sample(), None);

    mmio.set_tx_queue_attached(0, true);
    slot.as_mut().detach_completed(&mut mmio, cookie).unwrap();
}

#[test]
fn tx_slot_preserves_the_semantic_timeout_abort_order() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model().unwrap());
    let cookie = slot.as_mut().reserve(512, 100).unwrap();
    slot.as_mut().mark_hardware_owned(cookie).unwrap();

    let mut mmio = MockMmio::default();
    mmio.set_tx_timeout_pending(0, true);
    mmio.set_tx_queue_attached(0, true);

    assert_eq!(
        slot.as_mut().begin_timeout_abort(&mut mmio, cookie),
        Ok(true)
    );
    slot.as_mut()
        .finish_timeout_abort(&mut mmio, cookie)
        .unwrap();

    assert_eq!(slot.state(), TxSlotState::Free);
    assert!(!mmio.tx_queue_attached[0]);
    assert!(!mmio.tx_timeout_pending[0]);

    let invalidation = mmio
        .operations()
        .iter()
        .position(|operation| *operation == Operation::DisableTxQueue(0))
        .unwrap();
    let cca_release = mmio
        .operations()
        .iter()
        .position(|operation| *operation == Operation::ReleaseTxCca)
        .unwrap();
    let timeout_clear = mmio
        .operations()
        .iter()
        .position(|operation| {
            *operation == Operation::AcknowledgeTxEvent(0, MacTxDetachReason::Timeout)
        })
        .unwrap();
    assert!(invalidation < cca_release);
    assert!(cca_release < timeout_clear);
}

#[test]
fn tx_slot_disables_before_acknowledging_one_collision_queue() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model().unwrap());
    let cookie = slot.as_mut().reserve(512, 100).unwrap();
    slot.as_mut().mark_hardware_owned(cookie).unwrap();

    let mut mmio = MockMmio::default();
    mmio.set_tx_collision_pending(0, true);
    mmio.set_tx_queue_attached(0, true);

    assert_eq!(slot.as_mut().abort_collision(&mut mmio, cookie), Ok(true));
    assert_eq!(slot.state(), TxSlotState::Free);
    assert!(!mmio.tx_queue_attached[0]);
    assert!(!mmio.tx_collision_pending[0]);

    let disable = mmio
        .operations()
        .iter()
        .position(|operation| *operation == Operation::DisableTxQueue(0))
        .unwrap();
    let acknowledge = mmio
        .operations()
        .iter()
        .position(|operation| {
            *operation == Operation::AcknowledgeTxEvent(0, MacTxDetachReason::Collision)
        })
        .unwrap();
    assert!(disable < acknowledge);
}

#[test]
fn no_effect_keeps_dma_hardware_owned_and_faults_quarantine_it() {
    use oer_esp32s31_ieee80211_mac::tx::CompletionFault;
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model().unwrap());
    let cookie = slot.as_mut().reserve(512, 100).unwrap();
    slot.as_mut().mark_hardware_owned(cookie).unwrap();
    let mut hardware = MockMmio::default();
    hardware.set_tx_completion(0, MacTxCompletionObservation::new_model(1, 2));
    assert_eq!(
        slot.as_mut().acknowledge_completion(&mut hardware),
        Ok(Some(CompletionObservation::NoEffect))
    );
    assert_eq!(slot.state(), TxSlotState::HardwareOwned);
    assert_eq!(
        slot.as_mut().detach_completed(&mut hardware, cookie),
        Err(TxError::Stale)
    );
    assert!(matches!(slot.as_mut().buffer_mut(), Err(TxError::Busy)));
    hardware.set_tx_completion(0, MacTxCompletionObservation::new_model(1, 0xc0));
    assert_eq!(
        slot.as_mut().acknowledge_completion(&mut hardware),
        Err(TxError::Completion(CompletionFault::SecurityKeyError))
    );
    assert_eq!(slot.state(), TxSlotState::ResetRequired);
    assert!(matches!(slot.as_mut().buffer_mut(), Err(TxError::Busy)));
}
