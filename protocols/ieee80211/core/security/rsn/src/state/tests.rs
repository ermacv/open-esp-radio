#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects,
    clippy::unreachable
)]
use super::*;
use crate::frames::RsnTxFrame;
use oer_protocols_ieee80211_wire::eapol::{
    EAPOL_KEY_FIXED_LEN, EAPOL_KEY_PACKET_LEN, EAPOL_PACKET_TYPE_KEY, RSN_KEY_DESCRIPTOR_TYPE,
};

const PAIRWISE: u16 = 1 << 3;
const INSTALL: u16 = 1 << 6;
const ACK: u16 = 1 << 7;
const MIC: u16 = 1 << 8;
const SECURE: u16 = 1 << 9;
const ENCRYPTED: u16 = 1 << 12;
const MESSAGE3_CAPACITY: usize = 128;
const MESSAGE3_KEY_DATA: [u8; 24] = [0xa5; 24];

fn frame(
    interface: RsnInterface,
    peer: [u8; 6],
    info: u16,
    replay: u64,
    nonce: [u8; 32],
) -> OwnedEapolFrame<EAPOL_KEY_PACKET_LEN> {
    let mut bytes = [0; EAPOL_KEY_PACKET_LEN];
    bytes[0] = 2;
    bytes[1] = EAPOL_PACKET_TYPE_KEY;
    bytes[2..4].copy_from_slice(&(EAPOL_KEY_FIXED_LEN as u16).to_be_bytes());
    bytes[4] = RSN_KEY_DESCRIPTOR_TYPE;
    bytes[5..7].copy_from_slice(&(info | 2).to_be_bytes());
    bytes[9..17].copy_from_slice(&replay.to_be_bytes());
    bytes[17..49].copy_from_slice(&nonce);
    OwnedEapolFrame::try_copy(interface, peer, &bytes).unwrap()
}

fn message3(peer: [u8; 6], replay: u64, nonce: [u8; 32]) -> OwnedEapolFrame<MESSAGE3_CAPACITY> {
    let frame = RsnTxFrame::<MESSAGE3_CAPACITY>::message3(
        crate::Akm::Psk,
        peer,
        replay,
        nonce,
        [0; 8],
        &MESSAGE3_KEY_DATA,
    )
    .unwrap();
    OwnedEapolFrame::try_copy(RsnInterface::Station, peer, frame.as_bytes()).unwrap()
}

#[allow(clippy::too_many_arguments)]
fn raw_message3(
    peer: [u8; 6],
    replay: u64,
    nonce: [u8; 32],
    info: u16,
    key_length: u16,
    key_iv: [u8; 16],
    key_identifier: [u8; 8],
    key_data: &[u8],
) -> OwnedEapolFrame<MESSAGE3_CAPACITY> {
    let mut bytes = [0; MESSAGE3_CAPACITY];
    let packet_len = EAPOL_KEY_PACKET_LEN + key_data.len();
    bytes[0] = 2;
    bytes[1] = EAPOL_PACKET_TYPE_KEY;
    bytes[2..4].copy_from_slice(&((EAPOL_KEY_FIXED_LEN + key_data.len()) as u16).to_be_bytes());
    bytes[4] = RSN_KEY_DESCRIPTOR_TYPE;
    bytes[5..7].copy_from_slice(&(info | 2).to_be_bytes());
    bytes[7..9].copy_from_slice(&key_length.to_be_bytes());
    bytes[9..17].copy_from_slice(&replay.to_be_bytes());
    bytes[17..49].copy_from_slice(&nonce);
    bytes[49..65].copy_from_slice(&key_iv);
    bytes[73..81].copy_from_slice(&key_identifier);
    bytes[97..99].copy_from_slice(&(key_data.len() as u16).to_be_bytes());
    bytes[EAPOL_KEY_PACKET_LEN..packet_len].copy_from_slice(key_data);
    OwnedEapolFrame::try_copy(RsnInterface::Station, peer, &bytes[..packet_len]).unwrap()
}

