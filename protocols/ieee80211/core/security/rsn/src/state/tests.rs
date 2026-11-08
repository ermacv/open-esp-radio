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
    let message1 = RsnTransmit {
        message: RsnTxMessage::PairwiseMessage1,
        replay_counter: 20,
        retransmission: false,
    };
    assert_eq!(
        state.message1(false).unwrap(),
        RsnApAction::Transmit(message1)
    );
    state.record_built(message1).unwrap();

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
    let message3 = RsnTransmit {
        message: RsnTxMessage::PairwiseMessage3,
        replay_counter: 21,
        retransmission: false,
    };
    assert_eq!(
        state
            .complete_message3_preparation::<EAPOL_KEY_PACKET_LEN>(ticket, true)
            .unwrap(),
        RsnApAction::Transmit(message3)
    );
    state.record_built(message3).unwrap();

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

/// A handshake whose first Message 1 (counter `initial`) was built.
fn ap_state(station: [u8; 6], initial: u64) -> RsnApState {
    let mut state = RsnApState::new(crate::Akm::Psk, [1; 6], station, [3; 32], initial).unwrap();
    let RsnApAction::Transmit(message1) = state.message1(false).unwrap() else {
        panic!("Message 1 is a transmission");
    };
    state.record_built(message1).unwrap();
    state
}

/// The state of a handshake whose Message 2 (counter `answered`) verified,
/// awaiting Message 4 of the Message 3 it returns, which was built.
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
        RsnApAction::Transmit(transmit) => {
            state.record_built(transmit).unwrap();
            transmit
        }
        action => panic!("unexpected action: {action:?}"),
    }
}

/// A Message 1 retransmission whose frame was built.
fn built_message1_retry(state: &mut RsnApState) -> RsnTransmit {
    let retry = state.retransmit_message1().unwrap();
    state.record_built(retry).unwrap();
    retry
}

/// A Message 3 retransmission whose frame was built.
fn built_message3_retry(state: &mut RsnApState) -> RsnTransmit {
    let retry = state.retransmit_message3().unwrap();
    state.record_built(retry).unwrap();
    retry
}

fn message2(station: [u8; 6], replay: u64) -> OwnedEapolFrame<EAPOL_KEY_PACKET_LEN> {
    frame(
        RsnInterface::AccessPoint,
        station,
        PAIRWISE | MIC,
        replay,
        [4; 32],
    )
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
    let mut state = ap_state(station, 20);
    // Before Message 3 exists there is no Message 3 to retransmit.
    assert_eq!(state.retransmit_message3(), Err(RsnStateError::WrongPhase));
    let retry = built_message1_retry(&mut state);
    assert_eq!(
        retry,
        RsnTransmit {
            message: RsnTxMessage::PairwiseMessage1,
            replay_counter: 21,
            retransmission: true,
        }
    );
    assert_eq!(state.outstanding_transmit(), Ok(Some(retry)));
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
    assert_eq!(state.outstanding_transmit(), Ok(Some(message3)));
    assert_eq!(state.retransmit_message1(), Err(RsnStateError::WrongPhase));
    let retry = built_message3_retry(&mut state);
    assert_eq!(
        retry,
        RsnTransmit {
            message: RsnTxMessage::PairwiseMessage3,
            replay_counter: 23,
            retransmission: true,
        }
    );
    assert_eq!(state.outstanding_transmit(), Ok(Some(retry)));
}

#[test]
fn ap_only_built_transmissions_are_recorded() {
    let station = [2; 6];
    let mut state = ap_state(station, 20);
    // A counter of another step, or one never handed out, is not recorded.
    for transmit in [
        RsnTransmit {
            message: RsnTxMessage::PairwiseMessage3,
            replay_counter: 20,
            retransmission: false,
        },
        RsnTransmit {
            message: RsnTxMessage::PairwiseMessage1,
            replay_counter: 21,
            retransmission: true,
        },
    ] {
        assert_eq!(
            state.record_built(transmit),
            Err(RsnStateError::StaleCompletion)
        );
    }
    assert_eq!(
        state.on_frame(message2(station, 21)),
        Err(RsnStateError::ReplayCounterMismatch)
    );
}

#[test]
fn ap_message2_answering_an_earlier_message1_stays_awaited() {
    let station = [2; 6];
    let mut state = ap_state(station, 20);
    built_message1_retry(&mut state);
    // The station answered the first Message 1; the retransmission did not
    // reach it. Message 3 still takes a counter above both.
    let message3 = ap_awaiting_message4(&mut state, station, 20);
    assert_eq!(message3.replay_counter, 22);
}

#[test]
fn ap_a_message1_that_failed_to_build_is_not_awaited() {
    let station = [2; 6];
    let mut state = ap_state(station, 20);
    // The retry's counter is spent, but its frame was never built.
    assert_eq!(state.retransmit_message1().unwrap().replay_counter, 21);
    assert_eq!(
        state.on_frame(message2(station, 21)),
        Err(RsnStateError::ReplayCounterMismatch)
    );
    // The next retry takes a newer counter and is awaited; the hole stays.
    assert_eq!(built_message1_retry(&mut state).replay_counter, 22);
    assert_eq!(
        state.on_frame(message2(station, 21)),
        Err(RsnStateError::ReplayCounterMismatch)
    );
    assert_eq!(
        ap_awaiting_message4(&mut state, station, 22).replay_counter,
        23
    );
}

