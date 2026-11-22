//! `first-frame` (`tasks/358`, `tasks/360`): one frame of `L` octets from
//! the sender before the receiver's ring has served anything, on a fresh
//! boot of the receiver, and what the ring's descriptors and one window make
//! of it. The case records each boot against the prediction of `tasks/360`;
//! its verdict is that every length has its boots, not that the prediction
//! holds.

use oer_hil_services_radio_pair::{
    self as service, BringUp, BroughtUp, Counters, Expect, Expecting, ReceiveCounters, Record,
    Seat, SentOne, Single, Stop, Target, Targeted, Window, Windowed,
};
use oer_tools_hil_harness::{Cx, Failure};

use crate::{BRING_UP, RECEIVER, SENDER, STEP, keep, within};

/// The lengths: the edges of R's buffer, of the word rounding and of d0's.
pub(crate) const LENGTHS: [u16; 9] = [100, 192, 193, 230, 252, 253, 256, 300, 700];
/// Complete boots a length, and the boots a length may take in all.
const BOOTS: usize = 3;
const ATTEMPTS: usize = BOOTS + 5;
/// The window after the frame.
const WINDOW_MS: u32 = 1_000;
/// R's buffer: 256 octets, the 64-octet header below its floor.
const R_FRAME: u16 = 192;
const BUFFER: u16 = 256;
const FLOOR: u16 = 64;
/// The ring's descriptors; R's seat is the last.
const RING: usize = 16;

/// A descriptor's place: R, or the ring's `n`th.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) enum Place {
    R,
    Ring(usize),
}

/// `seats` (the ring's in order, then R's) by place.
fn placed(seats: &[Seat]) -> Vec<(Place, Seat)> {
    seats
        .iter()
        .enumerate()
        .map(|(index, seat)| {
            let place = if index == RING {
                Place::R
            } else {
                Place::Ring(index)
            };
            (place, *seat)
        })
        .collect()
}

/// `value` over `unit`, rounded up; `unit` is never zero here.
const fn units(value: u16, unit: u16) -> u16 {
    match value.checked_div(unit) {
        Some(whole) if value.is_multiple_of(unit) => whole,
        Some(whole) => whole.saturating_add(1),
        None => 0,
    }
}

/// `value` rounded up to a word.
const fn word_rounded(value: u16) -> u16 {
    units(value, 4).saturating_mul(4)
}

/// The places one frame of `length` octets may reach from R on: R, then as
/// many ring descriptors as its bytes past R's buffer fill.
fn reach(length: u16) -> Vec<Place> {
    let past = length.saturating_sub(R_FRAME);
    let ring = usize::from(units(past, BUFFER));
    core::iter::once(Place::R)
        .chain((0..ring).map(Place::Ring))
        .collect()
}

/// Nothing heard: every descriptor armed.
fn quiet(seats: &[Seat]) -> bool {
    !seats.is_empty() && seats.iter().all(|seat| *seat == Seat::Armed)
}

/// One frame of `length` and no more: something done, and every descriptor
/// that is not armed among the places that frame may reach.
fn one_frame(seats: &[Seat], length: u16) -> bool {
    let reach = reach(length);
    let changed: Vec<Place> = placed(seats)
        .into_iter()
        .filter(|(_, seat)| *seat != Seat::Armed)
        .map(|(place, _)| place)
        .collect();
    !changed.is_empty() && changed.iter().all(|place| reach.contains(place))
}

/// The prediction of `tasks/360` for a frame of `length`: the descriptors
/// it leaves (every other one armed), whether the window meets the length
/// anomaly, and the units it gives.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct Predicted {
    pub(crate) done: Vec<(Place, u16)>,
    pub(crate) anomaly: bool,
    pub(crate) singles: u32,
    pub(crate) chains: u32,
}

pub(crate) fn predicted(length: u16) -> Predicted {
    if length <= R_FRAME {
        return Predicted {
            done: vec![(Place::R, FLOOR.saturating_add(word_rounded(length)))],
            anomaly: false,
            singles: 0,
            chains: 0,
        };
    }
    let past = length.saturating_sub(R_FRAME);
    let ring = units(past, BUFFER);
    let last = past.saturating_sub(BUFFER.saturating_mul(ring.saturating_sub(1)));
    let terminal = word_rounded(last);
    let anomaly = ring == 1 && terminal < FLOOR;
    Predicted {
        done: vec![(Place::Ring(usize::from(ring.saturating_sub(1))), terminal)],
        anomaly,
        singles: u32::from(ring == 1 && !anomaly),
        chains: u32::from(ring > 1),
    }
}

