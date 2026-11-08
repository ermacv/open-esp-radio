//! The 4-way handshake through the runner and the model's key table: the
//! pairwise key goes in before Message 3, a lost Message 4's protected
//! retry is read by the installed key while the port is closed, and the
//! retried Message 3 installs nothing again.

use super::*;
use oer_protocols_ieee80211_contracts_port::VifRole;
use oer_protocols_ieee80211_contracts_security::Ccmp128Key;
use oer_protocols_ieee80211_core_security_cipher::{
    aes::SoftwareAes128, ccmp_mpdu::CcmpAssociation,
};
use oer_protocols_ieee80211_core_security_rsn::{
    Ptk, PtkContext,
    frames::{OwnedAssociationSecurityIes, OwnedRsnIe, RsnTxFrame},
};
use oer_protocols_ieee80211_testing_model::ModelAir;
use oer_protocols_ieee80211_wire::ccmp::{CcmpHeader, CcmpKeyId, CcmpPacketNumber};
use oer_protocols_ieee80211_wire::security::Spp;

/// The supplicant's RSN element: PSK, CCMP, CCMP.
const SUPPLICANT_RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];
const SNONCE: [u8; 32] = [8; 32];
const EAPOL: [u8; 2] = [0x88, 0x8e];

fn management(subtype: u8, body: &[u8]) -> Vec<u8> {
    let mut frame = std::vec![subtype << 4, 0, 0, 0];
    frame.extend_from_slice(&ACCESS_POINT);
    frame.extend_from_slice(&PEER);
    frame.extend_from_slice(&ACCESS_POINT);
    frame.extend_from_slice(&[0, 0]);
    frame.extend_from_slice(body);
    frame
}

fn association_request() -> Vec<u8> {
    let mut body = std::vec![0x31, 0x04, 10, 0];
    body.extend_from_slice(&[0, SSID.len() as u8]);
    body.extend_from_slice(SSID);
    body.extend_from_slice(&[1, 8, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]);
    body.extend_from_slice(&[50, 4, 0x30, 0x48, 0x60, 0x6c]);
    body.extend_from_slice(&SUPPLICANT_RSN);
    management(0, &body)
}

/// The station's EAPOL frame to the BSS, unprotected.
fn uplink(eapol: &[u8]) -> Vec<u8> {
    let mut frame = std::vec![0x08, 0x01, 0, 0];
    frame.extend_from_slice(&ACCESS_POINT);
    frame.extend_from_slice(&PEER);
    frame.extend_from_slice(&ACCESS_POINT);
    frame.extend_from_slice(&[0x20, 0]);
    frame.extend_from_slice(&[0xaa, 0xaa, 3, 0, 0, 0]);
    frame.extend_from_slice(&EAPOL);
    frame.extend_from_slice(eapol);
    frame
}

/// The station's EAPOL frame to the BSS, sealed under `key` as a station
/// does once it installed its PTK.
fn protected_uplink(key: &[u8; 16], eapol: &[u8]) -> Vec<u8> {
    let mut frame = std::vec![0x08, 0x41, 0, 0];
    frame.extend_from_slice(&ACCESS_POINT);
    frame.extend_from_slice(&PEER);
    frame.extend_from_slice(&ACCESS_POINT);
    frame.extend_from_slice(&[0x30, 0]);
    frame.extend_from_slice(
        &CcmpHeader::new(CcmpPacketNumber::new(1).unwrap(), CcmpKeyId::PAIRWISE).encode(),
    );
    frame.extend_from_slice(&[0xaa, 0xaa, 3, 0, 0, 0]);
    frame.extend_from_slice(&EAPOL);
    frame.extend_from_slice(eapol);
    frame.extend_from_slice(&[0; 8]);
    CcmpAssociation::new(Spp::NotNegotiated)
        .seal(&mut SoftwareAes128, &Ccmp128Key::new(key), &mut frame)
        .unwrap();
    frame
}

/// The EAPOL bodies the access point sent to the peer, in order.
fn eapol_sent(air: &ModelAir) -> Vec<Vec<u8>> {
    air.sent()
        .iter()
        .filter(|sent| sent.role == VifRole::AccessPoint)
        .map(|sent| sent.frame.clone())
        .filter(|frame| frame.first() == Some(&0x08) && frame.get(30..32) == Some(&EAPOL[..]))
        .map(|frame| frame[32..].to_vec())
        .collect()
}

