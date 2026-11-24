use hmac::Mac;
use oer_protocols_ieee80211_wire::eapol::GtkTx;

use super::*;
use crate::test_engine::Refusing;
use crate::{
    frames::{RsnGtk, RsnPlainKeyData},
    keys::RsnKeyKind,
};
use oer_protocols_ieee80211_core_security_cipher::aes::{RSN_KEY_DATA_CAPACITY, SoftwareAes128};

const LOCAL: [u8; 6] = [1; 6];
const AP: [u8; 6] = [2; 6];
const SNONCE: [u8; 32] = [3; 32];
const ANONCE: [u8; 32] = [4; 32];
const RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];

/// Take one frame, unwrapping its key data in software.
fn process(
    handshake: &mut RsnStaHandshake,
    frame: crate::OwnedEapolFrame<512>,
    pmk: &Pmk,
) -> Result<RsnStaEapol<512>, RsnStaError> {
    handshake.on_eapol(frame, pmk, &mut SoftwareAes128)
}

fn test_pmk() -> Pmk {
    let mut derivation = crate::PmkDerivation::new(b"password", b"ssid").unwrap();
    loop {
        if let Some(pmk) = derivation.step() {
            break pmk;
        }
    }
}

fn handshake() -> RsnStaHandshake {
    RsnStaHandshake::new(LOCAL, AP, SNONCE, &RSN, &RSN, &[]).unwrap()
}

/// The key data Message 3 carries in these tests: the access point's RSN
/// element and a GTK (key 2, bytes 0x5a).
fn message3_plain() -> RsnPlainKeyData<64> {
    let rsn = OwnedRsnIe::<64>::try_copy(&RSN).unwrap();
    let gtk = RsnGtk::new(
        oer_protocols_ieee80211_wire::ccmp::GroupKeyId::new(2).unwrap(),
        oer_protocols_ieee80211_wire::eapol::GtkTx::ReceiveOnly,
        &[0x5a; 16],
    );
    RsnPlainKeyData::<64>::build(rsn.as_bytes(), &gtk, None).unwrap()
}

/// The installation `request` leaves once both its keys are installed.
fn installed(request: RsnStaKeyInstallRequest) -> RsnStaInstalled {
    let (_pairwise, group) = request.into_parts();
    group.installed()
}

const REFUSED: oer_portable_crypto::KeyUnwrapError = oer_portable_crypto::KeyUnwrapError::Engine(
    oer_portable_crypto::block::BlockError::InvalidState { observation: 3 },
);

fn owned(frame: &RsnTxFrame<512>) -> crate::OwnedEapolFrame<512> {
    crate::OwnedEapolFrame::try_copy(RsnInterface::Station, AP, frame.as_bytes()).unwrap()
}

fn context() -> PtkContext {
    PtkContext {
        authenticator_address: AP,
        supplicant_address: LOCAL,
        authenticator_nonce: ANONCE,
        supplicant_nonce: SNONCE,
    }
}

fn encrypted_message3(
    ptk: &Ptk,
    key_rsc: [u8; 8],
    plain_key_data: &[u8],
) -> crate::OwnedEapolFrame<512> {
    let wrapped = software_wrap(ptk.kek(), plain_key_data).unwrap();
    let frame = RsnTxFrame::<512>::message3(
        crate::Akm::Psk,
        LOCAL,
        2,
        ANONCE,
        key_rsc,
        wrapped.as_bytes(),
    )
    .unwrap()
    .authenticate(ptk, &mut SoftwareAes128)
    .unwrap();
    owned(&frame)
}

/// The key data of the tests' Group Message 1: one GTK KDE (key 1, bytes
/// 0x6a) wrapped under `ptk`'s KEK.
fn group_key_data(ptk: &Ptk) -> oer_protocols_ieee80211_core_security_cipher::aes::RsnKeyData {
    software_wrap(ptk.kek(), &group_kde(1, 0x6a)).unwrap()
}

enum GroupStep {
    Install(RsnGroupKeyInstallRequest<512>),
    Retransmit(RsnTxFrame<512>),
}

/// Process one Group Message 1, unwrapping its key data in software.
fn group_message1(
    connected: &mut RsnConnectedSupplicant,
    frame: crate::OwnedEapolFrame<512>,
) -> Result<GroupStep, RsnConnectedProcessError> {
    match connected
        .on_group_message1(frame, &mut SoftwareAes128)
        .map_err(RsnConnectedProcessError::Supplicant)?
    {
        RsnConnectedAction::Retransmit(response) => Ok(GroupStep::Retransmit(response)),
        RsnConnectedAction::GroupKeyData(frame) => connected
            .prepare_group_key_install(frame, &mut SoftwareAes128)
            .map(GroupStep::Install),
    }
}

fn connected(ptk: Ptk) -> RsnConnectedSupplicant {
    let message3 =
        RsnTxFrame::<512>::message3(crate::Akm::Psk, LOCAL, 2, ANONCE, [0; 8], &[0x55; 24])
            .unwrap()
            .authenticate(&ptk, &mut SoftwareAes128)
            .unwrap();
    let completed_message3 = RsnCompletedMessage3::capture(message3.key_frame().unwrap());
    let (key_confirmation, key_encryption) = ptk.into_connected_keys();
    RsnConnectedSupplicant {
        authenticator: AP,
        key_confirmation,
        key_encryption,
        authenticator_security_ies: OwnedAssociationSecurityIes::try_copy_bytes(&RSN).unwrap(),
        completed_message3,
        frontier: Frontier::Message3,
        group_management: None,
    }
}

/// Whether a changed test frame carries a MIC over its changed bytes.
#[derive(Clone, Copy)]
enum Mic {
    Resigned,
    Stale,
}

