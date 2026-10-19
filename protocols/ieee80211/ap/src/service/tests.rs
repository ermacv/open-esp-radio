const TEST_HT_CAPABILITIES: oer_ieee80211_mac::ht::HtLocalCapabilities =
    oer_ieee80211_mac::ht::HtLocalCapabilities::new(0x100c, 0x03, 0xff, 0x01);

mod storage;
mod transmit;

use super::*;
use oer_ieee80211_mac::{
    channel::WifiChannel,
    ht::{ht_capability_ie, ht_peer_capabilities},
    security::AP_WPA2_PERSONAL_RSN_ELEMENT,
};
use oer_ieee80211_rsn::{
    EapolKeyMessage, OwnedEapolFrame, PtkContext, RsnInterface,
    aes::software_aes128_key_unwrap,
    frames::{OwnedAssociationSecurityIes, OwnedRsnIe, RsnGtk, RsnTxFrame, parse_gtk_key_data},
    state::{RsnApAction, RsnTicket},
};

const AP: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
const PEER: [u8; 6] = [0x02, 0, 0, 0, 0, 2];
const OTHER: [u8; 6] = [0x02, 0, 0, 0, 0, 3];
const WPA2_RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];
const SUPPLICANT_RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0x0c, 0,
];

fn association_security<'a>(rsn_ie: &'a [u8]) -> ApAssociationSecurityObservation<'a> {
    association_security_with_rsnxe(rsn_ie, None)
}

fn association_security_with_rsnxe<'a>(
    rsn_ie: &'a [u8],
    rsnxe: Option<&'a [u8]>,
) -> ApAssociationSecurityObservation<'a> {
    ApAssociationSecurityObservation {
        privacy: true,
        rsn_ie: Some(rsn_ie),
        rsn_ie_count: 1,
        rsnxe,
        rsnxe_count: u8::from(rsnxe.is_some()),
        legacy_wpa_present: false,
        malformed_elements: false,
    }
}

fn signed_message2(
    rsn_ie: &[u8],
    rsnxe: &[u8],
    authenticator_nonce: [u8; 32],
    supplicant_nonce: [u8; 32],
) -> OwnedEapolFrame<512> {
    let ptk = Pmk::derive(b"password", b"test-ap").unwrap().derive_ptk(
        oer_ieee80211_mac::security::rsn::Akm::Psk,
        PtkContext {
            authenticator_address: AP,
            supplicant_address: PEER,
            authenticator_nonce,
            supplicant_nonce,
        },
    );
    let rsn_ie = OwnedRsnIe::<64>::try_copy(rsn_ie).unwrap();
    let security_ies = OwnedAssociationSecurityIes::<128>::try_copy(&rsn_ie, rsnxe).unwrap();
    let message2 = RsnTxFrame::<512>::message2_with_security_ies(
        oer_ieee80211_mac::security::rsn::Akm::Psk,
        AP,
        9,
        supplicant_nonce,
        &security_ies,
    )
    .unwrap()
    .authenticate(&ptk);
    OwnedEapolFrame::try_copy(RsnInterface::AccessPoint, PEER, message2.as_bytes()).unwrap()
}

fn corrupt_mic(frame: OwnedEapolFrame<512>) -> OwnedEapolFrame<512> {
    let mut bytes = [0_u8; 512];
    let length = frame.as_bytes().len();
    bytes[..length].copy_from_slice(frame.as_bytes());
    bytes[81] ^= 1;
    OwnedEapolFrame::try_copy(RsnInterface::AccessPoint, PEER, &bytes[..length]).unwrap()
}

fn ht_capabilities() -> ApAssociationCapabilities {
    ApAssociationCapabilities {
        maximum_legacy_rate_500kbps: 108,
        short_preamble: true,
        ht: ht_peer_capabilities(&ht_capability_ie(
            TEST_HT_CAPABILITIES,
            oer_ieee80211_mac::channel::Channel::from_wifi_channel(WifiChannel::mhz20(6).unwrap()),
        )),
        qos_supported: true,
    }
}

const LEGACY_CAPABILITIES: ApAssociationCapabilities = ApAssociationCapabilities {
    maximum_legacy_rate_500kbps: 108,
    short_preamble: true,
    ht: None,
    qos_supported: false,
};

fn service(
    storage: &mut AccessPointPeerStorage,
) -> AccessPointService<'_, oer_ieee80211_mac::security::spp::Association> {
    AccessPointService::<oer_ieee80211_mac::security::spp::Association>::new(
        AP,
        Pmk::derive(b"password", b"test-ap").unwrap(),
        RsnGtk::new(
            oer_ieee80211_mac::ccmp::GroupKeyId::new(1).unwrap(),
            true,
            &[0x55; 16],
        ),
        AccessPointClientLimit::new(2).unwrap(),
        AccessPointInactiveTimeout::default(),
        storage.lend(),
    )
}

#[test]
fn runtime_limit_is_enforced_before_hardware_moves() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    assert_eq!(
        service.authenticate_open(PEER, oer_time::Instant::from_micros(0)),
        ApMlmeAction::AuthenticationResponse {
            peer: PEER,
            status: AP_STATUS_SUCCESS,
        }
    );
    assert_eq!(
        service.authenticate_open(OTHER, oer_time::Instant::from_micros(0)),
        ApMlmeAction::AuthenticationResponse {
            peer: OTHER,
            status: AP_STATUS_SUCCESS,
        }
    );
    let third = [0x02, 0, 0, 0, 0, 4];
    assert_eq!(
        service.authenticate_open(third, oer_time::Instant::from_micros(0)),
        ApMlmeAction::AuthenticationResponse {
            peer: third,
            status: AP_STATUS_TOO_MANY_STATIONS,
        }
    );
    assert_eq!(service.associated_count(), 0);
    assert_eq!(service.peers().count(), 2);
    assert_eq!(service.peer_status(PEER).unwrap().association_id, 1);
    assert_eq!(service.peer_status(OTHER).unwrap().association_id, 2);
}

#[test]
fn qos_sequence_spaces_are_independent_for_each_peer_and_tid() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    service.authenticate_open(PEER, oer_time::Instant::from_micros(0));
    service.authenticate_open(OTHER, oer_time::Instant::from_micros(0));

    assert_eq!(
        service.next_qos_sequence(PEER, 0),
        Some(SequenceNumber::new(0).unwrap())
    );
    assert_eq!(
        service.next_qos_sequence(PEER, 0),
        Some(SequenceNumber::new(1).unwrap())
    );
    assert_eq!(
        service.current_qos_sequence(PEER, 0),
        Some(SequenceNumber::new(2).unwrap())
    );
    assert_eq!(
        service.current_qos_sequence(OTHER, 0),
        Some(SequenceNumber::new(0).unwrap())
    );
    assert_eq!(
        service.next_qos_sequence(OTHER, 0),
        Some(SequenceNumber::new(0).unwrap())
    );
    assert_eq!(
        service.current_qos_sequence(PEER, 1),
        Some(SequenceNumber::new(0).unwrap())
    );
    assert_eq!(service.next_qos_sequence([0xff; 6], 0), None);
}

#[test]
fn peer_binding_rejects_a_reused_slot_generation() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    service.authenticate_open(PEER, oer_time::Instant::from_micros(0));
    let first = service.bind_peer(PEER).unwrap();
    assert_eq!(service.bound_peer_status(first).unwrap().address, PEER);

    service.remove_peer(PEER).unwrap();
    service.authenticate_open(OTHER, oer_time::Instant::from_micros(1));
    let second = service.bind_peer(OTHER).unwrap();
    assert_eq!(service.bound_peer_status(first), None);
    assert_eq!(service.bound_peer_status(second).unwrap().address, OTHER);
    assert_ne!(first, second);
}

#[test]
fn buffered_downlink_identity_cannot_cross_same_address_reassociation() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = AccessPointService::<oer_ieee80211_mac::security::spp::Association>::new_open(
        AP,
        AccessPointClientLimit::new(1).unwrap(),
        AccessPointInactiveTimeout::default(),
        storage.lend(),
    );
    let open_security = ApAssociationSecurityObservation {
        privacy: false,
        rsn_ie: None,
        rsn_ie_count: 0,
        rsnxe: None,
        rsnxe_count: 0,
        legacy_wpa_present: false,
        malformed_elements: false,
    };

    service.authenticate_open(PEER, oer_time::Instant::from_micros(0));
    service
        .associate_open(
            PEER,
            open_security,
            LEGACY_CAPABILITIES,
            oer_time::Instant::from_micros(1),
        )
        .unwrap();
    service
        .observe_power_save(
            ApPowerSaveObservation::Sleeping { peer: PEER },
            oer_time::Instant::from_micros(2),
        )
        .unwrap();
    let first = service.admit_downlink(PEER).unwrap();
    assert_eq!(first.disposition(), ApDownlinkDisposition::Buffer);
    service.commit_buffered_unicast(first.identity()).unwrap();
    let release = service
        .begin_buffered_unicast_release(first.identity())
        .unwrap()
        .unwrap();

    service.remove_peer(PEER).unwrap();
    service.authenticate_open(PEER, oer_time::Instant::from_micros(3));
    service
        .associate_open(
            PEER,
            open_security,
            LEGACY_CAPABILITIES,
            oer_time::Instant::from_micros(4),
        )
        .unwrap();
    let second = service.admit_downlink(PEER).unwrap();
    assert_eq!(
        first.identity().association_id(),
        second.identity().association_id()
    );
    assert_ne!(
        first.identity().association_epoch(),
        second.identity().association_epoch()
    );
    assert_eq!(
        service.bound_authorized_peer_status(first.identity()),
        None,
        "an old queue owner must not bind to the reused AID and MAC"
    );
    assert_eq!(
        service.complete_buffered_unicast_release(release, false),
        Err(ApServiceError::AssociationIdMismatch),
        "an affine release from the old epoch must fail closed"
    );
    assert_eq!(
        service.commit_buffered_unicast(first.identity()),
        Err(ApServiceError::UnknownPeer),
    );
}

