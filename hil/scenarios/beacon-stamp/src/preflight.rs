//! The lease's first stage: whether the capture can carry the run.
//!
//! Between the probe's write and its reset the probe's first boot runs and
//! transmits its frames; the preflight reads the capture of them. It
//! accepts once `FRAMES` of the probe's tagged frames (any run of it) are
//! in the capture, every one with a radiotap TSFT and a good FCS, and
//! refuses at once a capture of another link type, a probe frame without
//! TSFT, or nothing within `WINDOW`. It checks the stream and the presence
//! of the TSFT, not its origin: what clock the sniffer's TSFT counts is
//! established apart (the note's mt76 trace), and the decision's own
//! checks still apply to the measured run. No refusal is retried.

use crate::sniffer::{Result, Sniffer};
use crate::{capture, latch};
use std::time::{Duration, Instant};

/// How long the probe's first boot may take to show its frames: boot, RF
/// start, the clock separation and its first rounds.
pub const WINDOW: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(500);
/// Probe frames the preflight needs.
pub const FRAMES: usize = 3;

/// What the capture so far says.
#[derive(Debug, Eq, PartialEq)]
pub enum Look {
    /// Not enough yet.
    Wait,
    Accepted {
        frames: usize,
    },
    Refused(String),
}

/// Judge the capture's bytes so far.
pub fn look(bytes: &[u8]) -> Look {
    if bytes.len() < 24 {
        return Look::Wait;
    }
    let capture = match capture::parse_prefix(bytes) {
        Ok(capture) => capture,
        Err(error) => return Look::Refused(error),
    };
    // A record without TSFT keeps no frame to tell whose it was: any one
    // is a sniffer that does not stamp.
    if capture.without_tsft > 0 {
        return Look::Refused(format!(
            "{} captured frames carry no radiotap TSFT",
            capture.without_tsft
        ));
    }
    let frames = capture
        .frames
        .iter()
        .filter(|frame| frame.tag.is_some())
        .count();
    if frames >= FRAMES {
        Look::Accepted { frames }
    } else {
        Look::Wait
    }
}

/// What the preflight accepted: the probe frames it saw, and the latch
/// calibration of the first boot. The latch is recorded, not a condition
/// of the run: the decision compares intervals of equal frames, in which a
/// constant latch point cancels.
#[derive(Debug)]
pub struct Preflight {
    pub frames: usize,
    pub pairs: usize,
    pub unanswered: usize,
    pub unobserved: usize,
    pub contradictory: usize,
    pub latch: latch::Outcome,
    pub evidence: latch::Evidence,
}

/// Whether the capture holds every calibration frame of the first boot;
/// a malformed record refuses it.
fn calibrated(bytes: &[u8]) -> Result<Option<latch::Pairs>> {
    let capture = capture::parse_prefix(bytes)?;
    let found = latch::pairs(&capture, None);
    Ok((found.seen() >= latch::FRAMES).then_some(found))
}

/// Look at the capture until it is accepted or refused, within `WINDOW`;
/// then until the first boot's calibration frames are in, within the same
/// window, and decide the latch on what came.
pub fn wait<C, S: Sniffer<C>>(sniffer: &mut S) -> Result<Preflight> {
    let deadline = Instant::now() + WINDOW;
    let frames = loop {
        match look(&sniffer.captured()?) {
            Look::Accepted { frames } => break frames,
            Look::Refused(reason) => return Err(format!("the preflight refused: {reason}")),
            Look::Wait if Instant::now() >= deadline => {
                return Err(format!(
                    "the preflight refused: fewer than {FRAMES} probe frames with TSFT in {} s",
                    WINDOW.as_secs()
                ));
            }
            Look::Wait => std::thread::sleep(POLL),
        }
    };
    let found = loop {
        let bytes = sniffer.captured()?;
        if let Some(found) = calibrated(&bytes)? {
            break found;
        }
        if Instant::now() >= deadline {
            let capture = capture::parse_prefix(&bytes)?;
            break latch::pairs(&capture, None);
        }
        std::thread::sleep(POLL);
    };
    let (latch, evidence) = latch::decide(&found);
    Ok(Preflight {
        frames,
        pairs: found.pairs.len(),
        unanswered: found.unanswered,
        unobserved: found.unobserved,
        contradictory: found.contradictory,
        latch,
        evidence,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{Tag, synthetic};

    const TA: [u8; 6] = [0x60, 0x55, 0xf9, 1, 2, 3];

    fn tagged(n: u16) -> Vec<u8> {
        synthetic::record(
            u64::from(n) * 100_000,
            &synthetic::mpdu(
                0x80,
                TA,
                7,
                Tag {
                    kind: b'b',
                    arm: b'u',
                    n,
                    run: 1,
                    k: 7,
                },
            ),
        )
    }

    #[test]
    fn three_tagged_frames_with_tsft_are_accepted() {
        let bytes = synthetic::file(&[tagged(0), tagged(1), tagged(2)]);
        assert_eq!(look(&bytes), Look::Accepted { frames: 3 });
    }

    #[test]
    fn a_record_whose_declared_field_lies_past_it_len_refuses_at_once() {
        // A TSFT declared with it_len 8, then the tagged frames whole.
        let mut malformed = vec![0, 0, 8, 0];
        malformed.extend_from_slice(&1u32.to_le_bytes());
        malformed.extend_from_slice(&tagged(9)[8..]);
        let bytes = synthetic::file(&[tagged(0), malformed, tagged(1), tagged(2)]);
        let Look::Refused(reason) = look(&bytes) else {
            panic!("a malformed record was not refused");
        };
        assert!(
            reason.contains("record 2") && reason.contains("past it_len"),
            "{reason}"
        );
        assert!(calibrated(&bytes).is_err());
    }

    #[test]
    fn a_stream_still_short_waits() {
        assert_eq!(look(&[]), Look::Wait);
        let bytes = synthetic::file(&[tagged(0)]);
        assert_eq!(look(&bytes), Look::Wait);
        // Half a record more is still one frame.
        let mut partial = synthetic::file(&[tagged(0), tagged(1)]);
        partial.truncate(partial.len() - 5);
        assert_eq!(look(&partial), Look::Wait);
    }

    #[test]
    fn another_link_type_or_a_frame_without_tsft_refuses_at_once() {
        let mut ethernet = synthetic::file(&[]);
        ethernet[20] = 1;
        assert!(matches!(look(&ethernet), Look::Refused(_)));
        let mut untimed = vec![0, 0, 8, 0, 0, 0, 0, 0];
        untimed.extend_from_slice(&[0x80; 40]);
        let bytes = synthetic::file(&[tagged(0), untimed]);
        assert!(matches!(look(&bytes), Look::Refused(reason) if reason.contains("TSFT")));
    }
}
