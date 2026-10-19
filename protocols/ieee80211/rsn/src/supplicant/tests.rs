use hmac::Mac;

use super::*;
use crate::{
    aes::{
        RsnUnwrappedKeyData, SoftwareAesKeyUnwrapError, software_aes128_key_unwrap,
        software_aes128_key_wrap,
    },
    frames::{RsnGtk, RsnPlainKeyData},
    keys::RsnKeyKind,
};

const LOCAL: [u8; 6] = [1; 6];
const AP: [u8; 6] = [2; 6];
const SNONCE: [u8; 32] = [3; 32];
const ANONCE: [u8; 32] = [4; 32];
const RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];

/// Process one frame, completing a requested key-data unwrap in software.
fn process(
    supplicant: &mut RsnStaSupplicant,
    frame: crate::OwnedEapolFrame<512>,
    pmk: &Pmk,
) -> Result<RsnStaReady<512>, RsnStaProcessError<SoftwareAesKeyUnwrapError>> {
    match supplicant.on_frame(frame, pmk)? {
        RsnStaSupplicantAction::UnwrapKeyData(request) => {
            let unwrapped = software_aes128_key_unwrap(request.kek(), request.wrapped_key_data());
            supplicant.complete_key_data_unwrap(request, unwrapped)
        }
        RsnStaSupplicantAction::Ready(action) => Ok(action),
    }
}

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
    let wrapped = software_aes128_key_wrap(ptk.kek(), plain_key_data).unwrap();
    let frame = RsnTxFrame::<512>::message3(
        crate::Akm::Psk,
        LOCAL,
        2,
        ANONCE,
        key_rsc,
        wrapped.as_bytes(),
    )
    .unwrap()
    .authenticate(ptk);
    owned(&frame)
}

/// The plaintext a group key-data unwrap yields in these tests.
struct GroupKeyUnwrap {
    plain: [u8; 24],
}

enum GroupStep {
    Install(RsnGroupKeyInstallRequest<512>),
    Retransmit(RsnTxFrame<512>),
}

/// Process one Group Message 1, completing a requested unwrap with `unwrap`.
fn group_message1(
    connected: &mut RsnConnectedSupplicant,
    frame: crate::OwnedEapolFrame<512>,
    unwrap: &GroupKeyUnwrap,
) -> Result<GroupStep, RsnConnectedProcessError<()>> {
    match connected
        .on_group_message1(frame)
        .map_err(RsnConnectedProcessError::Supplicant)?
    {
        RsnConnectedAction::Retransmit(response) => Ok(GroupStep::Retransmit(response)),
        RsnConnectedAction::UnwrapGroupKeyData(request) => connected
            .complete_group_key_data_unwrap(
                request,
                Ok(RsnUnwrappedKeyData::try_copy(&unwrap.plain).unwrap()),
            )
            .map(GroupStep::Install),
    }
}

fn connected(ptk: Ptk) -> RsnConnectedSupplicant {
    let message3 =
        RsnTxFrame::<512>::message3(crate::Akm::Psk, LOCAL, 2, ANONCE, [0; 8], &[0x55; 24])
            .unwrap()
            .authenticate(&ptk);
    let completed_message3 = RsnCompletedMessage3::capture(message3.key_frame());
    let (key_confirmation, key_encryption) = ptk.into_connected_keys();
    RsnConnectedSupplicant {
        authenticator: AP,
        key_confirmation,
        key_encryption,
        authenticator_security_ies: OwnedAssociationSecurityIes::try_copy_bytes(&RSN).unwrap(),
        completed_message3,
        replay_counter: 2,
        completed_group_message1: None,
        management_protection: false,
    }
}

