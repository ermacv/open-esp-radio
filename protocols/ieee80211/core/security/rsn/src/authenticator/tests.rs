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
            authenticator_address: Eui48::new(AP),
            supplicant_address: Eui48::new(PEER),
            authenticator_nonce: ANONCE,
            supplicant_nonce: SNONCE,
        },
    )
}

/// A handshake whose association named `association_rsn`, with its
/// Message 1.
fn handshake(association_rsn: &[u8]) -> RsnApHandshake {
    let start = RsnApStart::new(
        Akm::Psk,
        Eui48::new(AP),
        Eui48::new(PEER),
        ANONCE,
        REPLAY,
        pmk().bind_association_security_ies(association_rsn),
    )
    .unwrap();
    let (handshake, message1, transmit) = RsnApHandshake::start::<512>(start);
    assert_eq!(
        message1.key_frame().unwrap().message(),
        EapolKeyMessage::PairwiseMessage1
    );
    assert_eq!(transmit.replay_counter, REPLAY);
    assert_eq!(handshake.latest(), Some(transmit));
    handshake
}

fn message2_at(rsn: &[u8], ptk: &Ptk, replay: u64) -> OwnedEapolFrame<512> {
    let rsn = OwnedRsnIe::<64>::try_copy(rsn).unwrap();
    let frame = RsnTxFrame::<512>::message2(Akm::Psk, Eui48::new(AP), replay, SNONCE, &rsn)
        .unwrap()
        .authenticate(ptk, &mut SoftwareAes128)
        .unwrap();
    OwnedEapolFrame::try_copy(
        RsnInterface::AccessPoint,
        Eui48::new(PEER),
        frame.as_bytes(),
    )
    .unwrap()
}

fn message2(rsn: &[u8], ptk: &Ptk) -> OwnedEapolFrame<512> {
    message2_at(rsn, ptk, REPLAY)
}

fn signed_message4(replay: u64) -> OwnedEapolFrame<512> {
    let answer = RsnTxFrame::<512>::message4(Akm::Psk, Eui48::new(AP), replay)
        .authenticate(&ptk(), &mut SoftwareAes128)
        .unwrap();
    OwnedEapolFrame::try_copy(
        RsnInterface::AccessPoint,
        Eui48::new(PEER),
        answer.as_bytes(),
    )
    .unwrap()
}

fn group_packet_number(value: u64) -> CcmpPacketNumber {
    CcmpPacketNumber::new(value).unwrap()
}

fn key_data() -> RsnPlainKeyData {
    let gtk = RsnGtk::new(
        GroupKeyId::new(1).unwrap(),
        oer_protocols_ieee80211_wire::eapol::GtkTx::TransmitAndReceive,
        &[0x55; 16],
    );
    RsnPlainKeyData::build(&AP_RSN, &gtk, None).unwrap()
}

fn material(key_data: &RsnPlainKeyData, packet_number: u64) -> Message3Material<'_, 128> {
    Message3Material {
        key_data,
        group_packet_number: group_packet_number(packet_number),
    }
}

/// A handshake whose first Message 3 (counter `REPLAY + 1`) was built.
fn awaiting_message4() -> RsnApHandshake {
    let mut handshake = handshake(&STATION_RSN);
    let key_data = key_data();
    let RsnApEapol::Message3(..) = handshake
        .on_eapol(
            &pmk(),
            message2(&STATION_RSN, &ptk()),
            material(&key_data, 0),
            &mut SoftwareAes128,
        )
        .unwrap()
    else {
        panic!("a verified Message 2 builds Message 3");
    };
    handshake
}

/// `frame` with one bit of its MIC flipped.
fn forged(frame: &OwnedEapolFrame<512>) -> OwnedEapolFrame<512> {
    let mut bytes = [0; 512];
    let len = frame.as_bytes().len();
    bytes[..len].copy_from_slice(frame.as_bytes());
    bytes[81] ^= 1;
    OwnedEapolFrame::try_copy(RsnInterface::AccessPoint, Eui48::new(PEER), &bytes[..len]).unwrap()
}

