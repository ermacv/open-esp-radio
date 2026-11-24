extern crate std;

use super::*;

const DATA: u16 = 0x0008;
const QOS_DATA: u16 = 0x0088;
const TO_DS: u16 = 0x0100;
const FROM_DS: u16 = 0x0200;
const MORE_FRAGMENTS: u16 = 0x0400;
const RETRY: u16 = 0x0800;
const QOS_AMSDU_PRESENT: u8 = 0x80;
use oer_portable_time::{Duration, Instant};

fn at(micros: u64) -> Instant {
    Instant::from_micros(micros)
}
use crate::duplicate::RetrySubfield;
use crate::fragmentation::MoreFragments;
use crate::seq;

const STA: [u8; 6] = [2, 0, 0, 0, 0, 1];
const AP: [u8; 6] = [2, 0, 0, 0, 0, 2];
const SOURCE: [u8; 6] = [2, 0, 0, 0, 0, 3];

fn fragment(
    role: DataInterfaceRole,
    sequence: u16,
    number: u8,
    more: MoreFragments,
    retry: RetrySubfield,
    payload: &[u8],
) -> [u8; 64] {
    let mut mpdu = [0_u8; 64];
    let mut control = DATA
        | match role {
            DataInterfaceRole::Station => FROM_DS,
            DataInterfaceRole::AccessPoint => TO_DS,
        };
    if more == MoreFragments::More {
        control |= MORE_FRAGMENTS;
    }
    if retry == RetrySubfield::Set {
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
        MoreFragments::More,
        RetrySubfield::Clear,
        &first_payload,
    );
    let second = fragment(
        DataInterfaceRole::Station,
        0x123,
        1,
        MoreFragments::Last,
        RetrySubfield::Clear,
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
                let frame = data.msdu();
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
        MoreFragments::More,
        RetrySubfield::Clear,
        &first_payload,
    );
    let mut changed = fragment(
        DataInterfaceRole::Station,
        7,
        1,
        MoreFragments::Last,
        RetrySubfield::Clear,
        &last_payload,
    );
    changed[16..22].copy_from_slice(&[2, 0, 0, 0, 0, 9]);
    let last = fragment(
        DataInterfaceRole::Station,
        7,
        1,
        MoreFragments::Last,
        RetrySubfield::Clear,
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
        Err(OpenDataReassemblyError::IdentityMismatch)
    );
    assert!(matches!(
        state.ingest(parsed(DataInterfaceRole::Station, &last, 1), at(3), |_| ()),
        Ok(OpenDataDefragmentation::Complete { .. })
    ));
}