fn resign_eapol_key_variant(
    original: &crate::OwnedEapolFrame<512>,
    ptk: &Ptk,
    resign: bool,
    mutate: impl FnOnce(&mut [u8]),
) -> crate::OwnedEapolFrame<512> {
    let mut bytes = [0; 512];
    let len = original.as_bytes().len();
    bytes[..len].copy_from_slice(original.as_bytes());
    mutate(&mut bytes[..len]);
    if resign {
        bytes[81..97].fill(0);
        let mut mac = hmac::Hmac::<sha1::Sha1>::new_from_slice(ptk.kck()).unwrap();
        mac.update(&bytes[..len]);
        let digest = mac.finalize().into_bytes();
        bytes[81..97].copy_from_slice(&digest[..16]);
    }
    crate::OwnedEapolFrame::try_copy(RsnInterface::Station, AP, &bytes[..len]).unwrap()
}

fn completed_connected() -> (RsnConnectedSupplicant, crate::OwnedEapolFrame<512>, Ptk) {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let expected_ptk = pmk.derive_ptk(crate::Akm::Psk, context());
    let mut supplicant = RsnStaSupplicant::try_new(LOCAL, AP, SNONCE, &RSN, &RSN, &[]).unwrap();
    let message1 = RsnTxFrame::<512>::message1(crate::Akm::Psk, LOCAL, 1, ANONCE).unwrap();
    assert!(matches!(
        process(&mut supplicant, owned(&message1), &pmk).unwrap(),
        RsnStaReady::Transmit(_)
    ));
    let rsn = OwnedRsnIe::<64>::try_copy(&RSN).unwrap();
    let gtk = RsnGtk::new(
        oer_ieee80211_mac::ccmp::GroupKeyId::new(2).unwrap(),
        false,
        &[0x5a; 16],
    );
    let plain = RsnPlainKeyData::<64>::build(rsn.as_bytes(), &gtk, None).unwrap();
    let message3 = encrypted_message3(&expected_ptk, [7, 6, 5, 4, 3, 2, 1, 0], plain.as_bytes());
    let duplicate = message3.clone();
    let RsnStaReady::InstallKeys(request) = process(&mut supplicant, message3, &pmk).unwrap()
    else {
        panic!("Message 3 must produce one key transaction")
    };
    let RsnStaReady::Transmit(_) = supplicant
        .complete_key_install::<512>(request, true)
        .unwrap()
    else {
        panic!("installed keys must produce Message 4")
    };
    (
        supplicant.into_connected().unwrap(),
        duplicate,
        expected_ptk,
    )
}

fn group_kde(key_id: u8, key: u8) -> [u8; 24] {
    let mut kde = [0; 24];
    kde[..8].copy_from_slice(&[0xdd, 22, 0, 0x0f, 0xac, 1, key_id, 0]);
    kde[8..].fill(key);
    kde
}

#[test]
fn resolves_m1_through_typed_install_and_authenticated_m4() {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let expected_ptk = pmk.derive_ptk(crate::Akm::Psk, context());
    let mut supplicant = RsnStaSupplicant::try_new(LOCAL, AP, SNONCE, &RSN, &RSN, &[]).unwrap();

    let message1 = RsnTxFrame::<512>::message1(crate::Akm::Psk, LOCAL, 1, ANONCE).unwrap();
    let RsnStaReady::Transmit(message2) = process(&mut supplicant, owned(&message1), &pmk).unwrap()
    else {
        panic!("Message 1 must produce Message 2")
    };
    assert_eq!(message2.key_frame().replay_counter(), 1);
    assert!(message2.key_frame().verify_mic(&expected_ptk));

    let rsn = OwnedRsnIe::<64>::try_copy(&RSN).unwrap();
    let gtk = RsnGtk::new(
        oer_ieee80211_mac::ccmp::GroupKeyId::new(2).unwrap(),
        false,
        &[0x5a; 16],
    );
    let plain = RsnPlainKeyData::<64>::build(rsn.as_bytes(), &gtk, None).unwrap();
    let rsc = [7, 6, 5, 4, 3, 2, 1, 0];
    let message3 = encrypted_message3(&expected_ptk, rsc, plain.as_bytes());
    let RsnStaReady::InstallKeys(request) = process(&mut supplicant, message3, &pmk).unwrap()
    else {
        panic!("Message 3 must produce one key transaction")
    };
    assert_eq!(request.replay_counter(), 2);
    assert!(request.encrypted_key_data());
    assert_eq!(request.plain_key_data_len(), plain.as_bytes().len());
    assert_eq!(request.pairwise().peer(), &AP);
    assert_eq!(
        request.pairwise().key().as_bytes(),
        expected_ptk.temporal_key()
    );
    assert_eq!(
        request.group().kind(),
        RsnKeyKind::Group {
            key_id: oer_ieee80211_mac::ccmp::GroupKeyId::new(2).unwrap(),
            transmit: false,
        }
    );
    assert_eq!(request.group().receive_sequence(), &rsc);
    assert_eq!(request.group().key().as_bytes(), &[0x5a; 16]);

    let RsnStaReady::Transmit(message4) = supplicant
        .complete_key_install::<512>(request, true)
        .unwrap()
    else {
        panic!("installed keys must produce Message 4")
    };
    assert_eq!(supplicant.phase(), RsnStaPhase::Completed);
    assert_eq!(message4.key_frame().replay_counter(), 2);
    assert!(message4.key_frame().verify_mic(&expected_ptk));
}

