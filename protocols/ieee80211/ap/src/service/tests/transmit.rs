use super::*;

fn begin(
    service: &mut AccessPointService<'_, oer_ieee80211_mac::security::spp::Association>,
    nonce: [u8; 32],
    replay: u64,
) -> ApEapolTransmit<512> {
    service.authenticate_open(PEER, oer_time::Instant::from_micros(0));
    service
        .associate_rsn(
            PEER,
            association_security(&WPA2_RSN),
            ht_capabilities(),
            nonce,
            replay,
            oer_time::Instant::from_micros(1),
        )
        .unwrap();
    service.begin_wpa2_frame(PEER).unwrap()
}

#[test]
fn message2_before_message1_terminal_cannot_arm_message3() {
    for acknowledged in [false, true] {
        let mut storage = AccessPointPeerStorage::new();
        let mut service = service(&mut storage);
        let (_message1, message1_context) = begin(&mut service, [7; 32], 9).into_parts();
        let message2 = signed_message2(&WPA2_RSN, &[], [7; 32], [8; 32]);
        let ApWpa2Progress::Transmit(message3) = service.on_eapol(PEER, message2).unwrap() else {
            panic!("verified Message 2 produces Message 3");
        };
        assert_eq!(service.next_wpa2_retry_deadline(), None);
        assert_eq!(
            service.observe_wpa2_transmit(
                message1_context,
                acknowledged,
                oer_time::Instant::from_micros(10),
            ),
            Ok(ApEapolTxCompletion::Superseded),
        );
        assert_eq!(service.next_wpa2_retry_deadline(), None);
        let (_message3, context) = message3.into_parts();
        assert_eq!(
            service.observe_wpa2_transmit(
                context,
                acknowledged,
                oer_time::Instant::from_micros(20)
            ),
            Ok(ApEapolTxCompletion::WindowArmed),
        );
        assert_eq!(
            service.next_wpa2_retry_deadline(),
            Some(oer_time::Instant::from_micros(100_020))
        );
    }
}

#[test]
fn message4_before_message3_terminal_keeps_the_handshake_complete() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    let (_message1, context) = begin(&mut service, [7; 32], 9).into_parts();
    service
        .observe_wpa2_transmit(context, true, oer_time::Instant::from_micros(2))
        .unwrap();
    let message2 = signed_message2(&WPA2_RSN, &[], [7; 32], [8; 32]);
    let ApWpa2Progress::Transmit(message3) = service.on_eapol(PEER, message2).unwrap() else {
        panic!("verified Message 2 produces Message 3");
    };
    let message4 = RsnTxFrame::<512>::message4(oer_ieee80211_mac::security::rsn::Akm::Psk, AP, 10)
        .unwrap()
        .authenticate(service.pending_ptk(PEER).unwrap());
    let message4 =
        OwnedEapolFrame::<512>::try_copy(RsnInterface::AccessPoint, PEER, message4.as_bytes())
            .unwrap();
    assert!(matches!(
        service.on_eapol(PEER, message4).unwrap(),
        ApWpa2Progress::AuthorizePeer
    ));
    let (_message3, context) = message3.into_parts();
    assert_eq!(
        service.observe_wpa2_transmit(context, true, oer_time::Instant::from_micros(20)),
        Ok(ApEapolTxCompletion::Superseded),
    );
    assert_eq!(service.next_wpa2_retry_deadline(), None);
    service
        .authorize(PEER, oer_time::Instant::from_micros(21))
        .unwrap();
    assert!(service.is_authorized(PEER));
}

#[test]
fn an_old_publication_does_not_refresh_a_reassociated_peer() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    let (_old, old_context) = begin(&mut service, [7; 32], 9).into_parts();
    let (_new, new_context) = begin(&mut service, [9; 32], 19).into_parts();
    service
        .observe_wpa2_transmit(new_context, false, oer_time::Instant::from_micros(20))
        .unwrap();
    let deadline = service.next_wpa2_retry_deadline();
    let activity = service.peer_status(PEER).unwrap().last_activity;
    assert_eq!(
        service.observe_wpa2_transmit(old_context, true, oer_time::Instant::from_micros(30)),
        Ok(ApEapolTxCompletion::Superseded),
    );
    assert_eq!(service.next_wpa2_retry_deadline(), deadline);
    assert_eq!(service.peer_status(PEER).unwrap().last_activity, activity);
}

#[test]
fn another_peer_authenticating_keeps_the_original_publication_current() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    let (_message1, context) = begin(&mut service, [7; 32], 9).into_parts();
    service.authenticate_open(OTHER, oer_time::Instant::from_micros(10));
    assert_eq!(
        service.observe_wpa2_transmit(context, true, oer_time::Instant::from_micros(20)),
        Ok(ApEapolTxCompletion::WindowArmed),
    );
    assert_eq!(
        service.next_wpa2_retry_deadline(),
        Some(oer_time::Instant::from_micros(1_000_020))
    );
}

#[test]
fn a_retry_terminal_does_not_postpone_its_next_deadline() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    let (_message1, context) = begin(&mut service, [7; 32], 9).into_parts();
    service
        .observe_wpa2_transmit(context, false, oer_time::Instant::from_micros(10))
        .unwrap();
    let ApWpa2RetryProgress::Transmit(retry) = service
        .take_due_wpa2_retry::<512>(oer_time::Instant::from_micros(100_010))
        .unwrap()
    else {
        panic!("the first unacknowledged M1 response window expired");
    };
    let deadline = service.next_wpa2_retry_deadline();
    let (_retry, context) = retry.into_parts();
    assert_eq!(
        service.observe_wpa2_transmit(context, true, oer_time::Instant::from_micros(500_000)),
        Ok(ApEapolTxCompletion::WindowRetained),
    );
    assert_eq!(service.next_wpa2_retry_deadline(), deadline);
}