#[test]
fn a_verified_message2_builds_message3_and_message4_authorizes() {
    let mut handshake = handshake(&STATION_RSN);
    let ptk = ptk();
    let key_data = key_data();
    assert!(handshake.ptk().is_none());
    assert_eq!(handshake.authorization(), Err(RsnApAwaiting::Message2));
    let RsnApEapol::Message3(message3, transmit) = handshake
        .on_eapol(
            &pmk(),
            message2(&STATION_RSN, &ptk),
            material(&key_data, 0x0605_0403_0201),
            &mut SoftwareAes128,
        )
        .unwrap()
    else {
        panic!("a verified Message 2 builds Message 3");
    };
    assert!(handshake.ptk().is_some());
    assert_eq!(handshake.authorization(), Err(RsnApAwaiting::Message4));
    assert_eq!(handshake.latest(), Some(transmit));
    let key_frame = message3.key_frame().unwrap();
    assert_eq!(key_frame.message(), EapolKeyMessage::PairwiseMessage3);
    assert_eq!(key_frame.replay_counter(), REPLAY + 1);
    assert_eq!(transmit.replay_counter, REPLAY + 1);
    assert!(!transmit.retransmission);
    assert_eq!(key_frame.key_receive_sequence(), &[1, 2, 3, 4, 5, 6, 0, 0]);
    assert!(key_frame.verify_mic(&ptk, &mut SoftwareAes128).unwrap());
    let plain = crate::test_unwrap_bytes(ptk.kek(), key_frame.key_data()).unwrap();
    assert!(parse_gtk_key_data(plain.as_bytes(), &AP_RSN, &[], None).is_ok());

    // A retransmission takes the next counter and the GTK's packet number
    // at its own time, with the same key data under the same PTK.
    let RsnApRetransmit::Message3(again, again_transmit) = handshake
        .retransmit::<512, _, _>(material(&key_data, 0x0605_0403_0209), &mut SoftwareAes128)
        .unwrap()
    else {
        panic!("Message 3 is retransmitted while Message 4 is awaited");
    };
    assert!(again_transmit.retransmission);
    assert!(again.retransmission());
    let again_frame = again.key_frame().unwrap();
    assert_eq!(again_frame.replay_counter(), REPLAY + 2);
    assert_eq!(again_transmit.replay_counter, REPLAY + 2);
    assert_eq!(handshake.latest(), Some(again_transmit));
    assert_eq!(
        again_frame.key_receive_sequence(),
        &[9, 2, 3, 4, 5, 6, 0, 0]
    );
    assert!(again_frame.verify_mic(&ptk, &mut SoftwareAes128).unwrap());
    let again_plain = crate::test_unwrap_bytes(ptk.kek(), again_frame.key_data()).unwrap();
    assert_eq!(again_plain.as_bytes(), plain.as_bytes());

    // Message 4 of the first Message 3 is still awaited after the retry;
    // the retry's own Message 4 then arrives after completion.
    assert!(matches!(
        handshake
            .on_eapol(
                &pmk(),
                signed_message4(REPLAY + 1),
                material(&key_data, 0),
                &mut SoftwareAes128
            )
            .unwrap(),
        RsnApEapol::Authorized
    ));
    assert!(handshake.authorization().is_ok());
    assert_eq!(
        handshake.ptk().map(Ptk::temporal_key),
        Some(ptk.temporal_key())
    );
    assert_eq!(handshake.latest(), None);
    assert!(matches!(
        handshake
            .on_eapol(
                &pmk(),
                signed_message4(REPLAY + 2),
                material(&key_data, 0),
                &mut SoftwareAes128
            )
            .unwrap(),
        RsnApEapol::None
    ));
    assert!(matches!(
        handshake
            .retransmit::<512, _, _>(material(&key_data, 0), &mut SoftwareAes128)
            .unwrap(),
        RsnApRetransmit::Authorized
    ));
}

#[test]
fn a_verified_message2_with_other_security_ies_ends_the_handshake() {
    let mut handshake = handshake(&STATION_RSN);
    let key_data = key_data();
    assert!(matches!(
        handshake
            .on_eapol(
                &pmk(),
                message2(&AP_RSN, &ptk()),
                material(&key_data, 0),
                &mut SoftwareAes128
            )
            .unwrap(),
        RsnApEapol::End(RsnApEnd::SecurityIesMismatch)
    ));
}