#[test]
fn invalid_message3_mic_is_ignored_and_valid_retry_can_install() {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let expected_ptk = pmk.derive_ptk(crate::Akm::Psk, context());
    let mut supplicant = RsnStaSupplicant::try_new(LOCAL, AP, SNONCE, &RSN, &RSN, &[]).unwrap();
    let message1 = RsnTxFrame::<512>::message1(crate::Akm::Psk, LOCAL, 1, ANONCE).unwrap();
    assert!(matches!(
        process(&mut supplicant, owned(&message1), &pmk).unwrap(),
        RsnStaReady::Transmit(_)
    ));
    let rsn = OwnedRsnIe::<64>::try_copy(&RSN).unwrap();
    let gtk = RsnGtk::new(
        oer_ieee80211_mac::ccmp::GroupKeyId::new(1).unwrap(),
        false,
        &[9; 16],
    );
    let plain = RsnPlainKeyData::<64>::build(rsn.as_bytes(), &gtk, None).unwrap();
    let message3 = encrypted_message3(&expected_ptk, [0; 8], plain.as_bytes());
    let mut bytes = [0; 512];
    let len = message3.as_bytes().len();
    bytes[..len].copy_from_slice(message3.as_bytes());
    bytes[81] ^= 1;
    let changed =
        crate::OwnedEapolFrame::<512>::try_copy(RsnInterface::Station, AP, &bytes[..len]).unwrap();
    assert!(matches!(
        process(&mut supplicant, changed, &pmk),
        Err(RsnStaProcessError::Supplicant(
            RsnStaSupplicantError::InvalidMessage3Mic
        ))
    ));
    assert_eq!(supplicant.phase(), RsnStaPhase::AwaitingMessage3);

    let valid_retry = encrypted_message3(&expected_ptk, [0; 8], plain.as_bytes());
    let RsnStaReady::InstallKeys(request) = process(&mut supplicant, valid_retry, &pmk).unwrap()
    else {
        panic!("valid M3 retry must retain the original join")
    };
    assert_eq!(request.replay_counter(), 2);
    assert_eq!(supplicant.phase(), RsnStaPhase::InstallingKeys);
}

