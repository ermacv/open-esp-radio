#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects,
    clippy::unreachable
)]

use super::*;
use crate::{
    EapolKeyMessage, RsnInterface,
    frames::{OwnedRsnIe, RsnGtk, parse_gtk_key_data},
};
use oer_protocols_ieee80211_core_security_cipher::aes::SoftwareAes128;
use oer_protocols_ieee80211_wire::{
    ccmp::{CcmpPacketNumber, GroupKeyId},
    security::rsn::Akm,
};

const AP: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
const PEER: [u8; 6] = [0x02, 0, 0, 0, 0, 2];
const ANONCE: [u8; 32] = [0x11; 32];
const SNONCE: [u8; 32] = [0x22; 32];
const REPLAY: u64 = 9;
const STATION_RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0x0c, 0,
];
const AP_RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];

fn pmk() -> Pmk {
    Pmk::from_bytes([0x33; 32])
}

fn ptk() -> Ptk {
    pmk().derive_ptk(
        Akm::Psk,
        PtkContext {
            authenticator_address: AP,
            supplicant_address: PEER,
            authenticator_nonce: ANONCE,
            supplicant_nonce: SNONCE,
        },
    )
}

/// An authenticator whose association named `association_rsn`, after
/// Message 1.
fn authenticator(association_rsn: &[u8]) -> RsnAuthenticator {
    let state = RsnApState::new(Akm::Psk, AP, PEER, ANONCE, REPLAY).unwrap();
    let mut authenticator =
        RsnAuthenticator::new(state, pmk().bind_association_security_ies(association_rsn));
    let (message1, transmit) = authenticator.message1::<512>().unwrap();
    assert_eq!(
        message1.key_frame().unwrap().message(),
        EapolKeyMessage::PairwiseMessage1
    );
    assert_eq!(transmit.replay_counter, REPLAY);
    authenticator
}

fn message2(rsn: &[u8], ptk: &Ptk) -> OwnedEapolFrame<512> {
    let rsn = OwnedRsnIe::<64>::try_copy(rsn).unwrap();
    let frame = RsnTxFrame::<512>::message2(Akm::Psk, AP, REPLAY, SNONCE, &rsn)
        .unwrap()
        .authenticate(ptk, &mut SoftwareAes128)
        .unwrap();
    OwnedEapolFrame::try_copy(RsnInterface::AccessPoint, PEER, frame.as_bytes()).unwrap()
}

fn group_packet_number(value: u64) -> CcmpPacketNumber {
    CcmpPacketNumber::new(value).unwrap()
}

fn key_data() -> RsnPlainKeyData {
    let gtk = RsnGtk::new(GroupKeyId::new(1).unwrap(), true, &[0x55; 16]);
    RsnPlainKeyData::build(&AP_RSN, &gtk, None).unwrap()
}

