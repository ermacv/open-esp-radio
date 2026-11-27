//! `ring-full` (`tasks/444`): the receiver's ring written whole, its tail
//! too, before its first service. Sixteen frames nobody acknowledges do it
//! (`HW_RX_UNACKNOWLEDGED_TAIL_WRITTEN_20261127`): the sender's broadcast
//! Probe Requests of an SSID no access point has. Then ordinary service,
//! under the sender's frames to the receiver: the ring goes on, and every
//! frame the receiver acknowledged is given.

use oer_hil_services_radio_pair::{
    self as service, Burst, Bursted, ReceiveCounters, Record, To, Window, Windowed,
};
use oer_tools_hil_harness::{Cx, Failure};

use crate::first_frame::{fresh_receiver, fresh_sender, ring, stop_receiver, target};
use crate::{RECEIVER, SENDER, STEP, keep, within};

/// Boots, and the boots it may take in all.
const BOOTS: usize = 5;
const ATTEMPTS: usize = BOOTS + 3;
/// The Probe Requests: twice the ring's sixteen descriptors.
const PROBES: u16 = 32;
/// The window, and the sender's frames to the receiver meanwhile.
const WINDOW_MS: u32 = 500;
const FRAMES: u16 = 200;
const LENGTH: u16 = 100;

/// How long a burst may take: each frame awaited, at most 20 ms.
fn burst_bound(frames: u16) -> std::time::Duration {
    STEP.saturating_add(std::time::Duration::from_millis(20 * u64::from(frames)))
}

/// The tail written: LAST the ring's last descriptor, NEXT zero, its word
/// completed.
fn tail_written(standing: &service::Standing) -> bool {
    standing.next == service::NextAt::Zero && standing.tail_word == service::TailWord::Completed
}

/// One boot's verdict once the tail was written: the ring went on, and
/// gave every frame of the sender the receiver acknowledged.
fn went_on(counts: &service::Counts, acknowledged: u16) -> Result<(), String> {
    if counts.patterns == 0 {
        return Err(format!("the ring stood: {counts:?}"));
    }
    if counts.patterns != u32::from(acknowledged) {
        return Err(format!(
            "{} of the sender's frames given, {acknowledged} acknowledged",
            counts.patterns
        ));
    }
    if counts.length_anomalies != 0 || counts.doorbell_anomalies != 0 {
        return Err(format!("anomalies: {counts:?}"));
    }
    Ok(())
}

pub(crate) fn ring_full(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let sender = fresh_sender(cx, None)?;
    let station = fresh_receiver(cx, sender)?;
    stop_receiver(cx)?;
    target(cx, station)?;
    let (mut complete, mut samples, mut refuted, mut short) =
        (0, Vec::new(), Vec::new(), Vec::new());
    for _ in 0..ATTEMPTS {
        if complete == BOOTS {
            break;
        }
        fresh_receiver(cx, sender)?;
        let probes: Bursted = cx.device(SENDER)?.call(
            Burst {
                to: To::ProbeRequest,
                frames: PROBES,
                gap_micros: 0,
            },
            burst_bound(PROBES),
        )?;
        let before = cx.device(RECEIVER)?.call(ReceiveCounters, STEP)?;
        let posted = cx.device(SENDER)?.post(Burst {
            to: To::Receiver { length: LENGTH },
            frames: FRAMES,
            gap_micros: 0,
        })?;
        let windowed = cx.device(RECEIVER)?.call(
            Window {
                millis: WINDOW_MS,
                record: Record::Counts,
            },
            within(WINDOW_MS),
        )?;
        let frames: Bursted = cx.device(SENDER)?.wait(posted, burst_bound(FRAMES))?;
        stop_receiver(cx)?;
        let standing = ring(&before)?.standing;
        let verdict = match (&windowed, frames) {
            _ if !tail_written(&standing) => None,
            (Windowed::Counted { counts, .. }, Bursted::Sent { acknowledged, .. }) => {
                Some(went_on(counts, acknowledged))
            }
            (windowed, frames) => Some(Err(format!("{windowed:?}, {frames:?}"))),
        };
        samples.push(serde_json::json!({
            "probes": probes,
            "standing": standing,
            "window": windowed,
            "frames": frames,
            "verdict": verdict.as_ref().map(|v| v.as_ref().err()),
        }));
        match verdict {
            None => short.push(format!("the tail not written: {standing:?}")),
            Some(verdict) => {
                complete = complete.saturating_add(1);
                if let Err(reason) = verdict {
                    refuted.push(reason);
                }
            }
        }
    }
    keep(cx, RECEIVER, "ring-full", &samples);
    if !refuted.is_empty() {
        return Err(Failure::Fail(refuted.join("; ")));
    }
    if complete < BOOTS {
        return Err(Failure::Incomplete(format!(
            "{complete} of {BOOTS} boots wrote the tail: {}",
            short.join("; ")
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standing(next: service::NextAt, tail_word: service::TailWord) -> service::Standing {
        service::Standing {
            last: Some(15),
            next,
            tail: 15,
            tail_word,
            cursor: service::Cursor::Steady,
        }
    }

    #[test]
    fn only_a_completed_tail_with_next_zero_is_the_tail_written() {
        use service::{NextAt, TailWord};
        assert!(tail_written(&standing(NextAt::Zero, TailWord::Completed)));
        assert!(!tail_written(&standing(NextAt::Ring(15), TailWord::Armed)));
        assert!(!tail_written(&standing(NextAt::Zero, TailWord::Armed)));
    }

    #[test]
    fn the_ring_goes_on_only_when_it_gives_every_acknowledged_frame() {
        let counts = |patterns| service::Counts {
            patterns,
            ..service::Counts::default()
        };
        assert_eq!(went_on(&counts(120), 120), Ok(()));
        assert!(went_on(&counts(0), 0).is_err(), "the ring stood");
        assert!(went_on(&counts(119), 120).is_err(), "a frame lost");
        let anomaly = service::Counts {
            length_anomalies: 1,
            ..counts(120)
        };
        assert!(went_on(&anomaly, 120).is_err());
    }
}
