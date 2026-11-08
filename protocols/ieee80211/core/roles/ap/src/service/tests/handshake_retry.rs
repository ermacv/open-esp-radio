//! Four-way handshakes whose Message 3 or 4 is lost or delayed, against a
//! station that applies wpa_supplicant's replay rule.

use super::*;
use oer_protocols_ieee80211_contracts_security::Ccmp128Key;
use oer_protocols_ieee80211_core_security_cipher::ccmp_mpdu::CcmpAssociation;
use oer_protocols_ieee80211_core_security_rsn::Ptk;
use oer_protocols_ieee80211_wire::ccmp::{CcmpHeader, CcmpKeyId, CcmpPacketNumber};
use oer_protocols_ieee80211_wire::security::Spp;

const ANONCE: [u8; 32] = [7; 32];
const SNONCE: [u8; 32] = [8; 32];

fn at(micros: u64) -> oer_portable_time::Instant {
    oer_portable_time::Instant::from_micros(micros)
}

fn station_ptk() -> Ptk {
    let mut derivation =
        oer_protocols_ieee80211_core_security_rsn::PmkDerivation::new(b"password", b"test-ap")
            .unwrap();
    let pmk = loop {
        if let Some(pmk) = derivation.step() {
            break pmk;
        }
    };
    pmk.derive_ptk(
        oer_protocols_ieee80211_wire::security::rsn::Akm::Psk,
        PtkContext {
            authenticator_address: AP,
            supplicant_address: PEER,
            authenticator_nonce: ANONCE,
            supplicant_nonce: SNONCE,
        },
    )
}

/// The station side of Message 3, modelled on wpa_supplicant
/// (`src/rsn_supp/wpa.c`): a MIC-bearing EAPOL-Key frame whose replay
/// counter does not exceed the last one that passed its MIC is dropped
/// (`wpa_sm_rx_eapol`); one that passes its MIC advances that counter
/// (`wpa_supplicant_verify_eapol_key_mic`), installs the GTK from the Key
/// RSC and is answered by Message 4 echoing its counter.
struct ReplayCheckingStation {
    ptk: Ptk,
    rx_replay_counter: Option<u64>,
}

/// The station's Message 4 and the GTK receive sequence it installed.
struct Message4 {
    frame: OwnedEapolFrame<512>,
    group_receive_sequence: CcmpPacketNumber,
}

impl ReplayCheckingStation {
    /// `None` when the station drops the frame as a replay.
    fn on_message3(&mut self, message3: &RsnTxFrame<512>) -> Option<Message4> {
        let key = message3.key_frame().unwrap();
        assert_eq!(key.message(), EapolKeyMessage::PairwiseMessage3);
        if self
            .rx_replay_counter
            .is_some_and(|last| key.replay_counter() <= last)
        {
            return None;
        }
        assert!(key.verify_mic(&self.ptk, &mut SoftwareAes128).unwrap());
        self.rx_replay_counter = Some(key.replay_counter());
        let plaintext = software_key_unwrap(self.ptk.kek(), key.key_data()).unwrap();
        parse_gtk_key_data(
            plaintext.as_bytes(),
            &AP_WPA2_PERSONAL_RSN_ELEMENT,
            &[],
            false,
        )
        .unwrap();
        let group_receive_sequence =
            CcmpPacketNumber::from_receive_sequence(*key.key_receive_sequence()).unwrap();
        let message4 = RsnTxFrame::<512>::message4(
            oer_protocols_ieee80211_wire::security::rsn::Akm::Psk,
            AP,
            key.replay_counter(),
        )
        .unwrap()
        .authenticate(&self.ptk, &mut SoftwareAes128)
        .unwrap();
        Some(Message4 {
            frame: OwnedEapolFrame::try_copy(RsnInterface::AccessPoint, PEER, message4.as_bytes())
                .unwrap(),
            group_receive_sequence,
        })
    }
}

/// A securing peer after Message 1 reached TX complete.
fn begin(
    service: &mut AccessPointService<'_, oer_protocols_ieee80211_wire::security::spp::Association>,
) {
    service.authenticate_open(PEER, at(0)).unwrap();
    service
        .associate_rsn(
            PEER,
            association_security(&WPA2_RSN),
            ht_capabilities(),
            ANONCE,
            9,
            at(1),
        )
        .unwrap();
    let (_message1, message1_context) = service.begin_wpa2_frame::<512>(PEER).unwrap().into_parts();
    service
        .observe_wpa2_transmit(message1_context, true, at(10))
        .unwrap();
}

/// Message 3 for the station's Message 2, its TX completion observed.
fn first_message3(
    service: &mut AccessPointService<'_, oer_protocols_ieee80211_wire::security::spp::Association>,
    gtk: &oer_protocols_ieee80211_core_upper_mac::ccmp::InstalledCcmpTxKey<TestGroupKey>,
) -> RsnTxFrame<512> {
    let ApWpa2Progress::Transmit(message3) = service
        .on_eapol(PEER, signed_message2(&WPA2_RSN, &[], ANONCE, SNONCE), gtk)
        .unwrap()
    else {
        panic!("a verified Message 2 produces Message 3");
    };
    let (message3, message3_context) = message3.into_parts();
    assert_eq!(
        service.observe_wpa2_transmit(message3_context, true, at(20)),
        Ok(ApEapolTxCompletion::WindowArmed)
    );
    message3
}