#[test]
fn connected_exact_duplicate_message3_retransmits_authenticated_m4() {
    let (mut connected, duplicate, ptk) = completed_connected();

    let RsnConnectedAction::Retransmit(message4) = connected.on_message3(duplicate).unwrap() else {
        panic!("exact repeat produces M4")
    };

    assert_eq!(
        message4.key_frame().message(),
        crate::EapolKeyMessage::PairwiseMessage4
    );
    assert_eq!(message4.key_frame().replay_counter(), 2);
    assert!(message4.retransmission());
    assert!(message4.key_frame().verify_mic(&ptk));
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
        connected.on_message3(wrong_interface),
        Err(RsnConnectedSupplicantError::WrongInterface)
    ));
    let wrong_peer = crate::OwnedEapolFrame::<512>::try_copy(
        RsnInterface::Station,
        [9; 6],
        duplicate.as_bytes(),
    )
    .unwrap();
    assert!(matches!(
        connected.on_message3(wrong_peer),
        Err(RsnConnectedSupplicantError::WrongPeer)
    ));

    let wrong_protocol = resign_eapol_key_variant(&duplicate, &ptk, true, |bytes| {
        bytes[0] ^= 1;
    });
    assert!(matches!(
        connected.on_message3(wrong_protocol),
        Err(RsnConnectedSupplicantError::ProtocolVersionMismatch)
    ));

    let wrong_descriptor = resign_eapol_key_variant(&duplicate, &ptk, true, |bytes| {
        let key_info = u16::from_be_bytes([bytes[5], bytes[6]]);
        bytes[5..7].copy_from_slice(&((key_info & !0x0007) | 1).to_be_bytes());
    });
    assert!(matches!(
        connected.on_message3(wrong_descriptor),
        Err(RsnConnectedSupplicantError::UnsupportedDescriptorVersion)
    ));

    let wrong_flags = resign_eapol_key_variant(&duplicate, &ptk, true, |bytes| {
        let key_info = u16::from_be_bytes([bytes[5], bytes[6]]) ^ (1 << 9);
        bytes[5..7].copy_from_slice(&key_info.to_be_bytes());
    });
    assert!(matches!(
        connected.on_message3(wrong_flags),
        Err(RsnConnectedSupplicantError::UnsupportedMessage)
    ));

    let wrong_replay = resign_eapol_key_variant(&duplicate, &ptk, true, |bytes| {
        bytes[9..17].copy_from_slice(&1_u64.to_be_bytes());
    });
    assert!(matches!(
        connected.on_message3(wrong_replay),
        Err(RsnConnectedSupplicantError::StaleReplayCounter)
    ));

    let wrong_anonce = resign_eapol_key_variant(&duplicate, &ptk, true, |bytes| {
        bytes[17] ^= 1;
    });
    assert!(matches!(
        connected.on_message3(wrong_anonce),
        Err(RsnConnectedSupplicantError::AuthenticatorNonceMismatch)
    ));
}

#[test]
fn connected_duplicate_message3_rejects_bad_mic_and_changed_commitment() {
    let (mut connected, duplicate, ptk) = completed_connected();

    let bad_mic = resign_eapol_key_variant(&duplicate, &ptk, false, |bytes| {
        bytes[81] ^= 1;
    });
    assert!(matches!(
        connected.on_message3(bad_mic),
        Err(RsnConnectedSupplicantError::InvalidMic)
    ));

    let changed_rsc = resign_eapol_key_variant(&duplicate, &ptk, true, |bytes| {
        bytes[65] ^= 1;
    });
    assert!(matches!(
        connected.on_message3(changed_rsc),
        Err(RsnConnectedSupplicantError::RetainedMessage3Mismatch)
    ));

    let changed_key_data = resign_eapol_key_variant(&duplicate, &ptk, true, |bytes| {
        bytes[99] ^= 1;
    });
    assert!(matches!(
        connected.on_message3(changed_key_data),
        Err(RsnConnectedSupplicantError::RetainedMessage3Mismatch)
    ));
}

#[test]
fn a_newer_completed_group_request_cannot_be_rolled_back_by_an_older_owner() {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let ptk = pmk.derive_ptk(crate::Akm::Psk, context());
    let mut connected = connected(pmk.derive_ptk(crate::Akm::Psk, context()));
    let frame = |counter| {
        RsnTxFrame::<512>::group_message1(crate::Akm::Psk, LOCAL, counter, [9; 8], &[0x55; 24])
            .unwrap()
            .authenticate(&ptk)
    };
    let unwrap = GroupKeyUnwrap {
        plain: group_kde(1, 0x6a),
    };
    let GroupStep::Install(older) =
        group_message1(&mut connected, owned(&frame(3)), &unwrap).unwrap()
    else {
        panic!()
    };
    let GroupStep::Install(newer) =
        group_message1(&mut connected, owned(&frame(4)), &unwrap).unwrap()
    else {
        panic!()
    };
    let response = connected.complete_group_key_install(newer, true).unwrap();
    assert_eq!(response.key_frame().replay_counter(), 4);
    assert!(matches!(
        connected.complete_group_key_install(older, true),
        Err(RsnConnectedSupplicantError::StaleCompletion)
    ));
    assert_eq!(connected.replay_counter(), 4);
}

