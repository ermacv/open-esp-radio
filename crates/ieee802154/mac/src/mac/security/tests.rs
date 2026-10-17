//! Expectations follow `otPlatRadioTransmit` of ESP-IDF's OpenThread port
//! and OpenThread's `Frame::SetFrameCounter` / `Frame::SetKeyId`.

use std::vec::Vec;

use super::MacKeys;
use crate::{AppliedSecurity, KeyIdMode};

/// `[PHR, MAC...]` of a 2006 data frame secured at ENC-MIC-32 in key
/// identifier mode 1: security header at image offset 10.
fn secured_mode_1() -> Vec<u8> {
    let mac = [
        0x69, 0x98, 0x07, 0x34, 0x12, 0x02, 0x00, 0x01, 0x00, // header
        0x0d, 0, 0, 0, 0, 0, // ENC-MIC-32 mode 1, counter, key index
        0xaa, 0, 0, 0, 0,
    ];
    let mut image = std::vec![mac.len() as u8 + 2];
    image.extend_from_slice(&mac);
    image
}

/// A first transmission takes a new frame counter and the current key
/// index and key; a retransmission keeps the frame's counter.
#[test]
fn first_transmissions_take_a_counter_and_retransmissions_keep_it() {
    let mut keys = MacKeys::new(3, [1; 16], [2; 16], [3; 16], 40);
    let mut image = secured_mode_1();

    let first = keys.transmit_security(false);
    assert_eq!(
        (first.frame_counter, first.key_id, first.key),
        (Some(40), Some(3), [2; 16])
    );
    assert_eq!(first.apply(&mut image), Some(KeyIdMode::Index));
    assert_eq!(image[11..16], [40, 0, 0, 0, 3]);

    // Another CCA attempt of the first transmission is a transmit of its own.
    let again = keys.transmit_security(false);
    again.apply(&mut image);
    assert_eq!(image[11..15], [41, 0, 0, 0]);

    // A retransmission keeps the frame's counter and key index, as the port
    // sets neither for `mIsARetx`, even after the key index changed.
    keys.set_keys(4, [2; 16], [3; 16], [4; 16]);
    let retry = keys.transmit_security(true);
    assert_eq!(
        (retry.frame_counter, retry.key_id, retry.key),
        (None, None, [3; 16])
    );
    retry.apply(&mut image);
    assert_eq!(image[11..16], [41, 0, 0, 0, 3]);
    assert_eq!(keys.frame_counter(), 42);
}

/// What a transmission wrote is reported for the stack to read back, and
/// writing it into the stack's copy of the frame gives the same header.
#[test]
fn applied_security_reproduces_the_written_header() {
    let mut keys = MacKeys::new(3, [1; 16], [2; 16], [3; 16], 0x0102_0304);
    let mut radio_copy = secured_mode_1();
    let first = keys.transmit_security(false);
    let mode = first.apply(&mut radio_copy).unwrap();
    let applied = first.applied(mode).unwrap();
    assert_eq!(
        applied,
        AppliedSecurity {
            frame_counter: 0x0102_0304,
            key_id: Some(3)
        }
    );
    let mut stack_copy = secured_mode_1();
    assert!(applied.write(&mut stack_copy));
    assert_eq!(stack_copy, radio_copy);

    // A retransmission wrote nothing.
    assert_eq!(keys.transmit_security(true).applied(mode), None);

    // Other key identifier modes carry no key index.
    let mut implicit = secured_mode_1();
    implicit[10] = 0x05;
    let security = keys.transmit_security(false);
    let mode = security.apply(&mut implicit).unwrap();
    assert_eq!(security.applied(mode).unwrap().key_id, None);

    let mut plain = secured_mode_1();
    plain[1] &= !0x08;
    assert!(!applied.write(&mut plain));
}

/// Unsecured frames and frames without room for their header are left.
#[test]
fn only_secured_frames_are_written() {
    let mut keys = MacKeys::new(3, [1; 16], [2; 16], [3; 16], 0);
    let security = keys.transmit_security(false);
    let mut plain = secured_mode_1();
    plain[1] &= !0x08;
    let before = plain.clone();
    assert_eq!(security.apply(&mut plain), None);
    assert_eq!(plain, before);

    let mut short = secured_mode_1();
    short.truncate(13);
    assert_eq!(security.apply(&mut short), None);

    let mut implicit = secured_mode_1();
    implicit[10] = 0x05;
    assert_eq!(security.apply(&mut implicit), Some(KeyIdMode::Implicit));
    assert_eq!(implicit[15], 0, "mode 0 carries no key index");
}

/// A CSL receiver's retransmission takes a new counter and keeps its key
/// index (`!mIsARetx || s_csl_period > 0`).
#[test]
fn csl_retransmissions_take_a_new_counter() {
    let mut keys = MacKeys::new(3, [1; 16], [2; 16], [3; 16], 7);
    let retry = keys.transmit_security_with_csl(true, true);
    assert_eq!((retry.frame_counter, retry.key_id), (Some(7), None));
    let first = keys.transmit_security_with_csl(false, true);
    assert_eq!((first.frame_counter, first.key_id), (Some(8), Some(3)));
    assert_eq!(
        keys.transmit_security_with_csl(true, false).frame_counter,
        None
    );
}
