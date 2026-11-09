//! The decision rule: who wrote octets 24..32 of each transmitted frame.
//!
//! Per kind (beacon, probe response, action) the rule returns one outcome:
//!
//! - H1 `CallerValue`: the frame went on the air with the caller's K.
//! - H2 `ApTsfAtTransmission`: the AP TSF at the frame's transmission.
//! - H3 `LocalTimeAtTransmission`: the MAC local time at transmission, a
//!   32-bit counter, in the low word and zero in the high word (the vendor
//!   writes `hal_now()` low and zero high): all eight octets predicted.
//! - H4 `CallerPlusElapsed`: K plus the time from publication to
//!   transmission.
//! - `Other`: a partial overwrite (some octets keep K in every frame and
//!   no clock explains the value), a counter sampled at the hardware
//!   handoff rather than at transmission, frames of one kind that follow
//!   different rules, or a change no clock explains.
//! - `Insufficient`: the run or the capture cannot separate the hypotheses.
//!
//! Every hypothesis explains all eight octets; a value whose low word fits
//! a clock but whose high word the hypothesis does not predict is other
//! behaviour.
//!
//! Run conditions. A run whose shared cleanup was not confirmed (an error,
//! a MAC still active, an unconfirmed walker stop, a failed final check)
//! or whose MAC reported RX buffer overflows decides nothing: its outcome
//! is kept as withheld evidence and the verdict is insufficient. An RX
//! overflow is not shown to leave transmission alone, so it never becomes
//! grounds for a Timestamp capability.
//!
//! Conditions of every outcome, which limit what it says of the vendor
//! path: AIFSN 2 and CW 0 on both queues (the vendor's VO uses CW 3..7);
//! probe responses unsolicited, to an absent receiver, without the
//! vendor's TX-control bit 27, which has no equivalent here; the blocker on
//! VI ahead of the VO subject, not in the subject's own queue.
//!
//! Clocks. The device gives, per frame, the pair (MAC local time, AP TSF)
//! read before and after its publication and when its completion was seen;
//! the capture gives the sniffer's TSFT S of each frame. No offset between
//! the device's clocks and the sniffer's is assumed: only differences
//! between frames are compared, and the device/sniffer rate is measured.
//!
//! Delay (owner's condition 1). A delayed round publishes the blocker,
//! then the subject `BLOCKER_LEAD` later. A delay counts only when the
//! capture shows that round's blocker on the air before the subject: one
//! transmitter sends one PPDU at a time, so the subject started after the
//! blocker ended, and its delay from publication is at least
//! `D_lower = A_blocker - (pub_subject - pub_blocker)` (device local time),
//! the blocker's airtime less the publication spacing. An undelayed round
//! counts only when no blocker of that round is on the air.
//!
//! Dependence on the delay (condition 2). For consecutive rounds n, n+1 of
//! one kind with different arms, `dS = S(n+1) - S(n)` and
//! `dpub = pub(n+1) - pub(n)`; with the device/sniffer rate `a` fitted
//! over every undelayed frame, the change of delay is
//! `dD = dS / a - dpub`. For a clock X, a stamp at transmission predicts
//! `dV = r_X * dS / a`; a stamp at publication predicts `dV = r_X * dpub`.
//! The two predictions differ by `r_X * dD`. A pair is decisive when
//! `|dD| >= D_MIN_US` and each tolerance below is at most `|dD|/4`. A
//! hypothesis "at transmission" holds only if every decisive pair's
//! residual against the transmission prediction is within `T_tx`, and
//! every confirmed-delayed frame's value lies at least `D_lower` past its
//! publication sample (less the read error); "at handoff" only if every
//! residual against the publication prediction is within `T_pub`. A value
//! merely inside the wide bracket [publication, completion] decides
//! nothing.
//!
//! Error budget, per decisive pair:
//! `T_tx = 2 Q_SNIFFER + 2 Q_DEVICE + max(J_SNIFFER_MIN, J_u) + rho_u * |dS|`,
//! `T_pub = 2 Q_DEVICE + R(n) + R(n+1) + L_HANDOFF`, where `J_u` is the
//! largest residual of the undelayed frames' rate fit (it bounds the
//! sniffer's timestamp jitter together with the undelayed frames' own
//! delay jitter), `rho_u` the fit's rate uncertainty (three standard
//! errors, at least 1 ppm) and `R` a frame's read error: its publication
//! bracket, from the local time before the first sample to after the
//! second.