/// A wrap the engine refuses ends the handshake; under PSK the MICs are
/// HMAC-SHA1 and take no engine, so only the wrap meets the refusal.
#[test]
fn a_refused_wrap_ends_the_handshake() {
    let mut handshake = handshake(&STATION_RSN);
    let key_data = key_data();
    assert!(matches!(
        handshake
            .on_eapol(
                &pmk(),
                message2(&STATION_RSN, &ptk()),
                material(&key_data, 0),
                &mut crate::test_engine::Refusing
            )
            .unwrap(),
        RsnApEapol::End(RsnApEnd::KeyWrap(KeyWrapError::Engine(_)))
    ));
    // A retransmission's refused wrap is its error; the handshake goes on.
    let mut waiting = awaiting_message4();
    assert!(matches!(
        waiting.retransmit::<512, _, _>(material(&key_data, 0), &mut crate::test_engine::Refusing),
        Err(RsnApError::KeyWrap(KeyWrapError::Engine(_)))
    ));
    assert!(matches!(
        waiting
            .on_eapol(
                &pmk(),
                signed_message4(REPLAY + 1),
                material(&key_data, 0),
                &mut SoftwareAes128
            )
            .unwrap(),
        RsnApEapol::Authorized
    ));
}

#[test]
fn a_message2_under_another_pmk_builds_no_message3() {
    let mut handshake = handshake(&STATION_RSN);
    let key_data = key_data();
    let other = Pmk::from_bytes([0x44; 32]).derive_ptk(
        Akm::Psk,
        PtkContext {
            authenticator_address: Eui48::new(AP),
            supplicant_address: Eui48::new(PEER),
            authenticator_nonce: ANONCE,
            supplicant_nonce: SNONCE,
        },
    );
    assert!(matches!(
        handshake.on_eapol(
            &pmk(),
            message2(&STATION_RSN, &other),
            material(&key_data, 0),
            &mut SoftwareAes128
        ),
        Ok(RsnApEapol::None)
    ));
    assert!(handshake.ptk().is_none());
}

#[test]
fn message4_before_message3_is_refused_and_a_retry_is_message1() {
    let mut handshake = handshake(&STATION_RSN);
    let key_data = key_data();
    assert!(matches!(
        handshake.on_eapol(
            &pmk(),
            signed_message4(REPLAY),
            material(&key_data, 0),
            &mut SoftwareAes128
        ),
        Err(RsnApError::Rejected(RsnStateError::UnexpectedMessage))
    ));
    // The refusal spent no counter: Message 1 is still awaited under its own.
    assert_eq!(handshake.latest().map(|t| t.replay_counter), Some(REPLAY));
    let RsnApRetransmit::Message1(retry, transmit) = handshake
        .retransmit::<512, _, _>(material(&key_data, 0), &mut SoftwareAes128)
        .unwrap()
    else {
        panic!("Message 1 is retransmitted while Message 2 is awaited");
    };
    assert!(transmit.retransmission);
    assert!(retry.retransmission());
    assert_eq!(retry.key_frame().unwrap().replay_counter(), REPLAY + 1);
    assert_eq!(retry.key_frame().unwrap().nonce(), &ANONCE);
    assert_eq!(handshake.latest(), Some(transmit));
}

#[test]
fn an_earlier_message1_stays_awaited_after_a_retransmission() {
    let mut handshake = handshake(&STATION_RSN);
    let key_data = key_data();
    handshake
        .retransmit::<512, _, _>(material(&key_data, 0), &mut SoftwareAes128)
        .unwrap();
    let ptk = ptk();
    // A counter that was never sent is refused as stale input.
    assert!(matches!(
        handshake.on_eapol(
            &pmk(),
            message2_at(&STATION_RSN, &ptk, REPLAY + 2),
            material(&key_data, 0),
            &mut SoftwareAes128
        ),
        Err(RsnApError::Rejected(RsnStateError::ReplayCounterMismatch))
    ));
    // Message 2 answering the first Message 1 builds Message 3.
    let RsnApEapol::Message3(message3, _) = handshake
        .on_eapol(
            &pmk(),
            message2(&STATION_RSN, &ptk),
            material(&key_data, 0),
            &mut SoftwareAes128,
        )
        .unwrap()
    else {
        panic!("Message 2 answering an awaited Message 1 builds Message 3");
    };
    assert_eq!(message3.key_frame().unwrap().replay_counter(), REPLAY + 2);
    // A repeat of the verified Message 2 builds nothing; another SNonce is
    // refused.
    assert!(matches!(
        handshake.on_eapol(
            &pmk(),
            message2(&STATION_RSN, &ptk),
            material(&key_data, 0),
            &mut SoftwareAes128
        ),
        Ok(RsnApEapol::None)
    ));
}