#[test]
fn bound_power_state_matches_general_semantics_and_rejects_slot_reuse() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = AccessPointService::<oer_ieee80211_mac::security::spp::Association>::new_open(
        AP,
        AccessPointClientLimit::new(2).unwrap(),
        AccessPointInactiveTimeout::new(10).unwrap(),
        storage.lend(),
    );
    service.authenticate_open(PEER, oer_time::Instant::from_micros(0));
    service
        .associate_open(
            PEER,
            ApAssociationSecurityObservation {
                privacy: false,
                rsn_ie: None,
                rsn_ie_count: 0,
                rsnxe: None,
                rsnxe_count: 0,
                legacy_wpa_present: false,
                malformed_elements: false,
            },
            LEGACY_CAPABILITIES,
            oer_time::Instant::from_micros(1_000),
        )
        .unwrap();
    let binding = service.bind_peer(PEER).unwrap();
    let initial_revision = service.status_revision();

    assert_eq!(
        service
            .observe_bound_power_state(
                binding,
                ApPeerPowerState::Active,
                oer_time::Instant::from_micros(2_000)
            )
            .unwrap(),
        ApPowerSaveAction::None,
    );
    assert_eq!(service.status_revision(), initial_revision);
    assert_eq!(
        service.peer_status(PEER).unwrap().deadline,
        oer_time::Instant::from_micros(10_002_000)
    );

    assert_eq!(
        service
            .observe_bound_power_state(
                binding,
                ApPeerPowerState::Sleeping,
                oer_time::Instant::from_micros(3_000)
            )
            .unwrap(),
        ApPowerSaveAction::StateChanged {
            peer: PEER,
            state: ApPeerPowerState::Sleeping,
            buffered_frames: 0,
        },
    );
    assert_eq!(service.status_revision(), initial_revision.wrapping_add(1));
    assert_eq!(
        service.peer_status(PEER).unwrap().deadline,
        oer_time::Instant::from_micros(10_003_000)
    );

    service.remove_peer(PEER).unwrap();
    service.authenticate_open(OTHER, oer_time::Instant::from_micros(4_000));
    assert_eq!(
        service.observe_bound_power_state(
            binding,
            ApPeerPowerState::Active,
            oer_time::Instant::from_micros(5_000)
        ),
        Err(ApServiceError::UnknownPeer),
        "a recycled table slot cannot inherit the old peer's PM update"
    );
}

#[test]
fn admitted_data_activity_is_coalesced_but_pm_edges_are_immediate() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = AccessPointService::<oer_ieee80211_mac::security::spp::Association>::new_open(
        AP,
        AccessPointClientLimit::new(2).unwrap(),
        AccessPointInactiveTimeout::new(10).unwrap(),
        storage.lend(),
    );
    service.authenticate_open(PEER, oer_time::Instant::from_micros(0));
    service
        .associate_open(
            PEER,
            ApAssociationSecurityObservation {
                privacy: false,
                rsn_ie: None,
                rsn_ie_count: 0,
                rsnxe: None,
                rsnxe_count: 0,
                legacy_wpa_present: false,
                malformed_elements: false,
            },
            LEGACY_CAPABILITIES,
            oer_time::Instant::from_micros(1_000),
        )
        .unwrap();
    let binding = service.bind_peer(PEER).unwrap();
    let associated = service.peer_status(PEER).unwrap();

    assert_eq!(
        service
            .observe_bound_data_power_state(
                binding,
                ApPeerPowerState::Active,
                oer_time::Instant::from_micros(2_000)
            )
            .unwrap(),
        ApPowerSaveAction::None,
    );
    assert_eq!(
        service.peer_status(PEER).unwrap().deadline,
        associated.deadline,
        "an unchanged data PM state must not rewrite the peer on every MPDU"
    );

    service
        .observe_bound_data_activity(binding, oer_time::Instant::from_micros(5_001_000))
        .unwrap();
    assert_eq!(
        service.peer_status(PEER).unwrap().deadline,
        oer_time::Instant::from_micros(15_001_000),
        "the half-timeout guard refreshes before expiry"
    );

    let revision = service.status_revision();
    assert_eq!(
        service
            .observe_bound_data_power_state(
                binding,
                ApPeerPowerState::Sleeping,
                oer_time::Instant::from_micros(5_002_000)
            )
            .unwrap(),
        ApPowerSaveAction::StateChanged {
            peer: PEER,
            state: ApPeerPowerState::Sleeping,
            buffered_frames: 0,
        },
    );
    assert_eq!(service.status_revision(), revision.wrapping_add(1));
    assert_eq!(
        service.peer_status(PEER).unwrap().deadline,
        oer_time::Instant::from_micros(15_002_000),
        "a PM transition is never delayed by activity coalescing"
    );

    service.remove_peer(PEER).unwrap();
    service.authenticate_open(OTHER, oer_time::Instant::from_micros(6_000_000));
    assert_eq!(
        service.observe_bound_data_activity(binding, oer_time::Instant::from_micros(6_001_000)),
        Err(ApServiceError::UnknownPeer),
        "coalescing must not weaken the slot-generation fence"
    );
}

#[test]
fn inactivity_timeout_is_bounded_and_defaults_to_vendor_policy() {
    assert_eq!(AccessPointInactiveTimeout::default().seconds(), 300);
    assert_eq!(AccessPointInactiveTimeout::new(9).unwrap_err().seconds(), 9);
    assert_eq!(AccessPointInactiveTimeout::new(10).unwrap().seconds(), 10);
    assert_eq!(
        AccessPointInactiveTimeout::new(3_600).unwrap().seconds(),
        3_600
    );
    assert_eq!(
        AccessPointInactiveTimeout::new(3_601)
            .unwrap_err()
            .seconds(),
        3_601
    );
}

#[test]
fn authenticated_peer_expires_at_the_recovered_fifteen_second_frontier() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    service.authenticate_open(PEER, oer_time::Instant::from_micros(1_000));
    assert_eq!(
        service.next_peer_deadline(),
        Some(oer_time::Instant::from_micros(15_001_000))
    );
    assert_eq!(
        service.begin_due_peer_close(oer_time::Instant::from_micros(15_000_999)),
        None
    );
    assert_eq!(
        service.begin_due_peer_close(oer_time::Instant::from_micros(15_001_000)),
        Some(ApPeerClose {
            peer: PEER,
            kind: ApPeerCloseKind::AuthenticationTimeout,
            was_associated: false,
            maximum_legacy_rate_500kbps: 2,
        })
    );
    assert_eq!(
        service.peer_status(PEER).unwrap().phase,
        ApPeerPhase::Closing
    );
    assert_eq!(service.associated_count(), 0);
}

#[test]
fn associated_activity_refreshes_the_configured_inactivity_frontier() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = AccessPointService::<oer_ieee80211_mac::security::spp::Association>::new(
        AP,
        Pmk::derive(b"password", b"test-ap").unwrap(),
        RsnGtk::new(
            oer_ieee80211_mac::ccmp::GroupKeyId::new(1).unwrap(),
            true,
            &[0x55; 16],
        ),
        AccessPointClientLimit::new(2).unwrap(),
        AccessPointInactiveTimeout::new(10).unwrap(),
        storage.lend(),
    );
    service.authenticate_open(PEER, oer_time::Instant::from_micros(0));
    service
        .associate_rsn(
            PEER,
            association_security(&WPA2_RSN),
            ht_capabilities(),
            [7; 32],
            9,
            oer_time::Instant::from_micros(2_000),
        )
        .unwrap();
    assert_eq!(
        service.next_peer_deadline(),
        Some(oer_time::Instant::from_micros(10_002_000))
    );
    let binding = service.bind_peer(PEER).expect("associated peer binding");
    service
        .observe_bound_activity(binding, oer_time::Instant::from_micros(5_000_000))
        .unwrap();
    assert_eq!(
        service.next_peer_deadline(),
        Some(oer_time::Instant::from_micros(15_000_000))
    );
    assert_eq!(
        service.begin_due_peer_close(oer_time::Instant::from_micros(14_999_999)),
        None
    );
    assert_eq!(
        service.begin_due_peer_close(oer_time::Instant::from_micros(15_000_000)),
        Some(ApPeerClose {
            peer: PEER,
            kind: ApPeerCloseKind::InactivityTimeout,
            was_associated: true,
            maximum_legacy_rate_500kbps: 108,
        })
    );
    assert_eq!(service.associated_count(), 0);
}