use crate::capture::{AirFrame, Capture};
use crate::device::{Kind, Record, Run};
use std::collections::HashMap;

/// The sniffer's TSFT resolution, microseconds.
pub const Q_SNIFFER_US: f64 = 1.0;
/// The resolution of the AP TSF and the MAC local time, microseconds.
pub const Q_DEVICE_US: f64 = 1.0;
/// The least allowance for the sniffer's receive-timestamp jitter between
/// two frames of one kind, length and rate, microseconds.
pub const J_SNIFFER_MIN_US: f64 = 4.0;
/// The largest device/sniffer rate difference accepted: twice the
/// +-25 ppm of IEEE Std 802.11-2020 15.4.4.5 / 17.3.9.7 on each side, with
/// margin.
pub const RHO_MAX_PPM: f64 = 100.0;
/// The largest residual of the undelayed frames' rate fit accepted: a
/// quality bound on the TSFT's scatter between frames. It does not show
/// what clock the TSFT counts or when it latched: a constant delay between
/// the air and the stamp is absorbed by the fit's offset and passes it.
pub const J_SNIFFER_MAX_US: f64 = 50.0;
/// The largest AP TSF/MAC local rate difference accepted.
pub const DEVICE_RATE_MAX_PPM: f64 = 100.0;
/// Between the publication samples and a value the hardware takes at its
/// handoff (descriptor fetch), microseconds.
pub const L_HANDOFF_US: f64 = 50.0;
/// A decisive pair's least change of delay, microseconds.
pub const D_MIN_US: f64 = 5_000.0;
/// A decisive pair's tolerances are each at most `|dD|` over this.
pub const SEPARATION: f64 = 4.0;
/// A kind needs this many matched frames and decisive pairs.
pub const FRAMES_MIN: usize = 8;
pub const PAIRS_MIN: usize = 4;
/// The MAC local time leads the AP TSF by at least this at the start, so
/// that no frame's bracket of one clock reaches the other's.
pub const CLOCK_SEPARATION_MIN_US: i64 = 1_000_000;
/// DSSS long preamble and PLCP header, microseconds (15.3.3).
const DSSS_PREAMBLE_US: f64 = 192.0;
const MAC_HEADER: usize = 24;
const FCS: usize = 4;