#[test]
fn a_verified_handshake_wraps_message3_and_authorizes_on_message4() {
    let mut authenticator = authenticator(&STATION_RSN);
    let ptk = ptk();
    let RsnAuthenticatorStep::PrepareMessage3(preparation) = authenticator
        .on_frame(&pmk(), message2(&STATION_RSN, &ptk), &mut SoftwareAes128)
        .unwrap()
    else {
        panic!("a verified Message 2 prepares Message 3");
    };
    // The PTK is pending only once Message 3 exists.
    assert!(authenticator.pending_ptk().is_none());
    let (message3, transmit) = authenticator
        .complete_message3::<512, _, _>(
            preparation,
            &key_data(),
            group_packet_number(0x0605_0403_0201),
            &mut SoftwareAes128,
        )
        .unwrap();
    assert!(authenticator.pending_ptk().is_some());
    let key_frame = message3.key_frame().unwrap();
    assert_eq!(key_frame.message(), EapolKeyMessage::PairwiseMessage3);
    assert_eq!(key_frame.replay_counter(), REPLAY + 1);
    assert_eq!(transmit.replay_counter, REPLAY + 1);
    assert_eq!(key_frame.key_receive_sequence(), &[1, 2, 3, 4, 5, 6, 0, 0]);
    assert!(key_frame.verify_mic(&ptk, &mut SoftwareAes128).unwrap());
    let plain = crate::test_unwrap_bytes(ptk.kek(), key_frame.key_data()).unwrap();
    assert!(parse_gtk_key_data(plain.as_bytes(), &AP_RSN, &[], false).is_ok());
    // A retransmission takes the next counter and the GTK's packet number
    // at its own time, with the same key data under the same PTK.
    let (again, again_transmit) = authenticator
        .retransmit_message3::<512, _, _>(
            &key_data(),
            group_packet_number(0x0605_0403_0209),
            &mut SoftwareAes128,
        )
        .unwrap();
    assert!(again_transmit.retransmission);
    let again_frame = again.key_frame().unwrap();
    assert_eq!(again_frame.replay_counter(), REPLAY + 2);
    assert_eq!(again_transmit.replay_counter, REPLAY + 2);
    assert_eq!(
        again_frame.key_receive_sequence(),
        &[9, 2, 3, 4, 5, 6, 0, 0]
    );
    assert!(again_frame.verify_mic(&ptk, &mut SoftwareAes128).unwrap());
    let again_plain = crate::test_unwrap_bytes(ptk.kek(), again_frame.key_data()).unwrap();
    assert_eq!(again_plain.as_bytes(), plain.as_bytes());

    // Message 4 of the first Message 3 is still awaited after the retry;
    // the retry's own Message 4 then arrives after completion.
    let signed_message4 = |replay| {
        let frame = RsnTxFrame::<512>::message4(Akm::Psk, AP, replay)
            .unwrap()
            .authenticate(&ptk, &mut SoftwareAes128)
            .unwrap();
        OwnedEapolFrame::<512>::try_copy(RsnInterface::AccessPoint, PEER, frame.as_bytes()).unwrap()
    };
    assert!(matches!(
        authenticator
            .on_frame(&pmk(), signed_message4(REPLAY + 1), &mut SoftwareAes128)
            .unwrap(),
        RsnAuthenticatorStep::Authorized
    ));
    assert_eq!(authenticator.phase(), RsnApPhase::Authorized);
    assert!(matches!(
        authenticator
            .on_frame(&pmk(), signed_message4(REPLAY + 2), &mut SoftwareAes128)
            .unwrap(),
        RsnAuthenticatorStep::None
    ));
    authenticator.forget_handshake_keys();
    assert!(authenticator.pending_ptk().is_none());
}

#[test]
fn a_verified_message2_with_other_security_ies_ends_the_handshake() {
    let mut authenticator = authenticator(&STATION_RSN);
    assert!(matches!(
        authenticator
            .on_frame(&pmk(), message2(&AP_RSN, &ptk()), &mut SoftwareAes128)
            .unwrap(),
        RsnAuthenticatorStep::Deauthenticate
    ));
    assert!(authenticator.pending_ptk().is_none());
}

#[test]
fn a_message2_under_another_pmk_never_prepares_message3() {
    let mut authenticator = authenticator(&STATION_RSN);
    let other = Pmk::from_bytes([0x44; 32]).derive_ptk(
        Akm::Psk,
        PtkContext {
            authenticator_address: AP,
            supplicant_address: PEER,
            authenticator_nonce: ANONCE,
            supplicant_nonce: SNONCE,
        },
    );
    let step = authenticator.on_frame(&pmk(), message2(&STATION_RSN, &other), &mut SoftwareAes128);
    assert!(!matches!(
        step,
        Ok(RsnAuthenticatorStep::PrepareMessage3(_))
    ));
    assert!(authenticator.pending_ptk().is_none());
}

