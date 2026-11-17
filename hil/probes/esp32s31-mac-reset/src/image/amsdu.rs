//! Case C's sender (`tasks/154`, with `159`'s provenance series): from this
//! board to the receiving probe (`esp32s31-rx-chain --features laps,amsdu`),
//! in rounds, one of each:
//!
//! 1. a QoS Data MPDU with A-MSDU Present carrying two short subframes;
//! 2. the same with one subframe;
//! 3. the control: case 1's bytes with A-MSDU Present clear;
//! 4. and 5. non-QoS Data of 700 and 1 500 octets whose body carries the
//!    frame's counter at every 64 octets, so that every buffer of a chain at
//!    the receiver names its frame.
//!
//! Every frame carries its counter: its case in the top four bits, its round
//! below. The layout is the receiver's too: change both together.
use super::transmit::{Descriptor, Frames, ORDINARY_SLOT, TIMEOUT, await_completion, start};
use super::*;

/// The rounds: at least 5 000 of the provenance series' units a geometry
/// over its two lengths, and as many of each A-MSDU case.
const ROUNDS: u32 = 2_600;
/// An A-MSDU subframe's MSDU: LLC/SNAP and a payload, the counter first.
const MSDU: usize = 100;
/// The QoS Data header: Frame Control, Duration, three addresses, Sequence
/// Control, QoS Control (IEEE 802.11-2020 9.3.2.1).
const QOS_HEADER: usize = 26;
/// A-MSDU subframe header: DA, SA, Length (IEEE 802.11-2020 9.3.2.2.2).
const SUBFRAME_HEADER: usize = 14;
/// The provenance series' lengths.
const PROVENANCE: [usize; 2] = [700, super::transmit::FRAME];
/// The provenance series' counter period and its first place, after the
/// header and the LLC/SNAP.
const PERIOD: usize = 64;
const BODY: usize = 32;
const SNAP: [u8; 8] = [0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0xb5];

/// One case of the round.
#[derive(Clone, Copy)]
enum Case {
    Amsdu2,
    Amsdu1,
    Control,
    Provenance(usize),
}

impl Case {
    const fn tag(self) -> u32 {
        match self {
            Self::Amsdu2 => 1,
            Self::Amsdu1 => 2,
            Self::Control => 3,
            Self::Provenance(0) => 4,
            Self::Provenance(_) => 5,
        }
    }

    /// The MPDU's length.
    const fn length(self) -> usize {
        match self {
            Self::Amsdu2 | Self::Control => QOS_HEADER + PADDED + LAST,
            Self::Amsdu1 => QOS_HEADER + LAST,
            Self::Provenance(at) => match at {
                0 => PROVENANCE[0],
                _ => PROVENANCE[1],
            },
        }
    }
}

/// A subframe's bytes: [`PADDED`] when another follows, [`LAST`] when not.
const LAST: usize = SUBFRAME_HEADER + MSDU;
const PADDED: usize = LAST.next_multiple_of(4);

/// The byte at `offset` of `case`'s frame `counter` from `station` to `peer`.
fn byte(case: Case, station: [u8; 6], peer: [u8; 6], counter: u32, offset: usize) -> u8 {
    let counter = counter.to_le_bytes();
    let address = |at: usize| match at {
        4..10 => peer.get(at - 4).copied(),
        10..16 => station.get(at - 10).copied(),
        16..22 => peer.get(at - 16).copied(),
        _ => None,
    };
    match case {
        Case::Provenance(_) => match offset {
            0 => 0x08,
            2 => 44,
            4..22 => address(offset).unwrap_or(0),
            24..32 => SNAP.get(offset - 24).copied().unwrap_or(0),
            _ if offset >= BODY && (offset - BODY) % PERIOD < 4 => {
                counter.get((offset - BODY) % PERIOD).copied().unwrap_or(0)
            }
            _ if offset < 24 => 0,
            _ => offset as u8,
        },
        Case::Amsdu2 | Case::Amsdu1 | Case::Control => {
            let present = !matches!(case, Case::Control);
            match offset {
                0 => 0x88,
                2 => 44,
                4..22 => address(offset).unwrap_or(0),
                // QoS Control: TID 0, A-MSDU Present (bit 7).
                24 => {
                    if present {
                        0x80
                    } else {
                        0
                    }
                }
                _ if offset < QOS_HEADER => 0,
                _ => {
                    // Within the subframes: the first padded, then the last.
                    let at = offset - QOS_HEADER;
                    let first = if matches!(case, Case::Amsdu1) {
                        LAST
                    } else {
                        PADDED
                    };
                    let (index, at) = if matches!(case, Case::Amsdu1) || at < first {
                        (0_u8, at)
                    } else {
                        (1, at - first)
                    };
                    let length = (MSDU as u16).to_be_bytes();
                    match at {
                        0..6 => peer.get(at).copied().unwrap_or(0),
                        6..12 => station.get(at - 6).copied().unwrap_or(0),
                        12..14 => length.get(at - 12).copied().unwrap_or(0),
                        14..22 => SNAP.get(at - 14).copied().unwrap_or(0),
                        22..26 => counter.get(at - 22).copied().unwrap_or(0),
                        _ if at < SUBFRAME_HEADER + MSDU => (at as u8) ^ (index << 7),
                        // Padding.
                        _ => 0,
                    }
                }
            }
        }
    }
}

/// What case C borrows from part two's series.
pub(super) struct AmsduParts<'a> {
    pub(super) frames: &'a Frames,
    pub(super) ordinary: &'a mut Descriptor,
    pub(super) station: [u8; 6],
    pub(super) peer: [u8; 6],
    pub(super) journal: &'a mut Journal,
}

/// The rounds, each frame awaited; per case: sent, acknowledged (status
/// 0), ACK timeouts (status 5), other ends.
pub(super) async fn series(parts: AmsduParts<'_>, access: &mut RadioAccess) -> Result<(), Failure> {
    let AmsduParts {
        frames,
        ordinary,
        station,
        peer,
        journal,
    } = parts;
    mark(90);
    let cases = [
        Case::Amsdu2,
        Case::Amsdu1,
        Case::Control,
        Case::Provenance(0),
        Case::Provenance(1),
    ];
    let mut ends = [[0_u32; 4]; 5];
    for round in 0..ROUNDS {
        for (at, case) in cases.into_iter().enumerate() {
            let counter = case.tag() << 28 | round;
            let length = case.length();
            let lease = frames.lease_frame(ORDINARY_SLOT, length, |offset| {
                byte(case, station, peer, counter, offset)
            })?;
            start(access, ordinary, lease, length, frames.power, TIMEOUT)?;
            let ended = await_completion(access, ordinary, 0).await?;
            if let Some(row) = ends.get_mut(at) {
                let column = match ended.status {
                    0 => 1,
                    5 => 2,
                    _ => 3,
                };
                row[0] = row[0].saturating_add(1);
                if let Some(cell) = row.get_mut(column) {
                    *cell = cell.saturating_add(1);
                }
            }
        }
    }
    for (case, row) in cases.iter().zip(ends) {
        journal.record(
            Kind::AmsduSent,
            &[
                case.tag(),
                case.length() as u32,
                row[0],
                row[1],
                row[2],
                row[3],
            ],
        );
    }
    mark(91);
    Ok(())
}