/// The clock a value follows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Family {
    ApTsf,
    LocalTime,
    CallerPlusElapsed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Other {
    /// Octet p (bit p) kept K in every frame; the others changed.
    PartialOverwrite { unchanged: u8 },
    /// The values follow the clock, sampled at the hardware handoff: the
    /// queue's wait is not in them.
    StampedAtHandoff(Family),
    /// The frames of this kind follow different rules.
    Inconsistent,
    /// The octets changed, and no clock and instant explains them.
    Unexplained,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shortfall {
    /// The probe's run did not end cleanly: its error and its cleanup.
    DeviceRun {
        error: Option<String>,
        mac: String,
        dma: String,
        finish: String,
    },
    /// The MAC reported RX buffer overflows during the run.
    RxOverflow {
        count: u32,
    },
    NoStartClock,
    NoEndClock,
    ClockSeparation {
        micros: i64,
    },
    DeviceClockRate {
        ppm: f64,
    },
    /// Too few undelayed frames to measure the sniffer's rate.
    RateFit {
        points: usize,
    },
    SnifferRate {
        ppm: f64,
    },
    /// The undelayed frames scatter beyond `J_SNIFFER_MAX_US` around the
    /// rate fit: the capture's timing is too coarse for the budget.
    SnifferJitter {
        micros: f64,
    },
    TagMismatch {
        n: u16,
    },
    Frames {
        matched: usize,
    },
    Pairs {
        decisive: usize,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    CallerValue,
    ApTsfAtTransmission,
    LocalTimeAtTransmission,
    CallerPlusElapsed,
    Other(Other),
    Insufficient(Shortfall),
}

impl Outcome {
    pub fn name(&self) -> String {
        match self {
            Self::CallerValue => "H1 caller-value".into(),
            Self::ApTsfAtTransmission => "H2 ap-tsf-at-transmission".into(),
            Self::LocalTimeAtTransmission => "H3 local-time-at-transmission".into(),
            Self::CallerPlusElapsed => "H4 caller-plus-elapsed".into(),
            Self::Other(other) => format!("other {other:?}"),
            Self::Insufficient(shortfall) => format!("insufficient-evidence {shortfall:?}"),
        }
    }

    pub const fn decided(&self) -> bool {
        !matches!(self, Self::Insufficient(_))
    }
}

/// What the outcome rests on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Evidence {
    pub matched: usize,
    /// Rounds whose subject the capture holds more than once.
    pub duplicates: usize,
    /// Completed rounds the capture does not hold.
    pub missing: usize,
    pub confirmed_delayed: usize,
    pub decisive: usize,
    pub min_delay_change_us: f64,
    pub unchanged: u8,
    pub worst_transmission_residual_us: f64,
    pub worst_handoff_residual_us: f64,
    pub largest_tolerance_us: f64,
    /// Read back after completion: octets equal to K, equal to the air's,
    /// neither.
    pub readback_kept: usize,
    pub readback_air: usize,
    pub readback_other: usize,
    /// The outcome a run condition withheld.
    pub withheld: Option<Outcome>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    pub kind: Kind,
    pub outcome: Outcome,
    pub evidence: Evidence,
}

/// The run-wide measurements every kind's verdict uses.
#[derive(Clone, Debug, PartialEq)]
pub struct Clocks {
    /// AP TSF microseconds per MAC local microsecond.
    pub tsf_rate: f64,
    /// Sniffer microseconds per MAC local microsecond.
    pub sniffer_rate: f64,
    pub sniffer_rate_uncertainty: f64,
    /// The undelayed frames' largest fit residual, microseconds.
    pub jitter_us: f64,
}

/// One completed round with its frame on the air.
#[derive(Clone, Copy)]
struct Joined<'a> {
    record: &'a Record,
    air: &'a AirFrame,
    value: u64,
    /// A confirmed delay's lower bound, microseconds.
    delay_lower: Option<f64>,
    undelayed: bool,
}

fn local_delta(from: u32, to: u32) -> f64 {
    f64::from(to.wrapping_sub(from) as i32)
}

fn tsft_delta(from: u64, to: u64) -> f64 {
    (i128::from(to) - i128::from(from)) as f64
}

/// A frame's read error: its publication bracket, microseconds.
fn read_error(record: &Record) -> f64 {
    local_delta(record.published.local_before, record.handed.local_after)
}

fn blocker_airtime_us(body: usize) -> f64 {
    DSSS_PREAMBLE_US + 8.0 * (MAC_HEADER + body + FCS) as f64
}

type Index<'a> = HashMap<(u8, u16), Vec<&'a AirFrame>>;

/// The width of a sniffer's TSFT when every value of the run's frames
/// fits 32 bits: mt76 (mt7915, the stand router's MT7981) reports the RX
/// descriptor's 32-bit timestamp, which wraps every 2^32 us.
const WRAP_32: u64 = 1 << 32;

/// The run's own frames in capture (arrival) order, their TSFT made
/// continuous across a 32-bit wrap when every value fits 32 bits.
fn ours(run: &Run, capture: &Capture) -> Vec<AirFrame> {
    let mut frames: Vec<AirFrame> = capture
        .frames
        .iter()
        .filter(|frame| {
            frame.transmitter == Some(run.ap) && frame.tag.is_some_and(|tag| tag.run == run.run)
        })
        .cloned()
        .collect();
    if frames.iter().all(|frame| frame.tsft < WRAP_32) {
        let mut epoch = 0u64;
        let mut previous: Option<u64> = None;
        for frame in &mut frames {
            if let Some(previous) = previous
                && frame.tsft < previous
                && previous - frame.tsft > WRAP_32 / 2
            {
                epoch = epoch.saturating_add(WRAP_32);
            }
            previous = Some(frame.tsft);
            frame.tsft = frame.tsft.saturating_add(epoch);
        }
    }
    frames
}