/// The Message 3 retransmission the response timeout produces.
fn retried_message3(
    service: &mut AccessPointService<'_, oer_protocols_ieee80211_wire::security::spp::Association>,
    gtk: &oer_protocols_ieee80211_core_upper_mac::ccmp::InstalledCcmpTxKey<TestGroupKey>,
) -> RsnTxFrame<512> {
    let ApWpa2RetryProgress::Transmit(retry) = service
        .take_due_wpa2_retry::<512, _>(service.next_wpa2_retry_deadline().unwrap(), gtk)
        .unwrap()
    else {
        panic!("the Message 3 response timeout retransmits");
    };
    let (retry, retry_context) = retry.into_parts();
    assert!(retry.retransmission());
    assert_eq!(
        service.observe_wpa2_transmit(retry_context, true, at(200_000)),
        Ok(ApEapolTxCompletion::WindowRetained)
    );
    retry
}

fn station() -> ReplayCheckingStation {
    ReplayCheckingStation {
        ptk: station_ptk(),
        rx_replay_counter: None,
    }
}

#[test]
fn a_lost_message4_is_recovered_by_a_message3_under_a_larger_counter() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    let gtk = gtk_numbers();
    begin(&mut service);
    // The BSS has sent group frames before this station joins.
    for _ in 0..5 {
        gtk.number().unwrap();
    }
    let message3 = first_message3(&mut service, &gtk);
    let mut station = station();
    let Some(Message4 {
        group_receive_sequence,
        ..
    }) = station.on_message3(&message3)
    else {
        panic!("the station accepts the first Message 3");
    };
    // The station accepts only group frames numbered after those handed out.
    assert_eq!(group_receive_sequence, gtk.packet_numbers().last());
    assert_eq!(group_receive_sequence.value(), 5);

    // Message 4 is lost; group traffic continues meanwhile. The same
    // Message 3 again would be dropped by this station.
    gtk.number().unwrap();
    gtk.number().unwrap();
    assert!(station.on_message3(&message3).is_none());
    let retry = retried_message3(&mut service, &gtk);
    let Some(Message4 {
        frame: message4,
        group_receive_sequence,
    }) = station.on_message3(&retry)
    else {
        panic!("the station accepts a retransmission under a larger counter");
    };
    assert_eq!(group_receive_sequence.value(), 7);

    // A Message 4 under a counter never sent is refused.
    let unsent = RsnTxFrame::<512>::message4(
        oer_protocols_ieee80211_wire::security::rsn::Akm::Psk,
        AP,
        message4.key_frame().unwrap().replay_counter() + 1,
    )
    .unwrap()
    .authenticate(&station.ptk, &mut SoftwareAes128)
    .unwrap();
    let unsent =
        OwnedEapolFrame::<512>::try_copy(RsnInterface::AccessPoint, PEER, unsent.as_bytes())
            .unwrap();
    assert!(matches!(
        service.on_eapol(PEER, unsent, &gtk).unwrap(),
        ApWpa2Progress::None
    ));
    assert!(!service.wpa2_authorized(PEER).unwrap());

    let duplicate =
        OwnedEapolFrame::<512>::try_copy(RsnInterface::AccessPoint, PEER, message4.as_bytes())
            .unwrap();
    assert!(matches!(
        service.on_eapol(PEER, message4, &gtk).unwrap(),
        ApWpa2Progress::AuthorizePeer
    ));
    assert_eq!(service.next_wpa2_retry_deadline(), None);
    // A duplicate after completion neither authorizes nor installs again.
    let again =
        OwnedEapolFrame::<512>::try_copy(RsnInterface::AccessPoint, PEER, duplicate.as_bytes())
            .unwrap();
    assert!(matches!(
        service.on_eapol(PEER, duplicate, &gtk).unwrap(),
        ApWpa2Progress::None
    ));
    service.authorize(PEER, at(300_000)).unwrap();
    assert!(matches!(
        service.on_eapol(PEER, again, &gtk).unwrap(),
        ApWpa2Progress::None
    ));
    assert!(service.is_authorized(PEER));
}

#[test]
fn a_delayed_message4_to_the_first_message3_authorizes_after_a_retry_was_sent() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    let gtk = gtk_numbers();
    begin(&mut service);
    let message3 = first_message3(&mut service, &gtk);
    let mut station = station();
    let delayed = station.on_message3(&message3).unwrap().frame;
    // The retry is prepared and sent, but never reaches the station.
    let lost_retry = retried_message3(&mut service, &gtk);
    assert!(matches!(
        service.on_eapol(PEER, delayed, &gtk).unwrap(),
        ApWpa2Progress::AuthorizePeer
    ));
    assert_eq!(service.next_wpa2_retry_deadline(), None);
    // The retry arriving late is answered, and that answer changes nothing.
    let late = station.on_message3(&lost_retry).unwrap().frame;
    assert!(matches!(
        service.on_eapol(PEER, late, &gtk).unwrap(),
        ApWpa2Progress::None
    ));
}

