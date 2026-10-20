//! The fragment parsers return on any input.

use super::*;
use crate::ccmp::CCMP_HEADER_LEN;
use crate::fuzz::{FULL, QUICK, inputs};

/// Fragment 0 of sequence 0x123 from the AP, with More Fragments, carrying
/// an LLC/SNAP header and two bytes.
const OPEN_FIRST: [u8; 34] = [
    0x08, 0x06, 0, 0, 2, 0, 0, 0, 0, 1, 2, 0, 0, 0, 0, 2, 2, 0, 0, 0, 0, 3, 0x30, 0x12, 0xaa, 0xaa,
    3, 0, 0, 0, 0x08, 0x00, 1, 2,
];
/// Fragment 1 of a QoS data MSDU to the AP, with Retry.
const QOS_RETRY: [u8; 29] = [
    0x88, 0x09, 0, 0, 2, 0, 0, 0, 0, 2, 2, 0, 0, 0, 0, 1, 2, 0, 0, 0, 0, 3, 0x31, 0x12, 5, 0, 3, 4,
    5,
];
/// A protected fragment from the AP: header, CCMP header, plaintext.
const CCMP_FIRST: [u8; 42] = [
    0x08, 0x46, 0, 0, 2, 0, 0, 0, 0, 1, 2, 0, 0, 0, 0, 2, 2, 0, 0, 0, 0, 3, 0x30, 0x12, 1, 0, 0,
    0x20, 0, 0, 0, 0, 0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1, 2,
];

fn read_identity(identity: OpenDataFragmentIdentity) {
    let _ = (
        identity.role(),
        identity.protection(),
        identity.receiver_address(),
        identity.transmitter_address(),
        identity.address3(),
        identity.sequence_number(),
        identity.tid(),
        identity.destination(),
        identity.source(),
    );
}

fn read_fragment(fragment: OpenDataFragment<'_>) {
    read_identity(fragment.identity());
    // Reassembly indexes its 16 fragment records by this number.
    assert!(fragment.fragment_number() < 16);
    let _ = (
        fragment.sequence_control(),
        fragment.more_fragments(),
        fragment.retry(),
        fragment.packet_number(),
        fragment.payload(),
    );
}

fn parse_all(bytes: &[u8]) {
    for role in [DataInterfaceRole::Station, DataInterfaceRole::AccessPoint] {
        if let Ok(fragment) = parse_open_data_fragment(role, bytes) {
            read_fragment(fragment);
        }
        if let Ok(identity) = parse_open_data_identity(role, bytes) {
            read_identity(identity);
        }
        // The CCMP header follows the 24-byte header (the QoS field is not
        // skipped: any eight bytes are a candidate header).
        let Some((_, body)) = bytes.split_at_checked(IEEE80211_LEGACY_DATA_HEADER_LEN) else {
            continue;
        };
        let Some((ccmp, plaintext)) = body.split_first_chunk::<CCMP_HEADER_LEN>() else {
            continue;
        };
        let Ok(ccmp) = CcmpHeader::parse(*ccmp) else {
            continue;
        };
        if let Ok(fragment) = parse_ccmp_data_fragment(role, bytes, plaintext, ccmp) {
            read_fragment(fragment);
        }
        if let Ok(identity) = parse_ccmp_data_identity(role, bytes) {
            read_identity(identity);
        }
    }
}

fn run(count: usize) {
    inputs(
        &[&OPEN_FIRST, &QOS_RETRY, &CCMP_FIRST],
        count,
        64,
        parse_all,
    );
}

/// Reassembly indexes its 16 fragment records by the parsed number.
#[test]
fn every_sequence_control_names_one_of_16_fragments() {
    let mut frame = OPEN_FIRST;
    for sequence_control in 0..=u16::MAX {
        frame[22..24].copy_from_slice(&sequence_control.to_le_bytes());
        if let Ok(fragment) = parse_open_data_fragment(DataInterfaceRole::Station, &frame) {
            assert_eq!(u16::from(fragment.fragment_number()), sequence_control & 15);
        }
    }
}

#[test]
fn fragment_parsers_never_panic() {
    assert!(parse_open_data_fragment(DataInterfaceRole::Station, &OPEN_FIRST).is_ok());
    assert!(parse_open_data_fragment(DataInterfaceRole::AccessPoint, &QOS_RETRY).is_ok());
    run(QUICK);
}

#[test]
#[ignore = "the full run"]
fn fragment_parsers_never_panic_full() {
    run(FULL);
}