fn resign_eapol_key_variant(
    original: &crate::OwnedEapolFrame<512>,
    ptk: &Ptk,
    mic: Mic,
    mutate: impl FnOnce(&mut [u8]),
) -> crate::OwnedEapolFrame<512> {
    let mut bytes = [0; 512];
    let len = original.as_bytes().len();
    bytes[..len].copy_from_slice(original.as_bytes());
    mutate(&mut bytes[..len]);
    if let Mic::Resigned = mic {
        bytes[81..97].fill(0);
        let mut mac = hmac::Hmac::<sha1::Sha1>::new_from_slice(ptk.kck()).unwrap();
        mac.update(&bytes[..len]);
        let digest = mac.finalize().into_bytes();
        bytes[81..97].copy_from_slice(&digest[..16]);
    }
    crate::OwnedEapolFrame::try_copy(RsnInterface::Station, AP, &bytes[..len]).unwrap()
}

fn completed_connected() -> (RsnConnectedSupplicant, crate::OwnedEapolFrame<512>, Ptk) {
    let (mut handshake, message3, pmk, expected_ptk) = awaiting_encrypted_message3();
    let duplicate = message3.clone();
    let RsnStaEapol::Install(request) = process(&mut handshake, message3, &pmk).unwrap() else {
        panic!("Message 3 must produce one key transaction")
    };
    let connected = handshake
        .complete::<512, _>(installed(request), &mut SoftwareAes128)
        .unwrap();
    (connected.supplicant, duplicate, expected_ptk)
}

fn group_kde(key_id: u8, key: u8) -> [u8; 24] {
    let mut kde = [0; 24];
    kde[..8].copy_from_slice(&[0xdd, 22, 0, 0x0f, 0xac, 1, key_id, 0]);
    kde[8..].fill(key);
    kde
}

#[test]
fn resolves_m1_through_typed_install_and_authenticated_m4() {
    let pmk = test_pmk();
    let expected_ptk = pmk.derive_ptk(crate::Akm::Psk, context());
    let mut handshake = handshake();

    let message1 = RsnTxFrame::<512>::message1(crate::Akm::Psk, LOCAL, 1, ANONCE).unwrap();
    let RsnStaEapol::Transmit(message2) = process(&mut handshake, owned(&message1), &pmk).unwrap()
    else {
        panic!("Message 1 must produce Message 2")
    };
    let key = message2.key_frame().unwrap();
    assert_eq!(key.replay_counter(), 1);
    assert_eq!(key.key_data(), &RSN);
    assert!(!message2.retransmission());
    assert!(key.verify_mic(&expected_ptk, &mut SoftwareAes128).unwrap());

    let plain = message3_plain();
    let rsc = [7, 6, 5, 4, 3, 2, 1, 0];
    let message3 = encrypted_message3(&expected_ptk, rsc, plain.as_bytes());
    let RsnStaEapol::Install(request) = process(&mut handshake, message3, &pmk).unwrap() else {
        panic!("Message 3 must produce one key transaction")
    };
    assert_eq!(request.replay_counter(), 2);
    assert_eq!(request.plain_key_data_len(), plain.as_bytes().len());
    assert_eq!(request.pairwise().peer(), &AP);
    assert_eq!(
        request.pairwise().key().as_bytes(),
        expected_ptk.temporal_key()
    );
    assert_eq!(
        request.group().kind(),
        RsnKeyKind::Group {
            key_id: oer_protocols_ieee80211_wire::ccmp::GroupKeyId::new(2).unwrap(),
            tx: GtkTx::ReceiveOnly,
        }
    );
    assert_eq!(request.group().receive_sequence(), &rsc);
    assert_eq!(request.group().key().as_bytes(), &[0x5a; 16]);

    let connected = handshake
        .complete::<512, _>(installed(request), &mut SoftwareAes128)
        .unwrap();
    let message4 = connected.message4;
    assert_eq!(
        message4.key_frame().unwrap().message(),
        crate::EapolKeyMessage::PairwiseMessage4
    );
    assert_eq!(message4.key_frame().unwrap().replay_counter(), 2);
    assert!(
        message4
            .key_frame()
            .unwrap()
            .verify_mic(&expected_ptk, &mut SoftwareAes128)
            .unwrap()
    );
    assert_eq!(connected.supplicant.replay_counter(), 2);
}

#[test]
fn invalid_message3_mic_is_refused_and_valid_retry_can_install() {
    let (mut handshake, message3, pmk, expected_ptk) = awaiting_encrypted_message3();
    let mut bytes = [0; 512];
    let len = message3.as_bytes().len();
    bytes[..len].copy_from_slice(message3.as_bytes());
    bytes[81] ^= 1;
    let changed =
        crate::OwnedEapolFrame::<512>::try_copy(RsnInterface::Station, AP, &bytes[..len]).unwrap();
    assert!(matches!(
        process(&mut handshake, changed, &pmk),
        Err(RsnStaError::Rejected(RsnStateError::InvalidMic))
    ));
    // The forged frame stored nothing: the real one installs.
    let valid_retry = encrypted_message3(&expected_ptk, [0; 8], message3_plain().as_bytes());
    let RsnStaEapol::Install(request) = process(&mut handshake, valid_retry, &pmk).unwrap() else {
        panic!("valid M3 retry must retain the original join")
    };
    assert_eq!(request.replay_counter(), 2);
}

#[test]
fn connected_exact_duplicate_message3_retransmits_authenticated_m4() {
    let (mut connected, duplicate, ptk) = completed_connected();

    let RsnConnectedAction::Retransmit(message4) = connected
        .on_message3(duplicate, &mut SoftwareAes128)
        .unwrap()
    else {
        panic!("exact repeat produces M4")
    };

    assert_eq!(
        message4.key_frame().unwrap().message(),
        crate::EapolKeyMessage::PairwiseMessage4
    );
    assert_eq!(message4.key_frame().unwrap().replay_counter(), 2);
    assert!(message4.retransmission());
    assert!(
        message4
            .key_frame()
            .unwrap()
            .verify_mic(&ptk, &mut SoftwareAes128)
            .unwrap()
    );
}