#[test]
fn connected_group_rekey_installs_once_and_retransmits_idempotently() {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let ptk = pmk.derive_ptk(crate::Akm::Psk, context());
    let frame = RsnTxFrame::<512>::group_message1(crate::Akm::Psk, LOCAL, 3, [9; 8], &[0x55; 24])
        .unwrap()
        .authenticate(&ptk);
    let mut connected = connected(pmk.derive_ptk(crate::Akm::Psk, context()));
    let unwrap = GroupKeyUnwrap {
        plain: group_kde(1, 0x6a),
    };
    let GroupStep::Install(request) =
        group_message1(&mut connected, owned(&frame), &unwrap).unwrap()
    else {
        panic!("new Group Message 1 must request one GTK replacement")
    };
    assert_eq!(request.replay_counter(), 3);
    assert_eq!(
        request.group().kind(),
        RsnKeyKind::Group {
            key_id: oer_ieee80211_mac::ccmp::GroupKeyId::new(1).unwrap(),
            transmit: false,
        }
    );
    assert_eq!(request.group().key().as_bytes(), &[0x6a; 16]);
    let response = connected.complete_group_key_install(request, true).unwrap();
    assert_eq!(
        response.key_frame().message(),
        crate::EapolKeyMessage::GroupMessage2
    );
    assert!(response.key_frame().verify_mic(&ptk));
    assert_eq!(connected.replay_counter(), 3);

    let GroupStep::Retransmit(repeated) =
        group_message1(&mut connected, owned(&frame), &unwrap).unwrap()
    else {
        panic!("repeated Group Message 1 must not reinstall GTK")
    };
    assert!(repeated.key_frame().verify_mic(&ptk));
}

#[test]
fn connected_group_rekey_authenticates_duplicate_before_cached_response() {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let ptk = pmk.derive_ptk(crate::Akm::Psk, context());
    let frame = RsnTxFrame::<512>::group_message1(crate::Akm::Psk, LOCAL, 3, [9; 8], &[0x55; 24])
        .unwrap()
        .authenticate(&ptk);
    let mut connected = connected(pmk.derive_ptk(crate::Akm::Psk, context()));
    let unwrap = GroupKeyUnwrap {
        plain: group_kde(1, 0x6a),
    };
    let GroupStep::Install(request) =
        group_message1(&mut connected, owned(&frame), &unwrap).unwrap()
    else {
        panic!("new Group Message 1 must request one GTK replacement")
    };
    connected.complete_group_key_install(request, true).unwrap();

    let mut bytes = [0; 512];
    let length = frame.as_bytes().len();
    bytes[..length].copy_from_slice(frame.as_bytes());
    bytes[81] ^= 1;
    let forged_duplicate =
        crate::OwnedEapolFrame::<512>::try_copy(RsnInterface::Station, AP, &bytes[..length])
            .unwrap();
    assert!(matches!(
        group_message1(&mut connected, forged_duplicate, &unwrap),
        Err(RsnConnectedProcessError::Supplicant(
            RsnConnectedSupplicantError::InvalidMic
        ))
    ));
}