#[test]
fn association_owns_a_bounded_wpa2_state() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    service.authenticate_open(PEER, oer_time::Instant::from_micros(0));
    assert_eq!(
        service
            .associate_rsn(
                PEER,
                association_security(&WPA2_RSN),
                ht_capabilities(),
                [7; 32],
                9,
                oer_time::Instant::from_micros(1),
            )
            .unwrap(),
        ApMlmeAction::AssociationResponse {
            peer: PEER,
            status: AP_STATUS_SUCCESS,
            association_id: Some(1),
        }
    );
    assert_eq!(
        service.peer_status(PEER).unwrap().phase,
        ApPeerPhase::Securing
    );
    assert_eq!(
        service
            .peer_status(PEER)
            .unwrap()
            .maximum_legacy_rate_500kbps,
        108
    );
    assert_eq!(service.associated_count(), 1);
    assert_eq!(
        service.begin_wpa2(PEER).unwrap(),
        ApMlmeAction::BeginWpa2 { peer: PEER }
    );
    assert!(matches!(
        service.wpa2_mut(PEER).unwrap().message1(false).unwrap(),
        RsnApAction::Transmit(_)
    ));
    let _ticket_type_is_owned: Option<RsnTicket> = None;
}

#[test]
fn association_rejects_a_peer_without_a_common_legacy_rate() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    service.authenticate_open(PEER, oer_time::Instant::from_micros(0));
    assert_eq!(
        service
            .associate_rsn(
                PEER,
                association_security(&WPA2_RSN),
                ApAssociationCapabilities {
                    maximum_legacy_rate_500kbps: 0,
                    short_preamble: true,
                    ht: None,
                    qos_supported: false,
                },
                [7; 32],
                9,
                oer_time::Instant::from_micros(1),
            )
            .unwrap(),
        ApMlmeAction::AssociationResponse {
            peer: PEER,
            status: AP_STATUS_UNSUPPORTED_RATES,
            association_id: None,
        }
    );
    assert_eq!(
        service.peer_status(PEER).unwrap().phase,
        ApPeerPhase::Authenticated
    );
}

#[test]
fn complete_four_way_handshake_retains_ptk_until_hardware_authorization() {
    const ANONCE: [u8; 32] = [7; 32];
    const SNONCE: [u8; 32] = [8; 32];

    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    service.authenticate_open(PEER, oer_time::Instant::from_micros(0));
    // Supplicants may add their own RSN capabilities. Message 3 must not
    // reflect those bytes back: it authenticates the AP's beacon RSN IE.
    service
        .associate_rsn(
            PEER,
            association_security(&SUPPLICANT_RSN),
            ht_capabilities(),
            ANONCE,
            9,
            oer_time::Instant::from_micros(1),
        )
        .unwrap();
    let (message1, message1_context) = service.begin_wpa2_frame::<512>(PEER).unwrap().into_parts();
    assert_eq!(
        message1.key_frame().message(),
        EapolKeyMessage::PairwiseMessage1
    );
    assert!(!message1.retransmission());
    service
        .observe_wpa2_transmit(message1_context, true, oer_time::Instant::from_micros(10))
        .unwrap();
    assert_eq!(
        service.next_wpa2_retry_deadline(),
        Some(oer_time::Instant::from_micros(1_000_010))
    );
    let ApWpa2RetryProgress::Transmit(retried_message1) = service
        .take_due_wpa2_retry::<512>(oer_time::Instant::from_micros(1_000_010))
        .unwrap()
    else {
        panic!("Message 1 response timeout must retransmit")
    };
    assert_eq!(*retried_message1.frame().peer(), PEER);
    assert!(retried_message1.frame().retransmission());
    assert_eq!(retried_message1.frame().as_bytes(), message1.as_bytes());

    let ptk = Pmk::derive(b"password", b"test-ap").unwrap().derive_ptk(
        oer_ieee80211_mac::security::rsn::Akm::Psk,
        PtkContext {
            authenticator_address: AP,
            supplicant_address: PEER,
            authenticator_nonce: ANONCE,
            supplicant_nonce: SNONCE,
        },
    );
    let rsn = OwnedRsnIe::<64>::try_copy(&SUPPLICANT_RSN).unwrap();
    let message2 = RsnTxFrame::<512>::message2(
        oer_ieee80211_mac::security::rsn::Akm::Psk,
        AP,
        9,
        SNONCE,
        &rsn,
    )
    .unwrap()
    .authenticate(&ptk);
    let message2 =
        OwnedEapolFrame::<512>::try_copy(RsnInterface::AccessPoint, PEER, message2.as_bytes())
            .unwrap();
    let ApWpa2Progress::Transmit(message3) = service.on_eapol(PEER, message2).unwrap() else {
        panic!("message 2 must produce message 3");
    };
    let (message3, message3_context) = message3.into_parts();
    assert_eq!(
        message3.key_frame().message(),
        EapolKeyMessage::PairwiseMessage3
    );
    assert!(message3.key_frame().verify_mic(&ptk));
    let plaintext = software_aes128_key_unwrap(ptk.kek(), message3.key_frame().key_data())
        .expect("AP wrapped its Message 3 key data");
    assert!(
        parse_gtk_key_data(
            plaintext.as_bytes(),
            &AP_WPA2_PERSONAL_RSN_ELEMENT,
            &[],
            false
        )
        .is_ok()
    );
    assert_eq!(service.next_wpa2_retry_deadline(), None);
    service
        .observe_wpa2_transmit(
            message3_context,
            true,
            oer_time::Instant::from_micros(2_000_000),
        )
        .unwrap();
    assert_eq!(
        service.next_wpa2_retry_deadline(),
        Some(oer_time::Instant::from_micros(2_100_000))
    );
    let ApWpa2RetryProgress::Transmit(retried_message3) = service
        .take_due_wpa2_retry::<512>(oer_time::Instant::from_micros(2_100_000))
        .unwrap()
    else {
        panic!("Message 3 response timeout must retransmit")
    };
    assert_eq!(*retried_message3.frame().peer(), PEER);
    assert!(retried_message3.frame().retransmission());
    assert_eq!(retried_message3.frame().as_bytes(), message3.as_bytes());
    assert!(matches!(
        parse_gtk_key_data(plaintext.as_bytes(), &SUPPLICANT_RSN, &[], false),
        Err(RsnFrameError::RsnIeMismatch)
    ));

    let message4 = RsnTxFrame::<512>::message4(oer_ieee80211_mac::security::rsn::Akm::Psk, AP, 10)
        .unwrap()
        .authenticate(&ptk);
    let message4 =
        OwnedEapolFrame::<512>::try_copy(RsnInterface::AccessPoint, PEER, message4.as_bytes())
            .unwrap();
    assert!(matches!(
        service.on_eapol(PEER, message4).unwrap(),
        ApWpa2Progress::AuthorizePeer
    ));
    assert_eq!(service.next_wpa2_retry_deadline(), None);
    assert!(service.pending_ptk(PEER).is_ok());
    service
        .authorize(PEER, oer_time::Instant::from_micros(2))
        .unwrap();
    assert_eq!(
        service.peer_status(PEER).unwrap().phase,
        ApPeerPhase::Authorized
    );
    assert_eq!(
        service.pending_ptk(PEER).err(),
        Some(ApServiceError::WrongPeerPhase)
    );
}

#[test]
fn message2_must_echo_the_exact_association_rsn() {
    const ANONCE: [u8; 32] = [7; 32];
    const SNONCE: [u8; 32] = [8; 32];

    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    service.authenticate_open(PEER, oer_time::Instant::from_micros(0));
    service
        .associate_rsn(
            PEER,
            association_security(&SUPPLICANT_RSN),
            ht_capabilities(),
            ANONCE,
            9,
            oer_time::Instant::from_micros(1),
        )
        .unwrap();

    assert!(matches!(
        service
            .on_eapol(PEER, signed_message2(&WPA2_RSN, &[], ANONCE, SNONCE))
            .unwrap(),
        ApWpa2Progress::DeauthenticatePeer
    ));
    assert_eq!(service.wpa2_mut(PEER).unwrap().phase(), RsnApPhase::Failed);
    assert!(service.pending_ptk(PEER).is_err());
}

