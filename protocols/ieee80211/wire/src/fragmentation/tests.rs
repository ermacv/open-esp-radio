extern crate std;

use super::*;
use oer_time::{Duration, Instant};

fn at(micros: u64) -> Instant {
    Instant::from_micros(micros)
}
use crate::sequence::seq;

const STA: [u8; 6] = [2, 0, 0, 0, 0, 1];
const AP: [u8; 6] = [2, 0, 0, 0, 0, 2];
const SOURCE: [u8; 6] = [2, 0, 0, 0, 0, 3];

fn fragment(
    role: DataInterfaceRole,
    sequence: u16,
    number: u8,
    more: bool,
    retry: bool,
    payload: &[u8],
) -> [u8; 64] {
    let mut mpdu = [0_u8; 64];
    let mut control = DATA
        | match role {
            DataInterfaceRole::Station => FROM_DS,
            DataInterfaceRole::AccessPoint => TO_DS,
        };
    if more {
        control |= MORE_FRAGMENTS;
    }
    if retry {
        control |= RETRY;
    }
    mpdu[..2].copy_from_slice(&control.to_le_bytes());
    match role {
        DataInterfaceRole::Station => {
            mpdu[4..10].copy_from_slice(&STA);
            mpdu[10..16].copy_from_slice(&AP);
            mpdu[16..22].copy_from_slice(&SOURCE);
        }
        DataInterfaceRole::AccessPoint => {
            mpdu[4..10].copy_from_slice(&AP);
            mpdu[10..16].copy_from_slice(&STA);
            mpdu[16..22].copy_from_slice(&SOURCE);
        }
    }
    mpdu[22..24].copy_from_slice(&((sequence << 4) | u16::from(number)).to_le_bytes());
    mpdu[24..24 + payload.len()].copy_from_slice(payload);
    mpdu
}

fn parsed<'a>(
    role: DataInterfaceRole,
    frame: &'a [u8; 64],
    payload_len: usize,
) -> OpenDataFragment<'a> {
    parse_open_data_fragment(role, &frame[..24 + payload_len]).unwrap()
}

#[test]
fn station_fragments_reassemble_one_exact_ethernet_view() {
    let first_payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1, 2];
    let second_payload = [3, 4, 5];
    let first = fragment(
        DataInterfaceRole::Station,
        0x123,
        0,
        true,
        false,
        &first_payload,
    );
    let second = fragment(
        DataInterfaceRole::Station,
        0x123,
        1,
        false,
        false,
        &second_payload,
    );
    let mut state = OpenDataDefragmenter::<2, 32>::new(Duration::from_micros(100));
    assert_eq!(
        state.ingest(
            parsed(DataInterfaceRole::Station, &first, first_payload.len()),
            at(1),
            |_| ()
        ),
        Ok(OpenDataDefragmentation::Buffered {
            expired: 0,
            evicted: None,
        })
    );
    let outcome = state
        .ingest(
            parsed(DataInterfaceRole::Station, &second, second_payload.len()),
            at(2),
            |data| {
                let frame = data.ethernet_frame();
                (
                    frame.destination,
                    frame.source,
                    frame.ether_type,
                    frame.payload.to_vec(),
                )
            },
        )
        .unwrap();
    assert_eq!(
        outcome,
        OpenDataDefragmentation::Complete {
            expired: 0,
            value: (STA, SOURCE, 0x0800, std::vec![1, 2, 3, 4, 5]),
        }
    );
    assert_eq!(state.active_contexts(), 0);
}

#[test]
fn changed_address_cannot_splice_an_active_sequence() {
    let first_payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
    let last_payload = [2];
    let first = fragment(
        DataInterfaceRole::Station,
        7,
        0,
        true,
        false,
        &first_payload,
    );
    let mut changed = fragment(
        DataInterfaceRole::Station,
        7,
        1,
        false,
        false,
        &last_payload,
    );
    changed[16..22].copy_from_slice(&[2, 0, 0, 0, 0, 9]);
    let last = fragment(
        DataInterfaceRole::Station,
        7,
        1,
        false,
        false,
        &last_payload,
    );
    let mut state = OpenDataDefragmenter::<1, 16>::new(Duration::from_micros(100));
    state
        .ingest(
            parsed(DataInterfaceRole::Station, &first, first_payload.len()),
            at(1),
            |_| (),
        )
        .unwrap();
    assert_eq!(
        state.ingest(
            parsed(DataInterfaceRole::Station, &changed, 1),
            at(2),
            |_| ()
        ),
        Err(OpenDataFragmentError::IdentityMismatch)
    );
    assert!(matches!(
        state.ingest(parsed(DataInterfaceRole::Station, &last, 1), at(3), |_| ()),
        Ok(OpenDataDefragmentation::Complete { .. })
    ));
}