/// A Data MPDU from the station to the access point carrying `eapol` in
/// LLC/SNAP, sealed by CCMP under `temporal_key` with packet number 1.
fn protected_eapol(temporal_key: &[u8; 16], eapol: &[u8]) -> ([u8; 256], usize) {
    const LLC_EAPOL: [u8; 8] = [0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e];
    let mut frame = [0_u8; 256];
    // Data, To DS, Protected; Address 1 BSSID, 2 station, 3 destination.
    frame[..2].copy_from_slice(&[0x08, 0x41]);
    frame[4..10].copy_from_slice(&AP);
    frame[10..16].copy_from_slice(&PEER);
    frame[16..22].copy_from_slice(&AP);
    frame[24..32].copy_from_slice(
        &CcmpHeader::new(CcmpPacketNumber::new(1).unwrap(), CcmpKeyId::PAIRWISE).encode(),
    );
    frame[32..40].copy_from_slice(&LLC_EAPOL);
    let end = 40 + eapol.len();
    frame[40..end].copy_from_slice(eapol);
    let length = end + 8;
    CcmpAssociation::new(Spp::NotNegotiated)
        .seal(
            &mut SoftwareAes128,
            &Ccmp128Key::new(temporal_key),
            &mut frame[..length],
        )
        .unwrap();
    (frame, length)
}

#[test]
fn a_retried_message4_under_the_new_ptk_is_readable_while_the_port_is_closed() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    let gtk = gtk_numbers();
    begin(&mut service);
    let message3 = first_message3(&mut service, &gtk);
    let mut station = station();
    // The station installed the PTK with the first Message 3; its Message 4
    // is lost, and it answers the retry under that PTK.
    station.on_message3(&message3).unwrap();
    let retry = retried_message3(&mut service, &gtk);
    let message4 = station.on_message3(&retry).unwrap().frame;
    let (mut mpdu, length) = protected_eapol(station.ptk.temporal_key(), message4.as_bytes());

    // Before Message 4 the controlled port is closed: no data is admitted,
    // yet the key that opens the station's protected EAPOL is available.
    assert!(!service.is_authorized(PEER));
    assert_eq!(
        service.admit_downlink(PEER).err(),
        Some(ApServiceError::WrongPeerPhase)
    );
    let temporal_key = Ccmp128Key::new(service.pending_ptk(PEER).unwrap().temporal_key());
    let body = CcmpAssociation::new(Spp::NotNegotiated)
        .open(&mut SoftwareAes128, &temporal_key, &mut mpdu[..length])
        .unwrap();
    let (llc, eapol) = body.split_at(8);
    assert_eq!(llc, [0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e]);
    let eapol = OwnedEapolFrame::<512>::try_copy(RsnInterface::AccessPoint, PEER, eapol).unwrap();
    assert!(matches!(
        service.on_eapol(PEER, eapol, &gtk).unwrap(),
        ApWpa2Progress::AuthorizePeer
    ));
    service.authorize(PEER, at(300_000)).unwrap();
    assert!(service.admit_downlink(PEER).is_ok());
}

#[test]
fn message3_names_only_the_installed_gtk_it_carries() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    begin(&mut service);
    for other in [
        test_group_key(2, TEST_GTK_GENERATION),
        test_group_key(1, TEST_GTK_GENERATION + 1),
    ] {
        assert_eq!(
            service
                .on_eapol(
                    PEER,
                    signed_message2(&WPA2_RSN, &[], ANONCE, SNONCE),
                    &other
                )
                .err(),
            Some(ApWpa2Error::GroupKeyMismatch)
        );
    }
    // The refusal changed nothing: the installed GTK continues the handshake.
    let gtk = gtk_numbers();
    first_message3(&mut service, &gtk);
}

#[test]
fn a_recreated_service_announces_the_continuing_numbering_of_the_same_gtk() {
    let mut storage = AccessPointPeerStorage::new();
    let gtk = gtk_numbers();
    {
        let mut first = service(&mut storage);
        begin(&mut first);
        gtk.number().unwrap();
        first_message3(&mut first, &gtk);
    }
    // The key stays installed while the service is created again.
    gtk.number().unwrap();
    gtk.number().unwrap();
    let mut second = service(&mut storage);
    begin(&mut second);
    let message3 = first_message3(&mut second, &gtk);
    let Some(Message4 {
        group_receive_sequence,
        ..
    }) = station().on_message3(&message3)
    else {
        panic!("the station accepts Message 3");
    };
    assert_eq!(group_receive_sequence.value(), 3);
}
