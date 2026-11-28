//! `first-frame` (`tasks/358`, `tasks/361`): one frame of `L` octets from
//! the sender before the receiver's ring has served anything, on a fresh
//! boot of the receiver. The ring's first descriptor is the walker's BASE
//! (`HW_RX_BASE_HEAD_20261122`): the frame is the ring's first unit, a single
//! in d0 or a chain from d0, and the first service gives it whole. The case
//! is the regression case of `tasks/358`'s defect, where a first frame of
//! 193 to 252 octets left a unit below the floor.

use oer_hil_services_radio_pair::{
    self as service, BringUp, BroughtUp, Counters, Expect, Expecting, ReceiveCounters, Record,
    Seat, SentOne, Single, Stop, Target, Targeted, Window, Windowed,
};
use oer_tools_hil_harness::{Cx, Failure};

use crate::{BRING_UP, RECEIVER, SENDER, STEP, keep, within};

/// The lengths: the edges of R's buffer, of the word rounding and of d0's.
/// d0 alone, a chain whose terminal is below the floor, at the floor, and of
/// three descriptors.
pub(crate) const LENGTHS: [u16; 4] = [100, 252, 253, 700];
/// Complete boots a length, and the boots a length may take in all.
const BOOTS: usize = 3;
const ATTEMPTS: usize = BOOTS + 5;
/// The window after the frame.
const WINDOW_MS: u32 = 1_000;
/// A unit's first buffer: 256 octets, the 64-octet header below its floor,
/// so 192 octets of frame; every later buffer of a chain 256.
const FIRST: u16 = 192;
const BUFFER: u16 = 256;
const FLOOR: u16 = 64;

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

/// The descriptors one frame of `length` octets takes from d0 on.
fn reach(length: u16) -> usize {
    usize::from(units(length.saturating_sub(FIRST), BUFFER)).saturating_add(1)
}

/// Nothing heard: every descriptor armed.
fn quiet(seats: &[Seat]) -> bool {
    !seats.is_empty() && seats.iter().all(|seat| *seat == Seat::Armed)
}

/// One frame of `length` and no more: something done, and every descriptor
/// that is not armed among those that frame takes from d0.
fn one_frame(seats: &[Seat], length: u16) -> bool {
    let reach = reach(length);
    let mut changed = seats
        .iter()
        .enumerate()
        .filter(|(_, seat)| **seat != Seat::Armed)
        .peekable();
    changed.peek().is_some() && changed.all(|(index, _)| index < reach)
}

/// What a frame of `length` leaves from d0 on: its terminal descriptor and
/// its `LENGTH` (every other descriptor armed, a chain's first ones read as
/// continuations), and the bytes its unit holds after the header.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct Expected {
    pub(crate) terminal: (usize, u16),
    pub(crate) written: u32,
}

pub(crate) fn expected(length: u16) -> Expected {
    let terminal = if length <= FIRST {
        (0, FLOOR.saturating_add(word_rounded(length)))
    } else {
        let past = length.saturating_sub(FIRST);
        let more = units(past, BUFFER);
        let last = past.saturating_sub(BUFFER.saturating_mul(more.saturating_sub(1)));
        (usize::from(more), word_rounded(last))
    };
    Expected {
        terminal,
        written: u32::from(word_rounded(length)),
    }
}

/// The descriptors a boot left done, by index and `LENGTH`.
fn done(seats: &[Seat]) -> Vec<(usize, u16)> {
    seats
        .iter()
        .enumerate()
        .filter_map(|(index, seat)| match seat {
            Seat::Done { length } => Some((index, *length)),
            Seat::Armed | Seat::Other => None,
        })
        .collect()
}

/// Whether a boot is the fixed ring's: the frame's one terminal descriptor
/// where its length puts it, no anomaly, and the window's one unit of the
/// sender's pattern of its length.
fn fixed(seats: &[Seat], counts: &service::Counts, length: u16) -> Result<(), String> {
    let expected = expected(length);
    let done = done(seats);
    if done != [expected.terminal] {
        return Err(format!(
            "{length}: done {done:?}, not {:?}",
            expected.terminal
        ));
    }
    if counts.length_anomalies != 0 {
        return Err(format!(
            "{length}: {} length anomalies",
            counts.length_anomalies
        ));
    }
    if (counts.patterns, counts.pattern_written) != (1, expected.written) {
        return Err(format!(
            "{length}: {} of the sender's units, the last of {} bytes, not one of {}",
            counts.patterns, counts.pattern_written, expected.written
        ));
    }
    Ok(())
}

