//! `ring-boots` (`tasks/444`): the receiver booted for each sample, its
//! ring left without a service after the bring-up while the sender sends,
//! then read: the cursor and the descriptors' words, then the first service
//! with each unit's MAC header, then a window. The air is the sender's
//! broadcast Probe Requests of a fixed SSID no access point has
//! (`PROBE_SSID`), twice the ring's count, slowly and back to back. It
//! records and judges nothing of the ring.

use oer_hil_services_radio_pair::{
    Burst, Bursted, Doorbell, Exit, Exited, FirstService, Header, ReceiveCounters, Record, Served,
    To, UnitHeader, Way, Window, Windowed,
};
use oer_tools_hil_harness::{Cx, Failure};

use crate::first_frame::{fresh_receiver, fresh_sender, ring, stop_receiver, target};
use crate::{RECEIVER, SENDER, STEP, keep, within};

/// Boots a kind of air.
const BOOTS: usize = 5;
/// Frames a sample's air sends, past the ring's sixteen descriptors; each
/// fits one descriptor.
const FRAMES: u16 = 32;
const LENGTH: u16 = 100;
/// The slow air's gap between frames.
const SLOW_GAP_MICROS: u32 = 50_000;
/// The window after the first service, and the sender's frames meanwhile.
const WINDOW_MS: u32 = 500;
const AFTER_FRAMES: u16 = 200;

/// The kinds of air: whom the frames are for, and their gap.
const AIRS: [(&str, To, u32); 2] = [
    ("probe-slow", To::ProbeRequest, SLOW_GAP_MICROS),
    ("probe-back-to-back", To::ProbeRequest, 0),
];

/// How long a burst may take: each frame awaited (at most 20 ms) and its
/// gap.
fn burst_bound(frames: u16, gap_micros: u32) -> std::time::Duration {
    let each = u64::from(gap_micros).saturating_add(20_000);
    STEP.saturating_add(std::time::Duration::from_micros(
        each.saturating_mul(u64::from(frames)),
    ))
}

fn burst(cx: &mut Cx<'_>, to: To, frames: u16, gap_micros: u32) -> Result<Bursted, Failure> {
    cx.device(SENDER)?.call(
        Burst {
            length: LENGTH,
            to,
            frames,
            gap_micros,
        },
        burst_bound(frames, gap_micros),
    )
}

/// The case: `BOOTS` boots of each kind of air, every sample kept.
pub(crate) fn ring_boots(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let sender = fresh_sender(cx, None)?;
    let station = fresh_receiver(cx, sender)?;
    stop_receiver(cx)?;
    target(cx, station)?;
    let mut failed = Vec::new();
    for (name, to, gap_micros) in AIRS {
        let mut samples = Vec::new();
        for _ in 0..BOOTS {
            fresh_receiver(cx, sender)?;
            let air = burst(cx, to, FRAMES, gap_micros)?;
            let before = cx.device(RECEIVER)?.call(ReceiveCounters, STEP)?;
            let served = cx.device(RECEIVER)?.call(FirstService, STEP)?;
            let mut headers = Vec::new();
            if let Served::Units { count, .. } = served {
                for index in 0..count {
                    let header: Header = cx.device(RECEIVER)?.call(UnitHeader { index }, STEP)?;
                    headers.push(header);
                }
            }
            let after_air = cx.device(SENDER)?.post(Burst {
                length: LENGTH,
                to: To::Receiver,
                frames: AFTER_FRAMES,
                gap_micros: 0,
            })?;
            let windowed = cx.device(RECEIVER)?.call(
                Window {
                    millis: WINDOW_MS,
                    record: Record::Counts,
                },
                within(WINDOW_MS),
            )?;
            let after_air: Bursted = cx
                .device(SENDER)?
                .wait(after_air, burst_bound(AFTER_FRAMES, 0))?;
            stop_receiver(cx)?;
            let ring = ring(&before)?;
            if !matches!(served, Served::Units { .. })
                || !matches!(windowed, Windowed::Counted { .. })
            {
                failed.push(format!("{name}: {served:?}, {windowed:?}"));
            }
            samples.push(serde_json::json!({
                "air": air,
                "standing": ring.standing,
                "controls": ring.controls,
                "seats": ring.seats,
                "counters": before,
                "served": served,
                "headers": headers,
                "window": windowed,
                "after_air": after_air,
            }));
        }
        keep(cx, RECEIVER, &format!("ring-boots-{name}"), &samples);
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(Failure::Fail(failed.join("; ")))
    }
}

/// The ways out of a ring written whole (`tasks/444`, the three series):
/// one service of the whole ring; one of all but the last and the doorbell;
/// a whole one, then BASE written by the probe, with and without the
/// doorbell.
const WAYS: [(&str, Way); 4] = [
    ("whole", Way::Whole),
    ("withhold-last", Way::WithholdLast),
    ("base-rung", Way::Base(Doorbell::Rung)),
    ("base-silent", Way::Base(Doorbell::Silent)),
];

/// `ring-exits`: `BOOTS` boots of each way, the ring written whole by the
/// sender's Probe Requests back to back, then the way on a quiet air, then
/// a window under the sender's frames. Every sample kept; it judges nothing
/// of the ring.
pub(crate) fn ring_exits(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let sender = fresh_sender(cx, None)?;
    let station = fresh_receiver(cx, sender)?;
    stop_receiver(cx)?;
    target(cx, station)?;
    let mut failed = Vec::new();
    for (name, way) in WAYS {
        let mut samples = Vec::new();
        for _ in 0..BOOTS {
            fresh_receiver(cx, sender)?;
            let air = burst(cx, To::ProbeRequest, FRAMES, 0)?;
            let before = cx.device(RECEIVER)?.call(ReceiveCounters, STEP)?;
            let exited = cx.device(RECEIVER)?.call(Exit { way }, STEP)?;
            let after_air = cx.device(SENDER)?.post(Burst {
                length: LENGTH,
                to: To::Receiver,
                frames: AFTER_FRAMES,
                gap_micros: 0,
            })?;
            let windowed = cx.device(RECEIVER)?.call(
                Window {
                    millis: WINDOW_MS,
                    record: Record::Counts,
                },
                within(WINDOW_MS),
            )?;
            let after_air: Bursted = cx
                .device(SENDER)?
                .wait(after_air, burst_bound(AFTER_FRAMES, 0))?;
            stop_receiver(cx)?;
            let ring = ring(&before)?;
            if !matches!(exited, Exited::Done { .. })
                || !matches!(windowed, Windowed::Counted { .. })
            {
                failed.push(format!("{name}: {exited:?}, {windowed:?}"));
            }
            samples.push(serde_json::json!({
                "air": air,
                "standing": ring.standing,
                "seats": ring.seats,
                "exited": exited,
                "window": windowed,
                "after_air": after_air,
            }));
        }
        keep(cx, RECEIVER, &format!("ring-exits-{name}"), &samples);
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(Failure::Fail(failed.join("; ")))
    }
}
