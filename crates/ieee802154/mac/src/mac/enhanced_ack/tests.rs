//! Expectations are encoded by hand from OpenThread `TxFrame::GenerateEnhAck`
//! and `BuildInfo::PrepareHeadersIn`, not taken from this module's output.

use std::{vec, vec::Vec};

use super::{EnhancedAckError, KeyIdMode, generate_enhanced_ack};
use crate::FrameView;
use crate::mac::security::MacKeys;

const EXT_DST: [u8; 8] = [0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17];
const EXT_SRC: [u8; 8] = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27];

fn ack(received: &[u8], pending: bool, ies: &[u8]) -> Result<Vec<u8>, EnhancedAckError> {
    generate_enhanced_ack(FrameView::new(received).unwrap(), pending, ies)
        .map(|ack| ack.bytes().to_vec())
}

fn concat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

/// Short to short with PAN ID compression (2015 row 14): the ACK carries
/// the destination PAN ID, the echoed sequence and the source as its
/// destination, with no PAN ID compression (row 3) and no source.
#[test]
fn short_addresses_echo_the_pan_and_address_the_source() {
    // Data, AR, PAN ID compression, short destination, 2015, short source.
    let received = [0x61, 0xa8, 0x5a, 0x34, 0x12, 0x01, 0x00, 0x02, 0x00, 0xaa];
    assert_eq!(
        ack(&received, false, &[]),
        Ok(vec![0x02, 0x28, 0x5a, 0x34, 0x12, 0x02, 0x00])
    );
    // The frame-pending bit.
    assert_eq!(
        ack(&received, true, &[]),
        Ok(vec![0x12, 0x28, 0x5a, 0x34, 0x12, 0x02, 0x00])
    );
}

/// Two extended addresses with PAN ID compression (row 8) carry no PAN ID,
/// so the ACK compresses its absent PAN ID (row 4).
#[test]
fn extended_addresses_without_a_pan_compress_the_ack() {
    let received = concat(&[&[0x61, 0xec, 0x07], &EXT_DST, &EXT_SRC]);
    assert_eq!(
        ack(&received, false, &[]),
        Ok(concat(&[&[0x42, 0x2c, 0x07], &EXT_SRC]))
    );
}

/// Two extended addresses without compression (row 7) carry only the
/// destination PAN ID, which the ACK echoes.
#[test]
fn extended_addresses_with_a_destination_pan_echo_it() {
    let received = concat(&[&[0x21, 0xec, 0x07, 0xcd, 0xab], &EXT_DST, &EXT_SRC]);
    assert_eq!(
        ack(&received, false, &[]),
        Ok(concat(&[&[0x02, 0x2c, 0x07, 0xcd, 0xab], &EXT_SRC]))
    );
}

/// With both PAN IDs present (row 10) the source PAN ID is echoed.
#[test]
fn a_present_source_pan_takes_precedence() {
    let received = concat(&[
        &[0x21, 0xe8, 0x09, 0x34, 0x12, 0x01, 0x00, 0x78, 0x56],
        &EXT_SRC,
    ]);
    assert_eq!(
        ack(&received, false, &[]),
        Ok(concat(&[&[0x02, 0x2c, 0x09, 0x78, 0x56], &EXT_SRC]))
    );
}