/// What a boot showed, in the prediction's terms.
fn observed(seats: &[Seat], counts: &service::Counts) -> Predicted {
    Predicted {
        done: placed(seats)
            .into_iter()
            .filter_map(|(place, seat)| match seat {
                Seat::Done { length } => Some((place, length)),
                Seat::Armed | Seat::Other => None,
            })
            .collect(),
        anomaly: counts.length_anomalies > 0,
        singles: counts.singles,
        chains: counts.chains,
    }
}

/// How one boot ended.
enum Boot {
    /// Exactly one frame of the length, the sender's end known.
    Complete(serde_json::Value),
    /// Something else reached the ring, or the sender's end is unknown.
    Incomplete(String),
}

/// The ring's state in a counters answer.
fn ring(counted: &Counters) -> Result<&service::RingState, Failure> {
    match counted {
        Counters::Counted {
            ring: Some(ring), ..
        } => Ok(ring),
        other => Err(Failure::Fail(format!(
            "receiver: no ring's state: {other:?}"
        ))),
    }
}

/// The receiver on a fresh boot, brought up and told its sender; its
/// station's address.
fn fresh_receiver(cx: &mut Cx<'_>, sender: [u8; 6]) -> Result<[u8; 6], Failure> {
    let receiver = cx.device(RECEIVER)?;
    receiver.reset()?;
    let station = match receiver.call(BringUp, BRING_UP)? {
        BroughtUp::Up { station, .. } => station,
        other => return Err(Failure::Fail(format!("receiver: bring-up: {other:?}"))),
    };
    match receiver.call(Expect { sender }, STEP)? {
        Expecting::Expecting => Ok(station),
        other => Err(Failure::Fail(format!("receiver: expect: {other:?}"))),
    }
}

/// The sender on a fresh boot, brought up and told its receiver; its
/// station's address.
fn fresh_sender(cx: &mut Cx<'_>, receiver: Option<[u8; 6]>) -> Result<[u8; 6], Failure> {
    let device = cx.device(SENDER)?;
    device.reset()?;
    let station = match device.call(BringUp, BRING_UP)? {
        BroughtUp::Up { station, .. } => station,
        other => return Err(Failure::Fail(format!("sender: bring-up: {other:?}"))),
    };
    if let Some(receiver) = receiver {
        target(cx, receiver)?;
    }
    Ok(station)
}

fn target(cx: &mut Cx<'_>, receiver: [u8; 6]) -> Result<(), Failure> {
    match cx.device(SENDER)?.call(Target { receiver }, STEP)? {
        Targeted::Targeted => Ok(()),
        other => Err(Failure::Fail(format!("sender: target: {other:?}"))),
    }
}

/// One boot of the receiver with one frame of `length`.
fn boot(cx: &mut Cx<'_>, length: u16, sender: [u8; 6]) -> Result<Boot, Failure> {
    fresh_receiver(cx, sender)?;
    let before = cx.device(RECEIVER)?.call(ReceiveCounters, STEP)?;
    if !quiet(&ring(&before)?.seats) {
        stop_receiver(cx)?;
        return Ok(Boot::Incomplete(
            "a frame reached the ring before the sender's".to_owned(),
        ));
    }
    let ended = cx.device(SENDER)?.call(Single { length }, STEP)?;
    let after = cx.device(RECEIVER)?.call(ReceiveCounters, STEP)?;
    let windowed = cx.device(RECEIVER)?.call(
        Window {
            millis: WINDOW_MS,
            record: Record::Counts,
        },
        within(WINDOW_MS),
    )?;
    stop_receiver(cx)?;
    let status = match ended {
        SentOne::Ended { status } => status,
        other => return Ok(Boot::Incomplete(format!("sender: single: {other:?}"))),
    };
    let after_ring = ring(&after)?;
    if !one_frame(&after_ring.seats, length) {
        return Ok(Boot::Incomplete(format!(
            "not one frame of {length} in the ring: {:?}",
            after_ring.seats
        )));
    }
    let Windowed::Counted { counts, .. } = &windowed else {
        return Err(Failure::Fail(format!("receiver: window: {windowed:?}")));
    };
    let seen = observed(&after_ring.seats, counts);
    let expected = predicted(length);
    Ok(Boot::Complete(serde_json::json!({
        "length": length,
        "status": status,
        "standing": after_ring.standing,
        "controls": after_ring.controls,
        "seats": after_ring.seats,
        "window": counts,
        "observed": seen,
        "predicted": expected,
        "as_predicted": seen == expected,
    })))
}