#[test]
fn unauthenticated_eapol_cannot_poison_or_refresh_a_securing_peer() {
    const ANONCE: [u8; 32] = [7; 32];
    const SNONCE: [u8; 32] = [8; 32];

    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    service.authenticate_open(PEER, oer_time::Instant::from_micros(0));
    service
        .associate_rsn(
            PEER,
            association_security(&SUPPLICANT_RSN),
            ht_capabilities(),
            ANONCE,
            9,
            oer_time::Instant::from_micros(1),
        )
        .unwrap();
    let original_deadline = service.peer_status(PEER).unwrap().deadline;

    let replay_mismatch =
        RsnTxFrame::<512>::message4(oer_ieee80211_mac::security::rsn::Akm::Psk, AP, 77).unwrap();
    let replay_mismatch = OwnedEapolFrame::<512>::try_copy(
        RsnInterface::AccessPoint,
        PEER,
        replay_mismatch.as_bytes(),
    )
    .unwrap();
    assert!(matches!(
        service.on_eapol(PEER, replay_mismatch).unwrap(),
        ApWpa2Progress::None
    ));

    let unsupported =
        RsnTxFrame::<512>::message1(oer_ieee80211_mac::security::rsn::Akm::Psk, AP, 9, ANONCE)
            .unwrap();
    let unsupported =
        OwnedEapolFrame::<512>::try_copy(RsnInterface::AccessPoint, PEER, unsupported.as_bytes())
            .unwrap();
    assert!(matches!(
        service.on_eapol(PEER, unsupported).unwrap(),
        ApWpa2Progress::None
    ));

    // The attacker also supplies mismatched association Key Data. The
    // mismatch is not actionable because this candidate's MIC is bad.
    let forged_m2 = corrupt_mic(signed_message2(&WPA2_RSN, &[], ANONCE, SNONCE));
    assert!(matches!(
        service.on_eapol(PEER, forged_m2).unwrap(),
        ApWpa2Progress::None
    ));
    assert_eq!(
        service.wpa2_mut(PEER).unwrap().phase(),
        RsnApPhase::AwaitingMessage2
    );
    assert!(service.pending_ptk(PEER).is_err());
    assert_eq!(
        service.peer_status(PEER).unwrap().deadline,
        original_deadline,
        "ignored EAPOL must not extend peer liveness"
    );

    let valid_m2 = signed_message2(&SUPPLICANT_RSN, &[], ANONCE, SNONCE);
    assert!(matches!(
        service.on_eapol(PEER, valid_m2).unwrap(),
        ApWpa2Progress::Transmit(_)
    ));
    assert_eq!(
        service.wpa2_mut(PEER).unwrap().phase(),
        RsnApPhase::AwaitingMessage4
    );

    // Neither forged nor even MIC-valid duplicate M2 directly elicits a
    // fresh M3. The finite authenticator retry timer owns retransmission.
    let duplicate_m2 = signed_message2(&SUPPLICANT_RSN, &[], ANONCE, SNONCE);
    assert!(matches!(
        service.on_eapol(PEER, duplicate_m2).unwrap(),
        ApWpa2Progress::None
    ));

    let ptk = Pmk::derive(b"password", b"test-ap").unwrap().derive_ptk(
        oer_ieee80211_mac::security::rsn::Akm::Psk,
        PtkContext {
            authenticator_address: AP,
            supplicant_address: PEER,
            authenticator_nonce: ANONCE,
            supplicant_nonce: SNONCE,
        },
    );
    let valid_m4 = RsnTxFrame::<512>::message4(oer_ieee80211_mac::security::rsn::Akm::Psk, AP, 10)
        .unwrap()
        .authenticate(&ptk);
    let valid_m4 =
        OwnedEapolFrame::try_copy(RsnInterface::AccessPoint, PEER, valid_m4.as_bytes()).unwrap();
    let forged_m4 = corrupt_mic(valid_m4.clone());
    assert!(matches!(
        service.on_eapol(PEER, forged_m4).unwrap(),
        ApWpa2Progress::None
    ));
    assert_eq!(
        service.wpa2_mut(PEER).unwrap().phase(),
        RsnApPhase::AwaitingMessage4
    );
    assert!(matches!(
        service.on_eapol(PEER, valid_m4).unwrap(),
        ApWpa2Progress::AuthorizePeer
    ));
}

#[test]
fn message2_must_echo_the_exact_association_rsnxe() {
    const ANONCE: [u8; 32] = [7; 32];
    const SNONCE: [u8; 32] = [8; 32];
    const RSNXE: [u8; 3] = [0xf4, 1, 0x20];

    let mut rejected_storage = AccessPointPeerStorage::new();
    let mut rejected = service(&mut rejected_storage);
    rejected.authenticate_open(PEER, oer_time::Instant::from_micros(0));
    rejected
        .associate_rsn(
            PEER,
            association_security_with_rsnxe(&SUPPLICANT_RSN, Some(&RSNXE)),
            ht_capabilities(),
            ANONCE,
            9,
            oer_time::Instant::from_micros(1),
        )
        .unwrap();
    assert!(matches!(
        rejected
            .on_eapol(PEER, signed_message2(&SUPPLICANT_RSN, &[], ANONCE, SNONCE),)
            .unwrap(),
        ApWpa2Progress::DeauthenticatePeer
    ));

    let mut accepted_storage = AccessPointPeerStorage::new();
    let mut accepted = service(&mut accepted_storage);
    accepted.authenticate_open(PEER, oer_time::Instant::from_micros(0));
    accepted
        .associate_rsn(
            PEER,
            association_security_with_rsnxe(&SUPPLICANT_RSN, Some(&RSNXE)),
            ht_capabilities(),
            ANONCE,
            9,
            oer_time::Instant::from_micros(1),
        )
        .unwrap();
    assert!(matches!(
        accepted
            .on_eapol(
                PEER,
                signed_message2(&SUPPLICANT_RSN, &RSNXE, ANONCE, SNONCE),
            )
            .unwrap(),
        ApWpa2Progress::Transmit(_)
    ));
}

#[test]
fn exhausted_pairwise_update_count_closes_the_peer() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    service.authenticate_open(PEER, oer_time::Instant::from_micros(0));
    service
        .associate_rsn(
            PEER,
            association_security(&WPA2_RSN),
            ht_capabilities(),
            [7; 32],
            9,
            oer_time::Instant::from_micros(1),
        )
        .unwrap();
    let (_message1, context) = service.begin_wpa2_frame::<512>(PEER).unwrap().into_parts();
    service
        .observe_wpa2_transmit(context, true, oer_time::Instant::from_micros(0))
        .unwrap();

    for deadline in [1_000_000, 2_000_000, 3_000_000] {
        assert!(matches!(
            service
                .take_due_wpa2_retry::<512>(oer_time::Instant::from_micros(deadline))
                .unwrap(),
            ApWpa2RetryProgress::Transmit(frame) if *frame.frame().peer() == PEER
        ));
    }
    assert!(matches!(
        service
            .take_due_wpa2_retry::<512>(oer_time::Instant::from_micros(4_000_000))
            .unwrap(),
        ApWpa2RetryProgress::Close(ApPeerClose {
            peer: PEER,
            kind: ApPeerCloseKind::Wpa2HandshakeTimeout,
            was_associated: true,
            ..
        })
    ));
    assert_eq!(
        service.peer_status(PEER).unwrap().phase,
        ApPeerPhase::Closing
    );
}

#[test]
fn invalid_rsn_does_not_open_the_controlled_port() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    service.authenticate_open(PEER, oer_time::Instant::from_micros(0));
    assert_eq!(
        service.associate_rsn(
            PEER,
            association_security(&[0x30, 0]),
            LEGACY_CAPABILITIES,
            [7; 32],
            9,
            oer_time::Instant::from_micros(1),
        ),
        Ok(ApMlmeAction::AssociationResponse {
            peer: PEER,
            status: AP_STATUS_INVALID_RSN,
            association_id: None,
        })
    );
    assert_eq!(
        service.peer_status(PEER).unwrap().phase,
        ApPeerPhase::Authenticated
    );
}

#[test]
fn management_sequence_wraps_at_twelve_bits() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    for expected in 0..=0x0fff {
        assert_eq!(
            service.next_management_sequence(),
            SequenceNumber::new(expected).unwrap()
        );
    }
    assert_eq!(
        service.next_management_sequence(),
        SequenceNumber::new(0).unwrap()
    );
}

#[test]
fn all_fifteen_aids_are_stable_and_reused_after_removal() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = AccessPointService::<oer_ieee80211_mac::security::spp::Association>::new(
        AP,
        Pmk::derive(b"password", b"test-ap").unwrap(),
        RsnGtk::new(
            oer_ieee80211_mac::ccmp::GroupKeyId::new(1).unwrap(),
            true,
            &[0x55; 16],
        ),
        AccessPointClientLimit::new(15).unwrap(),
        AccessPointInactiveTimeout::default(),
        storage.lend(),
    );
    for suffix in 1..=15_u8 {
        let peer = [0x02, 0, 0, 0, 1, suffix];
        assert_eq!(
            service.authenticate_open(peer, oer_time::Instant::from_micros(0)),
            ApMlmeAction::AuthenticationResponse {
                peer,
                status: AP_STATUS_SUCCESS,
            }
        );
        assert_eq!(
            service.peer_status(peer).unwrap().association_id,
            u16::from(suffix)
        );
        assert!(matches!(
            service
                .associate_rsn(
                    peer,
                    association_security(&WPA2_RSN),
                    ht_capabilities(),
                    [suffix; 32],
                    u64::from(suffix),
                    oer_time::Instant::from_micros(1),
                )
                .unwrap(),
            ApMlmeAction::AssociationResponse {
                status: AP_STATUS_SUCCESS,
                association_id: Some(_),
                ..
            }
        ));
    }
    assert_eq!(service.associated_count(), 15);
    let overflow = [0x02, 0, 0, 0, 2, 1];
    assert_eq!(
        service.authenticate_open(overflow, oer_time::Instant::from_micros(0)),
        ApMlmeAction::AuthenticationResponse {
            peer: overflow,
            status: AP_STATUS_TOO_MANY_STATIONS,
        }
    );
    let released = [0x02, 0, 0, 0, 1, 7];
    service.remove_peer(released).unwrap();
    assert_eq!(
        service.authenticate_open(overflow, oer_time::Instant::from_micros(0)),
        ApMlmeAction::AuthenticationResponse {
            peer: overflow,
            status: AP_STATUS_SUCCESS,
        }
    );
    assert_eq!(service.peer_status(overflow).unwrap().association_id, 7);
}