/// A frame secured at ENC-MIC-32 with key identifier mode 1 gets a secured
/// ACK: the same security control, a zero frame counter the caller sets,
/// the mirrored key index and a four-byte MIC placeholder.
#[test]
fn a_secured_frame_gets_a_secured_ack() {
    let received = [
        0x69, 0xa8, 0x11, 0x34, 0x12, 0x01, 0x00, 0x02, 0x00, // header
        0x0d, 0x01, 0x02, 0x03, 0x04, 0x03, // ENC-MIC-32, mode 1, counter, index 3
        0xde, 0xad, 0x00, 0x00, 0x00, 0x00,
    ];
    let mut generated =
        generate_enhanced_ack(FrameView::new(&received).unwrap(), false, &[]).unwrap();
    let security = generated.security().unwrap();
    assert_eq!(security.key_id_mode, KeyIdMode::Index);
    assert_eq!(security.key_index, Some(3));
    assert_eq!(
        generated.bytes(),
        [
            0x0a, 0x28, 0x11, 0x34, 0x12, 0x02, 0x00, // header, security enabled
            0x0d, 0x00, 0x00, 0x00, 0x00, 0x03, // counter to be set, index
            0x00, 0x00, 0x00, 0x00, // MIC
        ]
    );
    generated.set_frame_counter(0x0102_0304);
    assert_eq!(generated.bytes()[8..12], [0x04, 0x03, 0x02, 0x01]);
}

/// Header IEs follow the security header and set IE present; the MIC stays
/// last.
#[test]
fn header_ies_follow_the_security_header() {
    let ies = [0x04, 0x0d, 0xaa, 0xbb, 0xcc, 0xdd];
    let plain = [0x61, 0xa8, 0x5a, 0x34, 0x12, 0x01, 0x00, 0x02, 0x00];
    assert_eq!(
        ack(&plain, false, &ies),
        Ok(concat(&[&[0x02, 0x2a, 0x5a, 0x34, 0x12, 0x02, 0x00], &ies]))
    );
    let secured = [
        0x69, 0xa8, 0x11, 0x34, 0x12, 0x01, 0x00, 0x02, 0x00, 0x0d, 0x01, 0x02, 0x03, 0x04, 0x03,
    ];
    assert_eq!(
        ack(&secured, false, &ies),
        Ok(concat(&[
            &[0x0a, 0x2a, 0x11, 0x34, 0x12, 0x02, 0x00],
            &[0x0d, 0x00, 0x00, 0x00, 0x00, 0x03],
            &ies,
            &[0x00; 4],
        ]))
    );
}

/// Key identifier mode 0 has no key index; mode 2 keeps a zero key source.
#[test]
fn key_identifier_modes_size_the_security_header() {
    let implicit = [
        0x69, 0xa8, 0x11, 0x34, 0x12, 0x01, 0x00, 0x02, 0x00, 0x05, 0x01, 0x02, 0x03, 0x04,
    ];
    let generated = generate_enhanced_ack(FrameView::new(&implicit).unwrap(), false, &[]).unwrap();
    assert_eq!(generated.security().unwrap().key_index, None);
    assert_eq!(generated.bytes().len(), 7 + 5 + 4);

    let source4 = [
        0x69, 0xa8, 0x11, 0x34, 0x12, 0x01, 0x00, 0x02, 0x00, 0x15, 0x01, 0x02, 0x03, 0x04, 0xa1,
        0xa2, 0xa3, 0xa4, 0x07,
    ];
    let generated = generate_enhanced_ack(FrameView::new(&source4).unwrap(), false, &[]).unwrap();
    assert_eq!(
        generated.security().unwrap().key_id_mode,
        KeyIdMode::Source4
    );
    assert_eq!(
        generated.bytes()[7..17],
        [0x15, 0, 0, 0, 0, 0, 0, 0, 0, 0x07]
    );
}