#[test]
fn a_message3_that_failed_to_build_is_not_awaited_and_its_counter_stays_spent() {
    let mut handshake = awaiting_message4();
    let key_data = key_data();
    // 128 octets hold Message 1 but not Message 3.
    assert_eq!(
        handshake
            .retransmit::<128, _, _>(material(&key_data, 0), &mut SoftwareAes128)
            .err(),
        Some(RsnApError::Frame(RsnFrameError::CapacityExceeded))
    );
    assert_eq!(
        handshake.latest().map(|t| t.replay_counter),
        Some(REPLAY + 1)
    );
    assert!(matches!(
        handshake.on_eapol(
            &pmk(),
            signed_message4(REPLAY + 2),
            material(&key_data, 0),
            &mut SoftwareAes128
        ),
        Err(RsnApError::Rejected(RsnStateError::ReplayCounterMismatch))
    ));
    // The next retry takes a newer counter; the failed one is never reused.
    let RsnApRetransmit::Message3(retry, transmit) = handshake
        .retransmit::<512, _, _>(material(&key_data, 0), &mut SoftwareAes128)
        .unwrap()
    else {
        panic!("Message 3 is retransmitted");
    };
    assert_eq!(transmit.replay_counter, REPLAY + 3);
    assert_eq!(retry.key_frame().unwrap().replay_counter(), REPLAY + 3);
    assert!(matches!(
        handshake.on_eapol(
            &pmk(),
            signed_message4(REPLAY + 2),
            material(&key_data, 0),
            &mut SoftwareAes128
        ),
        Err(RsnApError::Rejected(RsnStateError::ReplayCounterMismatch))
    ));
    assert!(matches!(
        handshake
            .on_eapol(
                &pmk(),
                signed_message4(REPLAY + 3),
                material(&key_data, 0),
                &mut SoftwareAes128
            )
            .unwrap(),
        RsnApEapol::Authorized
    ));
}

/// A first Message 3 that fails to build leaves the handshake awaiting
/// Message 2 with its counter spent: the next Message 2, answering a
/// retransmitted Message 1, builds it under a newer counter.
#[test]
fn a_first_message3_that_failed_to_build_awaits_message2_again() {
    let key_data = key_data();
    let ptk = ptk();
    let mut small = handshake(&STATION_RSN);
    let small_frame = message2(&STATION_RSN, &ptk);
    // Message 3 is built in Message 2's frame capacity: 128 octets do not
    // hold it.
    let narrow = OwnedEapolFrame::<128>::try_copy(
        RsnInterface::AccessPoint,
        Eui48::new(PEER),
        small_frame.as_bytes(),
    )
    .unwrap();
    assert_eq!(
        small
            .on_eapol(&pmk(), narrow, material(&key_data, 0), &mut SoftwareAes128)
            .err(),
        Some(RsnApError::Frame(RsnFrameError::CapacityExceeded))
    );
    assert!(small.ptk().is_none());
    assert_eq!(small.latest().map(|t| t.replay_counter), Some(REPLAY));
    let RsnApRetransmit::Message1(_, retry) = small
        .retransmit::<512, _, _>(material(&key_data, 0), &mut SoftwareAes128)
        .unwrap()
    else {
        panic!("Message 1 is retransmitted while Message 2 is awaited");
    };
    assert_eq!(retry.replay_counter, REPLAY + 2);
    let RsnApEapol::Message3(_, transmit) = small
        .on_eapol(
            &pmk(),
            message2_at(&STATION_RSN, &ptk, REPLAY + 2),
            material(&key_data, 0),
            &mut SoftwareAes128,
        )
        .unwrap()
    else {
        panic!("Message 2 of the retransmitted Message 1 builds Message 3");
    };
    assert_eq!(transmit.replay_counter, REPLAY + 3);
}

#[test]
fn failed_message3_builds_do_not_evict_the_only_built_message3() {
    let mut handshake = awaiting_message4();
    let key_data = key_data();
    for _ in 0..RSN_AP_REPLAY_WINDOW + 2 {
        assert_eq!(
            handshake
                .retransmit::<128, _, _>(material(&key_data, 0), &mut SoftwareAes128)
                .err(),
            Some(RsnApError::Frame(RsnFrameError::CapacityExceeded))
        );
    }
    assert!(matches!(
        handshake
            .on_eapol(
                &pmk(),
                signed_message4(REPLAY + 1),
                material(&key_data, 0),
                &mut SoftwareAes128
            )
            .unwrap(),
        RsnApEapol::Authorized
    ));
}