#[test]
fn retry_out_of_order_timeout_and_oldest_eviction_are_bounded() {
    let payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
    let first = fragment(
        DataInterfaceRole::Station,
        1,
        0,
        MoreFragments::More,
        RetrySubfield::Clear,
        &payload,
    );
    let retry = fragment(
        DataInterfaceRole::Station,
        1,
        0,
        MoreFragments::More,
        RetrySubfield::Set,
        &payload,
    );
    let third = fragment(
        DataInterfaceRole::Station,
        1,
        2,
        MoreFragments::Last,
        RetrySubfield::Clear,
        &[2],
    );
    let other = fragment(
        DataInterfaceRole::Station,
        2,
        0,
        MoreFragments::More,
        RetrySubfield::Clear,
        &payload,
    );
    let newest = fragment(
        DataInterfaceRole::Station,
        3,
        0,
        MoreFragments::More,
        RetrySubfield::Clear,
        &payload,
    );
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
        Err(OpenDataReassemblyError::OutOfOrder {
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
    let expired = fragment(
        DataInterfaceRole::Station,
        4,
        0,
        MoreFragments::More,
        RetrySubfield::Clear,
        &payload,
    );
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
    let mut protected = fragment(
        DataInterfaceRole::Station,
        1,
        0,
        MoreFragments::More,
        RetrySubfield::Clear,
        &payload,
    );
    protected[1] |= 0x40;
    assert_eq!(
        parse_open_data_fragment(DataInterfaceRole::Station, &protected[..33]),
        Err(OpenDataFragmentError::Protected)
    );

    let mut qos = fragment(
        DataInterfaceRole::Station,
        1,
        0,
        MoreFragments::More,
        RetrySubfield::Clear,
        &payload,
    );
    qos[0] = u8::try_from(QOS_DATA).unwrap();
    qos[24] = QOS_AMSDU_PRESENT;
    assert_eq!(
        parse_open_data_fragment(DataInterfaceRole::Station, &qos[..35]),
        Err(OpenDataFragmentError::AmsduUnsupported)
    );

    let mut group = fragment(
        DataInterfaceRole::Station,
        2,
        0,
        MoreFragments::More,
        RetrySubfield::Clear,
        &payload,
    );
    group[4..10].fill(0xff);
    assert_eq!(
        parse_open_data_fragment(DataInterfaceRole::Station, &group[..33]),
        Err(OpenDataFragmentError::InvalidReceiver)
    );
    assert!(
        parse_open_data_identity(DataInterfaceRole::Station, &group[..33]).is_ok(),
        "ordinary group-address identity remains valid"
    );

    let mut group_destination = fragment(
        DataInterfaceRole::AccessPoint,
        2,
        0,
        MoreFragments::More,
        RetrySubfield::Clear,
        &payload,
    );
    group_destination[16..22].fill(0xff);
    assert_eq!(
        parse_open_data_fragment(DataInterfaceRole::AccessPoint, &group_destination[..33]),
        Err(OpenDataFragmentError::InvalidDestination)
    );
    assert!(
        parse_open_data_identity(DataInterfaceRole::AccessPoint, &group_destination[..33]).is_ok(),
        "ordinary To-DS group destination remains valid"
    );

    let first = fragment(
        DataInterfaceRole::AccessPoint,
        3,
        0,
        MoreFragments::More,
        RetrySubfield::Clear,
        &payload,
    );
    let second = fragment(
        DataInterfaceRole::AccessPoint,
        3,
        1,
        MoreFragments::Last,
        RetrySubfield::Clear,
        &[2, 3],
    );
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
        Err(OpenDataReassemblyError::ReassembledTooLarge { capacity: 10 })
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
        MoreFragments::More,
        RetrySubfield::Clear,
        &first_payload,
    );
    let final_fragment = fragment(
        DataInterfaceRole::Station,
        7,
        1,
        MoreFragments::Last,
        RetrySubfield::Clear,
        &[2],
    );
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
    let mut ordinary = fragment(
        DataInterfaceRole::Station,
        7,
        0,
        MoreFragments::Last,
        RetrySubfield::Clear,
        &[9],
    );
    ordinary[16..22].copy_from_slice(&[2, 0, 0, 0, 0, 9]);
    let ordinary_identity =
        parse_open_data_identity(DataInterfaceRole::Station, &ordinary[..25]).unwrap();
    assert_eq!(
        state.admit_unfragmented(ordinary_identity, RetrySubfield::Clear, Some(at(10))),
        Ok(OpenDataUnfragmentedAdmission::Admitted { expired: 0 })
    );
    assert_eq!(
        state.admit_unfragmented(ordinary_identity, RetrySubfield::Set, Some(at(4))),
        Ok(OpenDataUnfragmentedAdmission::Admitted { expired: 0 })
    );
}

#[test]
fn clock_wrap_expires_retained_bytes_before_sequence_reuse() {
    let payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
    let first = fragment(
        DataInterfaceRole::Station,
        9,
        0,
        MoreFragments::More,
        RetrySubfield::Clear,
        &payload,
    );
    let final_fragment = fragment(
        DataInterfaceRole::Station,
        9,
        1,
        MoreFragments::Last,
        RetrySubfield::Clear,
        &[2],
    );
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
        Err(OpenDataReassemblyError::Orphan { fragment_number: 1 })
    );
    assert_eq!(state.active_contexts(), 0);
}

fn ccmp_parsed<'a>(frame: &'a [u8; 64], payload: &'a [u8], pn: u64) -> OpenDataFragment<'a> {
    use oer_protocols_ieee80211_wire::ccmp::{CcmpHeader, CcmpKeyId};
    parse_ccmp_data_fragment(
        DataInterfaceRole::Station,
        frame,
        payload,
        CcmpHeader::new(CcmpPacketNumber::new(pn).unwrap(), CcmpKeyId::PAIRWISE),
    )
    .unwrap()
}

fn protected_fragment(number: u8, more: MoreFragments, retry: RetrySubfield) -> [u8; 64] {
    let mut frame = fragment(DataInterfaceRole::Station, 0x123, number, more, retry, &[1]);
    frame[1] |= 0x40;
    frame
}