#[test]
fn sta_completes_once_and_does_not_reinstall_on_message3_replay() {
    let local = [1; 6];
    let ap = [2; 6];
    let snonce = [3; 32];
    let anonce = [4; 32];
    let mut state = RsnStaState::new(crate::Akm::Psk, local, ap, snonce).unwrap();

    let ticket = match state
        .on_frame(frame(RsnInterface::Station, ap, PAIRWISE | ACK, 10, anonce))
        .unwrap()
    {
        RsnStaAction::DerivePtk { ticket, context } => {
            assert_eq!(context.authenticator_nonce, anonce);
            ticket
        }
        action => panic!("unexpected action: {action:?}"),
    };
    assert_eq!(
        state
            .complete_ptk::<EAPOL_KEY_PACKET_LEN>(ticket, true)
            .unwrap(),
        RsnStaAction::Transmit(RsnTransmit {
            message: RsnTxMessage::PairwiseMessage2,
            replay_counter: 10,
            retransmission: false,
        })
    );

    let message3 = message3(ap, 11, anonce);
    let (ticket, retained) = match state.on_frame(message3.clone()).unwrap() {
        RsnStaAction::VerifyMessage3Mic { ticket, frame } => (ticket, frame),
        action => panic!("unexpected action: {action:?}"),
    };
    let (ticket, retained) = match state.complete_message3_mic(ticket, retained, true).unwrap() {
        RsnStaAction::DecryptMessage3KeyData { ticket, frame } => (ticket, frame),
        action => panic!("unexpected action: {action:?}"),
    };
    let (ticket, retained) = match state.complete_key_data(ticket, retained, true).unwrap() {
        RsnStaAction::InstallKeys { ticket, frame } => (ticket, frame),
        action => panic!("unexpected action: {action:?}"),
    };
    assert_eq!(retained.key_frame().unwrap().replay_counter(), 11);
    assert_eq!(
        state
            .complete_key_install::<MESSAGE3_CAPACITY>(ticket, true)
            .unwrap(),
        RsnStaAction::Transmit(RsnTransmit {
            message: RsnTxMessage::PairwiseMessage4,
            replay_counter: 11,
            retransmission: false,
        })
    );
    assert_eq!(state.phase(), RsnStaPhase::Completed);
    assert_eq!(
        state.on_frame(message3).unwrap(),
        RsnStaAction::Transmit(RsnTransmit {
            message: RsnTxMessage::PairwiseMessage4,
            replay_counter: 11,
            retransmission: true,
        })
    );
    assert_eq!(state.phase(), RsnStaPhase::Completed);
}

#[test]
fn sta_rejects_stale_completion_and_stale_message3() {
    let mut state = RsnStaState::new(crate::Akm::Psk, [1; 6], [2; 6], [3; 32]).unwrap();
    let ticket = match state
        .on_frame(frame(
            RsnInterface::Station,
            [2; 6],
            PAIRWISE | ACK,
            7,
            [4; 32],
        ))
        .unwrap()
    {
        RsnStaAction::DerivePtk { ticket, .. } => ticket,
        _ => unreachable!(),
    };
    assert_eq!(
        state.complete_ptk::<EAPOL_KEY_PACKET_LEN>(RsnTicket(ticket.raw() + 1), true),
        Err(RsnStateError::StaleCompletion)
    );
    state
        .complete_ptk::<EAPOL_KEY_PACKET_LEN>(ticket, true)
        .unwrap();
    assert_eq!(
        state.on_frame(message3([2; 6], 7, [4; 32])),
        Err(RsnStateError::StaleReplayCounter)
    );
}

