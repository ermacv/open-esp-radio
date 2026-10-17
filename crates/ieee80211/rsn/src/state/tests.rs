use super::*;
use crate::{
    EAPOL_KEY_FIXED_LEN, EAPOL_KEY_PACKET_LEN, EAPOL_PACKET_TYPE_KEY, RSN_KEY_DESCRIPTOR_TYPE,
    frames::RsnTxFrame,
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
    assert_eq!(retained.key_frame().replay_counter(), 11);
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