#[test]
fn message4_before_message3_is_refused_and_unsolicited_frames_are_rejected() {
    let mut authenticator = authenticator(&STATION_RSN);
    let message4 = RsnTxFrame::<512>::message4(Akm::Psk, AP, REPLAY)
        .unwrap()
        .authenticate(&ptk(), &mut SoftwareAes128)
        .unwrap();
    let message4 =
        OwnedEapolFrame::<512>::try_copy(RsnInterface::AccessPoint, PEER, message4.as_bytes())
            .unwrap();
    assert!(matches!(
        authenticator.on_frame(&pmk(), message4, &mut SoftwareAes128),
        Err(RsnAuthenticatorError::Rejected(_)) | Ok(RsnAuthenticatorStep::None)
    ));
    assert!(matches!(
        authenticator.retransmit_message3::<512, _, _>(
            &key_data(),
            group_packet_number(0),
            &mut SoftwareAes128,
        ),
        Err(RsnAuthenticatorError::MissingPairwiseKey)
    ));
    // The refusal spent no counter: Message 1 is still awaited under its own.
    assert_eq!(
        authenticator
            .state()
            .outstanding_transmit()
            .map(|t| t.map(|t| t.replay_counter)),
        Ok(Some(REPLAY))
    );
}

#[test]
fn a_retransmitted_message1_takes_the_next_counter_and_an_earlier_answer_stays_awaited() {
    let mut authenticator = authenticator(&STATION_RSN);
    let (retry, transmit) = authenticator.retransmit_message1::<512>().unwrap();
    assert!(transmit.retransmission);
    assert!(retry.retransmission());
    assert_eq!(retry.key_frame().unwrap().replay_counter(), REPLAY + 1);
    assert_eq!(retry.key_frame().unwrap().nonce(), &ANONCE);
    let ptk = ptk();
    // A counter that was never sent is refused as stale input.
    let rsn = OwnedRsnIe::<64>::try_copy(&STATION_RSN).unwrap();
    let unsent = RsnTxFrame::<512>::message2(Akm::Psk, AP, REPLAY + 2, SNONCE, &rsn)
        .unwrap()
        .authenticate(&ptk, &mut SoftwareAes128)
        .unwrap();
    let unsent =
        OwnedEapolFrame::<512>::try_copy(RsnInterface::AccessPoint, PEER, unsent.as_bytes())
            .unwrap();
    assert!(matches!(
        authenticator.on_frame(&pmk(), unsent, &mut SoftwareAes128),
        Err(RsnAuthenticatorError::Rejected(
            RsnStateError::ReplayCounterMismatch
        ))
    ));
    // Message 2 answering the first Message 1 prepares Message 3.
    let answer = RsnTxFrame::<512>::message2(Akm::Psk, AP, REPLAY, SNONCE, &rsn)
        .unwrap()
        .authenticate(&ptk, &mut SoftwareAes128)
        .unwrap();
    let answer =
        OwnedEapolFrame::<512>::try_copy(RsnInterface::AccessPoint, PEER, answer.as_bytes())
            .unwrap();
    let RsnAuthenticatorStep::PrepareMessage3(preparation) = authenticator
        .on_frame(&pmk(), answer, &mut SoftwareAes128)
        .unwrap()
    else {
        panic!("Message 2 answering an awaited Message 1 prepares Message 3");
    };
    let (message3, _) = authenticator
        .complete_message3::<512, _, _>(
            preparation,
            &key_data(),
            group_packet_number(0),
            &mut SoftwareAes128,
        )
        .unwrap();
    assert_eq!(message3.key_frame().unwrap().replay_counter(), REPLAY + 2);
}

fn signed_message4(replay: u64) -> OwnedEapolFrame<512> {
    let answer = RsnTxFrame::<512>::message4(Akm::Psk, AP, replay)
        .unwrap()
        .authenticate(&ptk(), &mut SoftwareAes128)
        .unwrap();
    OwnedEapolFrame::try_copy(RsnInterface::AccessPoint, PEER, answer.as_bytes()).unwrap()
}

/// An authenticator whose first Message 3 (counter `REPLAY + 1`) was built.
fn awaiting_message4() -> RsnAuthenticator {
    let mut authenticator = authenticator(&STATION_RSN);
    let RsnAuthenticatorStep::PrepareMessage3(preparation) = authenticator
        .on_frame(&pmk(), message2(&STATION_RSN, &ptk()), &mut SoftwareAes128)
        .unwrap()
    else {
        panic!("a verified Message 2 prepares Message 3");
    };
    authenticator
        .complete_message3::<512, _, _>(
            preparation,
            &key_data(),
            group_packet_number(0),
            &mut SoftwareAes128,
        )
        .unwrap();
    authenticator
}