#[test]
fn ap_orders_crypto_install_transmit_and_authorization() {
    let ap = [1; 6];
    let station = [2; 6];
    let mut state = RsnApState::new(crate::Akm::Psk, ap, station, [3; 32], 20).unwrap();
    assert_eq!(
        state.message1(false).unwrap(),
        RsnApAction::Transmit(RsnTransmit {
            message: RsnTxMessage::PairwiseMessage1,
            replay_counter: 20,
            retransmission: false,
        })
    );

    let message2 = frame(
        RsnInterface::AccessPoint,
        station,
        PAIRWISE | MIC,
        20,
        [4; 32],
    );
    let (ticket, retained) = match state.on_frame(message2).unwrap() {
        RsnApAction::DerivePtk {
            ticket, message2, ..
        } => (ticket, message2),
        action => panic!("unexpected action: {action:?}"),
    };
    let (ticket, retained) = match state.complete_ptk(ticket, retained, true).unwrap() {
        RsnApAction::VerifyMessage2Mic { ticket, message2 } => (ticket, message2),
        action => panic!("unexpected action: {action:?}"),
    };
    let ticket = match state.complete_message2_mic(ticket, retained, true).unwrap() {
        RsnApAction::PrepareMessage3 { ticket } => ticket,
        action => panic!("unexpected action: {action:?}"),
    };
    assert_eq!(
        state
            .complete_message3_preparation::<EAPOL_KEY_PACKET_LEN>(ticket, true)
            .unwrap(),
        RsnApAction::Transmit(RsnTransmit {
            message: RsnTxMessage::PairwiseMessage3,
            replay_counter: 21,
            retransmission: false,
        })
    );

    let message4 = frame(
        RsnInterface::AccessPoint,
        station,
        PAIRWISE | MIC | SECURE,
        21,
        [0; 32],
    );
    let (ticket, retained) = match state.on_frame(message4).unwrap() {
        RsnApAction::VerifyMessage4Mic { ticket, message4 } => (ticket, message4),
        action => panic!("unexpected action: {action:?}"),
    };
    assert_eq!(
        state.complete_message4_mic(ticket, retained, true).unwrap(),
        RsnApAction::AuthorizePeer
    );
    assert_eq!(state.phase(), RsnApPhase::Authorized);
}

/// The state of a handshake whose Message 2 (counter `answered`) verified,
/// awaiting Message 4 of the Message 3 it returns.
fn ap_awaiting_message4(state: &mut RsnApState, station: [u8; 6], answered: u64) -> RsnTransmit {
    let message2 = frame(
        RsnInterface::AccessPoint,
        station,
        PAIRWISE | MIC,
        answered,
        [4; 32],
    );
    let (ticket, retained) = match state.on_frame(message2).unwrap() {
        RsnApAction::DerivePtk {
            ticket, message2, ..
        } => (ticket, message2),
        action => panic!("unexpected action: {action:?}"),
    };
    let (ticket, retained) = match state.complete_ptk(ticket, retained, true).unwrap() {
        RsnApAction::VerifyMessage2Mic { ticket, message2 } => (ticket, message2),
        action => panic!("unexpected action: {action:?}"),
    };
    let ticket = match state.complete_message2_mic(ticket, retained, true).unwrap() {
        RsnApAction::PrepareMessage3 { ticket } => ticket,
        action => panic!("unexpected action: {action:?}"),
    };
    match state
        .complete_message3_preparation::<EAPOL_KEY_PACKET_LEN>(ticket, true)
        .unwrap()
    {
        RsnApAction::Transmit(transmit) => transmit,
        action => panic!("unexpected action: {action:?}"),
    }
}

fn message4(station: [u8; 6], replay: u64) -> OwnedEapolFrame<EAPOL_KEY_PACKET_LEN> {
    frame(
        RsnInterface::AccessPoint,
        station,
        PAIRWISE | MIC | SECURE,
        replay,
        [0; 32],
    )
}

#[test]
fn ap_retransmissions_take_new_counters() {
    let station = [2; 6];
    let mut state = RsnApState::new(crate::Akm::Psk, [1; 6], station, [3; 32], 20).unwrap();
    // Before Message 3 exists there is no Message 3 to retransmit.
    assert_eq!(state.retransmit_message3(), Err(RsnStateError::WrongPhase));
    let retry = state.retransmit_message1().unwrap();
    assert_eq!(
        retry,
        RsnTransmit {
            message: RsnTxMessage::PairwiseMessage1,
            replay_counter: 21,
            retransmission: true,
        }
    );
    assert_eq!(state.outstanding_transmit(), Ok(retry));
    // Message 3 takes a counter above every Message 1 sent, and opens a new
    // window: Message 1's retransmission is behind it.
    let message3 = ap_awaiting_message4(&mut state, station, 21);
    assert_eq!(
        message3,
        RsnTransmit {
            message: RsnTxMessage::PairwiseMessage3,
            replay_counter: 22,
            retransmission: false,
        }
    );
    assert_eq!(state.outstanding_transmit(), Ok(message3));
    assert_eq!(state.retransmit_message1(), Err(RsnStateError::WrongPhase));
    let retry = state.retransmit_message3().unwrap();
    assert_eq!(
        retry,
        RsnTransmit {
            message: RsnTxMessage::PairwiseMessage3,
            replay_counter: 23,
            retransmission: true,
        }
    );
    assert_eq!(state.outstanding_transmit(), Ok(retry));
}