// SOURCE: IEEE 802.11-2020 12.5.3.4.4; independent behavioral reference:
// Linux v6.12 net/mac80211/rx.c ieee80211_rx_h_defragment compares the
// successor PN and installation key_color. No Linux code is incorporated.
#[test]
fn ccmp_fragments_require_the_immediately_following_packet_number() {
    let first = protected_fragment(0, MoreFragments::More, RetrySubfield::Clear);
    let last = protected_fragment(1, MoreFragments::Last, RetrySubfield::Clear);
    let llc = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
    for (previous, observed, valid) in [
        (10, 10, false),
        (10, 9, false),
        (10, 12, false),
        (255, 256, true),
        ((1 << 48) - 1, 0, false),
    ] {
        let mut state = OpenDataDefragmenter::<2, 32>::new(Duration::from_micros(100));
        state
            .ingest_in_epoch(
                ccmp_parsed(&first, &llc, previous),
                FragmentAdmissionEpoch::CcmpKeyInstallation(1),
                at(1),
                |_| (),
            )
            .unwrap();
        let mut deliveries = 0;
        let result = state.ingest_in_epoch(
            ccmp_parsed(&last, &[2], observed),
            FragmentAdmissionEpoch::CcmpKeyInstallation(1),
            at(2),
            |_| deliveries += 1,
        );
        assert_eq!(result.is_ok(), valid, "PN {previous} -> {observed}");
        assert_eq!(deliveries, usize::from(valid));
    }
}

#[test]
fn ccmp_fragments_cannot_use_the_implicit_open_epoch() {
    let first = protected_fragment(0, MoreFragments::More, RetrySubfield::Clear);
    let mut state = OpenDataDefragmenter::<2, 32>::new(Duration::from_micros(100));
    assert!(
        state
            .ingest(ccmp_parsed(&first, &[1], 10), at(1), |_| ())
            .is_err()
    );
}

#[test]
fn a_same_key_id_reinstallation_cannot_complete_or_retry_an_old_fragment_train() {
    let first = protected_fragment(0, MoreFragments::More, RetrySubfield::Clear);
    let last = protected_fragment(1, MoreFragments::Last, RetrySubfield::Clear);
    let retry = protected_fragment(0, MoreFragments::More, RetrySubfield::Set);
    let llc = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
    let mut state = OpenDataDefragmenter::<2, 32>::new(Duration::from_micros(100));
    state
        .ingest_in_epoch(
            ccmp_parsed(&first, &llc, 10),
            FragmentAdmissionEpoch::CcmpKeyInstallation(1),
            at(1),
            |_| (),
        )
        .unwrap();
    assert_eq!(
        state.ingest_in_epoch(
            ccmp_parsed(&last, &[2], 11),
            FragmentAdmissionEpoch::CcmpKeyInstallation(2),
            at(2),
            |_| panic!("mixed keys delivered")
        ),
        Err(OpenDataReassemblyError::IdentityMismatch)
    );
    assert_eq!(
        state.ingest_in_epoch(
            ccmp_parsed(&retry, &llc, 10),
            FragmentAdmissionEpoch::CcmpKeyInstallation(2),
            at(3),
            |_| panic!("old retry delivered")
        ),
        Err(OpenDataReassemblyError::IdentityMismatch)
    );
    assert!(matches!(
        state.ingest_in_epoch(
            ccmp_parsed(&last, &[2], 11),
            FragmentAdmissionEpoch::CcmpKeyInstallation(1),
            at(4),
            |_| ()
        ),
        Ok(OpenDataDefragmentation::Complete { .. })
    ));
}

#[test]
fn a_ccmp_fragment_retry_keeps_its_original_pn_and_bytes() {
    let first = protected_fragment(0, MoreFragments::More, RetrySubfield::Clear);
    let retry = protected_fragment(0, MoreFragments::More, RetrySubfield::Set);
    let last = protected_fragment(1, MoreFragments::Last, RetrySubfield::Clear);
    let llc = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
    let epoch = FragmentAdmissionEpoch::CcmpKeyInstallation(7);
    let mut state = OpenDataDefragmenter::<2, 32>::new(Duration::from_micros(100));
    state
        .ingest_in_epoch(ccmp_parsed(&first, &llc, 10), epoch, at(1), |_| ())
        .unwrap();
    assert!(matches!(
        state.ingest_in_epoch(ccmp_parsed(&retry, &llc, 10), epoch, at(2), |_| panic!(
            "retry delivered"
        )),
        Ok(OpenDataDefragmentation::Duplicate { .. })
    ));
    assert!(matches!(
        state.ingest_in_epoch(ccmp_parsed(&retry, &llc, 11), epoch, at(3), |_| panic!(
            "changed retry delivered"
        )),
        Err(OpenDataReassemblyError::RetryPacketNumberMismatch { .. })
    ));
    assert!(matches!(
        state.ingest_in_epoch(
            ccmp_parsed(&last, &[2], 11),
            epoch,
            at(4),
            |data| assert_eq!(data.msdu().payload, &[1, 2])
        ),
        Ok(OpenDataDefragmentation::Complete { .. })
    ));
}