/// The window holds the latest `RSN_AP_REPLAY_WINDOW` built Message 1:
/// an answer to an older one is refused.
#[test]
fn the_window_holds_only_the_latest_built_message1() {
    let mut handshake = handshake(&STATION_RSN);
    let key_data = key_data();
    for _ in 0..RSN_AP_REPLAY_WINDOW {
        handshake
            .retransmit::<512, _, _>(material(&key_data, 0), &mut SoftwareAes128)
            .unwrap();
    }
    let ptk = ptk();
    assert!(matches!(
        handshake.on_eapol(
            &pmk(),
            message2(&STATION_RSN, &ptk),
            material(&key_data, 0),
            &mut SoftwareAes128
        ),
        Err(RsnApError::Rejected(RsnStateError::ReplayCounterMismatch))
    ));
    assert!(matches!(
        handshake
            .on_eapol(
                &pmk(),
                message2_at(&STATION_RSN, &ptk, REPLAY + 1),
                material(&key_data, 0),
                &mut SoftwareAes128
            )
            .unwrap(),
        RsnApEapol::Message3(..)
    ));
}

#[test]
fn a_forged_message2_builds_no_message3_and_the_real_one_still_does() {
    let mut handshake = handshake(&STATION_RSN);
    let key_data = key_data();
    let genuine = message2(&STATION_RSN, &ptk());
    assert!(matches!(
        handshake.on_eapol(
            &pmk(),
            forged(&genuine),
            material(&key_data, 0),
            &mut SoftwareAes128
        ),
        Ok(RsnApEapol::None)
    ));
    assert!(handshake.ptk().is_none());
    assert_eq!(handshake.latest().map(|t| t.replay_counter), Some(REPLAY));
    assert!(matches!(
        handshake.on_eapol(&pmk(), genuine, material(&key_data, 0), &mut SoftwareAes128),
        Ok(RsnApEapol::Message3(..))
    ));
}

#[test]
fn a_forged_message4_authorizes_nobody_and_the_real_one_still_does() {
    let mut handshake = awaiting_message4();
    let key_data = key_data();
    let genuine = signed_message4(REPLAY + 1);
    assert!(matches!(
        handshake.on_eapol(
            &pmk(),
            forged(&genuine),
            material(&key_data, 0),
            &mut SoftwareAes128
        ),
        Ok(RsnApEapol::None)
    ));
    assert_eq!(handshake.authorization(), Err(RsnApAwaiting::Message4));
    assert!(matches!(
        handshake.on_eapol(&pmk(), genuine, material(&key_data, 0), &mut SoftwareAes128),
        Ok(RsnApEapol::Authorized)
    ));
    assert!(handshake.authorization().is_ok());
}

/// Input of another interface or peer, a zero nonce and an exhausted
/// counter are refused before any state changes.
#[test]
fn foreign_input_and_exhausted_counters_are_refused() {
    let key_data = key_data();
    let mut handshake = handshake(&STATION_RSN);
    let frame = message2(&STATION_RSN, &ptk());
    let other_peer = OwnedEapolFrame::<512>::try_copy(
        RsnInterface::AccessPoint,
        Eui48::new(AP),
        frame.as_bytes(),
    )
    .unwrap();
    assert!(matches!(
        handshake.on_eapol(
            &pmk(),
            other_peer,
            material(&key_data, 0),
            &mut SoftwareAes128
        ),
        Err(RsnApError::Rejected(RsnStateError::WrongPeer))
    ));
    let station =
        OwnedEapolFrame::<512>::try_copy(RsnInterface::Station, Eui48::new(PEER), frame.as_bytes())
            .unwrap();
    assert!(matches!(
        handshake.on_eapol(&pmk(), station, material(&key_data, 0), &mut SoftwareAes128),
        Err(RsnApError::Rejected(RsnStateError::WrongInterface))
    ));
    let binding = || pmk().bind_association_security_ies(&STATION_RSN);
    assert!(matches!(
        RsnApStart::new(
            Akm::Psk,
            Eui48::new(AP),
            Eui48::new(PEER),
            [0; 32],
            REPLAY,
            binding()
        ),
        Err(RsnStateError::ZeroNonce)
    ));
    assert!(matches!(
        RsnApStart::new(
            Akm::Psk,
            Eui48::new(AP),
            Eui48::new(PEER),
            ANONCE,
            u64::MAX,
            binding()
        ),
        Err(RsnStateError::ReplayCounterExhausted)
    ));
    // The counter before the last leaves none for a retransmitted Message 1.
    let start = RsnApStart::new(
        Akm::Psk,
        Eui48::new(AP),
        Eui48::new(PEER),
        ANONCE,
        u64::MAX - 1,
        binding(),
    )
    .unwrap();
    let (mut last, ..) = RsnApHandshake::start::<512>(start);
    assert!(matches!(
        last.retransmit::<512, _, _>(material(&key_data, 0), &mut SoftwareAes128),
        Err(RsnApError::ReplayCounterExhausted)
    ));
}

