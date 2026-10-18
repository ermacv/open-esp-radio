use super::*;

#[test]
fn discovery_owns_management_tx_without_creating_a_peer_or_association_completion() {
    exercise_probe_completion(0);
}

#[test]
fn probe_ack_timeout_retains_receiver_sequence_and_terminal_attempts() {
    exercise_probe_completion(5);
}

#[test]
fn probe_terminal_hardware_error_is_not_classified_as_a_missing_ack() {
    exercise_probe_completion(3);
}

fn exercise_probe_completion(status: u8) {
    let ap = [2, 0, 0, 0, 0, 1];
    let mut hardware = Hardware::default();
    let mut beacon = [0; AP_BEACON_CAPACITY];
    let mut peers = oer_ieee80211_ap::AccessPointPeerStorage::new();
    let mut pairwise = crate::security::ApPairwiseKeyStorage::new();
    let engine = ApEngine::start(
        &mut hardware,
        AccessPointService::new(
            ap,
            Pmk::derive(b"password", b"ap").unwrap(),
            RsnGtk::new(1, true, [7; 16]).unwrap(),
            oer_ieee80211_ap::AccessPointClientLimit::new(2).unwrap(),
            oer_ieee80211_ap::AccessPointInactiveTimeout::default(),
            &mut peers,
        ),
        &mut beacon,
        &mut pairwise,
        &WifiSsid::new(b"ap").unwrap(),
        oer_ieee80211_mac::channel::WifiChannel::mhz20(6).unwrap(),
        100,
        2,
        1,
    )
    .unwrap_or_else(|_| panic!("AP engine starts"));
    let mut slot = pin!(TxSlot::<512>::new_model());
    let mut mac = ApMac::new(
        engine,
        WifiTxResources {
            slot: slot.as_mut(),
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: Power,
            entropy: || 0,
            timer: SkipClock::new(),
        },
        ApTxConfig {
            publication_timeout: oer_time::Duration::from_micros(1_000),
        },
    );

    let peer = [2, 0, 0, 0, 0, 2];
    let mut request = [0; 28];
    request[0] = 0x40;
    request[4..10].fill(0xff);
    request[10..16].copy_from_slice(&peer);
    request[16..22].fill(0xff);
    request[24..].copy_from_slice(&[0, 2, b'a', b'p']);
    let mut output = [0; 256];
    let outcome = mac
        .publish_management(
            &mut hardware,
            &request,
            [0; 32],
            0,
            oer_time::Instant::from_micros(100),
            &mut output,
        )
        .unwrap();
    assert!(matches!(
        outcome,
        ApManagementOutcome::Response {
            begin_wpa2: false,
            ..
        }
    ));
    assert_eq!(
        mac.pending_publication_kind(),
        Some(ApPendingPublicationKind::ProbeResponse)
    );
    assert!(mac.engine().peer_status(peer).is_none());
    assert_eq!(hardware.publications, 1);
    assert!(
        mac.publish_management(
            &mut hardware,
            &request,
            [0; 32],
            0,
            oer_time::Instant::from_micros(101),
            &mut output
        )
        .is_err()
    );
    assert_eq!(
        hardware.publications, 1,
        "hardware owner must not be overwritten"
    );
    let sequence_control = u16::from_le_bytes([output[22], output[23]]);
    let mut terminal = None;
    for attempt in 0..32 {
        hardware.completion = Some(MacTxCompletionObservation::new_model(status, 0));
        let (progress, action) = mac
            .service_tx(
                &mut hardware,
                WifiTxWake::Interrupt {
                    events: oer_esp32s31_ieee80211_mac::irq::EVENT_TX_COMPLETE,
                },
                oer_time::Instant::from_micros(102 + attempt),
            )
            .unwrap();
        if progress == oer_esp32s31_ieee80211::tx::WifiTxProgress::Complete {
            terminal = Some(action);
            break;
        }
        assert!(
            mac.first_probe_failure().is_none(),
            "retry is not a terminal failure"
        );
    }
    if status == 0 {
        assert_eq!(terminal, Some(ApTxCompletionAction::None));
        assert!(mac.first_probe_failure().is_none());
    } else {
        assert_eq!(terminal, Some(ApTxCompletionAction::PublicationFailed));
        let failure = mac.first_probe_failure().unwrap();
        assert_eq!(failure.receiver, peer);
        assert_eq!(failure.sequence_control, sequence_control);
        assert_eq!(failure.started_at, oer_time::Instant::from_micros(100));
        assert!(failure.completed_at >= oer_time::Instant::from_micros(102));
        assert!(matches!(
            failure.outcome,
            OrdinaryTxOutcome::HardwareFailure(_)
        ));
        assert_eq!(
            failure.outcome.report().status.attempts,
            hardware.publications
        );
        assert_eq!(mac.observation().tx_failures.hardware_failures, 1);
    }
    assert_eq!(
        hardware.publications, 1,
        "discovery must not retain a retry series"
    );
    assert_eq!(
        mac.observation().tx_failures.probe_ack_timeouts,
        u8::from(status == 5)
    );
    // A new MAC must not mint discovery credit while the shared budget is empty.
    for index in 0..100 {
        request[10..16].copy_from_slice(&[2, 1, 0, 0, 0, index]);
        assert_eq!(
            mac.publish_management(
                &mut hardware,
                &request,
                [0; 32],
                0,
                oer_time::Instant::from_micros(103 + u64::from(index)),
                &mut output
            )
            .unwrap(),
            ApManagementOutcome::Ignored
        );
    }
    assert_eq!(hardware.publications, 1);
    assert!(matches!(
        mac.publish_management(
            &mut hardware,
            &request,
            [0; 32],
            0,
            oer_time::Instant::from_micros(10_100),
            &mut output
        )
        .unwrap(),
        ApManagementOutcome::Response { .. }
    ));
    hardware.completion = Some(MacTxCompletionObservation::new_model(0, 0));
    mac.service_tx(
        &mut hardware,
        WifiTxWake::Interrupt {
            events: oer_esp32s31_ieee80211_mac::irq::EVENT_TX_COMPLETE,
        },
        oer_time::Instant::from_micros(10_102),
    )
    .unwrap();
    let publications = hardware.publications;
    assert_eq!(mac.observation().association_responses_transmitted, 0);
    assert_eq!(mac.observation().authentication_responses_transmitted, 0);
    assert!(mac.engine().peer_status(peer).is_none());
    request[27] = b'x';
    assert_eq!(
        mac.publish_management(
            &mut hardware,
            &request,
            [0; 32],
            0,
            oer_time::Instant::from_micros(103),
            &mut output
        )
        .unwrap(),
        ApManagementOutcome::Ignored
    );
    assert_eq!(hardware.publications, publications);
    assert!(mac.try_into_parts().is_ok());
}
