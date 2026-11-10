//! The payload writer, against the checksum of the octets it holds.

extern crate std;

use std::vec;
use std::vec::Vec;

use oer_protocols_network_wire::checksum::Checksum;

use super::{Payload, PayloadFull};

/// Whatever is written, the region holds it in order and the sum is that of
/// the region's octets after what it continues.
#[test]
fn a_payload_holds_what_was_written_and_sums_it() {
    let before = [0x45, 0x00, 0x05];
    for length in [0, 1, 2, 27, 28, 29, 1472] {
        for head in [0, 1, 3, 28] {
            let mut region = vec![0xEE; length];
            let mut payload = Payload::new(&mut region, Checksum::new().with(&before), None);
            let header: Vec<u8> = (0..head).map(|at| at as u8 ^ 0xA5).collect();
            let fits = head <= length;
            assert_eq!(payload.put(&header).is_ok(), fits);
            let rest = payload.remaining();
            assert_eq!(rest, if fits { length - head } else { length });
            payload.repeat(0x5A, rest).unwrap();
            let (sum, _) = payload.finish().expect("every octet is written");
            let mut expected = if fits { header.clone() } else { Vec::new() };
            expected.resize(length, 0x5A);
            assert_eq!(region, expected, "{length} after {head}");
            assert_eq!(
                sum.finish(),
                Checksum::new().with(&before).with(&expected).finish(),
                "{length} after {head}"
            );
        }
    }
}

/// A payload not written whole has no sum: its frame does not go.
#[test]
fn a_payload_written_in_part_gives_no_sum() {
    let mut region = [0; 10];
    let mut payload = Payload::new(&mut region, Checksum::new(), None);
    payload.put(&[1, 2, 3]).unwrap();
    assert_eq!(payload.remaining(), 7);
    assert!(payload.finish().is_none());
}

/// A write past the region writes nothing: what was written stays as it was.
#[test]
fn a_write_past_the_end_writes_nothing() {
    let mut region = [0; 4];
    let mut payload = Payload::new(&mut region, Checksum::new(), None);
    payload.put(&[9, 9]).unwrap();
    assert_eq!(payload.repeat(7, 3), Err(PayloadFull));
    assert_eq!(payload.put(&[1, 2, 3]), Err(PayloadFull));
    assert_eq!(payload.remaining(), 2);
    payload.repeat(7, 2).unwrap();
    assert!(payload.finish().is_some());
    assert_eq!(region, [9, 9, 7, 7]);
}

/// A repeat over a run the region holds from its last frame writes nothing,
/// and sums the run; one the run does not cover is written.
#[test]
fn a_repeat_over_a_held_run_writes_nothing_and_sums_it() {
    let kept = oer_protocols_network_port::Held {
        at: 2,
        length: 6,
        octet: 0x5a,
    };
    // Marked where the run lies: an octet the repeat wrote would show.
    let mut region = [0x11; 8];
    let mut payload = Payload::new(&mut region, Checksum::new(), Some(kept));
    payload.put(&[1, 2]).unwrap();
    payload.repeat(0x5a, 6).unwrap();
    let (sum, left) = payload.finish().unwrap();
    assert_eq!(
        region,
        [1, 2, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11],
        "nothing written"
    );
    assert_eq!(
        sum.finish(),
        Checksum::new()
            .with(&[1, 2, 0x5a, 0x5a, 0x5a, 0x5a, 0x5a, 0x5a])
            .finish()
    );
    assert_eq!(left, Some(kept));
    // Another octet, or a run past the held one, is written.
    let mut region = [0x11; 8];
    let mut payload = Payload::new(&mut region, Checksum::new(), Some(kept));
    payload.put(&[1]).unwrap();
    payload.repeat(0x5a, 7).unwrap();
    assert!(payload.finish().is_some());
    assert_eq!(region, [1, 0x5a, 0x5a, 0x5a, 0x5a, 0x5a, 0x5a, 0x5a]);
}

/// Only a run to the region's end is held after it.
#[test]
fn a_run_followed_by_other_octets_is_not_held() {
    let mut region = [0; 8];
    let mut payload = Payload::new(&mut region, Checksum::new(), None);
    payload.repeat(0x5a, 6).unwrap();
    payload.put(&[1, 2]).unwrap();
    let (_, left) = payload.finish().unwrap();
    assert_eq!(left, None);
}