#[test]
fn connected_duplicate_message3_rejects_changed_protocol_fields() {
    let (mut connected, duplicate, ptk) = completed_connected();

    let wrong_interface = crate::OwnedEapolFrame::<512>::try_copy(
        RsnInterface::AccessPoint,
        AP,
        duplicate.as_bytes(),
    )
    .unwrap();
    assert!(matches!(
        connected.on_message3(wrong_interface, &mut SoftwareAes128),
        Err(RsnConnectedSupplicantError::WrongInterface)
    ));
    let wrong_peer = crate::OwnedEapolFrame::<512>::try_copy(
        RsnInterface::Station,
        [9; 6],
        duplicate.as_bytes(),
    )
    .unwrap();
    assert!(matches!(
        connected.on_message3(wrong_peer, &mut SoftwareAes128),
        Err(RsnConnectedSupplicantError::WrongPeer)
    ));

    let wrong_protocol = resign_eapol_key_variant(&duplicate, &ptk, Mic::Resigned, |bytes| {
        bytes[0] ^= 1;
    });
    assert!(matches!(
        connected.on_message3(wrong_protocol, &mut SoftwareAes128),
        Err(RsnConnectedSupplicantError::ProtocolVersionMismatch)
    ));

    let wrong_descriptor = resign_eapol_key_variant(&duplicate, &ptk, Mic::Resigned, |bytes| {
        let key_info = u16::from_be_bytes([bytes[5], bytes[6]]);
        bytes[5..7].copy_from_slice(&((key_info & !0x0007) | 1).to_be_bytes());
    });
    assert!(matches!(
        connected.on_message3(wrong_descriptor, &mut SoftwareAes128),
        Err(RsnConnectedSupplicantError::UnsupportedDescriptorVersion)
    ));

    let wrong_flags = resign_eapol_key_variant(&duplicate, &ptk, Mic::Resigned, |bytes| {
        let key_info = u16::from_be_bytes([bytes[5], bytes[6]]) ^ (1 << 9);
        bytes[5..7].copy_from_slice(&key_info.to_be_bytes());
    });
    assert!(matches!(
        connected.on_message3(wrong_flags, &mut SoftwareAes128),
        Err(RsnConnectedSupplicantError::UnsupportedMessage)
    ));

    let wrong_replay = resign_eapol_key_variant(&duplicate, &ptk, Mic::Resigned, |bytes| {
        bytes[9..17].copy_from_slice(&1_u64.to_be_bytes());
    });
    assert!(matches!(
        connected.on_message3(wrong_replay, &mut SoftwareAes128),
        Err(RsnConnectedSupplicantError::StaleReplayCounter)
    ));

    let wrong_anonce = resign_eapol_key_variant(&duplicate, &ptk, Mic::Resigned, |bytes| {
        bytes[17] ^= 1;
    });
    assert!(matches!(
        connected.on_message3(wrong_anonce, &mut SoftwareAes128),
        Err(RsnConnectedSupplicantError::AuthenticatorNonceMismatch)
    ));
}

#[test]
fn connected_duplicate_message3_rejects_bad_mic_and_changed_commitment() {
    let (mut connected, duplicate, ptk) = completed_connected();

    let bad_mic = resign_eapol_key_variant(&duplicate, &ptk, Mic::Stale, |bytes| {
        bytes[81] ^= 1;
    });
    assert!(matches!(
        connected.on_message3(bad_mic, &mut SoftwareAes128),
        Err(RsnConnectedSupplicantError::InvalidMic)
    ));

    let changed_rsc = resign_eapol_key_variant(&duplicate, &ptk, Mic::Resigned, |bytes| {
        bytes[65] ^= 1;
    });
    assert!(matches!(
        connected.on_message3(changed_rsc, &mut SoftwareAes128),
        Err(RsnConnectedSupplicantError::RetainedMessage3Mismatch)
    ));

    let changed_key_data = resign_eapol_key_variant(&duplicate, &ptk, Mic::Resigned, |bytes| {
        bytes[99] ^= 1;
    });
    assert!(matches!(
        connected.on_message3(changed_key_data, &mut SoftwareAes128),
        Err(RsnConnectedSupplicantError::RetainedMessage3Mismatch)
    ));
}

#[test]
fn a_newer_completed_group_request_cannot_be_rolled_back_by_an_older_owner() {
    let pmk = {
        let mut derivation = crate::PmkDerivation::new(b"password", b"ssid").unwrap();
        loop {
            if let Some(pmk) = derivation.step() {
                break pmk;
            }
        }
    };
    let ptk = pmk.derive_ptk(crate::Akm::Psk, context());
    let mut connected = connected(pmk.derive_ptk(crate::Akm::Psk, context()));
    let frame = |counter| {
        RsnTxFrame::<512>::group_message1(
            crate::Akm::Psk,
            LOCAL,
            counter,
            [9; 8],
            group_key_data(&ptk).as_bytes(),
        )
        .unwrap()
        .authenticate(&ptk, &mut SoftwareAes128)
        .unwrap()
    };
    let GroupStep::Install(older) = group_message1(&mut connected, owned(&frame(3))).unwrap()
    else {
        panic!()
    };
    let GroupStep::Install(newer) = group_message1(&mut connected, owned(&frame(4))).unwrap()
    else {
        panic!()
    };
    let response = connected.complete_group_key_install(newer).unwrap();
    assert_eq!(response.key_frame().unwrap().replay_counter(), 4);
    assert!(matches!(
        connected.complete_group_key_install(older),
        Err(RsnConnectedSupplicantError::StaleCompletion)
    ));
    assert_eq!(connected.replay_counter(), 4);
}

