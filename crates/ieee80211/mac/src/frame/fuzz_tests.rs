//! [`DecodedHeader::decode`] returns on any input.

use super::*;
use crate::fuzz::{FULL, QUICK, inputs};

/// A QoS data frame from the AP with an HT Control field and an LLC/SNAP body.
const QOS_HT_DATA: [u8; 41] = [
    0x88, 0x82, 0, 0, 2, 2, 2, 2, 2, 2, 4, 4, 4, 4, 4, 4, 6, 6, 6, 6, 6, 6, 0x10, 0, 7, 0, 0, 0, 0,
    0, 0xaa, 0xaa, 3, 0, 0, 0, 0x88, 0x8e, 1, 2, 3,
];
/// A four-address QoS data frame.
const FOUR_ADDRESS: [u8; 40] = [
    0x88, 0x03, 0, 0, 2, 2, 2, 2, 2, 2, 4, 4, 4, 4, 4, 4, 6, 6, 6, 6, 6, 6, 0x20, 0, 8, 8, 8, 8, 8,
    8, 5, 0, 0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0,
];
/// A beacon header (management, Order set) with the start of its body.
const BEACON: [u8; 32] = [
    0x80, 0x80, 0, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 0x30,
    0, 1, 2, 3, 4, 0, 0, 0, 0,
];

fn decode_all(bytes: &[u8]) {
    if let Some(header) = DecodedHeader::decode(bytes) {
        let control = header.control();
        let _ = (
            control.get(),
            control.with_protected(true),
            control.with_retry(true),
            control.retry(),
            control.is_data(),
            control.protected(),
            control.ds(),
            control.subtype(),
            control.more_fragments(),
            control.is_action_no_ack(),
            control.is_qos_data(),
            control.is_four_address(),
            control.has_ht_control(),
        );
        let _ = (
            header.duration(),
            header.receiver(),
            header.transmitter(),
            header.address3(),
            header.sequence_control(),
            header.address4(),
            header.qos(),
            header.ht_control(),
        );
        assert_eq!(header.frame(), bytes);
        assert!(bytes.ends_with(header.body()));
    }
}

fn run(count: usize) {
    inputs(
        &[&QOS_HT_DATA, &FOUR_ADDRESS, &BEACON],
        count,
        64,
        decode_all,
    );
}

#[test]
fn decode_never_panics() {
    for seed in [&QOS_HT_DATA[..], &FOUR_ADDRESS, &BEACON] {
        assert!(DecodedHeader::decode(seed).is_some());
    }
    run(QUICK);
}

#[test]
#[ignore = "the full run"]
fn decode_never_panics_full() {
    run(FULL);
}
