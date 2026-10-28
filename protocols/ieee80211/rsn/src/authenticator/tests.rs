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
use oer_ieee80211_cipher::aes::software_aes128_key_unwrap;
use oer_ieee80211_wire::{ccmp::GroupKeyId, security::rsn::Akm};

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
    let authenticator =
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
        .authenticate(ptk);
    OwnedEapolFrame::try_copy(RsnInterface::AccessPoint, PEER, frame.as_bytes()).unwrap()
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
        .on_frame(&pmk(), message2(&STATION_RSN, &ptk))
        .unwrap()
    else {
        panic!("a verified Message 2 prepares Message 3");
    };
    // The PTK is pending only once Message 3 exists.
    assert!(authenticator.pending_ptk().is_none());
    let (message3, transmit) = authenticator
        .complete_message3::<512, _>(preparation, &key_data())
        .unwrap();
    assert!(authenticator.pending_ptk().is_some());
    let key_frame = message3.key_frame().unwrap();
    assert_eq!(key_frame.message(), EapolKeyMessage::PairwiseMessage3);
    assert!(key_frame.verify_mic(&ptk));
    let plain = software_aes128_key_unwrap(ptk.kek(), key_frame.key_data()).unwrap();
    assert!(parse_gtk_key_data(plain.as_bytes(), &AP_RSN, &[], false).is_ok());
    // A retransmission carries the same key data under the same PTK.
    let again = authenticator
        .message3::<512, _>(transmit, &key_data())
        .unwrap();
    assert_eq!(again.as_bytes(), message3.as_bytes());

    let message4 = RsnTxFrame::<512>::message4(Akm::Psk, AP, REPLAY + 1)
        .unwrap()
        .authenticate(&ptk);
    let message4 =
        OwnedEapolFrame::<512>::try_copy(RsnInterface::AccessPoint, PEER, message4.as_bytes())
            .unwrap();
    assert!(matches!(
        authenticator.on_frame(&pmk(), message4).unwrap(),
        RsnAuthenticatorStep::Authorized
    ));
    assert_eq!(authenticator.phase(), RsnApPhase::Authorized);
    authenticator.forget_handshake_keys();
    assert!(authenticator.pending_ptk().is_none());
}

#[test]
fn a_verified_message2_with_other_security_ies_ends_the_handshake() {
    let mut authenticator = authenticator(&STATION_RSN);
    assert!(matches!(
        authenticator
            .on_frame(&pmk(), message2(&AP_RSN, &ptk()))
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
    let step = authenticator.on_frame(&pmk(), message2(&STATION_RSN, &other));
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
        .authenticate(&ptk());
    let message4 =
        OwnedEapolFrame::<512>::try_copy(RsnInterface::AccessPoint, PEER, message4.as_bytes())
            .unwrap();
    assert!(matches!(
        authenticator.on_frame(&pmk(), message4),
        Err(RsnAuthenticatorError::Rejected(_)) | Ok(RsnAuthenticatorStep::None)
    ));
    assert!(matches!(
        authenticator.message3::<512, _>(
            RsnTransmit {
                message: crate::state::RsnTxMessage::PairwiseMessage3,
                replay_counter: REPLAY + 1,
                retransmission: true,
            },
            &key_data()
        ),
        Err(RsnAuthenticatorError::MissingPairwiseKey)
    ));
}