const HEAD: [u8; 9] = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
const TAIL: [u8; 1] = [2];

fn epoch(generation: u64) -> FragmentAdmissionEpoch {
    FragmentAdmissionEpoch::OpenAssociation(generation)
}

/// Fragment `number` of sequence `sequence`: the head with More, the tail
/// last.
fn part(sequence: u16, number: u8, retry: RetrySubfield) -> ([u8; 64], usize) {
    let (more, payload): (MoreFragments, &[u8]) = if number == 0 {
        (MoreFragments::More, &HEAD)
    } else {
        (MoreFragments::Last, &TAIL)
    };
    (
        fragment(
            DataInterfaceRole::Station,
            sequence,
            number,
            more,
            retry,
            payload,
        ),
        payload.len(),
    )
}

fn identity(sequence: u16) -> OpenDataFragmentIdentity {
    let (frame, length) = part(sequence, 0, RetrySubfield::Clear);
    parsed(DataInterfaceRole::Station, &frame, length).identity()
}

fn take<const C: usize>(
    state: &mut OpenDataDefragmenter<C, 32>,
    sequence: u16,
    number: u8,
    retry: RetrySubfield,
    generation: u64,
    now: u64,
) -> Result<OpenDataDefragmentation<()>, OpenDataReassemblyError> {
    let (frame, length) = part(sequence, number, retry);
    state.ingest_in_epoch(
        parsed(DataInterfaceRole::Station, &frame, length),
        epoch(generation),
        at(now),
        |_| (),
    )
}

/// A completed train of `sequence` in `generation`, its fragments at `now`
/// and after: time only moves on.
fn complete<const C: usize>(
    state: &mut OpenDataDefragmenter<C, 32>,
    sequence: u16,
    generation: u64,
    now: u64,
) {
    take(state, sequence, 0, RetrySubfield::Clear, generation, now).unwrap();
    assert!(matches!(
        take(
            state,
            sequence,
            1,
            RetrySubfield::Clear,
            generation,
            now + 1
        ),
        Ok(OpenDataDefragmentation::Complete { .. })
    ));
}

/// Only the exact train, by identity and epoch, is discarded.
#[test]
fn discard_removes_only_its_exact_train() {
    let mut state = OpenDataDefragmenter::<2, 32>::new(Duration::from_micros(100));
    take(&mut state, 5, 0, RetrySubfield::Clear, 1, 1).unwrap();
    assert!(!state.discard(identity(5), epoch(2)), "another epoch");
    assert!(!state.discard(identity(6), epoch(1)), "another identity");
    assert_eq!(state.active_contexts(), 1);
    assert!(state.discard(identity(5), epoch(1)));
    assert_eq!(state.active_contexts(), 0);
    assert!(!state.discard(identity(5), epoch(1)), "gone");
}

/// An ordinary MPDU meets an active train: of its own train and epoch it
/// contradicts it; of its sequence space in another epoch it is another
/// identity; of another space it passes, without needing the clock.
#[test]
fn an_ordinary_mpdu_against_an_active_train() {
    let mut state = OpenDataDefragmenter::<2, 32>::new(Duration::from_micros(100));
    take(&mut state, 5, 0, RetrySubfield::Clear, 1, 1).unwrap();
    let admit = |state: &mut OpenDataDefragmenter<2, 32>, sequence, generation, now| {
        state.admit_unfragmented_in_epoch(
            identity(sequence),
            epoch(generation),
            RetrySubfield::Clear,
            now,
        )
    };
    assert_eq!(
        admit(&mut state, 5, 1, Some(at(2))),
        Err(OpenDataReassemblyError::MoreFragmentsMismatch)
    );
    assert_eq!(
        admit(&mut state, 5, 2, Some(at(2))),
        Err(OpenDataReassemblyError::IdentityMismatch)
    );
    assert_eq!(
        admit(&mut state, 6, 1, None),
        Ok(OpenDataUnfragmentedAdmission::Admitted { expired: 0 })
    );
}