/// How one boot ended.
enum Boot {
    /// Exactly one frame of the length, the sender's end known: its record
    /// and whether it is the fixed ring's.
    Complete(serde_json::Value, Result<(), String>),
    /// Something else reached the ring, or the sender's end is unknown.
    Incomplete(String),
}

/// The ring's state in a counters answer.
pub(crate) fn ring(counted: &Counters) -> Result<&service::RingState, Failure> {
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
pub(crate) fn fresh_receiver(cx: &mut Cx<'_>, sender: [u8; 6]) -> Result<[u8; 6], Failure> {
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
pub(crate) fn fresh_sender(cx: &mut Cx<'_>, receiver: Option<[u8; 6]>) -> Result<[u8; 6], Failure> {
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

pub(crate) fn target(cx: &mut Cx<'_>, receiver: [u8; 6]) -> Result<(), Failure> {
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
    let verdict = fixed(&after_ring.seats, counts, length);
    Ok(Boot::Complete(
        serde_json::json!({
            "length": length,
            "status": status,
            "standing": after_ring.standing,
            "controls": after_ring.controls,
            "seats": after_ring.seats,
            "window": counts,
            "expected": expected(length),
            "fixed": verdict.as_ref().err(),
        }),
        verdict,
    ))
}

pub(crate) fn stop_receiver(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let _stopped = cx.device(RECEIVER)?.call(Stop, STEP)?;
    Ok(())
}

/// The case: every length, `BOOTS` complete boots each, a boot a foreign
/// frame or an unknown end took repeated up to `ATTEMPTS`; it fails when a
/// complete boot is not the fixed ring's.
pub(crate) fn first_frame(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let sender = fresh_sender(cx, None)?;
    let mut receiver = None;
    let mut short = Vec::new();
    let mut refuted = Vec::new();
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
                Boot::Complete(record, verdict) => {
                    complete = complete.saturating_add(1);
                    table.push(record);
                    if let Err(reason) = verdict {
                        refuted.push(reason);
                    }
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
    if !refuted.is_empty() {
        return Err(Failure::Fail(refuted.join("; ")));
    }
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
    fn a_frame_from_d0_ends_where_its_length_puts_it() {
        assert_eq!(expected(100).terminal, (0, 164));
        assert_eq!(expected(192).terminal, (0, 256));
        assert_eq!(expected(193).terminal, (1, 4));
        assert_eq!(expected(252).terminal, (1, 60));
        assert_eq!(expected(253).terminal, (1, 64));
        assert_eq!(expected(448).terminal, (1, 256));
        assert_eq!(expected(449).terminal, (2, 4));
        assert_eq!(expected(700).terminal, (2, 252));
        assert_eq!(expected(253).written, 256);
        assert_eq!(expected(700).written, 700);
    }

    #[test]
    fn one_frame_is_what_its_length_takes_from_d0_and_nothing_beyond() {
        let mut seats = vec![Seat::Armed; 16];
        assert!(quiet(&seats));
        assert!(!one_frame(&seats, 300), "nothing done is no frame");
        seats[1] = Seat::Done { length: 108 };
        assert!(!quiet(&seats));
        assert!(one_frame(&seats, 300));
        assert!(!one_frame(&seats, 100), "a frame in d0 alone reaches no d1");
        seats[3] = Seat::Done { length: 60 };
        assert!(!one_frame(&seats, 300), "a second frame");
    }

    #[test]
    fn a_boot_is_the_fixed_ring_s_only_with_the_one_unit_of_its_length() {
        let mut seats = vec![Seat::Armed; 16];
        seats[1] = Seat::Done { length: 60 };
        let counts = service::Counts {
            chains: 1,
            patterns: 1,
            pattern_written: 252,
            ..service::Counts::default()
        };
        assert_eq!(fixed(&seats, &counts, 252), Ok(()));
        let standing = service::Counts {
            length_anomalies: 4_740,
            ..counts
        };
        assert!(fixed(&seats, &standing, 252).is_err(), "tasks/358's defect");
        let foreign = service::Counts {
            patterns: 0,
            ..counts
        };
        assert!(
            fixed(&seats, &foreign, 252).is_err(),
            "not the sender's frame"
        );
        let mut in_d0 = vec![Seat::Armed; 16];
        in_d0[0] = Seat::Done { length: 60 };
        assert!(fixed(&in_d0, &counts, 252).is_err(), "the old ring's d0");
    }
}