#[test]
fn bounded_peer_table_has_an_explicit_memory_ceiling() {
    // The service itself travels through the typed lifecycle, while all
    // fifteen WPA2 state machines remain in caller-owned static storage.
    assert!(
        core::mem::size_of::<AccessPointService<'_, oer_ieee80211_mac::security::spp::Association>>(
        ) <= 256
    );
    // Fifteen independently negotiated TX BlockAck sessions and the
    // per-peer HMAC-SHA1-128 Association-security commitments, exact
    // non-reusable RX association epochs, and eight independent QoS
    // sequence spaces remain explicit. The 16-byte sequence array is
    // required per receiver: sharing it across clients creates artificial
    // BlockAck holes. A WPA3 station keeps its own PMK and PMKID, and the
    // storage holds the ten-entry PMKSA cache. The bounded table still uses
    // no dynamic allocation. Each peer's WPA2 retry keeps its two intervals
    // as `oer_time::Duration` (8 bytes more per peer than microsecond `u32`s).
    // Each peer's TX Block Ack originator keeps, beside its session, its
    // negotiation's deadline, the next attempt's due time, its attempts,
    // retry interval and Dialog Token policy: the per-peer price of offers
    // that are retried. Management protection adds one eight-byte activation
    // and replay word plus six Message-3 IPN snapshot bytes per slot.
    // The arrays add 216 bytes including host alignment padding.
    assert!(
        core::mem::size_of::<AccessPointPeerStorage>() <= 7_616,
        "peer storage size {}",
        core::mem::size_of::<AccessPointPeerStorage>()
    );
}

#[test]
fn tx_block_ack_is_owned_by_the_exact_authorized_ht_peer() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    service.authenticate_open(PEER, oer_time::Instant::from_micros(1));
    service
        .associate_rsn(
            PEER,
            association_security(&WPA2_RSN),
            ht_capabilities(),
            [7; 32],
            9,
            oer_time::Instant::from_micros(1),
        )
        .unwrap();
    service.checked_peer_mut(PEER).unwrap().phase = ApPeerPhase::Authorized;

    service.authenticate_open(OTHER, oer_time::Instant::from_micros(1));
    service
        .associate_rsn(
            OTHER,
            association_security(&WPA2_RSN),
            LEGACY_CAPABILITIES,
            [8; 32],
            10,
            oer_time::Instant::from_micros(1),
        )
        .unwrap();
    service.checked_peer_mut(OTHER).unwrap().phase = ApPeerPhase::Authorized;

    let request = offer(&mut service, PEER, 100).unwrap();
    assert_eq!(
        u16::from_le_bytes([request.body[3], request.body[4]]) & 1,
        1,
        "AP requests only the source-owned baseline A-MSDU class"
    );
    assert_eq!(service.operational_tx_block_ack_window(PEER), None);
    assert!(!service.has_operational_tx_block_ack());
    // A live negotiation takes no second offer; a legacy peer gets none.
    assert!(offer(&mut service, PEER, 101).is_none());
    assert!(offer(&mut service, OTHER, 101).is_none());
    let response = BlockAckAction::AddbaResponse {
        dialog_token: request.dialog_token,
        status: 0,
        tid: AP_TX_BLOCK_ACK_TID,
        immediate: true,
        amsdu: false,
        window: AP_TX_BLOCK_ACK_WINDOW,
        timeout_tu: 0,
    };
    assert!(matches!(
        service.on_tx_block_ack_action(PEER, response),
        Ok(Some(TxBlockAckResponse::Operational(
            OperationalTxBlockAck {
                tid: AP_TX_BLOCK_ACK_TID,
                window: AP_TX_BLOCK_ACK_WINDOW,
                ..
            }
        )))
    ));
    assert!(service.peer_status(PEER).unwrap().tx_block_ack.is_some());
    assert_eq!(
        service.on_tx_block_ack_action(PEER, response),
        Ok(None),
        "a repeated response after the agreement is operational is stale"
    );
    assert!(service.peer_status(PEER).unwrap().tx_block_ack.is_some());
    assert!(
        !service
            .peer_status(PEER)
            .unwrap()
            .tx_block_ack
            .unwrap()
            .amsdu
    );
    assert!(service.peer_status(OTHER).unwrap().tx_block_ack.is_none());
    assert_eq!(
        service.operational_tx_block_ack_window(PEER),
        Some(AP_TX_BLOCK_ACK_WINDOW)
    );
    assert_eq!(service.operational_tx_block_ack_window(OTHER), None);
    assert!(service.has_operational_tx_block_ack());

    service
        .on_tx_block_ack_action(
            PEER,
            BlockAckAction::Delba {
                tid: AP_TX_BLOCK_ACK_TID,
                initiator: true,
                reason: 37,
            },
        )
        .unwrap();
    assert!(
        service.peer_status(PEER).unwrap().tx_block_ack.is_some(),
        "peer-originated RX DELBA cannot revoke the AP-originated TX agreement"
    );
    assert_eq!(
        service.operational_tx_block_ack_window(PEER),
        Some(AP_TX_BLOCK_ACK_WINDOW)
    );
    assert_eq!(service.operational_tx_block_ack_window(OTHER), None);
    assert!(service.has_operational_tx_block_ack());

    service
        .on_tx_block_ack_action(
            PEER,
            BlockAckAction::Delba {
                tid: AP_TX_BLOCK_ACK_TID,
                initiator: false,
                reason: 37,
            },
        )
        .unwrap();
    assert!(service.peer_status(PEER).unwrap().tx_block_ack.is_none());
    assert_eq!(service.operational_tx_block_ack_window(PEER), None);
    assert!(!service.has_operational_tx_block_ack());

    let request = offer(&mut service, PEER, 200).unwrap();
    let response = BlockAckAction::AddbaResponse {
        dialog_token: request.dialog_token,
        status: 0,
        tid: AP_TX_BLOCK_ACK_TID,
        immediate: true,
        amsdu: true,
        window: AP_TX_BLOCK_ACK_WINDOW,
        timeout_tu: 0,
    };
    service.on_tx_block_ack_action(PEER, response).unwrap();
    assert!(
        service
            .peer_status(PEER)
            .unwrap()
            .tx_block_ack
            .unwrap()
            .amsdu
    );
}

#[test]
fn addba_response_after_the_negotiation_timeout_is_dropped_as_stale() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    service.authenticate_open(PEER, oer_time::Instant::from_micros(1));
    service
        .associate_rsn(
            PEER,
            association_security(&WPA2_RSN),
            ht_capabilities(),
            [7; 32],
            9,
            oer_time::Instant::from_micros(1),
        )
        .unwrap();
    service.checked_peer_mut(PEER).unwrap().phase = ApPeerPhase::Authorized;

    let request = offer(&mut service, PEER, 100).unwrap();
    assert_eq!(
        service.next_tx_block_ack_deadline(),
        Some(request.alarm.deadline)
    );
    assert_eq!(
        service.expire_tx_block_ack(request.alarm.deadline),
        Some(PEER)
    );
    assert_eq!(service.next_tx_block_ack_deadline(), None);
    let late = BlockAckAction::AddbaResponse {
        dialog_token: request.dialog_token,
        status: 0,
        tid: AP_TX_BLOCK_ACK_TID,
        immediate: true,
        amsdu: false,
        window: AP_TX_BLOCK_ACK_WINDOW,
        timeout_tu: 0,
    };
    assert_eq!(service.on_tx_block_ack_action(PEER, late), Ok(None));
    assert_eq!(service.operational_tx_block_ack_window(PEER), None);

    // The late response must not complete a newer negotiation either.
    let retry = offer(&mut service, PEER, 200_000).unwrap();
    assert_ne!(retry.dialog_token, request.dialog_token);
    assert_eq!(service.on_tx_block_ack_action(PEER, late), Ok(None));
    assert_eq!(service.operational_tx_block_ack_window(PEER), None);
}

/// Queue one immediate attempt of `peer`'s TX Block Ack offer and take it
/// at `micros`.
fn offer(
    service: &mut AccessPointService<'_, oer_ieee80211_mac::security::spp::Association>,
    peer: [u8; 6],
    micros: u64,
) -> Option<AddbaRequest> {
    service
        .queue_tx_block_ack(
            peer,
            TxBlockAckRetry {
                attempts: 1,
                interval: oer_time::Duration::ZERO,
            },
        )
        .unwrap();
    service
        .take_tx_block_ack_offer(peer, oer_time::Instant::from_micros(micros))
        .unwrap()
}

