//! Case C's sender (`tasks/154`, with `159`'s provenance series): from this
//! board to the receiving probe, in rounds, one frame of each of
//! [`case_c`](crate::case_c)'s cases a round, each awaited, in the round's
//! drawn order; counted per case and per the frame before it.
use super::transmit::{Descriptor, Frames, ORDINARY_SLOT, TIMEOUT, await_completion, start};
use super::*;
use crate::case_c::{CASES, ROUNDS, before_index, byte, order};

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
    // Per case, by the frame before it (or none): acknowledged, ACK
    // timeouts, other ends.
    let mut ends = [[[0_u32; 3]; CASES.len() + 1]; CASES.len()];
    for round in 0..ROUNDS {
        for case in order(round) {
            let counter = case.counter(round);
            let length = case.length();
            let lease = frames.lease_frame(ORDINARY_SLOT, length, |offset| {
                byte(case, station, peer, counter, offset)
            })?;
            start(access, ordinary, lease, length, frames.power, TIMEOUT)?;
            let ended = await_completion(access, ordinary, 0).await?;
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
    for (case, row) in CASES.iter().zip(&ends) {
        // The tag; the acknowledged by the frame before; the ACK timeouts by
        // the frame before; the other ends in all.
        let mut values = [0_u32; 1 + 2 * (CASES.len() + 1) + 1];
        let (acked, rest) = values.split_at_mut(1 + CASES.len() + 1);
        let (timeouts, other) = rest.split_at_mut(CASES.len() + 1);
        if let Some(tag) = acked.first_mut() {
            *tag = case.tag();
        }
        for (before, ends) in row.iter().enumerate() {
            if let Some(cell) = acked.get_mut(1 + before) {
                *cell = ends[0];
            }
            if let Some(cell) = timeouts.get_mut(before) {
                *cell = ends[1];
            }
            if let Some(cell) = other.first_mut() {
                *cell = cell.saturating_add(ends[2]);
            }
        }
        journal.record(Kind::AmsduSent, &values);
    }
    mark(91);
    Ok(())
}