fn index<'a>(frames: &'a [AirFrame]) -> (Index<'a>, Vec<&'a AirFrame>) {
    let mut index: Index<'a> = HashMap::new();
    let mut ours = Vec::new();
    for frame in frames {
        let Some(tag) = frame.tag else {
            continue;
        };
        index.entry((tag.kind, tag.n)).or_default().push(frame);
        ours.push(frame);
    }
    ours.sort_by_key(|frame| frame.tsft);
    (index, ours)
}

struct Joining<'a> {
    joined: Vec<Joined<'a>>,
    duplicates: usize,
    missing: usize,
    mismatch: Option<u16>,
}

fn join<'a>(
    run: &Run,
    kind: Kind,
    records: impl Iterator<Item = &'a Record>,
    index: &Index<'a>,
    ours: &[&'a AirFrame],
) -> Joining<'a> {
    let airtime = blocker_airtime_us(run.blocker_body);
    let mut joining = Joining {
        joined: Vec::new(),
        duplicates: 0,
        missing: 0,
        mismatch: None,
    };
    for record in records.filter(|record| record.kind == kind && record.done.is_some()) {
        let frames = index.get(&(kind.code(), record.n));
        let air = match frames.map(Vec::as_slice) {
            Some([air]) if air.frame_control == kind.frame_control() => *air,
            Some([_, _, ..]) => {
                joining.duplicates += 1;
                continue;
            }
            _ => {
                joining.missing += 1;
                continue;
            }
        };
        let (Some(stamp), Some(tag)) = (air.stamp, air.tag) else {
            joining.missing += 1;
            continue;
        };
        if tag.k != record.k {
            joining.mismatch.get_or_insert(record.n);
            continue;
        }
        let blockers = index.get(&(b'x', record.n)).map(Vec::as_slice);
        let mut delay_lower = None;
        let mut undelayed = false;
        match (record.blocker, blockers) {
            (Some(blocker), Some([on_air])) => {
                let between = ours
                    .iter()
                    .any(|frame| frame.tsft > on_air.tsft && frame.tsft < air.tsft);
                if on_air.tsft < air.tsft && !between {
                    let spacing =
                        local_delta(blocker.published.local_after, record.published.local_before);
                    let lower = airtime - spacing;
                    if lower >= D_MIN_US {
                        delay_lower = Some(lower);
                    }
                }
            }
            (None, None) => undelayed = true,
            _ => {}
        }
        joining.joined.push(Joined {
            record,
            air,
            value: u64::from_le_bytes(stamp),
            delay_lower,
            undelayed,
        });
    }
    joining.joined.sort_by_key(|joined| joined.record.n);
    joining
}

/// Least squares of the sniffer's TSFT over the MAC local time at
/// publication, over the undelayed frames of every kind.
fn fit(points: &[(f64, f64)]) -> Option<(f64, f64, f64)> {
    let count = points.len();
    if count < FRAMES_MIN {
        return None;
    }
    let n = count as f64;
    let mean_x = points.iter().map(|p| p.0).sum::<f64>() / n;
    let mean_y = points.iter().map(|p| p.1).sum::<f64>() / n;
    let sxx: f64 = points.iter().map(|p| (p.0 - mean_x).powi(2)).sum();
    let sxy: f64 = points.iter().map(|p| (p.0 - mean_x) * (p.1 - mean_y)).sum();
    if sxx <= 0.0 {
        return None;
    }
    let slope = sxy / sxx;
    let intercept = mean_y - slope * mean_x;
    let residuals: Vec<f64> = points
        .iter()
        .map(|p| p.1 - (slope * p.0 + intercept))
        .collect();
    let sse: f64 = residuals.iter().map(|r| r * r).sum();
    let standard_error = (sse / (n - 2.0) / sxx).sqrt();
    let jitter = residuals.iter().fold(0.0f64, |m, r| m.max(r.abs()));
    Some((slope, standard_error, jitter))
}

