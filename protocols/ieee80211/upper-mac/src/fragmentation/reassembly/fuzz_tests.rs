//! Reassembly returns on any train of received MPDUs.
//!
//! An input is a train of records, each a length octet and that many MPDU
//! octets; its first octet selects the role. Every record that parses as an
//! Open or CCMP fragment is ingested, any other data MPDU is fenced as
//! unfragmented, and record length octets also advance the clock, so trains
//! time out, evict and overflow the small slots.

use std::vec::Vec;

use super::*;
use oer_ieee80211_wire::ccmp::{CCMP_HEADER_LEN, CcmpHeader};
use oer_ieee80211_wire::data::{DataInterfaceRole, IEEE80211_LEGACY_DATA_HEADER_LEN};
use oer_ieee80211_wire::fragmentation::OpenDataFragmentError;
use oer_ieee80211_wire::fragmentation::{
    parse_ccmp_data_fragment, parse_open_data_fragment, parse_open_data_identity,
};
use oer_ieee80211_wire::fuzz::{FULL, QUICK, inputs};

const STA: [u8; 6] = [2, 0, 0, 0, 0, 1];
const AP: [u8; 6] = [2, 0, 0, 0, 0, 2];
const SOURCE: [u8; 6] = [2, 0, 0, 0, 0, 3];

/// One data MPDU of sequence 0x123 from the AP (`station`) or to it.
fn mpdu(
    station: bool,
    number: u8,
    more: bool,
    retry: bool,
    pn: Option<u8>,
    body: &[u8],
) -> Vec<u8> {
    let mut control: u16 = 0x0008 | if station { 0x0200 } else { 0x0100 };
    if more {
        control |= 0x0400;
    }
    if retry {
        control |= 0x0800;
    }
    if pn.is_some() {
        control |= 0x4000;
    }
    let (receiver, transmitter) = if station { (STA, AP) } else { (AP, STA) };
    let mut frame = Vec::new();
    frame.extend_from_slice(&control.to_le_bytes());
    frame.extend_from_slice(&[0, 0]);
    frame.extend_from_slice(&receiver);
    frame.extend_from_slice(&transmitter);
    frame.extend_from_slice(&SOURCE);
    frame.extend_from_slice(&((0x123_u16 << 4) | u16::from(number)).to_le_bytes());
    if let Some(pn) = pn {
        frame.extend_from_slice(&[pn, 0, 0, 0x20, 0, 0, 0, 0]);
    }
    frame.extend_from_slice(body);
    frame
}

/// A train: the role octet, then each MPDU behind its length octet.
fn train(station: bool, frames: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = std::vec![u8::from(station)];
    for frame in frames {
        bytes.push(u8::try_from(frame.len()).unwrap());
        bytes.extend_from_slice(frame);
    }
    bytes
}

const LLC: [u8; 10] = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1, 2];

fn seeds() -> Vec<Vec<u8>> {
    let mut seeds = Vec::new();
    for station in [true, false] {
        seeds.push(train(
            station,
            &[
                mpdu(station, 0, true, false, None, &LLC),
                mpdu(station, 1, true, false, None, &[3, 4]),
                mpdu(station, 1, true, true, None, &[3, 4]),
                mpdu(station, 2, false, false, None, &[5]),
                mpdu(station, 2, false, true, None, &[5]),
                mpdu(station, 0, false, true, None, &LLC),
            ],
        ));
        seeds.push(train(
            station,
            &[
                mpdu(station, 0, true, false, Some(1), &LLC),
                mpdu(station, 1, false, false, Some(2), &[3, 4, 5]),
                mpdu(station, 1, false, true, Some(2), &[3, 4, 5]),
            ],
        ));
    }
    seeds
}

fn read(data: OpenReassembledData<'_>) -> usize {
    let _ = data.identity();
    let frame = data.ethernet_frame();
    let mut output = [0; 64];
    let _ = frame.copy_to(&mut output);
    frame.payload.len()
}

