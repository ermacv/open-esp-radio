#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects,
    clippy::unreachable
)]
use super::*;
use oer_ieee80211_cipher::bip::MANAGEMENT_MIC_ELEMENT_LEN;

use std::vec::Vec;

/// A group-addressed Deauthentication with Retry and Power Management set,
/// reason 7, protected under IGTK 4 at IPN 9. The MIC was computed
/// independently with Python's cryptography CMAC.
fn deauthentication(mic: [u8; 8], ipn: u8) -> Vec<u8> {
    let mut frame = Vec::new();
    frame.extend_from_slice(&[0xc0, 0x18, 0, 0]);
    frame.extend_from_slice(&[0xff; 6]);
    frame.extend_from_slice(&[2, 0, 0, 0, 0, 1]);
    frame.extend_from_slice(&[2, 0, 0, 0, 0, 1]);
    frame.extend_from_slice(&[0x10, 0x00]);
    frame.extend_from_slice(&[7, 0]);
    frame.extend_from_slice(&[76, 16, 4, 0, ipn, 0, 0, 0, 0, 0]);
    frame.extend_from_slice(&mic);
    frame
}

const MIC: [u8; 8] = [0xeb, 0x79, 0x2a, 0x77, 0x23, 0x5b, 0xf1, 0x7a];

fn receiver(ipn: u8) -> BipReceiver {
    BipReceiver::new(&RsnIgtk::new(4, [ipn, 0, 0, 0, 0, 0], &[0x77; 16]).unwrap())
}

#[test]
fn a_frame_under_the_igtk_verifies_once() {
    let mut bip = receiver(8);
    assert_eq!(bip.verify(&deauthentication(MIC, 9)), Ok(()));
    // The same IPN again is a replay.
    assert_eq!(
        bip.verify(&deauthentication(MIC, 9)),
        Err(BipReceiveError::Replay)
    );
}

#[test]
fn a_changed_frame_or_mic_fails() {
    let mut bip = receiver(8);
    let mut frame = deauthentication(MIC, 9);
    frame[24] = 8;
    assert_eq!(
        bip.verify(&frame),
        Err(BipReceiveError::Frame(BipError::InvalidMic))
    );
    let mut mic = MIC;
    mic[0] ^= 1;
    assert_eq!(
        bip.verify(&deauthentication(mic, 9)),
        Err(BipReceiveError::Frame(BipError::InvalidMic))
    );
    // A failed frame advances nothing.
    assert_eq!(bip.verify(&deauthentication(MIC, 9)), Ok(()));
}

#[test]
fn an_ipn_not_above_the_kde_ipn_is_a_replay() {
    let mut bip = receiver(9);
    assert_eq!(
        bip.verify(&deauthentication(MIC, 9)),
        Err(BipReceiveError::Replay)
    );
}

#[test]
fn a_frame_without_its_mic_element_or_another_key_fails() {
    let mut bip = receiver(8);
    let mut frame = deauthentication(MIC, 9);
    frame[26] = 75;
    assert_eq!(
        bip.verify(&frame),
        Err(BipReceiveError::Frame(BipError::MissingManagementMic))
    );
    let mut frame = deauthentication(MIC, 9);
    frame[28] = 5;
    assert_eq!(
        bip.verify(&frame),
        Err(BipReceiveError::Frame(BipError::UnknownKeyId))
    );
    assert_eq!(
        bip.verify(&[0; 30]),
        Err(BipReceiveError::Frame(BipError::Truncated))
    );
}

#[test]
fn a_repeated_igtk_keeps_its_replay_state_and_a_new_one_resets_it() {
    let mut bip = receiver(8);
    assert_eq!(bip.verify(&deauthentication(MIC, 9)), Ok(()));
    // The same IGTK redelivered with an older IPN.
    bip.rekey(&RsnIgtk::new(4, [2, 0, 0, 0, 0, 0], &[0x77; 16]).unwrap());
    assert_eq!(
        bip.verify(&deauthentication(MIC, 9)),
        Err(BipReceiveError::Replay)
    );
    // Another IGTK starts from its own IPN.
    bip.rekey(&RsnIgtk::new(5, [2, 0, 0, 0, 0, 0], &[0x11; 16]).unwrap());
    assert_eq!(bip.key_id(), 5);
}

#[test]
fn a_transmitter_reproduces_the_independent_mic_and_advances_its_ipn() {
    let igtk = RsnIgtk::new(4, [8, 0, 0, 0, 0, 0], &[0x77; 16]).unwrap();
    let mut transmitter = BipTransmitter::new(&igtk);
    let expected = deauthentication(MIC, 9);
    let mut frame = [0_u8; 26 + MANAGEMENT_MIC_ELEMENT_LEN];
    frame[..26].copy_from_slice(&expected[..26]);
    assert_eq!(transmitter.protect(&mut frame, 26), Ok(expected.len()));
    assert_eq!(frame[..], expected[..]);

    let mut receiver = BipReceiver::new(&igtk);
    assert_eq!(receiver.verify(&frame), Ok(()));
    let mut next = [0_u8; 26 + MANAGEMENT_MIC_ELEMENT_LEN];
    next[..26].copy_from_slice(&expected[..26]);
    transmitter.protect(&mut next, 26).unwrap();
    assert_eq!(next[30], 10);
    assert_eq!(receiver.verify(&next), Ok(()));
    assert_eq!(
        transmitter.protect(&mut next[..30], 26),
        Err(BipTransmitError::InvalidFrame)
    );
    assert_eq!(transmitter.packet_number(), [10, 0, 0, 0, 0, 0]);
}