fn clocks(run: &Run, all: &[Joined<'_>]) -> Result<Clocks, Shortfall> {
    let start = run.start.ok_or(Shortfall::NoStartClock)?;
    let end = run.end.ok_or(Shortfall::NoEndClock)?;
    let separation = i64::from(start.local_before) - i64::try_from(start.tsf).unwrap_or(i64::MAX);
    if separation < CLOCK_SEPARATION_MIN_US {
        return Err(Shortfall::ClockSeparation { micros: separation });
    }
    let local_span = local_delta(start.local_before, end.local_before);
    let tsf_rate = tsft_delta(start.tsf, end.tsf) / local_span;
    let ppm = (tsf_rate - 1.0) * 1e6;
    if !ppm.is_finite() || ppm.abs() > DEVICE_RATE_MAX_PPM {
        return Err(Shortfall::DeviceClockRate { ppm });
    }
    let Some(first) = all.iter().find(|joined| joined.undelayed) else {
        return Err(Shortfall::RateFit { points: 0 });
    };
    let points: Vec<(f64, f64)> = all
        .iter()
        .filter(|joined| joined.undelayed)
        .map(|joined| {
            (
                local_delta(
                    first.record.published.local_before,
                    joined.record.published.local_before,
                ),
                tsft_delta(first.air.tsft, joined.air.tsft),
            )
        })
        .collect();
    let (slope, error, jitter) = fit(&points).ok_or(Shortfall::RateFit {
        points: points.len(),
    })?;
    let ppm = (slope - 1.0) * 1e6;
    if ppm.abs() > RHO_MAX_PPM {
        return Err(Shortfall::SnifferRate { ppm });
    }
    if jitter > J_SNIFFER_MAX_US {
        return Err(Shortfall::SnifferJitter { micros: jitter });
    }
    Ok(Clocks {
        tsf_rate,
        sniffer_rate: slope,
        sniffer_rate_uncertainty: (3.0 * error).max(1e-6),
        jitter_us: jitter,
    })
}

/// Whether `value` lies in [start - q, start + width + q] on a wrapping
/// 32-bit clock.
fn in_window_32(value: u32, start: u32, width: f64) -> bool {
    let offset = local_delta(start, value);
    offset >= -Q_DEVICE_US && offset <= width + Q_DEVICE_US
}

/// The frame's value as clock `family` would read it, relative to its
/// publication sample, if it lies within the frame's device bracket.
fn offset(joined: &Joined<'_>, family: Family) -> Option<f64> {
    let record = joined.record;
    let done = record.done?.at;
    let width = local_delta(record.published.local_before, done.local_after);
    match family {
        Family::ApTsf => {
            let low = i128::from(record.published.tsf) - 1;
            let high = i128::from(done.tsf) + 1;
            let value = i128::from(joined.value);
            (value >= low && value <= high)
                .then(|| (value - i128::from(record.published.tsf)) as f64)
        }
        Family::LocalTime => {
            // The counter is 32 bits wide: the high word is zero.
            if joined.value >> 32 != 0 {
                return None;
            }
            let low = joined.value as u32;
            in_window_32(low, record.published.local_before, width)
                .then(|| local_delta(record.published.local_before, low))
        }
        Family::CallerPlusElapsed => {
            let elapsed = joined.value.wrapping_sub(record.k) as i64 as f64;
            (elapsed >= -Q_DEVICE_US && elapsed <= width + Q_DEVICE_US).then_some(elapsed)
        }
    }
}

/// The value's change between two frames as clock `family` counts it.
fn value_delta(from: &Joined<'_>, to: &Joined<'_>, family: Family) -> f64 {
    match family {
        Family::ApTsf => tsft_delta(from.value, to.value),
        Family::LocalTime => local_delta(from.value as u32, to.value as u32),
        Family::CallerPlusElapsed => {
            to.value.wrapping_sub(to.record.k) as i64 as f64
                - from.value.wrapping_sub(from.record.k) as i64 as f64
        }
    }
}

struct Pair<'a, 'b> {
    from: &'b Joined<'a>,
    to: &'b Joined<'a>,
    /// Sniffer interval in device microseconds.
    air: f64,
    published: f64,
    delay_change: f64,
    tolerance_transmission: f64,
    tolerance_handoff: f64,
}

fn pairs<'a, 'b>(joined: &'b [Joined<'a>], clocks: &Clocks) -> Vec<Pair<'a, 'b>> {
    let mut pairs = Vec::new();
    for window in joined.windows(2) {
        let [from, to] = window else { continue };
        if to.record.n != from.record.n.wrapping_add(1) || to.record.delayed == from.record.delayed
        {
            continue;
        }
        let confirmed = |joined: &Joined<'_>| {
            if joined.record.delayed {
                joined.delay_lower.is_some()
            } else {
                joined.undelayed
            }
        };
        if !confirmed(from) || !confirmed(to) {
            continue;
        }
        let sniffer = tsft_delta(from.air.tsft, to.air.tsft);
        let air = sniffer / clocks.sniffer_rate;
        let published = local_delta(
            from.record.published.local_before,
            to.record.published.local_before,
        );
        let delay_change = air - published;
        let tolerance_transmission = 2.0 * Q_SNIFFER_US
            + 2.0 * Q_DEVICE_US
            + J_SNIFFER_MIN_US.max(clocks.jitter_us)
            + clocks.sniffer_rate_uncertainty * sniffer.abs();
        let tolerance_handoff =
            2.0 * Q_DEVICE_US + read_error(from.record) + read_error(to.record) + L_HANDOFF_US;
        let bound = delay_change.abs() / SEPARATION;
        if delay_change.abs() >= D_MIN_US
            && tolerance_transmission <= bound
            && tolerance_handoff <= bound
        {
            pairs.push(Pair {
                from,
                to,
                air,
                published,
                delay_change,
                tolerance_transmission,
                tolerance_handoff,
            });
        }
    }
    pairs
}