fn drive(bytes: &[u8]) {
    let Some((&role, mut records)) = bytes.split_first() else {
        return;
    };
    let role = if role & 1 != 0 {
        DataInterfaceRole::Station
    } else {
        DataInterfaceRole::AccessPoint
    };
    let mut owner = OpenDataDefragmenter::<2, 32>::new(Duration::from_micros(100));
    let mut micros = 0_u64;
    while let Some((&length, rest)) = records.split_first() {
        let Some((frame, rest)) = rest.split_at_checked(usize::from(length)) else {
            return;
        };
        records = rest;
        micros = micros.wrapping_add(u64::from(length & 0x1f));
        let now = Instant::from_micros(micros);
        let epoch = u64::from(length >> 7);
        let fragment = parse_open_data_fragment(role, frame).ok().or_else(|| {
            let (_, body) = frame.split_at_checked(IEEE80211_LEGACY_DATA_HEADER_LEN)?;
            let (ccmp, plaintext) = body.split_first_chunk::<CCMP_HEADER_LEN>()?;
            let ccmp = CcmpHeader::parse(*ccmp).ok()?;
            parse_ccmp_data_fragment(role, frame, plaintext, ccmp).ok()
        });
        if let Some(fragment) = fragment {
            let _ = match length % 3 {
                0 => owner.ingest(fragment, now, read).map(|_| ()),
                1 => owner
                    .ingest_in_epoch(fragment, epoch, now, read)
                    .map(|_| ()),
                _ => match owner.preflight_in_epoch(fragment, epoch, now) {
                    Ok(OpenDataFragmentPreflight::Admitted(admission)) => {
                        admission.ingest(read).map(|_| ())
                    }
                    Ok(OpenDataFragmentPreflight::Duplicate { .. }) => Ok(()),
                    Err(error) => Err(error),
                },
            };
            if length == 0xff {
                owner.discard(fragment.identity(), epoch);
            }
        } else if let Ok(identity) = parse_open_data_identity(role, frame) {
            let clock = (length & 0x40 == 0).then_some(now);
            let _ = owner.admit_unfragmented_in_epoch(identity, epoch, length & 0x20 != 0, clock);
        } else if length == 0xfe {
            owner.forget_transmitter(STA);
            owner.forget_transmitter(AP);
        }
        let _ = owner.active_contexts();
    }
    owner.clear();
}

fn run(count: usize) {
    let seeds = seeds();
    let seeds: Vec<&[u8]> = seeds.iter().map(Vec::as_slice).collect();
    inputs(&seeds, count, 256, drive);
}

#[test]
fn seeds_reassemble() {
    let mut owner = OpenDataDefragmenter::<2, 32>::new(Duration::from_micros(100));
    let first = mpdu(true, 0, true, false, None, &LLC);
    let last = mpdu(true, 1, false, false, None, &[3, 4]);
    let first = parse_open_data_fragment(DataInterfaceRole::Station, &first).unwrap();
    let last = parse_open_data_fragment(DataInterfaceRole::Station, &last).unwrap();
    owner.ingest(first, Instant::from_micros(1), read).unwrap();
    assert_eq!(
        owner.ingest(last, Instant::from_micros(2), read),
        Ok(OpenDataDefragmentation::Complete {
            expired: 0,
            value: 4
        })
    );
}

/// The fragment counter's bound: a fragment 15 announcing another is
/// refused, so the expected fragment never passes 15.
#[test]
fn a_sixteenth_fragment_with_more_fragments_is_refused() {
    let mut owner = OpenDataDefragmenter::<2, 32>::new(Duration::from_micros(100));
    for number in 0..16_u8 {
        let body = if number == 0 { &LLC[..] } else { &[number][..] };
        let frame = mpdu(true, number, true, false, None, body);
        let fragment = parse_open_data_fragment(DataInterfaceRole::Station, &frame).unwrap();
        let outcome = owner.ingest(fragment, Instant::from_micros(1), read);
        if number == 15 {
            assert_eq!(outcome, Err(OpenDataFragmentError::TooManyFragments));
        } else {
            assert!(matches!(
                outcome,
                Ok(OpenDataDefragmentation::Buffered { .. })
            ));
        }
    }
}

#[test]
fn reassembly_never_panics() {
    run(QUICK);
}

#[test]
#[ignore = "the full run"]
fn reassembly_never_panics_full() {
    run(FULL);
}
