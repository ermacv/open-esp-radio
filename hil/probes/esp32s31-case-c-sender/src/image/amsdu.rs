//! Case C's sender (`tasks/154`, with `159`'s provenance series): from this
//! board to the receiving probe, in rounds, one frame of each of
//! [`case_c`](crate::case_c)'s cases a round, each awaited, in the round's
//! drawn order; counted per case and per the frame before it.
use super::transmit::{Descriptor, Frames};
#[cfg(not(feature = "ccmp"))]
use super::transmit::{ORDINARY_SLOT, TIMEOUT, await_completion, start};
use super::*;
#[cfg(not(feature = "ccmp"))]
use crate::case_c::{CASES, before_index, byte, order};

/// What a round borrows.
pub(super) struct AmsduParts<'a> {
    pub(super) frames: &'a Frames,
    pub(super) ordinary: &'a mut Descriptor,
    pub(super) station: [u8; 6],
    pub(super) peer: [u8; 6],
    pub(super) rounds: u32,
}

/// The rounds, each frame awaited; per case, by the frame before it:
/// acknowledged (status 0), ACK timeouts (status 5), other ends.
pub(super) async fn series(
    parts: AmsduParts<'_>,
    access: &mut RadioAccess,
) -> Result<heapless::Vec<service::CaseEnds, { service::CASES }>, Failure> {
    #[cfg(feature = "ccmp")]
    {
        super::ccmp::series(parts, access).await
    }
    #[cfg(not(feature = "ccmp"))]
    {
        rounds(parts, access).await
    }
}

/// Case C's rounds.
#[cfg(not(feature = "ccmp"))]
async fn rounds(
    parts: AmsduParts<'_>,
    access: &mut RadioAccess,
) -> Result<heapless::Vec<service::CaseEnds, { service::CASES }>, Failure> {
    let AmsduParts {
        frames,
        ordinary,
        station,
        peer,
        rounds,
    } = parts;
    // Per case, by the frame before it (or none): acknowledged, ACK
    // timeouts, other ends.
    let mut ends = [[[0_u32; 3]; CASES.len() + 1]; CASES.len()];
    for round in 0..rounds {
        for case in order(round) {
            let counter = case.counter(round);
            let length = case.length();
            let lease = frames.lease_frame(ORDINARY_SLOT, length, |offset| {
                byte(case, station, peer, counter, offset)
            })?;
            start(access, ordinary, lease, length, frames.power, TIMEOUT)?;
            let ended = await_completion(access, ordinary).await?;
            let at = CASES.iter().position(|each| *each == case);
            if let Some(cell) = at
                .and_then(|at| ends.get_mut(at))
                .and_then(|row| row.get_mut(before_index(round, case)))
                .and_then(|cell| {
                    cell.get_mut(match ended.status {
                        0 => 0,
                        5 => 1,
                        _ => 2,
                    })
                })
            {
                *cell = cell.saturating_add(1);
            }
        }
    }
    Ok(CASES
        .iter()
        .zip(&ends)
        .map(|(case, row)| service::CaseEnds {
            tag: case.tag(),
            acknowledged: row.map(|ends| ends[0]),
            timeouts: row.map(|ends| ends[1]),
            other: row.iter().map(|ends| ends[2]).fold(0, u32::saturating_add),
        })
        .collect())
}