#[test]
fn connected_group_rekey_installs_once_and_retransmits_idempotently() {
    let pmk = {
        let mut derivation = crate::PmkDerivation::new(b"password", b"ssid").unwrap();
        loop {
            if let Some(pmk) = derivation.step() {
                break pmk;
            }
        }
    };
    let ptk = pmk.derive_ptk(crate::Akm::Psk, context());
    let frame = RsnTxFrame::<512>::group_message1(
        crate::Akm::Psk,
        LOCAL,
        3,
        [9; 8],
        group_key_data(&ptk).as_bytes(),
    )
    .unwrap()
    .authenticate(&ptk, &mut SoftwareAes128)
    .unwrap();
    let mut connected = connected(pmk.derive_ptk(crate::Akm::Psk, context()));
    let GroupStep::Install(request) = group_message1(&mut connected, owned(&frame)).unwrap() else {
        panic!("new Group Message 1 must request one GTK replacement")
    };
    assert_eq!(request.replay_counter(), 3);
    assert_eq!(
        request.group().kind(),
        RsnKeyKind::Group {
            key_id: oer_protocols_ieee80211_wire::ccmp::GroupKeyId::new(1).unwrap(),
            tx: GtkTx::ReceiveOnly,
        }
    );
    assert_eq!(request.group().key().as_bytes(), &[0x6a; 16]);
    let response = connected.complete_group_key_install(request).unwrap();
    assert_eq!(
        response.key_frame().unwrap().message(),
        crate::EapolKeyMessage::GroupMessage2
    );
    assert!(
        response
            .key_frame()
            .unwrap()
            .verify_mic(&ptk, &mut SoftwareAes128)
            .unwrap()
    );
    assert_eq!(connected.replay_counter(), 3);

    let GroupStep::Retransmit(repeated) = group_message1(&mut connected, owned(&frame)).unwrap()
    else {
        panic!("repeated Group Message 1 must not reinstall GTK")
    };
    assert!(
        repeated
            .key_frame()
            .unwrap()
            .verify_mic(&ptk, &mut SoftwareAes128)
            .unwrap()
    );
}

#[test]
fn connected_group_rekey_authenticates_duplicate_before_cached_response() {
    let pmk = {
        let mut derivation = crate::PmkDerivation::new(b"password", b"ssid").unwrap();
        loop {
            if let Some(pmk) = derivation.step() {
                break pmk;
            }
        }
    };
    let ptk = pmk.derive_ptk(crate::Akm::Psk, context());
    let frame = RsnTxFrame::<512>::group_message1(
        crate::Akm::Psk,
        LOCAL,
        3,
        [9; 8],
        group_key_data(&ptk).as_bytes(),
    )
    .unwrap()
    .authenticate(&ptk, &mut SoftwareAes128)
    .unwrap();
    let mut connected = connected(pmk.derive_ptk(crate::Akm::Psk, context()));
    let GroupStep::Install(request) = group_message1(&mut connected, owned(&frame)).unwrap() else {
        panic!("new Group Message 1 must request one GTK replacement")
    };
    connected.complete_group_key_install(request).unwrap();

    let mut bytes = [0; 512];
    let length = frame.as_bytes().len();
    bytes[..length].copy_from_slice(frame.as_bytes());
    bytes[81] ^= 1;
    let forged_duplicate =
        crate::OwnedEapolFrame::<512>::try_copy(RsnInterface::Station, AP, &bytes[..length])
            .unwrap();
    assert!(matches!(
        group_message1(&mut connected, forged_duplicate),
        Err(RsnConnectedProcessError::Supplicant(
            RsnConnectedSupplicantError::InvalidMic
        ))
    ));
}

#[test]
fn connected_group_rekey_rejects_authenticated_changed_same_replay() {
    let pmk = {
        let mut derivation = crate::PmkDerivation::new(b"password", b"ssid").unwrap();
        loop {
            if let Some(pmk) = derivation.step() {
                break pmk;
            }
        }
    };
    let ptk = pmk.derive_ptk(crate::Akm::Psk, context());
    let frame = RsnTxFrame::<512>::group_message1(
        crate::Akm::Psk,
        LOCAL,
        3,
        [9; 8],
        group_key_data(&ptk).as_bytes(),
    )
    .unwrap()
    .authenticate(&ptk, &mut SoftwareAes128)
    .unwrap();
    let original = owned(&frame);
    let mut connected = connected(pmk.derive_ptk(crate::Akm::Psk, context()));
    let GroupStep::Install(request) = group_message1(&mut connected, original.clone()).unwrap()
    else {
        panic!("new Group Message 1 must request one GTK replacement")
    };
    connected.complete_group_key_install(request).unwrap();

    let changed_rsc = resign_eapol_key_variant(&original, &ptk, Mic::Resigned, |bytes| {
        bytes[65] ^= 1;
    });
    assert!(matches!(
        group_message1(&mut connected, changed_rsc),
        Err(RsnConnectedProcessError::Supplicant(
            RsnConnectedSupplicantError::RetainedGroupMessage1Mismatch
        ))
    ));

    let changed_key_data = resign_eapol_key_variant(&original, &ptk, Mic::Resigned, |bytes| {
        bytes[99] ^= 1;
    });
    assert!(matches!(
        group_message1(&mut connected, changed_key_data),
        Err(RsnConnectedProcessError::Supplicant(
            RsnConnectedSupplicantError::RetainedGroupMessage1Mismatch
        ))
    ));
}

#[test]
fn failed_group_rekey_does_not_retain_message1_commitment() {
    let pmk = {
        let mut derivation = crate::PmkDerivation::new(b"password", b"ssid").unwrap();
        loop {
            if let Some(pmk) = derivation.step() {
                break pmk;
            }
        }
    };
    let ptk = pmk.derive_ptk(crate::Akm::Psk, context());
    let frame = RsnTxFrame::<512>::group_message1(
        crate::Akm::Psk,
        LOCAL,
        3,
        [9; 8],
        group_key_data(&ptk).as_bytes(),
    )
    .unwrap()
    .authenticate(&ptk, &mut SoftwareAes128)
    .unwrap();
    let mut connected = connected(pmk.derive_ptk(crate::Akm::Psk, context()));
    let GroupStep::Install(request) = group_message1(&mut connected, owned(&frame)).unwrap() else {
        panic!("new Group Message 1 must request one GTK replacement")
    };
    // The installation did not take effect: its owner never completes it.
    drop(request);

    let GroupStep::Install(retry) = group_message1(&mut connected, owned(&frame)).unwrap() else {
        panic!("failed publication must leave the same frame eligible for retry")
    };
    connected.complete_group_key_install(retry).unwrap();
    assert!(matches!(
        group_message1(&mut connected, owned(&frame)).unwrap(),
        GroupStep::Retransmit(_)
    ));
}