#[test]
fn frames_that_cannot_be_acknowledged_are_refused() {
    let short = |fcf: u16, rest: &[u8]| concat(&[&fcf.to_le_bytes(), rest]);
    let addresses = [0x5a, 0x34, 0x12, 0x01, 0x00, 0x02, 0x00];
    // 2006 version.
    assert_eq!(
        ack(&short(0x8861, &addresses), false, &[]),
        Err(EnhancedAckError::NotVersion2015)
    );
    // No acknowledgement request.
    assert_eq!(
        ack(&short(0xa841, &addresses), false, &[]),
        Err(EnhancedAckError::NoAckRequest)
    );
    // Sequence number suppressed.
    assert_eq!(
        ack(&short(0xa961, &addresses), false, &[]),
        Err(EnhancedAckError::SequenceSuppressed)
    );
    // Broadcast destination.
    assert_eq!(
        ack(
            &short(0xa861, &[0x5a, 0x34, 0x12, 0xff, 0xff, 0x02, 0x00]),
            false,
            &[]
        ),
        Err(EnhancedAckError::NotUnicast)
    );
    // No source address.
    assert_eq!(
        ack(&short(0x2821, &[0x5a, 0x34, 0x12, 0x01, 0x00]), false, &[]),
        Err(EnhancedAckError::NoSource)
    );
    // Security level other than ENC-MIC-32.
    assert_eq!(
        ack(
            &short(
                0xa869,
                &[
                    0x5a, 0x34, 0x12, 0x01, 0x00, 0x02, 0x00, 0x0e, 0, 0, 0, 0, 1
                ]
            ),
            false,
            &[]
        ),
        Err(EnhancedAckError::UnsupportedSecurityLevel)
    );
    // Truncated source address.
    assert_eq!(
        ack(
            &short(0xa861, &[0x5a, 0x34, 0x12, 0x01, 0x00, 0x02]),
            false,
            &[]
        ),
        Err(EnhancedAckError::Malformed)
    );
    // The IEs do not fit one MAC frame.
    assert_eq!(
        ack(&short(0xa861, &addresses), false, &[0; 119]),
        Err(EnhancedAckError::TooLong)
    );
    assert!(ack(&short(0xa861, &addresses), false, &[0; 118]).is_ok());
}

fn secured(key_id_mode: u8, key_index: u8) -> Vec<u8> {
    vec![
        0x69,
        0xa8,
        0x11,
        0x34,
        0x12,
        0x01,
        0x00,
        0x02,
        0x00,
        0x05 | key_id_mode << 3,
        0x01,
        0x02,
        0x03,
        0x04,
        key_index,
    ]
}

/// The port selects the current key or a neighbour by key index, takes the
/// frame counter first and refuses other key identifiers.
#[test]
fn mac_keys_select_by_key_index_and_take_the_frame_counter() {
    let (previous, current, next) = ([1; 16], [2; 16], [3; 16]);
    let mut keys = MacKeys::new(5, previous, current, next, 100);
    for (index, key) in [(5, current), (4, previous), (6, next)] {
        let received = secured(1, index);
        let mut ack =
            generate_enhanced_ack(FrameView::new(&received).unwrap(), false, &[]).unwrap();
        let counter = keys.frame_counter();
        assert_eq!(keys.secure(&mut ack), Some(key));
        assert_eq!(ack.bytes()[8..12], counter.to_le_bytes());
    }
    assert_eq!(keys.frame_counter(), 103);

    for received in [secured(1, 7), secured(1, 0), secured(0, 5), secured(2, 5)] {
        // Mode 2 carries a key source before the index; this frame is short
        // of it, so only modes 0 and 1 reach `secure`.
        let Ok(mut ack) = generate_enhanced_ack(FrameView::new(&received).unwrap(), false, &[])
        else {
            continue;
        };
        let counter = keys.frame_counter();
        assert_eq!(keys.secure(&mut ack), None);
        assert_eq!(keys.frame_counter(), counter + 1);
    }

    // Neighbours do not wrap around the key identifier.
    let mut keys = MacKeys::new(0, previous, current, next, 0);
    let received = secured(1, 0xff);
    let mut ack = generate_enhanced_ack(FrameView::new(&received).unwrap(), false, &[]).unwrap();
    assert_eq!(keys.secure(&mut ack), None);

    // An unsecured ACK needs no key and takes no frame counter.
    let plain = [0x61, 0xa8, 0x5a, 0x34, 0x12, 0x01, 0x00, 0x02, 0x00];
    let mut ack = generate_enhanced_ack(FrameView::new(&plain).unwrap(), false, &[]).unwrap();
    assert_eq!(keys.secure(&mut ack), None);
    assert_eq!(keys.frame_counter(), 1);
}
