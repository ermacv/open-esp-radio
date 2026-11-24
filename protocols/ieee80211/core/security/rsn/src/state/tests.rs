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
        state.complete_ptk::<EAPOL_KEY_PACKET_LEN>(ticket).unwrap(),
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
    let (ticket, retained) = match state.complete_message3_mic(ticket, retained).unwrap() {
        RsnStaAction::DecryptMessage3KeyData { ticket, frame } => (ticket, frame),
        action => panic!("unexpected action: {action:?}"),
    };
    let (ticket, retained) = match state.complete_key_data(ticket, retained).unwrap() {
        RsnStaAction::InstallKeys { ticket, frame } => (ticket, frame),
        action => panic!("unexpected action: {action:?}"),
    };
    assert_eq!(retained.key_frame().unwrap().replay_counter(), 11);
    assert_eq!(
        state
            .complete_key_install::<MESSAGE3_CAPACITY>(ticket)
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
        state.complete_ptk::<EAPOL_KEY_PACKET_LEN>(RsnTicket(ticket.raw() + 1)),
        Err(RsnStateError::StaleCompletion)
    );
    state.complete_ptk::<EAPOL_KEY_PACKET_LEN>(ticket).unwrap();
    assert_eq!(
        state.on_frame(message3([2; 6], 7, [4; 32])),
        Err(RsnStateError::StaleReplayCounter)
    );
}

#[test]
fn constructors_reject_zero_nonces() {
    assert_eq!(
        RsnStaState::new(crate::Akm::Psk, [1; 6], [2; 6], [0; 32]).err(),
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
    state.complete_ptk::<EAPOL_KEY_PACKET_LEN>(ticket).unwrap();
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
        state.complete_ptk::<MESSAGE3_CAPACITY>(ticket).unwrap();
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

/// A station whose Message 3 (counter 11) awaits its MIC check.
fn sta_verifying_message3() -> (RsnStaState, RsnTicket, OwnedEapolFrame<MESSAGE3_CAPACITY>) {
    let ap = [2; 6];
    let anonce = [4; 32];
    let mut state = RsnStaState::new(crate::Akm::Psk, [1; 6], ap, [3; 32]).unwrap();
    let RsnStaAction::DerivePtk { ticket, .. } = state
        .on_frame(frame(RsnInterface::Station, ap, PAIRWISE | ACK, 10, anonce))
        .unwrap()
    else {
        panic!("Message 1 derives the PTK");
    };
    state.complete_ptk::<EAPOL_KEY_PACKET_LEN>(ticket).unwrap();
    let RsnStaAction::VerifyMessage3Mic { ticket, frame } =
        state.on_frame(message3(ap, 11, anonce)).unwrap()
    else {
        panic!("Message 3 is verified");
    };
    (state, ticket, frame)
}

#[test]
fn sta_a_failed_message3_mic_awaits_message3_again_and_closes_its_ticket() {
    let (mut state, ticket, frame) = sta_verifying_message3();
    state.fail_message3_mic(ticket, frame.clone()).unwrap();
    assert_eq!(state.phase(), RsnStaPhase::AwaitingMessage3);
    assert!(state.complete_message3_mic(ticket, frame.clone()).is_err());
    assert_eq!(state.phase(), RsnStaPhase::AwaitingMessage3);
    // The real Message 3 is accepted afterwards, under the same counter.
    assert!(matches!(
        state.on_frame(frame).unwrap(),
        RsnStaAction::VerifyMessage3Mic { .. }
    ));
}

#[test]
fn sta_failed_key_data_and_key_install_fail_the_handshake_and_close_their_tickets() {
    let (mut state, ticket, frame) = sta_verifying_message3();
    let RsnStaAction::DecryptMessage3KeyData { ticket, frame } =
        state.complete_message3_mic(ticket, frame).unwrap()
    else {
        panic!("encrypted key data is unwrapped");
    };
    state.fail_key_data(ticket, frame.clone()).unwrap();
    assert_eq!(state.phase(), RsnStaPhase::Failed);
    assert!(state.complete_key_data(ticket, frame).is_err());
    assert_eq!(state.phase(), RsnStaPhase::Failed);

    let (mut state, ticket, frame) = sta_verifying_message3();
    let RsnStaAction::DecryptMessage3KeyData { ticket, frame } =
        state.complete_message3_mic(ticket, frame).unwrap()
    else {
        panic!("encrypted key data is unwrapped");
    };
    let RsnStaAction::InstallKeys { ticket, .. } = state.complete_key_data(ticket, frame).unwrap()
    else {
        panic!("its keys are installed next");
    };
    state.fail_key_install(ticket).unwrap();
    assert_eq!(state.phase(), RsnStaPhase::Failed);
    assert!(
        state
            .complete_key_install::<MESSAGE3_CAPACITY>(ticket)
            .is_err()
    );
    assert_eq!(state.phase(), RsnStaPhase::Failed);
}