#[test]
fn connected_group_rekey_rejects_bad_mic_and_stale_replay() {
    let pmk = {
        let mut derivation = crate::PmkDerivation::new(b"password", b"ssid").unwrap();
        loop {
            if let Some(pmk) = derivation.step() {
                break pmk;
            }
        }
    };
    let ptk = pmk.derive_ptk(crate::Akm::Psk, context());
    let valid = RsnTxFrame::<512>::group_message1(
        crate::Akm::Psk,
        LOCAL,
        3,
        [0; 8],
        group_key_data(&ptk).as_bytes(),
    )
    .unwrap()
    .authenticate(&ptk, &mut SoftwareAes128)
    .unwrap();
    let mut bytes = [0; 512];
    bytes[..valid.as_bytes().len()].copy_from_slice(valid.as_bytes());
    bytes[81] ^= 1;
    let invalid = crate::OwnedEapolFrame::<512>::try_copy(
        RsnInterface::Station,
        AP,
        &bytes[..valid.as_bytes().len()],
    )
    .unwrap();
    let mut connected = connected(pmk.derive_ptk(crate::Akm::Psk, context()));
    assert!(matches!(
        group_message1(&mut connected, invalid),
        Err(RsnConnectedProcessError::Supplicant(
            RsnConnectedSupplicantError::InvalidMic
        ))
    ));

    let stale = RsnTxFrame::<512>::group_message1(
        crate::Akm::Psk,
        LOCAL,
        1,
        [0; 8],
        group_key_data(&ptk).as_bytes(),
    )
    .unwrap()
    .authenticate(&ptk, &mut SoftwareAes128)
    .unwrap();
    assert!(matches!(
        group_message1(&mut connected, owned(&stale)),
        Err(RsnConnectedProcessError::Supplicant(
            RsnConnectedSupplicantError::StaleReplayCounter
        ))
    ));
}

#[test]
fn response_deadlines_expire_exactly_at_their_total_wait() {
    use oer_portable_time::{Duration, Instant};

    let started = Instant::from_micros(500);
    for (wait, timeout) in [
        (RsnStaResponseWait::Message1, RSN_STA_MESSAGE1_TIMEOUT),
        (RsnStaResponseWait::Message3, RSN_STA_MESSAGE3_TIMEOUT),
    ] {
        let deadline = RsnStaResponseDeadline::start(wait, started).unwrap();
        let expiry = started.checked_add(timeout).unwrap();
        assert_eq!(deadline.deadline(), expiry);
        assert_eq!(
            deadline.poll(expiry.checked_sub(Duration::from_micros(1)).unwrap()),
            RsnStaDeadlineEvent::Pending
        );
        assert_eq!(
            deadline.poll(expiry),
            RsnStaDeadlineEvent::Expired {
                wait,
                elapsed: timeout,
            }
        );
    }
    assert_eq!(RSN_STA_MESSAGE1_TIMEOUT, Duration::from_secs(3));
    assert_eq!(RSN_STA_MESSAGE3_TIMEOUT, Duration::from_secs(6));
}

#[test]
fn a_response_deadline_past_the_time_range_is_refused() {
    let late = oer_portable_time::Instant::from_micros(u64::MAX);
    assert_eq!(
        RsnStaResponseDeadline::start(RsnStaResponseWait::Message1, late),
        None
    );
}

/// A handshake past Message 1 and the encrypted Message 3 it now awaits.
fn awaiting_encrypted_message3() -> (RsnStaHandshake, crate::OwnedEapolFrame<512>, Pmk, Ptk) {
    let pmk = test_pmk();
    let ptk = pmk.derive_ptk(crate::Akm::Psk, context());
    let mut handshake = handshake();
    let message1 = RsnTxFrame::<512>::message1(crate::Akm::Psk, LOCAL, 1, ANONCE).unwrap();
    assert!(matches!(
        process(&mut handshake, owned(&message1), &pmk).unwrap(),
        RsnStaEapol::Transmit(_)
    ));
    let message3 = encrypted_message3(&ptk, [0; 8], message3_plain().as_bytes());
    (handshake, message3, pmk, ptk)
}

#[test]
fn message3_key_data_unwraps_in_the_step_into_install() {
    let (mut handshake, message3, pmk, _) = awaiting_encrypted_message3();
    let RsnStaEapol::Install(install) = process(&mut handshake, message3.clone(), &pmk).unwrap()
    else {
        panic!("initial key installation")
    };
    assert_eq!(install.replay_counter(), 2);
    assert_eq!(install.group().key().as_bytes(), &[0x5a; 16]);
    // While its keys are installed, a repeat of the same Message 3 does
    // nothing, an older one is stale and a newer one unexpected.
    assert!(matches!(
        process(&mut handshake, message3.clone(), &pmk),
        Ok(RsnStaEapol::None)
    ));
    let ptk = test_pmk().derive_ptk(crate::Akm::Psk, context());
    let at = |counter: u64| {
        resign_eapol_key_variant(&message3, &ptk, Mic::Resigned, |bytes| {
            bytes[9..17].copy_from_slice(&counter.to_be_bytes());
        })
    };
    assert!(matches!(
        process(&mut handshake, at(1), &pmk),
        Err(RsnStaError::Rejected(RsnStateError::StaleReplayCounter))
    ));
    assert!(matches!(
        process(&mut handshake, at(3), &pmk),
        Err(RsnStaError::Rejected(RsnStateError::UnexpectedMessage))
    ));
    // A Message 1 at or below the accepted one does nothing; a newer one is
    // unexpected: the PTK being installed is not replaced.
    let message1 = |counter| {
        owned(&RsnTxFrame::<512>::message1(crate::Akm::Psk, LOCAL, counter, [9; 32]).unwrap())
    };
    assert!(matches!(
        process(&mut handshake, message1(1), &pmk),
        Ok(RsnStaEapol::None)
    ));
    assert!(matches!(
        process(&mut handshake, message1(5), &pmk),
        Err(RsnStaError::Rejected(RsnStateError::UnexpectedMessage))
    ));
    let connected = handshake
        .complete::<512, _>(installed(install), &mut SoftwareAes128)
        .unwrap();
    assert_eq!(connected.message4.key_frame().unwrap().replay_counter(), 2);
}

