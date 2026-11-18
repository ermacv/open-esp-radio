//! The rebind probe's sender (`tasks/193`, probe-only): 500-octet frames to
//! the receiving rebind probe, each awaited, each the first 500 octets of
//! case C's 700-octet provenance frame, so a frame in the wrong buffer, or
//! bytes in a retired one, are seen by content. With `stream-isolated` a
//! pause follows each frame.
use super::amsdu::AmsduParts;
use super::transmit::{ORDINARY_SLOT, TIMEOUT, await_completion, start, tx_phase};
use super::*;
use crate::case_c::{Case, byte};
use oer_portable_embassy_time::EmbassyClock;
use oer_portable_time::{Clock, Duration, Timer};

/// The frame's length: with the receiver's RX-control header and the FCS,
/// one 640-byte buffer.
const LENGTH: usize = 500;
/// The wait after the peer is learned (the injector's burst ends within it).
const QUIET_MS: u32 = 1_500;
/// How long the stream runs: past the receiver's fifteen segments of
/// 2.5 s from its first frame.
const RUN_MS: u32 = 45_000;
/// `stream-isolated`: the pause after each frame.
#[cfg(feature = "stream-isolated")]
const PAUSE_US: u64 = 3_000;

pub(super) async fn series(parts: AmsduParts<'_>, access: &mut RadioAccess) -> Result<(), Failure> {
    let AmsduParts {
        frames,
        ordinary,
        station,
        peer,
        journal,
    } = parts;
    mark(94);
    if let Some(at) = EmbassyClock
        .now()
        .checked_add(Duration::from_millis(QUIET_MS))
    {
        EmbassyClock.wait_until(at).await;
    }
    let end = EmbassyClock
        .now()
        .checked_add(Duration::from_millis(RUN_MS))
        .ok_or((tx_phase::FRAME, 0x70))?;
    let mut ends = [0_u32; 3];
    let mut sent = 0_u32;
    while EmbassyClock.now() < end {
        let counter = Case::Provenance700.counter(sent);
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
        sent = sent.saturating_add(1);
        #[cfg(feature = "stream-isolated")]
        if let Some(at) = EmbassyClock
            .now()
            .checked_add(Duration::from_micros(PAUSE_US))
        {
            EmbassyClock.wait_until(at).await;
        }
    }
    journal.record(Kind::AmsduSent, &[0x5354, sent, ends[0], ends[1], ends[2]]);
    mark(95);
    Ok(())
}