#[test]
fn a_message3_that_failed_to_build_is_not_awaited_and_its_counter_stays_spent() {
    let mut authenticator = awaiting_message4();
    // A one-octet frame cannot hold Message 3.
    assert_eq!(
        authenticator
            .retransmit_message3::<1, _, _>(
                &key_data(),
                group_packet_number(0),
                &mut SoftwareAes128
            )
            .err(),
        Some(RsnAuthenticatorError::Frame(
            RsnFrameError::CapacityExceeded
        ))
    );
    assert!(matches!(
        authenticator.on_frame(&pmk(), signed_message4(REPLAY + 2), &mut SoftwareAes128),
        Err(RsnAuthenticatorError::Rejected(
            RsnStateError::ReplayCounterMismatch
        ))
    ));
    // The next retry takes a newer counter; the failed one is never reused.
    let (retry, transmit) = authenticator
        .retransmit_message3::<512, _, _>(&key_data(), group_packet_number(0), &mut SoftwareAes128)
        .unwrap();
    assert_eq!(transmit.replay_counter, REPLAY + 3);
    assert_eq!(retry.key_frame().unwrap().replay_counter(), REPLAY + 3);
    assert!(matches!(
        authenticator.on_frame(&pmk(), signed_message4(REPLAY + 2), &mut SoftwareAes128),
        Err(RsnAuthenticatorError::Rejected(
            RsnStateError::ReplayCounterMismatch
        ))
    ));
    assert!(matches!(
        authenticator
            .on_frame(&pmk(), signed_message4(REPLAY + 3), &mut SoftwareAes128)
            .unwrap(),
        RsnAuthenticatorStep::Authorized
    ));
}

#[test]
fn failed_message3_builds_do_not_evict_the_only_built_message3() {
    let mut authenticator = awaiting_message4();
    for _ in 0..crate::state::RSN_AP_REPLAY_WINDOW + 2 {
        assert_eq!(
            authenticator
                .retransmit_message3::<1, _, _>(
                    &key_data(),
                    group_packet_number(0),
                    &mut SoftwareAes128
                )
                .err(),
            Some(RsnAuthenticatorError::Frame(
                RsnFrameError::CapacityExceeded
            ))
        );
    }
    assert!(matches!(
        authenticator
            .on_frame(&pmk(), signed_message4(REPLAY + 1), &mut SoftwareAes128)
            .unwrap(),
        RsnAuthenticatorStep::Authorized
    ));
}

#[test]
fn a_message1_that_failed_to_build_is_not_awaited() {
    let mut authenticator = authenticator(&STATION_RSN);
    assert_eq!(
        authenticator.retransmit_message1::<1>().err(),
        Some(RsnAuthenticatorError::Frame(
            RsnFrameError::CapacityExceeded
        ))
    );
    let ptk = ptk();
    let rsn = OwnedRsnIe::<64>::try_copy(&STATION_RSN).unwrap();
    let answer = |replay| {
        let frame = RsnTxFrame::<512>::message2(Akm::Psk, AP, replay, SNONCE, &rsn)
            .unwrap()
            .authenticate(&ptk, &mut SoftwareAes128)
            .unwrap();
        OwnedEapolFrame::<512>::try_copy(RsnInterface::AccessPoint, PEER, frame.as_bytes()).unwrap()
    };
    assert!(matches!(
        authenticator.on_frame(&pmk(), answer(REPLAY + 1), &mut SoftwareAes128),
        Err(RsnAuthenticatorError::Rejected(
            RsnStateError::ReplayCounterMismatch
        ))
    ));
    let (_, transmit) = authenticator.retransmit_message1::<512>().unwrap();
    assert_eq!(transmit.replay_counter, REPLAY + 2);
    assert!(matches!(
        authenticator.on_frame(&pmk(), answer(REPLAY + 1), &mut SoftwareAes128),
        Err(RsnAuthenticatorError::Rejected(
            RsnStateError::ReplayCounterMismatch
        ))
    ));
    assert!(matches!(
        authenticator
            .on_frame(&pmk(), answer(REPLAY + 2), &mut SoftwareAes128)
            .unwrap(),
        RsnAuthenticatorStep::PrepareMessage3(_)
    ));
}