#[test]
fn failed_message3_key_data_unwrap_ends_the_handshake_with_the_engine_error() {
    let (mut handshake, message3, pmk, _) = awaiting_encrypted_message3();
    // Under PSK the MIC is HMAC-SHA1 and takes no engine: only the unwrap
    // meets the refusal.
    assert!(matches!(
        handshake.on_eapol(message3, &pmk, &mut Refusing),
        Ok(RsnStaEapol::End(RsnStaEnd::KeyUnwrap(REFUSED)))
    ));
}

#[test]
fn message3_key_data_beyond_the_bound_ends_the_handshake() {
    let (mut handshake, _, pmk, ptk) = awaiting_encrypted_message3();
    let long = [0x11; RSN_KEY_DATA_CAPACITY + 8];
    let message3 = RsnTxFrame::<1024>::message3(crate::Akm::Psk, LOCAL, 2, ANONCE, [0; 8], &long)
        .unwrap()
        .authenticate(&ptk, &mut SoftwareAes128)
        .unwrap();
    let message3 =
        crate::OwnedEapolFrame::<1024>::try_copy(RsnInterface::Station, AP, message3.as_bytes())
            .unwrap();
    assert!(matches!(
        handshake.on_eapol(message3, &pmk, &mut SoftwareAes128),
        Ok(RsnStaEapol::End(RsnStaEnd::KeyDataTooLong))
    ));
}

#[test]
fn message3_key_data_that_does_not_parse_ends_the_handshake() {
    let (mut handshake, _, pmk, ptk) = awaiting_encrypted_message3();
    let message3 = encrypted_message3(&ptk, [0; 8], &[0; 24]);
    assert!(matches!(
        process(&mut handshake, message3, &pmk),
        Ok(RsnStaEapol::End(RsnStaEnd::KeyData(_)))
    ));
}

#[test]
fn an_authenticated_group_frame_waits_without_blocking_new_authentication() {
    let pmk = {
        let mut derivation = crate::PmkDerivation::new(b"password", b"ssid").unwrap();
        loop {
            if let Some(pmk) = derivation.step() {
                break pmk;
            }
        }
    };
    let ptk = pmk.derive_ptk(crate::Akm::Psk, context());
    let frame = RsnTxFrame::<512>::group_message1(
        crate::Akm::Psk,
        LOCAL,
        3,
        [9; 8],
        group_key_data(&ptk).as_bytes(),
    )
    .unwrap()
    .authenticate(&ptk, &mut SoftwareAes128)
    .unwrap();
    let mut connected = connected(pmk.derive_ptk(crate::Akm::Psk, context()));
    let RsnConnectedAction::GroupKeyData(waiting) = connected
        .on_group_message1(owned(&frame), &mut SoftwareAes128)
        .unwrap()
    else {
        panic!("new Group Message 1 must wait for its key-data unwrap")
    };
    assert_eq!(waiting.replay_counter(), 3);
    let newer = RsnTxFrame::<512>::group_message1(crate::Akm::Psk, LOCAL, 4, [9; 8], &[0x66; 24])
        .unwrap()
        .authenticate(&ptk, &mut SoftwareAes128)
        .unwrap();
    let RsnConnectedAction::GroupKeyData(newer_waiting) = connected
        .on_group_message1(owned(&newer), &mut SoftwareAes128)
        .unwrap()
    else {
        panic!("a newer frame must authenticate while the old one waits")
    };
    assert_eq!(newer_waiting.replay_counter(), 4);

    let install = connected
        .prepare_group_key_install(waiting, &mut SoftwareAes128)
        .unwrap();
    assert_eq!(install.replay_counter(), 3);
    assert_eq!(install.group().key().as_bytes(), &[0x6a; 16]);
    connected.complete_group_key_install(install).unwrap();
    assert_eq!(connected.replay_counter(), 3);
}

#[test]
fn a_waiting_group_frame_behind_the_frontier_is_stale() {
    let pmk = {
        let mut derivation = crate::PmkDerivation::new(b"password", b"ssid").unwrap();
        loop {
            if let Some(pmk) = derivation.step() {
                break pmk;
            }
        }
    };
    let ptk = pmk.derive_ptk(crate::Akm::Psk, context());
    let frame = |counter| {
        RsnTxFrame::<512>::group_message1(
            crate::Akm::Psk,
            LOCAL,
            counter,
            [9; 8],
            group_key_data(&ptk).as_bytes(),
        )
        .unwrap()
        .authenticate(&ptk, &mut SoftwareAes128)
        .unwrap()
    };
    let mut connected = connected(pmk.derive_ptk(crate::Akm::Psk, context()));
    let RsnConnectedAction::GroupKeyData(older) = connected
        .on_group_message1(owned(&frame(3)), &mut SoftwareAes128)
        .unwrap()
    else {
        panic!()
    };
    let GroupStep::Install(newer) = group_message1(&mut connected, owned(&frame(4))).unwrap()
    else {
        panic!()
    };
    connected.complete_group_key_install(newer).unwrap();
    assert!(matches!(
        connected.prepare_group_key_install(older, &mut SoftwareAes128),
        Err(RsnConnectedProcessError::Supplicant(
            RsnConnectedSupplicantError::StaleCompletion
        ))
    ));
}