fn unchanged(joined: &[Joined<'_>]) -> u8 {
    let mut mask = 0xffu8;
    for frame in joined {
        let air = frame.value.to_le_bytes();
        let k = frame.record.k.to_le_bytes();
        for (octet, (a, b)) in air.iter().zip(k.iter()).enumerate() {
            if a != b {
                mask &= !(1u8 << octet);
            }
        }
    }
    mask
}

/// The one family every frame's value fits, or the reason none does.
fn family(joined: &[Joined<'_>]) -> Result<Family, Other> {
    let families = [Family::ApTsf, Family::LocalTime, Family::CallerPlusElapsed];
    let fits = |family: Family| joined.iter().all(|frame| offset(frame, family).is_some());
    let matching: Vec<Family> = families.into_iter().filter(|f| fits(*f)).collect();
    match matching.as_slice() {
        [family] => Ok(*family),
        [] => {
            let some = families
                .iter()
                .any(|f| joined.iter().any(|frame| offset(frame, *f).is_some()))
                || joined.iter().any(|frame| frame.value == frame.record.k);
            Err(if some {
                Other::Inconsistent
            } else {
                Other::Unexplained
            })
        }
        _ => Err(Other::Unexplained),
    }
}

/// Whether the frames of `family` were stamped at transmission, at the
/// handoff, or neither, over the decisive pairs.
fn instant(
    joined: &[Joined<'_>],
    pairs: &[Pair<'_, '_>],
    family: Family,
    clocks: &Clocks,
    evidence: &mut Evidence,
) -> Outcome {
    let rate = match family {
        Family::ApTsf => clocks.tsf_rate,
        Family::LocalTime | Family::CallerPlusElapsed => 1.0,
    };
    let mut transmission = true;
    let mut handoff = true;
    for pair in pairs {
        let change = value_delta(pair.from, pair.to, family);
        let (at_transmission, at_handoff) = match family {
            Family::CallerPlusElapsed => (pair.delay_change, 0.0),
            Family::ApTsf | Family::LocalTime => (pair.air * rate, pair.published * rate),
        };
        let residual_transmission = (change - at_transmission).abs();
        let residual_handoff = (change - at_handoff).abs();
        evidence.worst_transmission_residual_us = evidence
            .worst_transmission_residual_us
            .max(residual_transmission);
        evidence.worst_handoff_residual_us =
            evidence.worst_handoff_residual_us.max(residual_handoff);
        transmission &= residual_transmission <= pair.tolerance_transmission;
        handoff &= residual_handoff <= pair.tolerance_handoff;
    }
    // In the device's own clocks too: a delayed frame's value lies at
    // least its confirmed delay past its publication.
    for frame in joined {
        if let (Some(lower), Some(offset)) = (frame.delay_lower, offset(frame, family)) {
            let slack = read_error(frame.record) + 2.0 * Q_DEVICE_US;
            transmission &= offset >= lower * rate - slack;
        }
    }
    match (transmission, handoff) {
        (true, false) => match family {
            Family::ApTsf => Outcome::ApTsfAtTransmission,
            Family::LocalTime => Outcome::LocalTimeAtTransmission,
            Family::CallerPlusElapsed => Outcome::CallerPlusElapsed,
        },
        (false, true) => Outcome::Other(Other::StampedAtHandoff(family)),
        _ => Outcome::Other(Other::Unexplained),
    }
}

/// The run's own condition that withholds every kind's outcome.
fn run_condition(run: &Run) -> Option<Shortfall> {
    if run.error.is_some() || !run.stops.confirmed() {
        return Some(Shortfall::DeviceRun {
            error: run.error.clone(),
            mac: run.stops.mac.clone(),
            dma: run.stops.dma.clone(),
            finish: run.stops.finish.clone(),
        });
    }
    (run.rx_full != 0).then_some(Shortfall::RxOverflow { count: run.rx_full })
}

fn verdict(
    run: &Run,
    kind: Kind,
    joining: Joining<'_>,
    clocks: &Result<Clocks, Shortfall>,
) -> Verdict {
    let joined = &joining.joined;
    let mut evidence = Evidence {
        matched: joined.len(),
        duplicates: joining.duplicates,
        missing: joining.missing,
        confirmed_delayed: joined.iter().filter(|j| j.delay_lower.is_some()).count(),
        unchanged: unchanged(joined),
        ..Evidence::default()
    };
    for frame in joined {
        match frame.record.readback {
            Some(octets) if octets == frame.record.k.to_le_bytes() => evidence.readback_kept += 1,
            Some(octets) if octets == frame.value.to_le_bytes() => evidence.readback_air += 1,
            _ => evidence.readback_other += 1,
        }
    }
    let outcome = (|| {
        if let Some(n) = joining.mismatch {
            return Outcome::Insufficient(Shortfall::TagMismatch { n });
        }
        let clocks = match clocks {
            Ok(clocks) => clocks,
            Err(shortfall) => return Outcome::Insufficient(shortfall.clone()),
        };
        if joined.len() < FRAMES_MIN {
            return Outcome::Insufficient(Shortfall::Frames {
                matched: joined.len(),
            });
        }
        let pairs = pairs(joined, clocks);
        evidence.decisive = pairs.len();
        evidence.min_delay_change_us = pairs
            .iter()
            .map(|pair| pair.delay_change.abs())
            .fold(f64::INFINITY, f64::min);
        evidence.largest_tolerance_us = pairs
            .iter()
            .map(|pair| pair.tolerance_transmission.max(pair.tolerance_handoff))
            .fold(0.0, f64::max);
        if pairs.len() < PAIRS_MIN {
            return Outcome::Insufficient(Shortfall::Pairs {
                decisive: pairs.len(),
            });
        }
        if evidence.unchanged == 0xff {
            return Outcome::CallerValue;
        }
        // A family first: K plus a short elapsed time keeps K's high
        // octets without being a partial overwrite.
        match family(joined) {
            Ok(family) => instant(joined, &pairs, family, clocks, &mut evidence),
            Err(_) if evidence.unchanged != 0 => Outcome::Other(Other::PartialOverwrite {
                unchanged: evidence.unchanged,
            }),
            Err(other) => Outcome::Other(other),
        }
    })();
    let outcome = match run_condition(run) {
        Some(condition) if outcome.decided() => {
            evidence.withheld = Some(outcome);
            Outcome::Insufficient(condition)
        }
        _ => outcome,
    };
    Verdict {
        kind,
        outcome,
        evidence,
    }
}

/// The run-wide clocks and one verdict per kind.
pub fn decide(run: &Run, capture: &Capture) -> (Result<Clocks, Shortfall>, Vec<Verdict>) {
    let frames = ours(run, capture);
    let (index, ours) = index(&frames);
    let joinings: Vec<(Kind, Joining<'_>)> = Kind::ALL
        .iter()
        .map(|kind| (*kind, join(run, *kind, run.records.iter(), &index, &ours)))
        .collect();
    let all: Vec<Joined<'_>> = joinings
        .iter()
        .flat_map(|(_, joining)| joining.joined.iter().copied())
        .collect();
    let clocks = clocks(run, &all);
    let verdicts = joinings
        .into_iter()
        .map(|(kind, joining)| verdict(run, kind, joining, &clocks))
        .collect();
    (clocks, verdicts)
}

#[cfg(test)]
mod tests;