#[test]
fn transmit_ipns_never_wrap_and_failed_capacity_checks_consume_nothing() {
    let igtk =
        crate::frames::RsnIgtk::new(4, [0xfe, 0xff, 0xff, 0xff, 0xff, 0xff], &[0x66; 16]).unwrap();
    let mut sender = BipTransmitter::new(&igtk);
    let mut receiver = BipReceiver::new(&igtk);
    let mut packet = [0; 64];
    packet[0] = 0xd0;
    packet[4..10].fill(0xff);
    packet[10..22].fill(2);
    packet[24..26].copy_from_slice(&[0, 4]);
    let original = packet;
    assert_eq!(
        sender.protect(&mut packet[..26], 26),
        Err(BipTransmitError::InvalidFrame)
    );
    assert_eq!(packet, original);
    assert_eq!(sender.packet_number(), igtk.packet_number());
    let length = sender.protect(&mut packet, 26).unwrap();
    assert_eq!(sender.packet_number(), [0xff; 6]);
    assert_eq!(&packet[30..36], &[0xff; 6]);
    assert_eq!(receiver.verify(&packet[..length]), Ok(()));
    assert_eq!(
        receiver.verify(&packet[..length]),
        Err(BipReceiveError::Replay)
    );
    let saved = packet;
    for _ in 0..2 {
        assert_eq!(
            sender.protect(&mut packet, 26),
            Err(BipTransmitError::Exhausted)
        );
        assert_eq!(sender.packet_number(), [0xff; 6]);
        assert_eq!(packet, saved);
    }
    // A correctly authenticated, older IPN cannot roll back the maximum frontier.
    let older = RsnIgtk::new(4, [0xfd, 0xff, 0xff, 0xff, 0xff, 0xff], &[0x66; 16]).unwrap();
    let length = BipTransmitter::new(&older)
        .protect(&mut packet, 26)
        .unwrap();
    assert_eq!(
        receiver.verify(&packet[..length]),
        Err(BipReceiveError::Replay)
    );

    let exhausted = crate::frames::RsnIgtk::new(4, [0xff; 6], &[0x66; 16]).unwrap();
    let mut sender = BipTransmitter::new(&exhausted);
    let saved = packet;
    for _ in 0..2 {
        assert_eq!(
            sender.protect(&mut packet, 26),
            Err(BipTransmitError::Exhausted)
        );
        assert_eq!(sender.packet_number(), exhausted.packet_number());
        assert_eq!(packet, saved);
    }
}

#[test]
fn invalid_lengths_do_not_mutate_the_frame_or_consume_an_ipn() {
    let igtk = RsnIgtk::new(4, [0; 6], &[0x77; 16]).unwrap();
    let mut sender = BipTransmitter::new(&igtk);
    let mut frame = [0xab; 64];
    for length in [0, 23, 64, usize::MAX] {
        assert_eq!(
            sender.protect(&mut frame, length),
            Err(BipTransmitError::InvalidFrame)
        );
        assert_eq!(frame, [0xab; 64]);
        assert_eq!(sender.packet_number(), [0; 6]);
    }
}

#[test]
fn ipn_advancement_carries_octets_and_refuses_both_domain_and_integer_overflow() {
    use oer_ieee80211_upper_mac::packet_number::{PacketNumberExhausted, advance};

    let last = BipPacketNumber::from_bytes([0xff, 0, 0, 0, 0, 0]);
    assert_eq!(advance(last, 1).unwrap().to_bytes(), [0, 1, 0, 0, 0, 0]);
    assert_eq!(advance(last, u64::MAX), Err(PacketNumberExhausted));
    let maximum = BipPacketNumber::from_bytes([0xff; 6]);
    assert_eq!(advance(maximum, 1), Err(PacketNumberExhausted));
    assert_eq!(maximum.to_bytes(), [0xff; 6]);
}

#[test]
fn forged_higher_ipns_and_repeated_key_delivery_cannot_roll_back_the_frontier() {
    let mut bip = receiver(8);
    assert_eq!(
        bip.verify(&deauthentication(MIC, 100)),
        Err(BipReceiveError::Frame(BipError::InvalidMic))
    );
    assert_eq!(bip.verify(&deauthentication(MIC, 9)), Ok(()));
    bip.rekey(&RsnIgtk::new(4, [10, 0, 0, 0, 0, 0], &[0x77; 16]).unwrap());
    bip.rekey(&RsnIgtk::new(4, [0; 6], &[0x77; 16]).unwrap());
    let igtk = RsnIgtk::new(4, [9, 0, 0, 0, 0, 0], &[0x77; 16]).unwrap();
    let mut sender = BipTransmitter::new(&igtk);
    let mut frame = deauthentication(MIC, 9);
    let length = sender.protect(&mut frame, 26).unwrap();
    assert_eq!(bip.verify(&frame[..length]), Err(BipReceiveError::Replay));
    let length = sender.protect(&mut frame, 26).unwrap();
    assert_eq!(bip.verify(&frame[..length]), Ok(()));
    // New material starts its own epoch, even when it reuses the Key ID.
    let replacement = RsnIgtk::new(4, [0; 6], &[0x11; 16]).unwrap();
    bip.rekey(&replacement);
    let length = BipTransmitter::new(&replacement)
        .protect(&mut frame, 26)
        .unwrap();
    assert_eq!(bip.verify(&frame[..length]), Ok(()));
}
