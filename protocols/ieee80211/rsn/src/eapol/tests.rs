#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects,
    clippy::unreachable
)]
use super::*;

const KEY_INFO_PAIRWISE: u16 = KeyInformation::EMPTY.with_pairwise(true).raw();
const KEY_INFO_INSTALL: u16 = KeyInformation::EMPTY.with_install(true).raw();
const KEY_INFO_ACK: u16 = KeyInformation::EMPTY.with_ack(true).raw();
const KEY_INFO_MIC: u16 = KeyInformation::EMPTY.with_mic(true).raw();
const KEY_INFO_SECURE: u16 = KeyInformation::EMPTY.with_secure(true).raw();

fn key_packet(key_info: u16, replay_counter: u64) -> [u8; EAPOL_KEY_PACKET_LEN] {
    let mut packet = [0; EAPOL_KEY_PACKET_LEN];
    packet[0] = 2;
    packet[1] = EAPOL_PACKET_TYPE_KEY;
    packet[2..4].copy_from_slice(&(EAPOL_KEY_FIXED_LEN as u16).to_be_bytes());
    packet[4] = RSN_KEY_DESCRIPTOR_TYPE;
    packet[5..7].copy_from_slice(&key_info.to_be_bytes());
    packet[9..17].copy_from_slice(&replay_counter.to_be_bytes());
    packet
}

#[test]
fn parses_and_classifies_pairwise_messages() {
    let message1 = key_packet(KEY_INFO_PAIRWISE | KEY_INFO_ACK | 2, 7);
    let parsed = EapolKeyFrame::parse(&message1).unwrap();
    assert_eq!(parsed.message(), EapolKeyMessage::PairwiseMessage1);
    assert_eq!(parsed.key_info().descriptor_version(), 2);
    assert_eq!(parsed.replay_counter(), 7);

    let message3 = key_packet(
        KEY_INFO_PAIRWISE | KEY_INFO_ACK | KEY_INFO_MIC | KEY_INFO_INSTALL | KEY_INFO_SECURE | 2,
        8,
    );
    assert_eq!(
        EapolKeyFrame::parse(&message3).unwrap().message(),
        EapolKeyMessage::PairwiseMessage3
    );
}

#[test]
fn rejects_ambiguous_lengths_and_non_rsn_descriptors() {
    let mut packet = key_packet(KEY_INFO_PAIRWISE | KEY_INFO_ACK | 2, 1);
    packet[2..4].copy_from_slice(&0_u16.to_be_bytes());
    assert_eq!(
        EapolKeyFrame::parse(&packet),
        Err(EapolParseError::LengthMismatch)
    );

    let mut packet = key_packet(KEY_INFO_PAIRWISE | KEY_INFO_ACK | 2, 1);
    packet[4] = 254;
    assert_eq!(
        EapolKeyFrame::parse(&packet),
        Err(EapolParseError::NotRsnKeyDescriptor)
    );
}

#[test]
fn a_damaged_owned_frame_returns_the_parser_error_instead_of_panicking() {
    let packet = key_packet(KEY_INFO_PAIRWISE | KEY_INFO_ACK | 2, 9);
    let mut owned =
        OwnedEapolFrame::<128>::try_copy(RsnInterface::Station, [1; 6], &packet).unwrap();
    owned.bytes[1] = 0;
    assert_eq!(owned.key_frame(), Err(EapolParseError::NotKeyPacket));
    assert_eq!(owned.as_bytes().len(), packet.len());
}