#[test]
fn connected_group_rekey_rejects_authenticated_changed_same_replay() {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let ptk = pmk.derive_ptk(crate::Akm::Psk, context());
    let frame = RsnTxFrame::<512>::group_message1(crate::Akm::Psk, LOCAL, 3, [9; 8], &[0x55; 24])
        .unwrap()
        .authenticate(&ptk);
    let original = owned(&frame);
    let mut connected = connected(pmk.derive_ptk(crate::Akm::Psk, context()));
    let unwrap = GroupKeyUnwrap {
        plain: group_kde(1, 0x6a),
    };
    let GroupStep::Install(request) =
        group_message1(&mut connected, original.clone(), &unwrap).unwrap()
    else {
        panic!("new Group Message 1 must request one GTK replacement")
    };
    connected.complete_group_key_install(request, true).unwrap();

    let changed_rsc = resign_eapol_key_variant(&original, &ptk, true, |bytes| {
        bytes[65] ^= 1;
    });
    assert!(matches!(
        group_message1(&mut connected, changed_rsc, &unwrap),
        Err(RsnConnectedProcessError::Supplicant(
            RsnConnectedSupplicantError::RetainedGroupMessage1Mismatch
        ))
    ));

    let changed_key_data = resign_eapol_key_variant(&original, &ptk, true, |bytes| {
        bytes[99] ^= 1;
    });
    assert!(matches!(
        group_message1(&mut connected, changed_key_data, &unwrap),
        Err(RsnConnectedProcessError::Supplicant(
            RsnConnectedSupplicantError::RetainedGroupMessage1Mismatch
        ))
    ));
}

#[test]
fn failed_group_rekey_does_not_retain_message1_commitment() {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let ptk = pmk.derive_ptk(crate::Akm::Psk, context());
    let frame = RsnTxFrame::<512>::group_message1(crate::Akm::Psk, LOCAL, 3, [9; 8], &[0x55; 24])
        .unwrap()
        .authenticate(&ptk);
    let mut connected = connected(pmk.derive_ptk(crate::Akm::Psk, context()));
    let unwrap = GroupKeyUnwrap {
        plain: group_kde(1, 0x6a),
    };
    let GroupStep::Install(request) =
        group_message1(&mut connected, owned(&frame), &unwrap).unwrap()
    else {
        panic!("new Group Message 1 must request one GTK replacement")
    };
    assert_eq!(
        connected.complete_group_key_install(request, false),
        Err(RsnConnectedSupplicantError::InstallFailed)
    );

    let GroupStep::Install(retry) = group_message1(&mut connected, owned(&frame), &unwrap).unwrap()
    else {
        panic!("failed publication must leave the same frame eligible for retry")
    };
    connected.complete_group_key_install(retry, true).unwrap();
    assert!(matches!(
        group_message1(&mut connected, owned(&frame), &unwrap).unwrap(),
        GroupStep::Retransmit(_)
    ));
}

#[test]
fn connected_group_rekey_rejects_bad_mic_and_stale_replay() {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let ptk = pmk.derive_ptk(crate::Akm::Psk, context());
    let valid = RsnTxFrame::<512>::group_message1(crate::Akm::Psk, LOCAL, 3, [0; 8], &[0x55; 24])
        .unwrap()
        .authenticate(&ptk);
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
    let unwrap = GroupKeyUnwrap {
        plain: group_kde(1, 0x6a),
    };
    assert!(matches!(
        group_message1(&mut connected, invalid, &unwrap),
        Err(RsnConnectedProcessError::Supplicant(
            RsnConnectedSupplicantError::InvalidMic
        ))
    ));

    let stale = RsnTxFrame::<512>::group_message1(crate::Akm::Psk, LOCAL, 1, [0; 8], &[0x55; 24])
        .unwrap()
        .authenticate(&ptk);
    assert!(matches!(
        group_message1(&mut connected, owned(&stale), &unwrap),
        Err(RsnConnectedProcessError::Supplicant(
            RsnConnectedSupplicantError::StaleReplayCounter
        ))
    ));
}

#[test]
fn response_deadlines_expire_exactly_at_their_total_wait() {
    use oer_time::{Duration, Instant};

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
    let late = oer_time::Instant::from_micros(u64::MAX);
    assert_eq!(
        RsnStaResponseDeadline::start(RsnStaResponseWait::Message1, late),
        None
    );
}