#[test]
fn an_unanswered_or_unsent_offer_is_retried_after_the_interval_while_attempts_remain() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    service.authenticate_open(PEER, oer_time::Instant::from_micros(1));
    service
        .associate_rsn(
            PEER,
            association_security(&WPA2_RSN),
            ht_capabilities(),
            [7; 32],
            9,
            oer_time::Instant::from_micros(1),
        )
        .unwrap();
    // Before authorization the peer is offered nothing.
    let retry = TxBlockAckRetry {
        attempts: 3,
        interval: oer_time::Duration::from_millis(1_000),
    };
    assert_eq!(service.queue_tx_block_ack(PEER, retry), Ok(false));
    service.checked_peer_mut(PEER).unwrap().phase = ApPeerPhase::Authorized;
    assert_eq!(service.queue_tx_block_ack(PEER, retry), Ok(true));
    let at = |micros| oer_time::Instant::from_micros(micros);

    // The first request goes and is not answered.
    let first = service
        .take_tx_block_ack_offer(PEER, at(100))
        .unwrap()
        .unwrap();
    assert_eq!(
        service.expire_tx_block_ack(first.alarm.deadline),
        Some(PEER)
    );
    // The second is due one interval after the timeout.
    let due = first.alarm.deadline.saturating_add(retry.interval);
    assert_eq!(
        service.take_tx_block_ack_offer(PEER, at(due.as_micros() - 1)),
        Ok(None)
    );
    let second = service.take_tx_block_ack_offer(PEER, due).unwrap().unwrap();
    assert_ne!(second.dialog_token, first.dialog_token);
    // It does not leave; the third and last is due after the interval.
    service.tx_block_ack_offer_failed(PEER, due).unwrap();
    let last = due.saturating_add(retry.interval);
    let third = service
        .take_tx_block_ack_offer(PEER, last)
        .unwrap()
        .unwrap();
    // The peer declines: no attempt follows, whatever the time.
    let declined = BlockAckAction::AddbaResponse {
        dialog_token: third.dialog_token,
        status: 37,
        tid: AP_TX_BLOCK_ACK_TID,
        immediate: true,
        amsdu: false,
        window: 0,
        timeout_tu: 0,
    };
    assert_eq!(
        service.on_tx_block_ack_action(PEER, declined),
        Ok(Some(TxBlockAckResponse::Rejected(37)))
    );
    assert_eq!(
        service.take_tx_block_ack_offer(PEER, at(u64::MAX / 2)),
        Ok(None)
    );
    assert_eq!(service.operational_tx_block_ack_window(PEER), None);
}

#[test]
fn bss_protection_follows_associated_erp_ht_and_preamble_membership() {
    use oer_ieee80211_mac::protection::{
        ApBssProtection, ErpProtection, HtOperationProtection, HtProtectionMode,
    };

    let mut storage = AccessPointPeerStorage::new();
    let mut service = AccessPointService::<oer_ieee80211_mac::security::spp::Association>::new_open(
        AP,
        AccessPointClientLimit::new(2).unwrap(),
        AccessPointInactiveTimeout::default(),
        storage.lend(),
    );
    let open = ApAssociationSecurityObservation {
        privacy: false,
        rsn_ie: None,
        rsn_ie_count: 0,
        rsnxe: None,
        rsnxe_count: 0,
        legacy_wpa_present: false,
        malformed_elements: false,
    };
    // An authenticated 11b station is not yet a BSS member.
    service.authenticate_open(OTHER, oer_time::Instant::from_micros(0));
    assert_eq!(service.bss_protection(true), ApBssProtection::default());

    // A 20-MHz-only, non-greenfield HT peer in a 40-MHz BSS.
    service.authenticate_open(PEER, oer_time::Instant::from_micros(1));
    service
        .associate_open(
            PEER,
            open,
            ht_capabilities(),
            oer_time::Instant::from_micros(2),
        )
        .unwrap();
    let ht_only = HtOperationProtection {
        mode: HtProtectionMode::TwentyMhz,
        non_greenfield_present: true,
    };
    assert_eq!(
        service.bss_protection(true),
        ApBssProtection {
            non_erp_present: false,
            erp: ErpProtection::NONE,
            ht: ht_only,
        }
    );
    assert_eq!(
        service.bss_protection(false).ht.mode,
        HtProtectionMode::None
    );

    // A non-ERP peer without short preamble requires ERP protection with
    // Barker preambles and places the BSS in non-HT mixed mode.
    service
        .associate_open(
            OTHER,
            open,
            ApAssociationCapabilities {
                maximum_legacy_rate_500kbps: 22,
                short_preamble: false,
                ht: None,
                qos_supported: false,
            },
            oer_time::Instant::from_micros(3),
        )
        .unwrap();
    let mixed = service.bss_protection(true);
    assert!(mixed.non_erp_present);
    assert_eq!(mixed.erp, ErpProtection::new(true, true));
    assert_eq!(mixed.erp_information(), 0x07);
    assert_eq!(mixed.ht.mode, HtProtectionMode::NonHtMixed);
}

#[test]
fn a_station_resuming_a_pmksa_is_refused_by_an_access_point_without_a_cache() {
    let mut storage = AccessPointPeerStorage::new();
    let service = AccessPointService::<oer_ieee80211_mac::security::spp::Association>::new(
        AP,
        Pmk::derive(b"password", b"test-ap").unwrap(),
        RsnGtk::new(
            oer_ieee80211_mac::ccmp::GroupKeyId::new(1).unwrap(),
            true,
            &[0x55; 16],
        ),
        AccessPointClientLimit::new(2).unwrap(),
        AccessPointInactiveTimeout::new(10).unwrap(),
        storage.lend(),
    );
    assert!(service.matches_association_security(association_security(&WPA2_RSN)));
    let mut resuming = [0_u8; 40];
    resuming[..22].copy_from_slice(&WPA2_RSN);
    resuming[1] = 38;
    resuming[22..24].copy_from_slice(&1_u16.to_le_bytes());
    resuming[24..40].copy_from_slice(&[0x5a; 16]);
    assert!(!service.matches_association_security(association_security(&resuming)));
}

const SAE_STATION_RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 8, 0x80, 0,
];
const SAE_PMK: [u8; 32] = [0x3c; 32];
const SAE_PMKID: [u8; 16] = [0x4d; 16];

fn wpa3_service(
    storage: &mut AccessPointPeerStorage,
) -> AccessPointService<'_, oer_ieee80211_mac::security::spp::Association> {
    AccessPointService::<oer_ieee80211_mac::security::spp::Association>::new_wpa3(
        AP,
        RsnGtk::new(
            oer_ieee80211_mac::ccmp::GroupKeyId::new(1).unwrap(),
            true,
            &[0x55; 16],
        ),
        RsnIgtk::new(4, [0; 6], &[0x66; 16]).unwrap(),
        AccessPointClientLimit::new(2).unwrap(),
        AccessPointInactiveTimeout::default(),
        storage.lend(),
    )
}

fn sae_station_rsn_with_pmkid(pmkid: [u8; 16]) -> [u8; 40] {
    let mut rsn = [0_u8; 40];
    rsn[..22].copy_from_slice(&SAE_STATION_RSN);
    rsn[1] = 38;
    rsn[22..24].copy_from_slice(&1_u16.to_le_bytes());
    rsn[24..].copy_from_slice(&pmkid);
    rsn
}

fn association_status(action: ApMlmeAction) -> u16 {
    let ApMlmeAction::AssociationResponse { status, .. } = action else {
        panic!("association answers with a response");
    };
    status
}

