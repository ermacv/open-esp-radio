use super::*;
use crate::beacon::ApBeacon;
use crate::security_policy::ApSecurityPolicy;
use crate::service::{
    AccessPointClientLimit, AccessPointInactiveTimeout, AccessPointPeerStorage,
    ApGroupKeyInstallation,
};
use oer_protocols_ieee80211_core_security_rsn::frames::RsnGtk;
use oer_protocols_ieee80211_core_upper_mac::ccmp::{CcmpPacketNumberStep, SoftwareCcmpKey};
use oer_protocols_ieee80211_wire::{
    ap::profile::{LegacyRates, WmmParameters},
    beacon::AP_BEACON_CAPACITY,
    channel::ChannelWidth,
    extensions::wmm::WmmAcParameters,
    ht::HtLocalCapabilities,
    security::spp::Association,
    sequence::SequenceNumber,
    ssid::WifiSsid,
};

const AP: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
const PEER: [u8; 6] = [0x02, 0, 0, 0, 0, 2];
const SSID: &[u8] = b"test-ap";
/// The supplicant's RSN element: PSK, CCMP, CCMP.
const SUPPLICANT_RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];

const ADVERTISEMENT: Advertisement = match Advertisement::new(
    LegacyRates::new(
        [0x8b, 0x96, 0x82, 0x84, 0x0c, 0x18, 0x30, 0x60],
        [0x6c, 0x12, 0x24, 0x48],
    ),
    HtLocalCapabilities::new(0x100c, 0x03, 0xff, 0x01),
    match WmmParameters::new(
        4,
        false,
        [WmmAcParameters {
            admission_control_mandatory: false,
            aifsn: oer_portable_wire::U8::constant::<3>(),
            ecw_min: oer_portable_wire::U8::constant::<4>(),
            ecw_max: oer_portable_wire::U8::constant::<10>(),
            txop_limit_units_32_us: 0,
        }; 4],
    ) {
        Some(value) => value,
        None => panic!("the WMM parameters fit their fields"),
    },
    0x0421,
) {
    Some(value) => value,
    None => panic!("the advertisement leaves Privacy to the security mode"),
};

/// The GTK the test BSS announces, numbered by software.
struct GroupKey;

impl SoftwareCcmpKey for GroupKey {}

impl ApGroupTransmitKey for GroupKey {
    fn installation(&self) -> ApGroupKeyInstallation {
        ApGroupKeyInstallation {
            key_id: GroupKeyId::new(1).unwrap(),
            generation: 3,
        }
    }
}

fn group_key() -> InstalledCcmpTxKey<GroupKey> {
    InstalledCcmpTxKey::install(GroupKey, CcmpPacketNumberStep::ONE)
}

fn channel() -> Channel {
    Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap()
}

fn engine<'s>(
    storage: &'s mut AccessPointPeerStorage,
    beacon: &'s mut [u8; AP_BEACON_CAPACITY],
) -> ApEngine<'s, Association> {
    let pmk = {
        let mut derivation =
            oer_protocols_ieee80211_core_security_rsn::PmkDerivation::new(b"password", SSID)
                .unwrap();
        loop {
            if let Some(pmk) = derivation.step() {
                break pmk;
            }
        }
    };
    let service = AccessPointService::<Association>::new(
        AP,
        pmk,
        RsnGtk::new(GroupKeyId::new(1).unwrap(), true, &[0x55; 16]),
        3,
        AccessPointClientLimit::new(2).unwrap(),
        AccessPointInactiveTimeout::default(),
        storage.lend(),
    )
    .unwrap();
    let beacon = ApBeacon::new(
        beacon,
        &ADVERTISEMENT,
        AP,
        &WifiSsid::new(SSID).unwrap(),
        channel(),
        100,
        2,
        SequenceNumber::ZERO,
        ApSecurityPolicy::Wpa2Personal,
    )
    .unwrap();
    ApEngine::new(
        service,
        beacon,
        ADVERTISEMENT,
        channel(),
        GroupKeyId::new(1).unwrap(),
    )
}

const fn at(micros: u64) -> Instant {
    Instant::from_micros(micros)
}

/// A management frame of `subtype` from the peer to the AP with `body`.
fn management(subtype: u8, receiver: [u8; 6], bssid: [u8; 6], body: &[u8]) -> std::vec::Vec<u8> {
    let mut frame = std::vec![subtype << 4, 0, 0, 0];
    frame.extend_from_slice(&receiver);
    frame.extend_from_slice(&PEER);
    frame.extend_from_slice(&bssid);
    frame.extend_from_slice(&[0, 0]);
    frame.extend_from_slice(body);
    frame
}