#[test]
fn ap_a_message3_that_failed_to_build_is_not_awaited() {
    let station = [2; 6];
    let mut state = ap_state(station, 20);
    assert_eq!(
        ap_awaiting_message4(&mut state, station, 20).replay_counter,
        21
    );
    assert_eq!(state.retransmit_message3().unwrap().replay_counter, 22);
    assert_eq!(
        state.on_frame(message4(station, 22)),
        Err(RsnStateError::ReplayCounterMismatch)
    );
    assert_eq!(
        state
            .outstanding_transmit()
            .map(|t| t.map(|t| t.replay_counter)),
        Ok(Some(21))
    );
    assert_eq!(built_message3_retry(&mut state).replay_counter, 23);
    assert_eq!(
        state.on_frame(message4(station, 22)),
        Err(RsnStateError::ReplayCounterMismatch)
    );
    // The earlier built Message 3 is still awaited.
    assert!(matches!(
        state.on_frame(message4(station, 21)),
        Ok(RsnApAction::VerifyMessage4Mic { .. })
    ));
}

#[test]
fn ap_message3_owns_no_window_until_its_frame_is_built() {
    let station = [2; 6];
    let mut state = ap_state(station, 20);
    let (ticket, retained) = match state.on_frame(message2(station, 20)).unwrap() {
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
    let RsnApAction::Transmit(message3) = state
        .complete_message3_preparation::<EAPOL_KEY_PACKET_LEN>(ticket, true)
        .unwrap()
    else {
        panic!("Message 3 is a transmission");
    };
    // Message 1's window is behind; Message 3's opens with its built frame.
    assert_eq!(state.outstanding_transmit(), Ok(None));
    assert_eq!(
        state.on_frame(message4(station, 21)),
        Err(RsnStateError::ReplayCounterMismatch)
    );
    state.record_built(message3).unwrap();
    assert_eq!(state.outstanding_transmit(), Ok(Some(message3)));
}

#[test]
fn ap_failed_builds_do_not_evict_built_transmissions() {
    let station = [2; 6];
    let mut state = ap_state(station, 20);
    assert_eq!(
        ap_awaiting_message4(&mut state, station, 20).replay_counter,
        21
    );
    for _ in 0..RSN_AP_REPLAY_WINDOW + 2 {
        state.retransmit_message3().unwrap();
    }
    assert!(matches!(
        state.on_frame(message4(station, 21)),
        Ok(RsnApAction::VerifyMessage4Mic { .. })
    ));

    let mut state = ap_state(station, 20);
    for _ in 0..RSN_AP_REPLAY_WINDOW + 2 {
        state.retransmit_message1().unwrap();
    }
    assert!(matches!(
        state.on_frame(message2(station, 20)),
        Ok(RsnApAction::DerivePtk { .. })
    ));
}

#[test]
fn ap_completion_binds_the_answer_it_verifies() {
    let station = [2; 6];
    let mut state = ap_state(station, 20);
    built_message1_retry(&mut state);
    let ticket = match state.on_frame(message2(station, 20)).unwrap() {
        RsnApAction::DerivePtk { ticket, .. } => ticket,
        action => panic!("unexpected action: {action:?}"),
    };
    // Another awaited Message 2 cannot stand in for the one being verified.
    assert_eq!(
        state.complete_ptk(ticket, message2(station, 21), true),
        Err(RsnStateError::RetainedFrameMismatch)
    );

    let mut state = ap_state(station, 20);
    ap_awaiting_message4(&mut state, station, 20);
    built_message3_retry(&mut state);
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
    let mut state = ap_state(station, 20);
    assert_eq!(
        ap_awaiting_message4(&mut state, station, 20).replay_counter,
        21
    );
    assert_eq!(built_message3_retry(&mut state).replay_counter, 22);
    assert_eq!(built_message3_retry(&mut state).replay_counter, 23);
    // Message 1's counter is not a Message 3 counter, though it is near.
    assert_eq!(
        state.on_frame(message4(station, 20)),
        Err(RsnStateError::ReplayCounterMismatch)
    );
    // Message 4 of the first Message 3, after both retries were built.
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
fn ap_replay_window_holds_only_the_latest_built_transmissions() {
    let station = [2; 6];
    let mut state = ap_state(station, 20);
    assert_eq!(
        ap_awaiting_message4(&mut state, station, 20).replay_counter,
        21
    );
    let mut latest = 21;
    for _ in 0..RSN_AP_REPLAY_WINDOW {
        latest = built_message3_retry(&mut state).replay_counter;
    }
    assert_eq!(
        state
            .outstanding_transmit()
            .map(|t| t.map(|t| t.replay_counter)),
        Ok(Some(latest))
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
        state.on_frame(message4(station, 22)),
        Ok(RsnApAction::VerifyMessage4Mic { .. })
    ));
}

#[test]
fn ap_message2_outside_the_message1_window_is_refused() {
    let station = [2; 6];
    let mut state = ap_state(station, 20);
    let mut latest = 20;
    for _ in 0..RSN_AP_REPLAY_WINDOW {
        latest = built_message1_retry(&mut state).replay_counter;
    }
    for replay in [20, latest + 1] {
        assert_eq!(
            state.on_frame(message2(station, replay)),
            Err(RsnStateError::ReplayCounterMismatch)
        );
    }
}

#[test]
fn ap_message1_retransmission_keeps_a_counter_for_message3() {
    let mut state = ap_state([2; 6], u64::MAX - 2);
    assert_eq!(
        built_message1_retry(&mut state).replay_counter,
        u64::MAX - 1
    );
    // One more would leave Message 3 without a larger counter.
    assert_eq!(
        state.retransmit_message1(),
        Err(RsnStateError::ReplayCounterExhausted)
    );
    assert_eq!(
        state
            .outstanding_transmit()
            .map(|transmit| transmit.map(|transmit| transmit.replay_counter)),
        Ok(Some(u64::MAX - 1))
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