#[test]
fn a_wpa3_handshake_uses_the_sae_pmk_and_delivers_the_igtk() {
    const ANONCE: [u8; 32] = [7; 32];
    const SNONCE: [u8; 32] = [8; 32];
    let mut storage = AccessPointPeerStorage::new();
    let mut service = wpa3_service(&mut storage);
    assert_eq!(
        service.authenticate_sae(
            PEER,
            Pmk::from_bytes(SAE_PMK),
            SAE_PMKID,
            oer_time::Instant::from_micros(0)
        ),
        Ok(AP_STATUS_SUCCESS)
    );
    let action = service
        .associate_rsn(
            PEER,
            association_security(&SAE_STATION_RSN),
            ht_capabilities(),
            ANONCE,
            9,
            oer_time::Instant::from_micros(1),
        )
        .unwrap();
    assert_eq!(association_status(action), AP_STATUS_SUCCESS);
    let message1 = service.begin_wpa2_frame::<512>(PEER).unwrap();
    let (_frame, context) = message1.into_parts();
    service
        .observe_wpa2_transmit(context, true, oer_time::Instant::from_micros(2))
        .unwrap();

    let ptk = Pmk::from_bytes(SAE_PMK).derive_ptk(
        oer_ieee80211_mac::security::rsn::Akm::Sae,
        PtkContext {
            authenticator_address: AP,
            supplicant_address: PEER,
            authenticator_nonce: ANONCE,
            supplicant_nonce: SNONCE,
        },
    );
    let rsn = OwnedRsnIe::<64>::try_copy(&SAE_STATION_RSN).unwrap();
    let message2 = RsnTxFrame::<512>::message2(
        oer_ieee80211_mac::security::rsn::Akm::Sae,
        AP,
        9,
        SNONCE,
        &rsn,
    )
    .unwrap()
    .authenticate(&ptk);
    let message2 =
        OwnedEapolFrame::<512>::try_copy(RsnInterface::AccessPoint, PEER, message2.as_bytes())
            .unwrap();
    let ApWpa2Progress::Transmit(message3) = service.on_eapol(PEER, message2).unwrap() else {
        panic!("message 2 must produce message 3");
    };
    let (message3, message3_context) = message3.into_parts();
    assert!(message3.key_frame().verify_mic(&ptk));
    let plaintext = software_aes128_key_unwrap(ptk.kek(), message3.key_frame().key_data())
        .expect("AP wrapped its Message 3 key data");
    let keys = parse_gtk_key_data(
        plaintext.as_bytes(),
        &oer_ieee80211_mac::security::AP_WPA3_PERSONAL_RSN_ELEMENT,
        &oer_ieee80211_mac::security::AP_SAE_H2E_RSNX_ELEMENT,
        true,
    )
    .unwrap();
    assert_eq!(keys.igtk.unwrap().key(), &[0x66; 16]);
    service
        .observe_wpa2_transmit(message3_context, true, oer_time::Instant::from_micros(3))
        .unwrap();
}

#[test]
fn a_wpa3_association_resumes_only_a_known_pmkid() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = wpa3_service(&mut storage);
    let associate =
        |service: &mut AccessPointService<'_, oer_ieee80211_mac::security::spp::Association>,
         rsn: &[u8]| {
            association_status(
                service
                    .associate_rsn(
                        PEER,
                        association_security(rsn),
                        ht_capabilities(),
                        [7; 32],
                        9,
                        oer_time::Instant::from_micros(1),
                    )
                    .unwrap(),
            )
        };

    // Open System authentication without a cached association.
    service.authenticate_open(PEER, oer_time::Instant::from_micros(0));
    assert_eq!(
        associate(&mut service, &SAE_STATION_RSN),
        AP_STATUS_INVALID_PMKID
    );
    assert_eq!(
        associate(&mut service, &sae_station_rsn_with_pmkid(SAE_PMKID)),
        AP_STATUS_INVALID_PMKID
    );

    // An SAE exchange caches the association; Open System then resumes it.
    service
        .authenticate_sae(
            PEER,
            Pmk::from_bytes(SAE_PMK),
            SAE_PMKID,
            oer_time::Instant::from_micros(0),
        )
        .unwrap();
    service.authenticate_open(PEER, oer_time::Instant::from_micros(0));
    assert_eq!(
        associate(&mut service, &sae_station_rsn_with_pmkid([0x11; 16])),
        AP_STATUS_INVALID_PMKID
    );
    assert_eq!(
        associate(&mut service, &sae_station_rsn_with_pmkid(SAE_PMKID)),
        AP_STATUS_SUCCESS
    );
}

#[test]
fn a_wpa3_access_point_refuses_psk_and_stations_without_protection() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = wpa3_service(&mut storage);
    service
        .authenticate_sae(
            PEER,
            Pmk::from_bytes(SAE_PMK),
            SAE_PMKID,
            oer_time::Instant::from_micros(0),
        )
        .unwrap();
    assert!(!service.matches_association_security(association_security(&WPA2_RSN)));
    let mut unprotected = SAE_STATION_RSN;
    unprotected[20] = 0;
    assert!(!service.matches_association_security(association_security(&unprotected)));
    assert_eq!(
        association_status(
            service
                .associate_rsn(
                    PEER,
                    association_security(&WPA2_RSN),
                    ht_capabilities(),
                    [7; 32],
                    9,
                    oer_time::Instant::from_micros(1),
                )
                .unwrap()
        ),
        AP_STATUS_INVALID_RSN
    );
}

/// Finish the protocol handshake; the service caller installs the returned
/// pairwise key before recording authorization.
fn begin_management_handshake(
    service: &mut AccessPointService<'_, oer_ieee80211_mac::security::spp::Association>,
    peer: [u8; 6],
) -> (Ptk, ApEapolTransmit<512>) {
    let now = oer_time::Instant::from_micros(1);
    service
        .authenticate_sae(peer, Pmk::from_bytes(SAE_PMK), SAE_PMKID, now)
        .unwrap();
    let action = service
        .associate_rsn(
            peer,
            association_security(&SAE_STATION_RSN),
            ht_capabilities(),
            [7; 32],
            9,
            now,
        )
        .unwrap();
    assert_eq!(association_status(action), AP_STATUS_SUCCESS);
    let ptk = Pmk::from_bytes(SAE_PMK).derive_ptk(
        oer_ieee80211_mac::security::rsn::Akm::Sae,
        PtkContext {
            authenticator_address: AP,
            supplicant_address: peer,
            authenticator_nonce: [7; 32],
            supplicant_nonce: [8; 32],
        },
    );
    let rsn = OwnedRsnIe::<64>::try_copy(&SAE_STATION_RSN).unwrap();
    let message2 = RsnTxFrame::<512>::message2(
        oer_ieee80211_mac::security::rsn::Akm::Sae,
        AP,
        9,
        [8; 32],
        &rsn,
    )
    .unwrap()
    .authenticate(&ptk);
    let message2 =
        OwnedEapolFrame::<512>::try_copy(RsnInterface::AccessPoint, peer, message2.as_bytes())
            .unwrap();
    let ApWpa2Progress::Transmit(message3) = service.on_eapol(peer, message2).unwrap() else {
        panic!("verified Message 2 produces Message 3");
    };
    assert_eq!(
        service.management_tx_protection(
            peer,
            oer_ieee80211_mac::ap::ApManagementSubtype::Action,
            &[8, 0, 1, 0]
        ),
        ApManagementProtection::Plaintext
    );
    (ptk, message3)
}

fn authorize_management_peer(
    service: &mut AccessPointService<'_, oer_ieee80211_mac::security::spp::Association>,
    peer: [u8; 6],
) {
    let (ptk, _) = begin_management_handshake(service, peer);
    let now = oer_time::Instant::from_micros(1);
    let message4 = RsnTxFrame::<512>::message4(oer_ieee80211_mac::security::rsn::Akm::Sae, AP, 10)
        .unwrap()
        .authenticate(&ptk);
    let message4 =
        OwnedEapolFrame::<512>::try_copy(RsnInterface::AccessPoint, peer, message4.as_bytes())
            .unwrap();
    assert!(matches!(
        service.on_eapol(peer, message4).unwrap(),
        ApWpa2Progress::AuthorizePeer
    ));
    service.authorize(peer, now).unwrap();
}

#[test]
fn management_protection_follows_key_authorization_and_survives_closure() {
    use oer_ieee80211_mac::ap::ApManagementSubtype as Kind;
    let mut storage = AccessPointPeerStorage::new();
    let mut service = wpa3_service(&mut storage);
    authorize_management_peer(&mut service, PEER);
    assert_eq!(
        service.management_tx_protection(PEER, Kind::Action, &[8, 0, 1, 0]),
        ApManagementProtection::Pairwise
    );
    assert_eq!(
        service.management_tx_protection(PEER, Kind::Action, &[4, 4]),
        ApManagementProtection::Plaintext
    );
    assert_eq!(
        service.management_tx_protection([0xff; 6], Kind::Action, &[0, 4]),
        ApManagementProtection::GroupIntegrity
    );
    let deadline = service.peer_status(PEER).unwrap().deadline;
    assert_eq!(service.begin_due_peer_close(deadline).unwrap().peer, PEER);
    assert_eq!(
        service.management_tx_protection(PEER, Kind::Deauthentication, &[3, 0]),
        ApManagementProtection::Pairwise
    );
    service.remove_peer(PEER).unwrap();
    assert_eq!(
        service.management_tx_protection(PEER, Kind::Deauthentication, &[3, 0]),
        ApManagementProtection::Plaintext
    );
}