/// A supplicant past Message 1 and the encrypted Message 3 it now awaits.
fn awaiting_encrypted_message3() -> (RsnStaSupplicant, crate::OwnedEapolFrame<512>, Pmk, Ptk) {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let ptk = pmk.derive_ptk(crate::Akm::Psk, context());
    let mut supplicant = RsnStaSupplicant::try_new(LOCAL, AP, SNONCE, &RSN, &RSN, &[]).unwrap();
    let message1 = RsnTxFrame::<512>::message1(crate::Akm::Psk, LOCAL, 1, ANONCE).unwrap();
    assert!(matches!(
        supplicant.on_frame::<512>(owned(&message1), &pmk).unwrap(),
        RsnStaSupplicantAction::Ready(RsnStaReady::Transmit(_))
    ));
    let rsn = OwnedRsnIe::<64>::try_copy(&RSN).unwrap();
    let gtk = RsnGtk::new(
        oer_ieee80211_mac::ccmp::GroupKeyId::new(2).unwrap(),
        false,
        &[0x5a; 16],
    );
    let plain = RsnPlainKeyData::<64>::build(rsn.as_bytes(), &gtk, None).unwrap();
    let message3 = encrypted_message3(&ptk, [0; 8], plain.as_bytes());
    (supplicant, message3, pmk, ptk)
}

#[test]
fn message3_key_data_request_carries_the_ptk_kek_and_completes_into_install() {
    let (mut supplicant, message3, pmk, ptk) = awaiting_encrypted_message3();
    let wrapped_len = message3.key_frame().key_data().len();
    let RsnStaSupplicantAction::UnwrapKeyData(request) =
        supplicant.on_frame(message3, &pmk).unwrap()
    else {
        panic!("encrypted Message 3 key data must be requested for unwrap")
    };
    assert_eq!(supplicant.phase(), RsnStaPhase::DecryptingKeyData);
    assert_eq!(request.kek(), ptk.kek());
    assert_eq!(request.wrapped_key_data().len(), wrapped_len);
    assert_eq!(request.replay_counter(), 2);

    let unwrapped = software_aes128_key_unwrap(request.kek(), request.wrapped_key_data());
    let RsnStaReady::InstallKeys(install) = supplicant
        .complete_key_data_unwrap(request, unwrapped)
        .unwrap()
    else {
        panic!("initial key installation")
    };
    assert_eq!(install.replay_counter(), 2);
    assert!(install.encrypted_key_data());
    assert_eq!(install.group().key().as_bytes(), &[0x5a; 16]);
    assert_eq!(supplicant.phase(), RsnStaPhase::InstallingKeys);
}

#[test]
fn failed_message3_key_data_unwrap_fails_the_handshake_with_the_backend_error() {
    let (mut supplicant, message3, pmk, _) = awaiting_encrypted_message3();
    let RsnStaSupplicantAction::UnwrapKeyData(request) =
        supplicant.on_frame(message3, &pmk).unwrap()
    else {
        panic!("encrypted Message 3 key data must be requested for unwrap")
    };
    assert!(matches!(
        supplicant.complete_key_data_unwrap(request, Err::<RsnUnwrappedKeyData, _>(9_u8)),
        Err(RsnStaProcessError::KeyUnwrap(9))
    ));
    assert_eq!(supplicant.phase(), RsnStaPhase::Failed);
}

#[test]
fn completed_handshake_repeated_m3_only_grants_group_installation() {
    let (mut supplicant, message3, pmk, ptk) = awaiting_encrypted_message3();
    let retry = resign_eapol_key_variant(&message3, &ptk, true, |bytes| {
        bytes[9..17].copy_from_slice(&3_u64.to_be_bytes());
    });
    let RsnStaReady::InstallKeys(initial) = process(&mut supplicant, message3, &pmk).unwrap()
    else {
        panic!("initial installation")
    };
    let RsnStaReady::Transmit(_) = supplicant
        .complete_key_install::<512>(initial, true)
        .unwrap()
    else {
        panic!("initial M4")
    };
    let RsnStaReady::RetryInstall(group) = process(&mut supplicant, retry, &pmk).unwrap() else {
        panic!("completed M3 must never grant another pairwise installation")
    };
    assert_eq!(group.replay_counter(), 3);
    assert_eq!(group.group().key().as_bytes(), &[0x5a; 16]);
    let RsnStaReady::Transmit(message4) = supplicant
        .complete_retry_install::<512>(group, true)
        .unwrap()
    else {
        panic!("retry M4")
    };
    assert_eq!(message4.key_frame().replay_counter(), 3);
    assert!(message4.key_frame().verify_mic(&ptk));
}

