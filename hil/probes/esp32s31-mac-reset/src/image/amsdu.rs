//! Case C's sender (`tasks/154`, with `159`'s provenance series): from this
//! board to the receiving probe, in rounds, one frame of each of
//! [`case_c`](crate::case_c)'s cases a round, each awaited.
use super::transmit::{Descriptor, Frames, ORDINARY_SLOT, TIMEOUT, await_completion, start};
use super::*;
use crate::case_c::{CASES, ROUNDS, byte};

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
    let mut ends = [[0_u32; 4]; CASES.len()];
    for round in 0..ROUNDS {
        for (at, case) in CASES.into_iter().enumerate() {
            let counter = case.counter(round);
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
    for (case, row) in CASES.iter().zip(ends) {
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