#[test]
fn retry_out_of_order_timeout_and_oldest_eviction_are_bounded() {
    let payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
    let first = fragment(DataInterfaceRole::Station, 1, 0, true, false, &payload);
    let retry = fragment(DataInterfaceRole::Station, 1, 0, true, true, &payload);
    let third = fragment(DataInterfaceRole::Station, 1, 2, false, false, &[2]);
    let other = fragment(DataInterfaceRole::Station, 2, 0, true, false, &payload);
    let newest = fragment(DataInterfaceRole::Station, 3, 0, true, false, &payload);
    let mut state = OpenDataDefragmenter::<2, 32>::new(Duration::from_micros(10));
    state
        .ingest(
            parsed(DataInterfaceRole::Station, &first, payload.len()),
            at(1),
            |_| (),
        )
        .unwrap();
    assert_eq!(
        state.ingest(
            parsed(DataInterfaceRole::Station, &retry, payload.len()),
            at(2),
            |_| ()
        ),
        Ok(OpenDataDefragmentation::Duplicate { expired: 0 })
    );
    assert_eq!(
        state.ingest(parsed(DataInterfaceRole::Station, &third, 1), at(3), |_| ()),
        Err(OpenDataFragmentError::OutOfOrder {
            expected: 1,
            observed: 2,
        })
    );
    state
        .ingest(
            parsed(DataInterfaceRole::Station, &other, payload.len()),
            at(4),
            |_| (),
        )
        .unwrap();
    let outcome = state
        .ingest(
            parsed(DataInterfaceRole::Station, &newest, payload.len()),
            at(5),
            |_| (),
        )
        .unwrap();
    assert!(matches!(
        outcome,
        OpenDataDefragmentation::Buffered {
            evicted: Some(identity),
            ..
        } if identity.sequence_number() == seq(1)
    ));
    let expired = fragment(DataInterfaceRole::Station, 4, 0, true, false, &payload);
    assert!(matches!(
        state.ingest(
            parsed(DataInterfaceRole::Station, &expired, payload.len()),
            at(20),
            |_| ()
        ),
        Ok(OpenDataDefragmentation::Buffered { expired: 2, .. })
    ));
}