#[test]
fn management_replay_and_policy_rejections_leave_activity_unchanged() {
    use oer_ieee80211_mac::{ap::ApManagementSubtype as Kind, ccmp::CcmpPacketNumber};
    let mut storage = AccessPointPeerStorage::new();
    let mut service = wpa3_service(&mut storage);
    authorize_management_peer(&mut service, PEER);
    let status = service.peer_status(PEER).unwrap();
    let second_peer = [2, 0, 0, 0, 0, 3];
    assert_ne!(second_peer, PEER);
    authorize_management_peer(&mut service, second_peer);
    let proof = |pn| ApManagementRxProtection::PairwiseVerified(CcmpPacketNumber::new(pn).unwrap());
    assert_eq!(
        service.receive_management(
            PEER,
            Kind::Deauthentication,
            &[3, 0],
            ApManagementRxProtection::Plaintext
        ),
        ApManagementRx::Rejected(ApManagementRejection::Unprotected)
    );
    let query = [8, 0, 0x12, 0x34];
    assert_eq!(
        service.receive_management(PEER, Kind::Action, &query, proof(7)),
        ApManagementRx::Accepted(ApManagementAction::SaQueryResponse {
            transaction: [0x12, 0x34]
        })
    );
    assert!(matches!(
        service.receive_management(second_peer, Kind::Action, &query, proof(1)),
        ApManagementRx::Accepted(_)
    ));
    assert_eq!(
        service.receive_management(PEER, Kind::Action, &[8], proof(100)),
        ApManagementRx::Rejected(ApManagementRejection::Malformed)
    );
    // Repeated authorization of the same key cannot reopen its replay state.
    service
        .authorize(PEER, oer_time::Instant::from_micros(1))
        .unwrap();
    assert_eq!(
        service.receive_management(PEER, Kind::Action, &query, proof(7)),
        ApManagementRx::Rejected(ApManagementRejection::Replay)
    );
    assert_eq!(
        service.receive_management(PEER, Kind::Action, &query, proof(6)),
        ApManagementRx::Rejected(ApManagementRejection::Replay)
    );
    assert_eq!(
        service.receive_management(PEER, Kind::Action, &[4, 4], proof(100)),
        ApManagementRx::Rejected(ApManagementRejection::UnexpectedProtection)
    );
    assert!(matches!(
        service.receive_management(PEER, Kind::Action, &query, proof(8)),
        ApManagementRx::Accepted(_)
    ));
    assert_eq!(
        service.peer_status(PEER).unwrap().last_activity,
        status.last_activity
    );
    service.remove_peer(PEER).unwrap();
    authorize_management_peer(&mut service, PEER);
    assert_ne!(
        service.peer_status(PEER).unwrap().association_epoch,
        status.association_epoch
    );
    assert!(matches!(
        service.receive_management(PEER, Kind::Action, &query, proof(1)),
        ApManagementRx::Accepted(_)
    ));
}

#[test]
fn the_bss_owns_one_monotonic_bip_frontier_and_refuses_invalid_group_frames() {
    use oer_ieee80211_mac::ap::{ApManagementFrame, ApManagementSubtype};
    use oer_ieee80211_rsn::bip::BipReceiver;
    let mut storage = AccessPointPeerStorage::new();
    let mut service = wpa3_service(&mut storage);
    let mut receiver = BipReceiver::new(service.igtk().unwrap());
    let mut first = [0; 64];
    let len = ApManagementFrame {
        subtype: ApManagementSubtype::Action,
        access_point: AP,
        peer: [0xff; 6],
        sequence_number: SequenceNumber::ZERO,
        body: &[0, 4, 37, 3, 0, 6, 3],
    }
    .encode(&mut first)
    .unwrap();
    let mut invalid = first;
    invalid[10] ^= 2;
    assert_eq!(
        service.protect_group_management(&mut invalid, len),
        Err(ApGroupManagementError::InvalidFrame)
    );
    let mut second = first;
    let protected = service.protect_group_management(&mut first, len).unwrap();
    assert_eq!(receiver.verify(&first[..protected]), Ok(()));
    assert!(receiver.verify(&first[..protected]).is_err());
    let protected = service.protect_group_management(&mut second, len).unwrap();
    assert_eq!(receiver.verify(&second[..protected]), Ok(()));
    assert_eq!(&first[len + 4..len + 10], &[1, 0, 0, 0, 0, 0]);
    assert_eq!(&second[len + 4..len + 10], &[2, 0, 0, 0, 0, 0]);
    let (ptk, message3) = begin_management_handshake(&mut service, PEER);
    let (message3, message3_context) = message3.into_parts();
    let plaintext = software_aes128_key_unwrap(ptk.kek(), message3.key_frame().key_data()).unwrap();
    let keys = parse_gtk_key_data(
        plaintext.as_bytes(),
        &oer_ieee80211_mac::security::AP_WPA3_PERSONAL_RSN_ELEMENT,
        &oer_ieee80211_mac::security::AP_SAE_H2E_RSNX_ELEMENT,
        true,
    )
    .unwrap();
    let delivered = keys.igtk.unwrap();
    assert_eq!(delivered.packet_number(), [2, 0, 0, 0, 0, 0]);
    let mut new_peer = BipReceiver::new(&delivered);
    assert_eq!(
        new_peer.verify(&second[..protected]),
        Err(oer_ieee80211_rsn::bip::BipError::Replay)
    );
    service
        .observe_wpa2_transmit(message3_context, true, oer_time::Instant::from_micros(10))
        .unwrap();
    // Group TX can advance while Message 4 is missing; a retransmitted M3
    // must retain its original authenticated bytes, including the IPN.
    let mut third = second;
    let protected = service.protect_group_management(&mut third, len).unwrap();
    assert_eq!(new_peer.verify(&third[..protected]), Ok(()));
    assert_eq!(service.igtk().unwrap().packet_number(), [3, 0, 0, 0, 0, 0]);
    let ApWpa2RetryProgress::Transmit(repeated) = service
        .take_due_wpa2_retry::<512>(service.next_wpa2_retry_deadline().unwrap())
        .unwrap()
    else {
        panic!("Message 3 response timeout must retransmit");
    };
    assert_eq!(*repeated.frame().peer(), PEER);
    assert!(repeated.frame().retransmission());
    assert_eq!(repeated.frame().as_bytes(), message3.as_bytes());
}

#[test]
fn non_spp_ap_explicitly_refuses_required_clients_for_each_scope() {
    use oer_ieee80211_mac::security::{Spp, SppCapability, SppError, spp};
    fn check<S: spp::SppScope>() {
        for (capability, expected) in [
            (SppCapability::NotCapable, Ok(Spp::NotNegotiated)),
            (SppCapability::Capable, Ok(Spp::NotNegotiated)),
            (
                SppCapability::Required,
                Err(ApAssociationSecurityError::Spp(
                    SppError::PeerRequiresSupport,
                )),
            ),
        ] {
            let mut rsn = WPA2_RSN;
            rsn[20..22].copy_from_slice(&capability.rsn_bits().to_le_bytes());
            let observed = admit_association_security::<S>(
                ApSecurityPolicy::Wpa2Personal,
                association_security(&rsn),
            )
            .map(|selected| match selected {
                AssociationSecurity::Rsn(selected) => selected.spp,
                AssociationSecurity::Open => panic!("RSN request must not downgrade"),
            });
            assert_eq!(observed, expected);
        }
    }
    check::<spp::Interface>();
    check::<spp::Association>();
}

#[test]
fn interface_scoped_ap_sends_refusal_for_required_spp_without_admitting_peer() {
    use oer_ieee80211_mac::security::{SppCapability, spp};
    for capability in [
        SppCapability::NotCapable,
        SppCapability::Capable,
        SppCapability::Required,
    ] {
        let mut storage = AccessPointPeerStorage::new();
        let mut service = AccessPointService::<spp::Interface>::new(
            AP,
            Pmk::derive(b"password", b"test-ap").unwrap(),
            RsnGtk::new(
                oer_ieee80211_mac::ccmp::GroupKeyId::new(1).unwrap(),
                true,
                &[0x55; 16],
            ),
            AccessPointClientLimit::new(2).unwrap(),
            AccessPointInactiveTimeout::default(),
            storage.lend(),
        );
        let mut rsn = WPA2_RSN;
        rsn[20..22].copy_from_slice(&capability.rsn_bits().to_le_bytes());
        assert_eq!(
            service.associate_rsn(
                PEER,
                association_security(&rsn),
                ht_capabilities(),
                [7; 32],
                9,
                oer_time::Instant::EPOCH,
            ),
            Err(ApServiceError::UnknownPeer)
        );
        service.authenticate_open(PEER, oer_time::Instant::EPOCH);
        let action = service
            .associate_rsn(
                PEER,
                association_security(&rsn),
                ht_capabilities(),
                [7; 32],
                9,
                oer_time::Instant::EPOCH,
            )
            .unwrap();
        if capability == SppCapability::Required {
            assert_eq!(
                action,
                ApMlmeAction::AssociationResponse {
                    peer: PEER,
                    status: AP_STATUS_INVALID_RSN,
                    association_id: None
                }
            );
            assert_eq!(service.associated_count(), 0);
            assert_eq!(
                service.peer_status(PEER).unwrap().phase,
                ApPeerPhase::Authenticated
            );
        } else {
            assert_eq!(
                action,
                ApMlmeAction::AssociationResponse {
                    peer: PEER,
                    status: AP_STATUS_SUCCESS,
                    association_id: Some(1)
                }
            );
            assert_eq!(service.associated_count(), 1);
            assert_eq!(
                service.associate_rsn(
                    PEER,
                    association_security(&rsn),
                    ht_capabilities(),
                    [7; 32],
                    9,
                    oer_time::Instant::EPOCH,
                ),
                Err(ApServiceError::WrongPeerPhase)
            );
        }
    }
}