#[test]
fn ap_message2_answering_an_earlier_message1_stays_awaited() {
    let station = [2; 6];
    let mut state = RsnApState::new(crate::Akm::Psk, [1; 6], station, [3; 32], 20).unwrap();
    state.retransmit_message1().unwrap();
    // The station answered the first Message 1; the retransmission did not
    // reach it. Message 3 still takes a counter above both.
    let message3 = ap_awaiting_message4(&mut state, station, 20);
    assert_eq!(message3.replay_counter, 22);
}

#[test]
fn ap_completion_binds_the_answer_it_verifies() {
    let station = [2; 6];
    let mut state = RsnApState::new(crate::Akm::Psk, [1; 6], station, [3; 32], 20).unwrap();
    state.retransmit_message1().unwrap();
    let (ticket, _) = match state
        .on_frame(frame(
            RsnInterface::AccessPoint,
            station,
            PAIRWISE | MIC,
            20,
            [4; 32],
        ))
        .unwrap()
    {
        RsnApAction::DerivePtk {
            ticket, message2, ..
        } => (ticket, message2),
        action => panic!("unexpected action: {action:?}"),
    };
    // Another awaited Message 2 cannot stand in for the one being verified.
    let other = frame(
        RsnInterface::AccessPoint,
        station,
        PAIRWISE | MIC,
        21,
        [4; 32],
    );
    assert_eq!(
        state.complete_ptk(ticket, other, true),
        Err(RsnStateError::RetainedFrameMismatch)
    );

    let mut state = RsnApState::new(crate::Akm::Psk, [1; 6], station, [3; 32], 20).unwrap();
    ap_awaiting_message4(&mut state, station, 20);
    state.retransmit_message3().unwrap();
    let ticket = match state.on_frame(message4(station, 21)).unwrap() {
        RsnApAction::VerifyMessage4Mic { ticket, .. } => ticket,
        action => panic!("unexpected action: {action:?}"),
    };
    assert_eq!(
        state.complete_message4_mic(ticket, message4(station, 22), true),
        Err(RsnStateError::RetainedFrameMismatch)
    );
}

#[test]
fn ap_message4_answering_an_earlier_message3_stays_awaited_until_completion() {
    let station = [2; 6];
    let mut state = RsnApState::new(crate::Akm::Psk, [1; 6], station, [3; 32], 20).unwrap();
    assert_eq!(
        ap_awaiting_message4(&mut state, station, 20).replay_counter,
        21
    );
    assert_eq!(state.retransmit_message3().unwrap().replay_counter, 22);
    assert_eq!(state.retransmit_message3().unwrap().replay_counter, 23);
    // Message 1's counter is not a Message 3 counter, though it is near.
    assert_eq!(
        state.on_frame(message4(station, 20)),
        Err(RsnStateError::ReplayCounterMismatch)
    );
    // Message 4 of the first Message 3, after both retries were prepared.
    let (ticket, retained) = match state.on_frame(message4(station, 21)).unwrap() {
        RsnApAction::VerifyMessage4Mic { ticket, message4 } => (ticket, message4),
        action => panic!("unexpected action: {action:?}"),
    };
    // A duplicate while its MIC is verified changes nothing.
    assert_eq!(state.on_frame(message4(station, 23)), Ok(RsnApAction::None));
    assert_eq!(
        state.complete_message4_mic(ticket, retained, true).unwrap(),
        RsnApAction::AuthorizePeer
    );
    // The window closed with the step: no later Message 4 authorizes again,
    // whatever its counter.
    for replay in [21, 22, 23, 24] {
        assert_eq!(
            state.on_frame(message4(station, replay)),
            Ok(RsnApAction::None)
        );
    }
    assert_eq!(state.phase(), RsnApPhase::Authorized);
}