/// An ordinary MPDU meets completed trains: its retry of the exact train is
/// a duplicate; of its space in another epoch, another identity; of another
/// space it passes, without the clock; a fresh one clears only its own
/// space's fingerprint.
#[test]
fn an_ordinary_mpdu_against_completed_trains() {
    let mut state = OpenDataDefragmenter::<2, 32>::new(Duration::from_micros(100));
    complete(&mut state, 5, 1, 1);
    complete(&mut state, 6, 1, 3);
    let admit = |state: &mut OpenDataDefragmenter<2, 32>, sequence, generation, retry, now| {
        state.admit_unfragmented_in_epoch(identity(sequence), epoch(generation), retry, now)
    };
    assert_eq!(
        admit(&mut state, 5, 1, RetrySubfield::Set, Some(at(10))),
        Ok(OpenDataUnfragmentedAdmission::Duplicate { expired: 0 })
    );
    assert_eq!(
        admit(&mut state, 5, 2, RetrySubfield::Set, Some(at(10))),
        Err(OpenDataReassemblyError::IdentityMismatch)
    );
    assert_eq!(
        admit(&mut state, 7, 1, RetrySubfield::Set, None),
        Ok(OpenDataUnfragmentedAdmission::Admitted { expired: 0 })
    );
    assert_eq!(
        admit(&mut state, 7, 1, RetrySubfield::Set, Some(at(10))),
        Ok(OpenDataUnfragmentedAdmission::Admitted { expired: 0 })
    );
    // A fresh ordinary MPDU of sequence 5 clears 5's fingerprint, not 6's.
    assert_eq!(
        admit(&mut state, 5, 1, RetrySubfield::Clear, None),
        Ok(OpenDataUnfragmentedAdmission::Admitted { expired: 0 })
    );
    assert_eq!(
        admit(&mut state, 5, 1, RetrySubfield::Set, Some(at(10))),
        Ok(OpenDataUnfragmentedAdmission::Admitted { expired: 0 })
    );
    assert_eq!(
        admit(&mut state, 6, 1, RetrySubfield::Set, Some(at(10))),
        Ok(OpenDataUnfragmentedAdmission::Duplicate { expired: 0 })
    );
}

/// A fragment meets the trains it does not belong to: a retried head of a
/// new sequence starts its train beside an active or a completed one; an
/// orphan of a new sequence is an orphan; the retries of a completed
/// train's fragments are duplicates.
#[test]
fn a_fragment_against_trains_of_other_sequences() {
    let mut state = OpenDataDefragmenter::<3, 32>::new(Duration::from_micros(100));
    complete(&mut state, 5, 1, 1);
    take(&mut state, 6, 0, RetrySubfield::Clear, 1, 3).unwrap();
    assert!(matches!(
        take(&mut state, 7, 0, RetrySubfield::Set, 1, 4),
        Ok(OpenDataDefragmentation::Buffered { .. })
    ));
    assert_eq!(
        take(&mut state, 8, 1, RetrySubfield::Clear, 1, 5),
        Err(OpenDataReassemblyError::Orphan { fragment_number: 1 })
    );
    for number in [0, 1] {
        assert_eq!(
            take(&mut state, 5, number, RetrySubfield::Set, 1, 6),
            Ok(OpenDataDefragmentation::Duplicate { expired: 0 }),
            "fragment {number}"
        );
    }
}

/// A fresh head of a sequence supersedes that sequence's completed
/// fingerprint in another slot: a retry of the old head's bytes is then
/// checked against the new train, and refused.
#[test]
fn a_fresh_head_supersedes_its_sequence_s_completed_fingerprint() {
    let mut state = OpenDataDefragmenter::<3, 32>::new(Duration::from_micros(100));
    complete(&mut state, 5, 1, 1);
    let other_head = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 9];
    let fresh = fragment(
        DataInterfaceRole::Station,
        5,
        0,
        MoreFragments::More,
        RetrySubfield::Clear,
        &other_head,
    );
    assert!(matches!(
        state.ingest_in_epoch(
            parsed(DataInterfaceRole::Station, &fresh, other_head.len()),
            epoch(1),
            at(5),
            |_| ()
        ),
        Ok(OpenDataDefragmentation::Buffered { .. })
    ));
    assert_eq!(
        take(&mut state, 5, 0, RetrySubfield::Set, 1, 6),
        Err(OpenDataReassemblyError::RetryPayloadMismatch { fragment_number: 0 })
    );
}