#[test]
fn protected_amsdu_overflow_and_lifecycle_edges_fail_closed() {
    let payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
    let mut protected = fragment(DataInterfaceRole::Station, 1, 0, true, false, &payload);
    protected[1] |= 0x40;
    assert_eq!(
        parse_open_data_fragment(DataInterfaceRole::Station, &protected[..33]),
        Err(OpenDataFragmentError::Protected)
    );

    let mut qos = fragment(DataInterfaceRole::Station, 1, 0, true, false, &payload);
    qos[0] = QOS_DATA as u8;
    qos[24] = QOS_AMSDU_PRESENT;
    assert_eq!(
        parse_open_data_fragment(DataInterfaceRole::Station, &qos[..35]),
        Err(OpenDataFragmentError::AmsduUnsupported)
    );

    let mut group = fragment(DataInterfaceRole::Station, 2, 0, true, false, &payload);
    group[4..10].fill(0xff);
    assert_eq!(
        parse_open_data_fragment(DataInterfaceRole::Station, &group[..33]),
        Err(OpenDataFragmentError::InvalidReceiver)
    );
    assert!(
        parse_open_data_identity(DataInterfaceRole::Station, &group[..33]).is_ok(),
        "ordinary group-address identity remains valid"
    );

    let mut group_destination =
        fragment(DataInterfaceRole::AccessPoint, 2, 0, true, false, &payload);
    group_destination[16..22].fill(0xff);
    assert_eq!(
        parse_open_data_fragment(DataInterfaceRole::AccessPoint, &group_destination[..33]),
        Err(OpenDataFragmentError::InvalidDestination)
    );
    assert!(
        parse_open_data_identity(DataInterfaceRole::AccessPoint, &group_destination[..33]).is_ok(),
        "ordinary To-DS group destination remains valid"
    );

    let first = fragment(DataInterfaceRole::AccessPoint, 3, 0, true, false, &payload);
    let second = fragment(DataInterfaceRole::AccessPoint, 3, 1, false, false, &[2, 3]);
    let mut state = OpenDataDefragmenter::<1, 10>::new(Duration::from_micros(100));
    state
        .ingest(
            parsed(DataInterfaceRole::AccessPoint, &first, payload.len()),
            at(1),
            |_| (),
        )
        .unwrap();
    assert_eq!(
        state.ingest(
            parsed(DataInterfaceRole::AccessPoint, &second, 2),
            at(2),
            |_| ()
        ),
        Err(OpenDataFragmentError::ReassembledTooLarge { capacity: 10 })
    );
    assert_eq!(state.active_contexts(), 0);

    state
        .ingest(
            parsed(DataInterfaceRole::AccessPoint, &first, payload.len()),
            at(3),
            |_| (),
        )
        .unwrap();
    assert_eq!(state.forget_transmitter(STA), 1);
    assert_eq!(state.clear(), 0);
}

#[test]
fn fresh_ordinary_wrap_replaces_stale_fragment_completion_identity() {
    let first_payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
    let first = fragment(
        DataInterfaceRole::Station,
        7,
        0,
        true,
        false,
        &first_payload,
    );
    let final_fragment = fragment(DataInterfaceRole::Station, 7, 1, false, false, &[2]);
    let mut state = OpenDataDefragmenter::<1, 32>::new(Duration::from_micros(100));
    state
        .ingest(
            parsed(DataInterfaceRole::Station, &first, first_payload.len()),
            at(1),
            |_| (),
        )
        .unwrap();
    assert!(matches!(
        state.ingest(
            parsed(DataInterfaceRole::Station, &final_fragment, 1),
            at(2),
            |_| (),
        ),
        Ok(OpenDataDefragmentation::Complete { .. })
    ));

    // A 12-bit sequence number can wrap while the bounded completion
    // fingerprint is still live. A fresh ordinary MPDU with a different
    // third address owns the reused sequence; its retry is left to the
    // ordinary duplicate filter instead of colliding with stale fragment
    // identity here.
    let mut ordinary = fragment(DataInterfaceRole::Station, 7, 0, false, false, &[9]);
    ordinary[16..22].copy_from_slice(&[2, 0, 0, 0, 0, 9]);
    let ordinary_identity =
        parse_open_data_identity(DataInterfaceRole::Station, &ordinary[..25]).unwrap();
    assert_eq!(
        state.admit_unfragmented(ordinary_identity, false, Some(at(3))),
        Ok(OpenDataUnfragmentedAdmission::Admitted { expired: 0 })
    );
    assert_eq!(
        state.admit_unfragmented(ordinary_identity, true, Some(at(4))),
        Ok(OpenDataUnfragmentedAdmission::Admitted { expired: 0 })
    );
}

#[test]
fn clock_wrap_expires_retained_bytes_before_sequence_reuse() {
    let payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
    let first = fragment(DataInterfaceRole::Station, 9, 0, true, false, &payload);
    let final_fragment = fragment(DataInterfaceRole::Station, 9, 1, false, false, &[2]);
    let mut state = OpenDataDefragmenter::<1, 32>::new(Duration::from_micros(100));
    state
        .ingest(
            parsed(DataInterfaceRole::Station, &first, payload.len()),
            at(u64::MAX - 1),
            |_| (),
        )
        .unwrap();

    assert_eq!(
        state.ingest(
            parsed(DataInterfaceRole::Station, &final_fragment, 1),
            at(1),
            |_| (),
        ),
        Err(OpenDataFragmentError::Orphan { fragment_number: 1 })
    );
    assert_eq!(state.active_contexts(), 0);
}