#[test]
fn ap_replay_window_holds_only_the_latest_counters() {
    let station = [2; 6];
    let mut state = RsnApState::new(crate::Akm::Psk, [1; 6], station, [3; 32], 20).unwrap();
    assert_eq!(
        ap_awaiting_message4(&mut state, station, 20).replay_counter,
        21
    );
    for _ in 0..RSN_AP_REPLAY_WINDOW {
        state.retransmit_message3().unwrap();
    }
    let latest = 21 + RSN_AP_REPLAY_WINDOW;
    assert_eq!(
        state
            .outstanding_transmit()
            .map(|transmit| transmit.replay_counter),
        Ok(latest)
    );
    // The first Message 3 fell out of the window, and no counter above the
    // latest was ever sent.
    for replay in [21, latest + 1, 0, u64::MAX] {
        assert_eq!(
            state.on_frame(message4(station, replay)),
            Err(RsnStateError::ReplayCounterMismatch)
        );
    }
    assert!(matches!(
        state.on_frame(message4(station, latest + 1 - RSN_AP_REPLAY_WINDOW)),
        Ok(RsnApAction::VerifyMessage4Mic { .. })
    ));
}

#[test]
fn ap_message2_outside_the_message1_window_is_refused() {
    let station = [2; 6];
    let mut state = RsnApState::new(crate::Akm::Psk, [1; 6], station, [3; 32], 20).unwrap();
    for _ in 0..RSN_AP_REPLAY_WINDOW {
        state.retransmit_message1().unwrap();
    }
    for replay in [20, 21 + RSN_AP_REPLAY_WINDOW] {
        assert_eq!(
            state.on_frame(frame(
                RsnInterface::AccessPoint,
                station,
                PAIRWISE | MIC,
                replay,
                [4; 32]
            )),
            Err(RsnStateError::ReplayCounterMismatch)
        );
    }
}

#[test]
fn ap_message1_retransmission_keeps_a_counter_for_message3() {
    let mut state =
        RsnApState::new(crate::Akm::Psk, [1; 6], [2; 6], [3; 32], u64::MAX - 2).unwrap();
    assert_eq!(
        state
            .retransmit_message1()
            .map(|transmit| transmit.replay_counter),
        Ok(u64::MAX - 1)
    );
    // One more would leave Message 3 without a larger counter.
    assert_eq!(
        state.retransmit_message1(),
        Err(RsnStateError::ReplayCounterExhausted)
    );
    assert_eq!(
        state
            .outstanding_transmit()
            .map(|transmit| transmit.replay_counter),
        Ok(u64::MAX - 1)
    );
}

#[test]
fn ap_peer_table_is_fixed_and_reports_full() {
    let mut peers = RsnApPeers::<2>::new();
    peers
        .insert(RsnApState::new(crate::Akm::Psk, [1; 6], [2; 6], [3; 32], 1).unwrap())
        .unwrap();
    assert_eq!(
        peers.insert(RsnApState::new(crate::Akm::Psk, [1; 6], [2; 6], [4; 32], 2).unwrap()),
        Err(RsnApPeerError::DuplicatePeer)
    );
    peers
        .insert(RsnApState::new(crate::Akm::Psk, [1; 6], [5; 6], [6; 32], 3).unwrap())
        .unwrap();
    assert_eq!(
        peers.insert(RsnApState::new(crate::Akm::Psk, [1; 6], [7; 6], [8; 32], 4).unwrap()),
        Err(RsnApPeerError::Full)
    );
    assert_eq!(peers.len(), 2);
    assert!(peers.remove(&[2; 6]).is_some());
    assert_eq!(peers.len(), 1);
}

#[test]
fn constructors_reject_zero_nonces() {
    assert_eq!(
        RsnStaState::new(crate::Akm::Psk, [1; 6], [2; 6], [0; 32]).err(),
        Some(RsnStateError::ZeroNonce)
    );
    assert_eq!(
        RsnApState::new(crate::Akm::Psk, [1; 6], [2; 6], [0; 32], 1).err(),
        Some(RsnStateError::ZeroNonce)
    );
}

#[test]
fn encrypted_message3_requires_owned_key_data() {
    let mut state = RsnStaState::new(crate::Akm::Psk, [1; 6], [2; 6], [3; 32]).unwrap();
    let ticket = match state
        .on_frame(frame(
            RsnInterface::Station,
            [2; 6],
            PAIRWISE | ACK,
            1,
            [4; 32],
        ))
        .unwrap()
    {
        RsnStaAction::DerivePtk { ticket, .. } => ticket,
        _ => unreachable!(),
    };
    state
        .complete_ptk::<EAPOL_KEY_PACKET_LEN>(ticket, true)
        .unwrap();
    assert_eq!(
        state.on_frame(raw_message3(
            [2; 6],
            2,
            [4; 32],
            PAIRWISE | ACK | MIC | INSTALL | SECURE | ENCRYPTED,
            RSN_CCMP_TEMPORAL_KEY_LEN,
            [0; 16],
            [0; 8],
            &[],
        )),
        Err(RsnStateError::MissingEncryptedKeyData)
    );
    assert_eq!(state.phase(), RsnStaPhase::AwaitingMessage3);
}