#[test]
fn group_key_data_request_owns_verified_bytes_without_blocking_new_authentication() {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let ptk = pmk.derive_ptk(crate::Akm::Psk, context());
    let frame = RsnTxFrame::<512>::group_message1(crate::Akm::Psk, LOCAL, 3, [9; 8], &[0x55; 24])
        .unwrap()
        .authenticate(&ptk);
    let mut connected = connected(pmk.derive_ptk(crate::Akm::Psk, context()));
    let RsnConnectedAction::UnwrapGroupKeyData(request) =
        connected.on_group_message1(owned(&frame)).unwrap()
    else {
        panic!("new Group Message 1 must request its key-data unwrap")
    };
    assert_eq!(request.kek(), ptk.kek());
    assert_eq!(request.wrapped_key_data(), &[0x55; 24]);
    assert_eq!(request.replay_counter(), 3);
    let newer = RsnTxFrame::<512>::group_message1(crate::Akm::Psk, LOCAL, 4, [9; 8], &[0x66; 24])
        .unwrap()
        .authenticate(&ptk);
    let RsnConnectedAction::UnwrapGroupKeyData(newer_request) =
        connected.on_group_message1(owned(&newer)).unwrap()
    else {
        panic!("a newer frame must authenticate while the old request is owned")
    };
    assert_eq!(newer_request.replay_counter(), 4);
    assert_eq!(newer_request.wrapped_key_data(), &[0x66; 24]);

    let install = connected
        .complete_group_key_data_unwrap(
            request,
            Ok::<_, ()>(RsnUnwrappedKeyData::try_copy(&group_kde(1, 0x6a)).unwrap()),
        )
        .unwrap();
    assert_eq!(install.replay_counter(), 3);
    assert_eq!(install.group().key().as_bytes(), &[0x6a; 16]);
    connected.complete_group_key_install(install, true).unwrap();
    assert_eq!(connected.replay_counter(), 3);
}

#[test]
fn failed_group_key_data_unwrap_returns_the_error_and_releases_the_supplicant() {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let ptk = pmk.derive_ptk(crate::Akm::Psk, context());
    let frame = RsnTxFrame::<512>::group_message1(crate::Akm::Psk, LOCAL, 3, [9; 8], &[0x55; 24])
        .unwrap()
        .authenticate(&ptk);
    let mut connected = connected(pmk.derive_ptk(crate::Akm::Psk, context()));

    let RsnConnectedAction::UnwrapGroupKeyData(request) =
        connected.on_group_message1(owned(&frame)).unwrap()
    else {
        panic!("new Group Message 1 must request its key-data unwrap")
    };
    assert!(matches!(
        connected.complete_group_key_data_unwrap(request, Err::<RsnUnwrappedKeyData, _>(5_u8)),
        Err(RsnConnectedProcessError::KeyUnwrap(5))
    ));
    assert_eq!(connected.replay_counter(), 2);

    // Undecodable plaintext fails as a frame error and retains nothing.
    let RsnConnectedAction::UnwrapGroupKeyData(request) =
        connected.on_group_message1(owned(&frame)).unwrap()
    else {
        panic!("a failed unwrap must leave the frame eligible for retry")
    };
    assert!(matches!(
        connected.complete_group_key_data_unwrap(
            request,
            Ok::<_, ()>(RsnUnwrappedKeyData::try_copy(&[0; 24]).unwrap()),
        ),
        Err(RsnConnectedProcessError::Supplicant(
            RsnConnectedSupplicantError::Frame(_)
        ))
    ));
    assert!(matches!(
        connected.on_group_message1(owned(&frame)),
        Ok(RsnConnectedAction::UnwrapGroupKeyData(_))
    ));
}