/// The window holds the latest `RSN_AP_REPLAY_WINDOW` built Message 3:
/// a Message 4 answering an older one is refused.
#[test]
fn the_window_holds_only_the_latest_built_message3() {
    let mut handshake = awaiting_message4();
    let key_data = key_data();
    for _ in 0..RSN_AP_REPLAY_WINDOW {
        handshake
            .retransmit::<512, _, _>(material(&key_data, 0), &mut SoftwareAes128)
            .unwrap();
    }
    assert!(matches!(
        handshake.on_eapol(
            &pmk(),
            signed_message4(REPLAY + 1),
            material(&key_data, 0),
            &mut SoftwareAes128
        ),
        Err(RsnApError::Rejected(RsnStateError::ReplayCounterMismatch))
    ));
    assert!(matches!(
        handshake
            .on_eapol(
                &pmk(),
                signed_message4(REPLAY + 2),
                material(&key_data, 0),
                &mut SoftwareAes128
            )
            .unwrap(),
        RsnApEapol::Authorized
    ));
}

/// Every built Message 1 of a full window stays awaited: the original and
/// each retransmission, whichever the station answers.
#[test]
fn every_built_message1_of_a_full_window_is_awaited() {
    let key_data = key_data();
    let ptk = ptk();
    for answered in 0..RSN_AP_REPLAY_WINDOW as u64 {
        let mut handshake = handshake(&STATION_RSN);
        for _ in 1..RSN_AP_REPLAY_WINDOW {
            handshake
                .retransmit::<512, _, _>(material(&key_data, 0), &mut SoftwareAes128)
                .unwrap();
        }
        assert!(
            matches!(
                handshake
                    .on_eapol(
                        &pmk(),
                        message2_at(&STATION_RSN, &ptk, REPLAY + answered),
                        material(&key_data, 0),
                        &mut SoftwareAes128
                    )
                    .unwrap(),
                RsnApEapol::Message3(..)
            ),
            "the Message 1 under counter {} is awaited",
            REPLAY + answered
        );
    }
}

/// While Message 4 is awaited, a repeat of the verified Message 2 is
/// ignored, and a Message 2 with another SNonce is refused as peer input:
/// neither builds Message 3 again.
#[test]
fn a_second_message2_is_ignored_with_its_snonce_and_refused_with_another() {
    let mut handshake = awaiting_message4();
    let key_data = key_data();
    assert!(matches!(
        handshake.on_eapol(
            &pmk(),
            message2(&STATION_RSN, &ptk()),
            material(&key_data, 0),
            &mut SoftwareAes128
        ),
        Ok(RsnApEapol::None)
    ));
    let rsn = OwnedRsnIe::<64>::try_copy(&STATION_RSN).unwrap();
    let other = RsnTxFrame::<512>::message2(Akm::Psk, Eui48::new(AP), REPLAY, [0x23; 32], &rsn)
        .unwrap()
        .authenticate(&ptk(), &mut SoftwareAes128)
        .unwrap();
    let other = OwnedEapolFrame::<512>::try_copy(
        RsnInterface::AccessPoint,
        Eui48::new(PEER),
        other.as_bytes(),
    )
    .unwrap();
    assert!(matches!(
        handshake.on_eapol(&pmk(), other, material(&key_data, 0), &mut SoftwareAes128),
        Err(RsnApError::Rejected(RsnStateError::UnexpectedMessage))
    ));
    // Message 3's window is as it was.
    assert_eq!(
        handshake.latest().map(|t| t.replay_counter),
        Some(REPLAY + 1)
    );
}