#[test]
fn sta_binds_message3_fields_to_the_accepted_message1() {
    fn awaiting_message3() -> RsnStaState {
        let mut state = RsnStaState::new(crate::Akm::Psk, [1; 6], [2; 6], [3; 32]).unwrap();
        let ticket = match state
            .on_frame(frame(
                RsnInterface::Station,
                [2; 6],
                PAIRWISE | ACK,
                1,
                [4; 32],
            ))
            .unwrap()
        {
            RsnStaAction::DerivePtk { ticket, .. } => ticket,
            action => panic!("unexpected action: {action:?}"),
        };
        state
            .complete_ptk::<MESSAGE3_CAPACITY>(ticket, true)
            .unwrap();
        state
    }

    let mut state = awaiting_message3();
    assert_eq!(
        state.on_frame(message3([2; 6], 2, [5; 32])),
        Err(RsnStateError::AuthenticatorNonceMismatch)
    );
    assert_eq!(state.phase(), RsnStaPhase::AwaitingMessage3);

    let mut state = awaiting_message3();
    assert_eq!(
        state.on_frame(raw_message3(
            [2; 6],
            2,
            [4; 32],
            PAIRWISE | ACK | MIC | INSTALL | SECURE,
            RSN_CCMP_TEMPORAL_KEY_LEN,
            [0; 16],
            [0; 8],
            &MESSAGE3_KEY_DATA,
        )),
        Err(RsnStateError::InvalidMessage3KeyInfo)
    );
    assert_eq!(state.phase(), RsnStaPhase::AwaitingMessage3);

    let mut state = awaiting_message3();
    assert_eq!(
        state.on_frame(raw_message3(
            [2; 6],
            2,
            [4; 32],
            PAIRWISE | ACK | MIC | INSTALL | SECURE | ENCRYPTED,
            0,
            [0; 16],
            [0; 8],
            &MESSAGE3_KEY_DATA,
        )),
        Err(RsnStateError::InvalidKeyLength)
    );
    assert_eq!(state.phase(), RsnStaPhase::AwaitingMessage3);

    let mut state = awaiting_message3();
    assert_eq!(
        state.on_frame(raw_message3(
            [2; 6],
            2,
            [4; 32],
            PAIRWISE | ACK | MIC | INSTALL | ENCRYPTED,
            RSN_CCMP_TEMPORAL_KEY_LEN,
            [0; 16],
            [0; 8],
            &MESSAGE3_KEY_DATA,
        )),
        Err(RsnStateError::InvalidMessage3KeyInfo)
    );
    assert_eq!(state.phase(), RsnStaPhase::AwaitingMessage3);

    let mut state = awaiting_message3();
    assert_eq!(
        state.on_frame(raw_message3(
            [2; 6],
            2,
            [4; 32],
            PAIRWISE | ACK | MIC | INSTALL | SECURE | ENCRYPTED,
            RSN_CCMP_TEMPORAL_KEY_LEN,
            [1; 16],
            [0; 8],
            &MESSAGE3_KEY_DATA,
        )),
        Err(RsnStateError::NonzeroKeyIv)
    );
    assert_eq!(state.phase(), RsnStaPhase::AwaitingMessage3);

    let mut state = awaiting_message3();
    assert_eq!(
        state.on_frame(raw_message3(
            [2; 6],
            2,
            [4; 32],
            PAIRWISE | ACK | MIC | INSTALL | SECURE | ENCRYPTED,
            RSN_CCMP_TEMPORAL_KEY_LEN,
            [0; 16],
            [1; 8],
            &MESSAGE3_KEY_DATA,
        )),
        Err(RsnStateError::NonzeroKeyIdentifier)
    );
    assert_eq!(state.phase(), RsnStaPhase::AwaitingMessage3);
}