fn probe_request() -> std::vec::Vec<u8> {
    let mut body = std::vec![0, SSID.len() as u8];
    body.extend_from_slice(SSID);
    body.extend_from_slice(&[1, 4, 0x82, 0x84, 0x8b, 0x96]);
    management(4, [0xff; 6], [0xff; 6], &body)
}

fn authentication() -> std::vec::Vec<u8> {
    management(11, AP, AP, &[0, 0, 1, 0, 0, 0])
}

fn association_request() -> std::vec::Vec<u8> {
    let mut body = std::vec![0x31, 0x04, 10, 0];
    body.extend_from_slice(&[0, SSID.len() as u8]);
    body.extend_from_slice(SSID);
    body.extend_from_slice(&[1, 8, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]);
    body.extend_from_slice(&[50, 4, 0x30, 0x48, 0x60, 0x6c]);
    body.extend_from_slice(&SUPPLICANT_RSN);
    management(0, AP, AP, &body)
}

/// A To-DS data frame from the peer to the BSS carrying `ether_type`.
fn uplink(ether_type: u16, payload: &[u8]) -> std::vec::Vec<u8> {
    let mut frame = std::vec![0x08, 0x01, 0, 0];
    frame.extend_from_slice(&AP);
    frame.extend_from_slice(&PEER);
    frame.extend_from_slice(&[0x02, 0, 0, 0, 0, 9]);
    frame.extend_from_slice(&[0x10, 0]);
    frame.extend_from_slice(&[0xaa, 0xaa, 3, 0, 0, 0]);
    frame.extend_from_slice(&ether_type.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// Run the open authentication and the RSN association of the peer.
fn associate(engine: &mut ApEngine<'_, Association>) {
    let mut output = [0; 512];
    let request = authentication();
    let received = engine
        .receive(
            &request,
            Integrity::Unverified,
            at(1),
            Some([7; 32]),
            1,
            &mut output,
        )
        .unwrap();
    assert!(matches!(
        received,
        ApReceived::Respond {
            after: ApAfter::Nothing,
            ..
        }
    ));
    let request = association_request();
    let received = engine
        .receive(
            &request,
            Integrity::Unverified,
            at(2),
            Some([7; 32]),
            1,
            &mut output,
        )
        .unwrap();
    assert_eq!(
        received,
        ApReceived::Respond {
            frame: ApFrame {
                len: match received {
                    ApReceived::Respond { frame, .. } => frame.len,
                    _ => 0,
                },
                kind: ApFrameKind::Management { peer: PEER },
            },
            after: ApAfter::BeginWpa2 { peer: PEER },
        }
    );
}

#[test]
fn a_probe_for_the_bss_is_answered_once_per_interval() {
    let (mut storage, mut beacon) = (AccessPointPeerStorage::new(), [0; AP_BEACON_CAPACITY]);
    let mut engine = engine(&mut storage, &mut beacon);
    let mut output = [0; 512];
    let probe = probe_request();
    let ApReceived::Respond { frame, after } = engine
        .receive(
            &probe,
            Integrity::Unverified,
            at(0),
            Some([0; 32]),
            0,
            &mut output,
        )
        .unwrap()
    else {
        panic!("the probe is answered");
    };
    assert_eq!(frame.kind, ApFrameKind::Management { peer: PEER });
    assert_eq!(after, ApAfter::Nothing);
    // A Probe Response, from the BSS, to the peer.
    assert_eq!(output[0], 0x50);
    assert_eq!(output[4..10], PEER);
    assert_eq!(output[16..22], AP);
    let again = engine
        .receive(
            &probe,
            Integrity::Unverified,
            at(9_999),
            Some([0; 32]),
            0,
            &mut output,
        )
        .unwrap();
    assert_eq!(again, ApReceived::Nothing);
    let later = engine
        .receive(
            &probe,
            Integrity::Unverified,
            at(10_000),
            Some([0; 32]),
            0,
            &mut output,
        )
        .unwrap();
    assert!(matches!(later, ApReceived::Respond { .. }));
}

#[test]
fn an_rsn_association_starts_the_handshake_and_its_eapol_reaches_the_authenticator() {
    let (mut storage, mut beacon) = (AccessPointPeerStorage::new(), [0; AP_BEACON_CAPACITY]);
    let mut engine = engine(&mut storage, &mut beacon);
    associate(&mut engine);
    let message1 = engine.begin_wpa2(PEER).unwrap();
    let (frame, _context) = message1.into_parts();
    let mut output = [0; 512];
    let written = engine.write_eapol(&frame, &mut output).unwrap();
    assert_eq!(written.kind, ApFrameKind::Management { peer: PEER });
    // A From-DS data frame to the peer with the EAPOL EtherType.
    assert_eq!(output[1] & 0x03, 0x02);
    assert_eq!(output[4..10], PEER);
    assert_eq!(output[30..32], EAPOL_ETHER_TYPE.to_be_bytes());
    // The peer's EAPOL comes to the authenticator; its other data does not
    // reach the network before it is authorized.
    let eapol = uplink(EAPOL_ETHER_TYPE, &[1, 3, 0, 0]);
    assert_eq!(
        engine
            .receive(
                &eapol,
                Integrity::Unverified,
                at(3),
                Some([0; 32]),
                0,
                &mut output
            )
            .unwrap(),
        ApReceived::Eapol {
            peer: PEER,
            body: &[1, 3, 0, 0],
        }
    );
    let ip = uplink(0x0800, &[0x45]);
    assert_eq!(
        engine
            .receive(
                &ip,
                Integrity::Unverified,
                at(3),
                Some([0; 32]),
                0,
                &mut output
            )
            .unwrap(),
        ApReceived::Nothing
    );
    // A malformed EAPOL-Key frame changes nothing.
    assert!(matches!(
        engine.eapol(PEER, &[1, 3, 0, 0], &group_key()).unwrap(),
        ApEapol::Nothing
    ));
}

#[test]
fn the_network_reaches_only_an_authorized_peer_and_the_group() {
    let (mut storage, mut beacon) = (AccessPointPeerStorage::new(), [0; AP_BEACON_CAPACITY]);
    let mut engine = engine(&mut storage, &mut beacon);
    associate(&mut engine);
    let mut output = [0; 512];
    assert_eq!(
        engine
            .encapsulate(PEER, AP, 0x0800, &[0x45], &mut output)
            .unwrap(),
        None,
        "the peer is not authorized"
    );
    let group = engine
        .encapsulate([0xff; 6], AP, 0x0806, &[1, 2], &mut output)
        .unwrap()
        .unwrap();
    assert_eq!(group.kind, ApFrameKind::Group);
    // Protected, From-DS, to the group, with room for the CCMP header.
    assert_eq!(output[1], 0x42);
    assert_eq!(output[4..10], [0xff; 6]);
    assert_eq!(group.len, 24 + CCMP_HEADER_LEN + 8 + 2);
}

#[test]
fn the_beacon_follows_the_tsf_from_its_restart() {
    let (mut storage, mut beacon) = (AccessPointPeerStorage::new(), [0; AP_BEACON_CAPACITY]);
    let mut engine = engine(&mut storage, &mut beacon);
    let mut output = [0; 512];
    assert!(engine.beacon(at(5), &mut output).unwrap().is_none());
    assert_eq!(engine.next_beacon(), None, "no TSF runs");
    engine.tsf_started(at(1_000));
    assert_eq!(engine.next_beacon(), Some(at(1_000)));
    let (frame, fresh, _context) = engine.beacon(at(1_250), &mut output).unwrap().unwrap();
    assert_eq!(frame.kind, ApFrameKind::Beacon);
    // The Timestamp is the TSF at the preparation: 250 µs after the restart.
    assert_eq!(output[24..32], 250_u64.to_le_bytes());
    // Fresh until the next TBTT on the TSF, one interval (100 TU) on.
    assert_eq!(fresh, at(1_000 + 102_400));
    assert_eq!(engine.next_beacon(), Some(fresh));
    assert!(engine.beacon(at(50_000), &mut output).unwrap().is_none());
    engine.tsf_stopped();
    assert_eq!(engine.next_beacon(), None);
}

#[test]
fn a_departing_peer_is_forgotten() {
    let (mut storage, mut beacon) = (AccessPointPeerStorage::new(), [0; AP_BEACON_CAPACITY]);
    let mut engine = engine(&mut storage, &mut beacon);
    associate(&mut engine);
    let mut output = [0; 512];
    let deauthentication = management(12, AP, AP, &[3, 0]);
    assert_eq!(
        engine
            .receive(
                &deauthentication,
                Integrity::Unverified,
                at(4),
                Some([0; 32]),
                0,
                &mut output
            )
            .unwrap(),
        ApReceived::PeerLeft { peer: PEER }
    );
    assert!(engine.service().peer_status(PEER).is_none());
}