fn stop_receiver(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let _stopped = cx.device(RECEIVER)?.call(Stop, STEP)?;
    Ok(())
}

/// The case: every length, `BOOTS` complete boots each, a boot a foreign
/// frame or an unknown end took repeated up to `ATTEMPTS`.
pub(crate) fn first_frame(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let sender = fresh_sender(cx, None)?;
    let mut receiver = None;
    let mut short = Vec::new();
    let mut table = Vec::new();
    for length in LENGTHS {
        let mut complete = 0;
        let mut incomplete = Vec::new();
        for _attempt in 0..ATTEMPTS {
            if complete == BOOTS {
                break;
            }
            if receiver.is_none() {
                let station = fresh_receiver(cx, sender)?;
                stop_receiver(cx)?;
                target(cx, station)?;
                receiver = Some(station);
            }
            match boot(cx, length, sender)? {
                Boot::Complete(record) => {
                    complete = complete.saturating_add(1);
                    table.push(record);
                }
                Boot::Incomplete(reason) => {
                    // A sender whose step failed refuses its next frame: a
                    // fresh boot of it, told its receiver again.
                    if reason.starts_with("sender:") {
                        fresh_sender(cx, receiver)?;
                    }
                    incomplete.push(reason);
                }
            }
        }
        keep(
            cx,
            RECEIVER,
            &format!("first-frame-{length}-incomplete"),
            &incomplete,
        );
        if complete < BOOTS {
            short.push(format!("{length}: {complete} of {BOOTS} boots"));
        }
    }
    keep(cx, RECEIVER, "first-frame", &table);
    if short.is_empty() {
        Ok(())
    } else {
        Err(Failure::Incomplete(short.join("; ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prediction_is_the_table_of_tasks_360() {
        let done = |length: u16| predicted(length).done;
        assert_eq!(done(100), [(Place::R, 164)]);
        assert_eq!(done(192), [(Place::R, 256)]);
        for (length, terminal) in [(193, 4), (230, 40), (252, 60)] {
            let p = predicted(length);
            assert_eq!(p.done, [(Place::Ring(0), terminal)], "{length}");
            assert!(p.anomaly && p.singles == 0, "{length}");
        }
        for (length, terminal) in [(253, 64), (256, 64), (300, 108)] {
            let p = predicted(length);
            assert_eq!(p.done, [(Place::Ring(0), terminal)], "{length}");
            assert!(!p.anomaly && p.singles == 1, "{length}");
        }
        let p = predicted(700);
        assert_eq!(p.done, [(Place::Ring(1), 252)]);
        assert_eq!((p.anomaly, p.singles, p.chains), (false, 0, 1));
    }

    #[test]
    fn one_frame_is_what_its_length_can_reach_and_nothing_beyond() {
        let mut seats = vec![Seat::Armed; RING + 1];
        assert!(quiet(&seats));
        assert!(!one_frame(&seats, 300), "nothing done is no frame");
        seats[0] = Seat::Done { length: 108 };
        assert!(!quiet(&seats));
        assert!(one_frame(&seats, 300));
        assert!(
            !one_frame(&seats, 100),
            "a frame in R reaches no ring descriptor"
        );
        seats[2] = Seat::Done { length: 60 };
        assert!(!one_frame(&seats, 300), "a second frame");
        let mut in_r = vec![Seat::Armed; RING + 1];
        in_r[RING] = Seat::Done { length: 164 };
        assert!(one_frame(&in_r, 100));
        let mut chain = vec![Seat::Armed; RING + 1];
        chain[1] = Seat::Done { length: 252 };
        assert!(one_frame(&chain, 700));
    }
}
