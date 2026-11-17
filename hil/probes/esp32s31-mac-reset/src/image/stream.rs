//! Block B's sender (`tasks/165`): [`FRAMES`] frames of [`LENGTH`] octets to
//! the receiving rebind probe, each awaited, each the first [`LENGTH`]
//! octets of case C's 700-octet provenance frame: its counter at every 64
//! octets of the body, so that a frame in the wrong buffer, or bytes in a
//! retired one, are seen by content.
use super::amsdu::AmsduParts;
use super::transmit::{ORDINARY_SLOT, TIMEOUT, await_completion, start};
use super::*;
use crate::case_c::{Case, byte};

/// The frame's length: with the receiver's 64-octet RX-control header and
/// the FCS, one 640-byte buffer.
const LENGTH: usize = 500;
const FRAMES: u32 = 90_000;

pub(super) async fn series(parts: AmsduParts<'_>, access: &mut RadioAccess) -> Result<(), Failure> {
    let AmsduParts {
        frames,
        ordinary,
        station,
        peer,
        journal,
        power_24m: _,
    } = parts;
    mark(94);
    let mut ends = [0_u32; 3];
    for sequence in 0..FRAMES {
        let counter = Case::Provenance700.counter(sequence);
        let lease = frames.lease_frame(ORDINARY_SLOT, LENGTH, |offset| {
            byte(Case::Provenance700, station, peer, counter, offset)
        })?;
        start(access, ordinary, lease, LENGTH, frames.power, TIMEOUT)?;
        let ended = await_completion(access, ordinary, 0).await?;
        let at = match ended.status {
            0 => 0,
            5 => 1,
            _ => 2,
        };
        if let Some(cell) = ends.get_mut(at) {
            *cell = cell.saturating_add(1);
        }
    }
    journal.record(Kind::StreamSent, &[FRAMES, ends[0], ends[1], ends[2]]);
    mark(95);
    Ok(())
}