#[test]
fn failed_group_key_data_unwrap_returns_the_error_and_releases_the_supplicant() {
    let pmk = {
        let mut derivation = crate::PmkDerivation::new(b"password", b"ssid").unwrap();
        loop {
            if let Some(pmk) = derivation.step() {
                break pmk;
            }
        }
    };
    let ptk = pmk.derive_ptk(crate::Akm::Psk, context());
    let frame = RsnTxFrame::<512>::group_message1(
        crate::Akm::Psk,
        LOCAL,
        3,
        [9; 8],
        group_key_data(&ptk).as_bytes(),
    )
    .unwrap()
    .authenticate(&ptk, &mut SoftwareAes128)
    .unwrap();
    let mut connected = connected(pmk.derive_ptk(crate::Akm::Psk, context()));

    let RsnConnectedAction::GroupKeyData(waiting) = connected
        .on_group_message1(owned(&frame), &mut SoftwareAes128)
        .unwrap()
    else {
        panic!("new Group Message 1 must wait for its key-data unwrap")
    };
    assert!(matches!(
        connected.prepare_group_key_install(waiting, &mut Refusing),
        Err(RsnConnectedProcessError::KeyUnwrap(REFUSED))
    ));
    assert_eq!(connected.replay_counter(), 2);

    // Key data that fails its integrity check retains nothing.
    let forged = RsnTxFrame::<512>::group_message1(crate::Akm::Psk, LOCAL, 3, [9; 8], &[0x55; 24])
        .unwrap()
        .authenticate(&ptk, &mut SoftwareAes128)
        .unwrap();
    let RsnConnectedAction::GroupKeyData(waiting) = connected
        .on_group_message1(owned(&forged), &mut SoftwareAes128)
        .unwrap()
    else {
        panic!("a failed unwrap must leave the frame eligible for retry")
    };
    assert!(matches!(
        connected.prepare_group_key_install(waiting, &mut SoftwareAes128),
        Err(RsnConnectedProcessError::KeyUnwrap(
            oer_portable_crypto::KeyUnwrapError::IntegrityCheckFailed
        ))
    ));

    // Undecodable plaintext fails as a frame error and retains nothing.
    let undecodable = RsnTxFrame::<512>::group_message1(
        crate::Akm::Psk,
        LOCAL,
        3,
        [9; 8],
        software_wrap(ptk.kek(), &[0; 24]).unwrap().as_bytes(),
    )
    .unwrap()
    .authenticate(&ptk, &mut SoftwareAes128)
    .unwrap();
    let RsnConnectedAction::GroupKeyData(waiting) = connected
        .on_group_message1(owned(&undecodable), &mut SoftwareAes128)
        .unwrap()
    else {
        panic!()
    };
    assert!(matches!(
        connected.prepare_group_key_install(waiting, &mut SoftwareAes128),
        Err(RsnConnectedProcessError::Supplicant(
            RsnConnectedSupplicantError::Frame(_)
        ))
    ));
    assert!(matches!(
        connected.on_group_message1(owned(&frame), &mut SoftwareAes128),
        Ok(RsnConnectedAction::GroupKeyData(_))
    ));
}

#[test]
fn retained_authenticator_ies_fail_locally_during_initial_key_install() {
    let (mut handshake, message3, pmk, _) = awaiting_encrypted_message3();
    handshake.invalidate_security_ies_for_test();
    assert_eq!(
        process(&mut handshake, message3, &pmk).err(),
        Some(RsnStaError::RetainedSecurityIes(
            RsnFrameError::InvalidRsnIe
        ))
    );
}

#[test]
fn retained_authenticator_ies_fail_locally_during_connected_key_install() {
    let (mut handshake, message3, pmk, ptk) = awaiting_encrypted_message3();
    let RsnStaEapol::Install(request) = process(&mut handshake, message3, &pmk).unwrap() else {
        panic!()
    };
    let mut connected = handshake
        .complete::<512, _>(installed(request), &mut SoftwareAes128)
        .unwrap()
        .supplicant;
    let rsn = OwnedRsnIe::<64>::try_copy(&RSN).unwrap();
    let gtk = RsnGtk::new(
        oer_protocols_ieee80211_wire::ccmp::GroupKeyId::new(2).unwrap(),
        oer_protocols_ieee80211_wire::eapol::GtkTx::ReceiveOnly,
        &[0x5a; 16],
    );
    let plain = RsnPlainKeyData::<64>::build(rsn.as_bytes(), &gtk, None).unwrap();
    let message3 = encrypted_message3(&ptk, [0; 8], plain.as_bytes());
    let retry = resign_eapol_key_variant(&message3, &ptk, Mic::Resigned, |bytes| bytes[16] = 3);
    let RsnConnectedAction::GroupKeyData(waiting) =
        connected.on_message3(retry, &mut SoftwareAes128).unwrap()
    else {
        panic!()
    };
    connected.invalidate_security_ies_for_test();
    assert!(matches!(
        connected.prepare_group_key_install(waiting, &mut SoftwareAes128),
        Err(RsnConnectedProcessError::Supplicant(
            RsnConnectedSupplicantError::RetainedSecurityIes(RsnFrameError::InvalidRsnIe)
        ))
    ));
}

#[test]
fn management_frames_are_protected_only_when_both_elements_are_capable() {
    let mut capable = RSN;
    capable[20] = 0x80; // MFPC
    let bip = Some(oer_protocols_ieee80211_wire::security::GroupManagementCipher::BipCmac128);
    for (own, authenticator, expected) in [
        (&capable, &capable, bip),
        (&capable, &RSN, None),
        (&RSN, &capable, None),
        (&RSN, &RSN, None),
    ] {
        let handshake = RsnStaHandshake::new(LOCAL, AP, SNONCE, own, authenticator, &[]).unwrap();
        assert_eq!(handshake.group_management(), expected);
    }
}

/// RFC 3394 wrap over the software engine, as a composition runs it.
fn software_wrap(
    kek: &[u8; 16],
    plaintext: &[u8],
) -> Result<
    oer_protocols_ieee80211_core_security_cipher::aes::RsnKeyData,
    oer_portable_crypto::rfc3394::KeyWrapError,
> {
    let plaintext = oer_portable_crypto::KeyData::try_copy(plaintext)
        .map_err(|_| oer_portable_crypto::rfc3394::KeyWrapError::CapacityExceeded)?;
    oer_portable_crypto::block::wrap_now(
        &mut oer_protocols_ieee80211_core_security_cipher::aes::SoftwareAes128,
        oer_portable_crypto::KeyWrapRequest::new(
            oer_portable_crypto::Aes128Key::copy_from(kek),
            plaintext,
        ),
    )
    .into_wrapped()
}

