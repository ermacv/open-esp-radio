//! A four-way handshake whose Message 4 is lost, against a station that
//! applies wpa_supplicant's replay rule.

use super::*;
use oer_protocols_ieee80211_core_security_rsn::Ptk;
use oer_protocols_ieee80211_wire::ccmp::CcmpPacketNumber;

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

#[test]
fn a_lost_message4_is_recovered_by_a_message3_under_a_larger_counter() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    let gtk = gtk_numbers();
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

    // The BSS has sent group frames before this station joins.
    for _ in 0..5 {
        gtk.number().unwrap();
    }
    let ApWpa2Progress::Transmit(message3) = service
        .on_eapol(
            PEER,
            signed_message2(&WPA2_RSN, &[], ANONCE, SNONCE),
            gtk.packet_numbers(),
        )
        .unwrap()
    else {
        panic!("a verified Message 2 produces Message 3");
    };
    let (message3, message3_context) = message3.into_parts();
    let mut station = ReplayCheckingStation {
        ptk: station_ptk(),
        rx_replay_counter: None,
    };
    let Some(Message4 {
        frame: lost_message4,
        group_receive_sequence,
    }) = station.on_message3(&message3)
    else {
        panic!("the station accepts the first Message 3");
    };
    // The station accepts only group frames the BSS sends from now on.
    assert_eq!(group_receive_sequence, gtk.packet_numbers().last());
    assert_eq!(group_receive_sequence.value(), 5);
    assert_eq!(
        service.observe_wpa2_transmit(message3_context, true, at(20)),
        Ok(ApEapolTxCompletion::WindowArmed)
    );

    // Message 4 is lost; group traffic continues meanwhile. The same
    // Message 3 again would be dropped by this station.
    gtk.number().unwrap();
    gtk.number().unwrap();
    assert!(station.on_message3(&message3).is_none());
    let ApWpa2RetryProgress::Transmit(retry) = service
        .take_due_wpa2_retry::<512>(
            service.next_wpa2_retry_deadline().unwrap(),
            gtk.packet_numbers(),
        )
        .unwrap()
    else {
        panic!("the Message 3 response timeout retransmits");
    };
    let (retry, retry_context) = retry.into_parts();
    assert!(retry.retransmission());
    let Some(Message4 {
        frame: message4,
        group_receive_sequence,
    }) = station.on_message3(&retry)
    else {
        panic!("the station accepts a retransmission under a larger counter");
    };
    assert_eq!(group_receive_sequence.value(), 7);
    assert_eq!(
        service.observe_wpa2_transmit(retry_context, true, at(200_000)),
        Ok(ApEapolTxCompletion::WindowRetained)
    );

    // The first Message 4 arriving late is not the awaited answer any more.
    assert!(matches!(
        service
            .on_eapol(PEER, lost_message4, gtk.packet_numbers())
            .unwrap(),
        ApWpa2Progress::None
    ));
    assert!(!service.wpa2_authorized(PEER).unwrap());
    assert!(matches!(
        service
            .on_eapol(PEER, message4, gtk.packet_numbers())
            .unwrap(),
        ApWpa2Progress::AuthorizePeer
    ));
    assert!(service.wpa2_authorized(PEER).unwrap());
    assert_eq!(service.next_wpa2_retry_deadline(), None);
}