/// An EAPOL-Key frame's replay counter and nonce (IEEE 802.11-2020
/// 12.7.2, Figure 12-32).
fn counter_and_nonce(eapol: &[u8]) -> (u64, [u8; 32]) {
    (
        u64::from_be_bytes(eapol[9..17].try_into().unwrap()),
        eapol[17..49].try_into().unwrap(),
    )
}

fn station_ptk(anonce: [u8; 32]) -> Ptk {
    pmk().derive_ptk(
        oer_protocols_ieee80211_wire::security::rsn::Akm::Psk,
        PtkContext {
            authenticator_address: ACCESS_POINT,
            supplicant_address: PEER,
            authenticator_nonce: anonce,
            supplicant_nonce: SNONCE,
        },
    )
}

fn message2(ptk: &Ptk, replay: u64) -> Vec<u8> {
    let rsn_ie = OwnedRsnIe::<64>::try_copy(&SUPPLICANT_RSN).unwrap();
    let ies = OwnedAssociationSecurityIes::<128>::try_copy(&rsn_ie, &[]).unwrap();
    RsnTxFrame::<512>::message2_with_security_ies(
        oer_protocols_ieee80211_wire::security::rsn::Akm::Psk,
        ACCESS_POINT,
        replay,
        SNONCE,
        &ies,
    )
    .unwrap()
    .authenticate(ptk, &mut SoftwareAes128)
    .unwrap()
    .as_bytes()
    .to_vec()
}

fn message4(ptk: &Ptk, replay: u64) -> Vec<u8> {
    RsnTxFrame::<512>::message4(
        oer_protocols_ieee80211_wire::security::rsn::Akm::Psk,
        ACCESS_POINT,
        replay,
    )
    .unwrap()
    .authenticate(ptk, &mut SoftwareAes128)
    .unwrap()
    .as_bytes()
    .to_vec()
}

#[test]
fn a_lost_message4_is_read_protected_by_the_key_installed_before_message3() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, _device, shared);
        let meta = ModelRxMeta::new(channel(), -40, -90);
        let script = async {
            counted.idle(64).await;
            air.receive(&management(11, &[0, 0, 1, 0, 0, 0]), meta);
            counted.idle(64).await;
            air.receive(&association_request(), meta);
            // The response's end, then Message 1.
            for _ in 0..4 {
                counted.advance(&air, Duration::from_millis(1)).await;
            }
            let sent = eapol_sent(&air);
            assert_eq!(sent.len(), 1, "Message 1 after the association");
            let (replay1, anonce) = counter_and_nonce(&sent[0]);
            let ptk = station_ptk(anonce);
            assert_eq!(air.installed_pairwise(VifRole::AccessPoint), 0);

            // Message 2: the key goes in, then Message 3; the port stays
            // closed.
            air.receive(&uplink(&message2(&ptk, replay1)), meta);
            for _ in 0..4 {
                counted.advance(&air, Duration::from_millis(1)).await;
            }
            assert_eq!(air.installed_pairwise(VifRole::AccessPoint), 1);
            let sent = eapol_sent(&air);
            assert_eq!(sent.len(), 2, "Message 3 once the key is in");
            assert_eq!(shared.status().authorized, 0);

            // Message 4 is lost; Message 3's retry comes, and installs
            // nothing again.
            let mut retries = sent.len();
            for _ in 0..100 {
                counted.advance(&air, Duration::from_millis(100)).await;
                retries = eapol_sent(&air).len();
                if retries > 2 {
                    break;
                }
            }
            assert_eq!(retries, 3, "Message 3 is retried");
            assert_eq!(air.installed_pairwise(VifRole::AccessPoint), 1);
            let (replay3, _) = counter_and_nonce(&eapol_sent(&air)[2]);

            // The station answers the retry under its PTK: the hardware reads
            // it only with the key the runner installed.
            let protected = protected_uplink(ptk.temporal_key(), &message4(&ptk, replay3));
            assert!(
                air.receive_protected(&protected, meta, VifRole::AccessPoint),
                "the installed key opens the station's Message 4"
            );
            for _ in 0..4 {
                counted.advance(&air, Duration::from_millis(1)).await;
            }
            assert_eq!(shared.status().authorized, 1);
            assert_eq!(air.installed_pairwise(VifRole::AccessPoint), 1);
            stop_and_finish(&counted, &air, &shared).await;
        };
        let runner = stopped(drive(runner.run(&shared, &counted), script).await);
        assert!(runner.association.is_some());
        assert_eq!(air.installed_pairwise(VifRole::AccessPoint), 0);
    }));
}
