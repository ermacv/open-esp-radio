use super::*;

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
    assert_eq!(bip.verify(&deauthentication(MIC, 9)), Err(BipError::Replay));
}

#[test]
fn a_changed_frame_or_mic_fails() {
    let mut bip = receiver(8);
    let mut frame = deauthentication(MIC, 9);
    frame[24] = 8;
    assert_eq!(bip.verify(&frame), Err(BipError::InvalidMic));
    let mut mic = MIC;
    mic[0] ^= 1;
    assert_eq!(
        bip.verify(&deauthentication(mic, 9)),
        Err(BipError::InvalidMic)
    );
    // A failed frame advances nothing.
    assert_eq!(bip.verify(&deauthentication(MIC, 9)), Ok(()));
}

#[test]
fn an_ipn_not_above_the_kde_ipn_is_a_replay() {
    let mut bip = receiver(9);
    assert_eq!(bip.verify(&deauthentication(MIC, 9)), Err(BipError::Replay));
}

#[test]
fn a_frame_without_its_mic_element_or_another_key_fails() {
    let mut bip = receiver(8);
    let mut frame = deauthentication(MIC, 9);
    frame[26] = 75;
    assert_eq!(bip.verify(&frame), Err(BipError::MissingManagementMic));
    let mut frame = deauthentication(MIC, 9);
    frame[28] = 5;
    assert_eq!(bip.verify(&frame), Err(BipError::UnknownKeyId));
    assert_eq!(bip.verify(&[0; 30]), Err(BipError::Truncated));
}

#[test]
fn a_repeated_igtk_keeps_its_replay_state_and_a_new_one_resets_it() {
    let mut bip = receiver(8);
    assert_eq!(bip.verify(&deauthentication(MIC, 9)), Ok(()));
    // The same IGTK redelivered with an older IPN.
    bip.rekey(&RsnIgtk::new(4, [2, 0, 0, 0, 0, 0], &[0x77; 16]).unwrap());
    assert_eq!(bip.verify(&deauthentication(MIC, 9)), Err(BipError::Replay));
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
    assert_eq!(transmitter.protect(&mut frame, 26), Some(expected.len()));
    assert_eq!(frame[..], expected[..]);

    let mut receiver = BipReceiver::new(&igtk);
    assert_eq!(receiver.verify(&frame), Ok(()));
    let mut next = [0_u8; 26 + MANAGEMENT_MIC_ELEMENT_LEN];
    next[..26].copy_from_slice(&expected[..26]);
    transmitter.protect(&mut next, 26).unwrap();
    assert_eq!(next[30], 10);
    assert_eq!(receiver.verify(&next), Ok(()));
    assert_eq!(transmitter.protect(&mut next[..30], 26), None);
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
    assert_eq!(sender.protect(&mut packet[..26], 26), None);
    let length = sender.protect(&mut packet, 26).unwrap();
    assert_eq!(receiver.verify(&packet[..length]), Ok(()));
    let saved = packet;
    assert_eq!(sender.protect(&mut packet, 26), None);
    assert_eq!(packet, saved);
    let exhausted = crate::frames::RsnIgtk::new(4, [0xff; 6], &[0x66; 16]).unwrap();
    assert_eq!(
        BipTransmitter::new(&exhausted).protect(&mut packet, 26),
        None
    );
}