/// A repeat of the accepted Message 1 answers Message 2 again; a newer one
/// restarts with its ANonce and PTK; an older one is stale.
#[test]
fn a_repeated_message1_rebuilds_message2_and_a_newer_one_restarts() {
    let pmk = test_pmk();
    let mut handshake = handshake();
    let message1 = |counter, nonce| {
        owned(&RsnTxFrame::<512>::message1(crate::Akm::Psk, LOCAL, counter, nonce).unwrap())
    };
    let RsnStaEapol::Transmit(first) = process(&mut handshake, message1(5, ANONCE), &pmk).unwrap()
    else {
        panic!("Message 1 builds Message 2")
    };
    let RsnStaEapol::Transmit(again) = process(&mut handshake, message1(5, ANONCE), &pmk).unwrap()
    else {
        panic!("a repeated Message 1 builds Message 2 again")
    };
    assert!(again.retransmission());
    assert_eq!(again.as_bytes(), first.as_bytes());
    assert!(matches!(
        process(&mut handshake, message1(4, ANONCE), &pmk),
        Err(RsnStaError::Rejected(RsnStateError::StaleReplayCounter))
    ));
    let newer_nonce = [0x44; 32];
    let RsnStaEapol::Transmit(restart) =
        process(&mut handshake, message1(6, newer_nonce), &pmk).unwrap()
    else {
        panic!("a newer Message 1 restarts")
    };
    assert!(!restart.retransmission());
    let newer_ptk = pmk.derive_ptk(
        crate::Akm::Psk,
        PtkContext {
            authenticator_nonce: newer_nonce,
            ..context()
        },
    );
    assert!(
        restart
            .key_frame()
            .unwrap()
            .verify_mic(&newer_ptk, &mut SoftwareAes128)
            .unwrap()
    );
    // Message 3 of the first ANonce no longer matches.
    let message3 = encrypted_message3(
        &pmk.derive_ptk(crate::Akm::Psk, context()),
        [0; 8],
        message3_plain().as_bytes(),
    );
    assert!(matches!(
        process(&mut handshake, message3, &pmk),
        Err(RsnStaError::Rejected(
            RsnStateError::AuthenticatorNonceMismatch
        ))
    ));
}

/// A zero nonce and an element that is not one make no handshake.
#[test]
fn new_refuses_a_zero_nonce_and_invalid_elements() {
    assert_eq!(
        RsnStaHandshake::new(LOCAL, AP, [0; 32], &RSN, &RSN, &[]).err(),
        Some(RsnStaNewError::ZeroNonce)
    );
    assert!(matches!(
        RsnStaHandshake::new(LOCAL, AP, SNONCE, &[0xdd, 0], &RSN, &[]),
        Err(RsnStaNewError::Frame(_))
    ));
    assert!(matches!(
        RsnStaHandshake::new(LOCAL, AP, SNONCE, &RSN, &[0x30, 2, 1, 0], &[]),
        Err(RsnStaNewError::Element(_))
    ));
}

/// Message 3 before any Message 1, or at the accepted Message 1's counter,
/// is refused and changes nothing.
#[test]
fn message3_is_bound_to_an_accepted_message1() {
    let pmk = test_pmk();
    let ptk = pmk.derive_ptk(crate::Akm::Psk, context());
    let mut fresh = handshake();
    assert!(matches!(
        process(
            &mut fresh,
            encrypted_message3(&ptk, [0; 8], message3_plain().as_bytes()),
            &pmk
        ),
        Err(RsnStaError::Rejected(RsnStateError::UnexpectedMessage))
    ));
    let (mut handshake, message3, pmk, ptk) = awaiting_encrypted_message3();
    let at_message1 = resign_eapol_key_variant(&message3, &ptk, Mic::Resigned, |bytes| {
        bytes[9..17].copy_from_slice(&1_u64.to_be_bytes());
    });
    assert!(matches!(
        process(&mut handshake, at_message1, &pmk),
        Err(RsnStaError::Rejected(RsnStateError::StaleReplayCounter))
    ));
    assert!(matches!(
        process(&mut handshake, message3, &pmk),
        Ok(RsnStaEapol::Install(_))
    ));
}

/// An engine that refuses Message 4's MIC ends the handshake with that
/// cause. PSK-SHA256 computes its MICs with AES-CMAC on the engine.
#[test]
fn a_refused_message4_mic_ends_the_handshake() {
    let mut rsn = RSN;
    rsn[19] = 6; // PSK-SHA256
    let pmk = test_pmk();
    let ptk = pmk.derive_ptk(crate::Akm::PskSha256, context());
    let mut handshake = RsnStaHandshake::new(LOCAL, AP, SNONCE, &rsn, &rsn, &[]).unwrap();
    let message1 = RsnTxFrame::<512>::message1(crate::Akm::PskSha256, LOCAL, 1, ANONCE).unwrap();
    assert!(matches!(
        process(&mut handshake, owned(&message1), &pmk),
        Ok(RsnStaEapol::Transmit(_))
    ));
    let rsn_ie = OwnedRsnIe::<64>::try_copy(&rsn).unwrap();
    let gtk = RsnGtk::new(
        oer_protocols_ieee80211_wire::ccmp::GroupKeyId::new(2).unwrap(),
        oer_protocols_ieee80211_wire::eapol::GtkTx::ReceiveOnly,
        &[0x5a; 16],
    );
    let plain = RsnPlainKeyData::<64>::build(rsn_ie.as_bytes(), &gtk, None).unwrap();
    let wrapped = software_wrap(ptk.kek(), plain.as_bytes()).unwrap();
    let message3 = RsnTxFrame::<512>::message3(
        crate::Akm::PskSha256,
        LOCAL,
        2,
        ANONCE,
        [0; 8],
        wrapped.as_bytes(),
    )
    .unwrap()
    .authenticate(&ptk, &mut SoftwareAes128)
    .unwrap();
    let RsnStaEapol::Install(request) = process(&mut handshake, owned(&message3), &pmk).unwrap()
    else {
        panic!("Message 3 installs")
    };
    assert!(matches!(
        handshake.complete::<512, _>(installed(request), &mut Refusing),
        Err(RsnStaEnd::Engine(_))
    ));
}
