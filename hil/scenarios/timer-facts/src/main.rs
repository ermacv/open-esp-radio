//! SYSTIMER facts on the stand (`tasks/380`, `tasks/381`): a target loaded
//! into a comparator whose enable and interrupt enable were written once at
//! boot, over the probe `esp32s31-timer-facts`. One case a row of the
//! tasks' tables; every number the image answers is a measure of the
//! results file.
//!
//! a, b and f pass when every trial gave the expected outcome. c′, d′, e′,
//! g′ and h pass when answered, and their tables are the result, except
//! for two refuting outcomes, which fail their case: a future target left
//! without an alarm at its time (d′, e′, f), and in h no alarm while the
//! counter read after the strobe still had the target ahead.
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]

use std::process::ExitCode;
use std::time::Duration;

use oer_hil_services_timer_facts::{
    Apart, Attempt, COUNTER_BITS, Calibrate, Calibrated, Case as Load, Concurrent, Far, Looped,
    MOST_ATTEMPTS, MOST_TRIALS, Mixed, New, Order, Ran, Reached, Request, Run, SWEEP_NEW, Short,
    Shot, Spread, Standing, Sweep, Swept, TOLERANCE, Tally, Timed, Words, WordsRead,
};
use oer_protocols_hil_agent::base::Service as _;
use oer_tools_hil_harness::{Case, Cx, Failure, Scenario, check};

const SCENARIO: Scenario = Scenario::new("timer-facts", "esp32s31-timer-facts", &[Request::KEYS]);

const CASES: [Case; 9] = [
    Case {
        name: "a-loaded-over-a-standing-target",
        run: loaded_over_a_standing_target,
    },
    Case {
        name: "b-loaded-after-an-alarm",
        run: loaded_after_an_alarm,
    },
    Case {
        name: "c2-behind-the-counter",
        run: behind_the_counter,
    },
    Case {
        name: "d2-strobe-across-the-standing-expiry",
        run: strobe_across_the_standing_expiry,
    },
    Case {
        name: "e2-the-high-word-changed-by-the-load",
        run: high_word_changed_by_the_load,
    },
    Case {
        name: "f-hart-1-beside",
        run: hart_1_beside,
    },
    Case {
        name: "g2-far-targets-around-the-half",
        run: far_targets,
    },
    Case {
        name: "h-the-short-timer",
        run: short_timer,
    },
    Case {
        name: "enables-at-the-end",
        run: enables_at_the_end,
    },
];

/// Trials of each cell of a, b, c′, e′ and f.
const TRIALS: u32 = 1_000;
/// The window of d′ and h on each side, in quarter ticks: 10 ticks.
const QUARTERS: i16 = 40;
/// The trials of d′ and h at each offset.
const ATTEMPTS: u8 = 8;
/// One command's answer: a run of the most trials takes about 0.1 s.
const ANSWER: Duration = Duration::from_secs(5);

fn main() -> ExitCode {
    oer_tools_hil_harness::main(&SCENARIO, &CASES)
}

/// The image's rate: the cycles of `ticks`.
#[derive(Clone, Copy)]
struct Rate {
    ticks: u32,
    cycles: u32,
}

impl Rate {
    fn cycles_of(self, ticks: u64) -> i64 {
        u128::from(ticks)
            .saturating_mul(u128::from(self.cycles))
            .checked_div(u128::from(self.ticks))
            .and_then(|cycles| i64::try_from(cycles).ok())
            .unwrap_or(i64::MAX)
    }

    /// The cycles of `quarters` quarter ticks, signed.
    fn cycles_of_quarters(self, quarters: i16) -> i64 {
        let magnitude = i128::from(quarters.unsigned_abs())
            .saturating_mul(i128::from(self.cycles))
            .checked_div(i128::from(self.ticks).saturating_mul(4))
            .and_then(|cycles| i64::try_from(cycles).ok())
            .unwrap_or(i64::MAX);
        if quarters < 0 {
            magnitude.saturating_neg()
        } else {
            magnitude
        }
    }
}

fn rate(cx: &mut Cx<'_>) -> Result<Rate, Failure> {
    match cx.call(Calibrate, ANSWER)? {
        Calibrated::Rate { ticks, cycles } => {
            cx.measure(
                "rate",
                serde_json::json!({ "ticks": ticks, "cycles": cycles }),
            );
            Ok(Rate { ticks, cycles })
        }
        Calibrated::Missed => Err(Failure::Incomplete(
            "the image's rate measurement saw no alarm".into(),
        )),
    }
}

/// Two spreads as one.
fn spread(a: Spread, b: Spread) -> Spread {
    Spread {
        count: a.count.saturating_add(b.count),
        least: a.least.min(b.least),
        most: a.most.max(b.most),
        sum: a.sum.saturating_add(b.sum),
    }
}

/// Two tallies as one.
fn merged(a: Tally, b: Tally) -> Tally {
    let mut after_strobe = a.after_strobe;
    for (bin, more) in after_strobe.iter_mut().zip(b.after_strobe) {
        *bin = bin.saturating_add(more);
    }
    Tally {
        trials: a.trials.saturating_add(b.trials),
        lost: a.lost.saturating_add(b.lost),
        before_strobe: a.before_strobe.saturating_add(b.before_strobe),
        after_strobe,
        early: a.early.saturating_add(b.early),
        on_time: a.on_time.saturating_add(b.on_time),
        first: spread(a.first, b.first),
        at: spread(a.at, b.at),
    }
}

/// `trials` of `load`, in runs of the most an answer takes, as one tally,
/// recorded as `name`.
fn tally(cx: &mut Cx<'_>, name: &str, load: Load, trials: u32) -> Result<Tally, Failure> {
    let mut total = Tally::EMPTY;
    let mut left = trials;
    while left > 0 {
        let run = u16::try_from(left).unwrap_or(u16::MAX).min(MOST_TRIALS);
        match cx.call(
            Run {
                case: load,
                trials: run,
            },
            ANSWER,
        )? {
            Ran::Done(part) => total = merged(total, part),
            Ran::Refused(refusal) => {
                return Err(Failure::Incomplete(format!("{name}: refused {refusal:?}")));
            }
        }
        left = left.saturating_sub(u32::from(run));
    }
    cx.measure(name, serde_json::to_value(total).unwrap_or_default());
    Ok(total)
}

/// Every trial acted, saw no alarm before its strobe, and exactly one
/// after it, at its expected cycle.
fn exactly_one(name: &str, tally: Tally) -> Result<(), Failure> {
    check(
        tally.lost == 0
            && tally.before_strobe == 0
            && tally.after_strobe == [0, tally.trials, 0]
            && tally.early == 0
            && tally.on_time == tally.trials,
        format!("{name}: {tally:?}"),
    )
}

/// The trials of a future target left without an alarm at its time.
const fn unanswered(tally: Tally) -> u16 {
    tally
        .trials
        .saturating_sub(tally.lost)
        .saturating_sub(tally.on_time)
}

/// a, its two loads: the new target later and earlier than the standing one.
fn case_a(cx: &mut Cx<'_>, prefix: &str) -> Result<(), Failure> {
    for (name, load) in [("later", Load::Later), ("earlier", Load::Earlier)] {
        let name = format!("{prefix}{name}");
        let tally = tally(cx, &name, load, TRIALS)?;
        exactly_one(&name, tally)?;
    }
    Ok(())
}

fn loaded_over_a_standing_target(cx: &mut Cx<'_>) -> Result<(), Failure> {
    rate(cx)?;
    case_a(cx, "a/")
}

fn loaded_after_an_alarm(cx: &mut Cx<'_>) -> Result<(), Failure> {
    rate(cx)?;
    let tally = tally(cx, "b/after-alarm", Load::AfterAlarm, TRIALS)?;
    exactly_one("b/after-alarm", tally)
}

const fn over_name(over: Standing) -> &'static str {
    match over {
        Standing::Reached => "over-reached",
        Standing::Ahead => "over-ahead",
    }
}

/// c′: every distance over both standing targets, each its own measure.
fn behind_the_counter(cx: &mut Cx<'_>) -> Result<(), Failure> {
    rate(cx)?;
    for over in [Standing::Reached, Standing::Ahead] {
        for ticks in [9, 16, 1_000, 1_000_000] {
            let name = format!("c2/{}/behind-{ticks}", over_name(over));
            tally(cx, &name, Load::Behind { ticks, over }, TRIALS)?;
        }
    }
    Ok(())
}

/// Whether a d′ attempt's future target raised: an alarm within the
/// tolerance of its cycle, counted from the standing target's.
fn future_raised(attempt: &Attempt, new_at: i64, tolerance: i64) -> bool {
    match attempt {
        Attempt::Lost => true,
        Attempt::Made { first, second, .. } => {
            [*first, *second].into_iter().flatten().any(|cycle| {
                i64::from(cycle).saturating_sub(new_at).unsigned_abs() <= tolerance.unsigned_abs()
            })
        }
    }
}

/// d′'s sweep: every strobe offset, both new targets, each attempt as it
/// happened. Answers the count of future targets left without an alarm.
fn sweep(cx: &mut Cx<'_>, rate: Rate, prefix: &str) -> Result<u32, Failure> {
    let new_at = rate.cycles_of(SWEEP_NEW);
    let tolerance = rate.cycles_of(TOLERANCE);
    let mut unraised = 0_u32;
    for (label, new) in [("future", New::Future), ("behind", New::Behind)] {
        let mut rows = Vec::new();
        for quarters in QUARTERS.saturating_neg()..=QUARTERS {
            let offset = i32::try_from(rate.cycles_of_quarters(quarters)).unwrap_or(i32::MAX);
            let attempts = match cx.call(
                Sweep {
                    offset,
                    new,
                    trials: ATTEMPTS,
                },
                ANSWER,
            )? {
                Swept::Done(attempts) => attempts,
                Swept::Refused(refusal) => {
                    return Err(Failure::Incomplete(format!(
                        "{prefix}{label} at {offset}: refused {refusal:?}"
                    )));
                }
            };
            if let New::Future = new {
                let missing = attempts
                    .iter()
                    .filter(|attempt| !future_raised(attempt, new_at, tolerance))
                    .count();
                unraised = unraised.saturating_add(u32::try_from(missing).unwrap_or(u32::MAX));
            }
            rows.push(serde_json::json!({
                "quarters": quarters,
                "asked": offset,
                "attempts": attempts,
            }));
        }
        cx.measure(format!("{prefix}{label}"), serde_json::Value::Array(rows));
    }
    cx.measure(format!("{prefix}future-without-alarm"), unraised.into());
    Ok(unraised)
}

fn strobe_across_the_standing_expiry(cx: &mut Cx<'_>) -> Result<(), Failure> {
    const {
        if ATTEMPTS as usize > MOST_ATTEMPTS {
            panic!("a sweep's attempts fit one answer");
        }
    };
    let rate = rate(cx)?;
    let unraised = sweep(cx, rate, "d2/")?;
    check(
        unraised == 0,
        format!("d2: {unraised} future target(s) left without an alarm at their time"),
    )
}

/// e′: the four ways and the strobe alone. A trial without an alarm at
/// the new target refutes.
fn high_word_changed_by_the_load(cx: &mut Cx<'_>) -> Result<(), Failure> {
    rate(cx)?;
    let cells = [
        ("passed/high-first", Mixed::Passed, Order::HighFirst),
        ("passed/low-first", Mixed::Passed, Order::LowFirst),
        ("ahead/high-first", Mixed::Ahead, Order::HighFirst),
        ("ahead/low-first", Mixed::Ahead, Order::LowFirst),
        ("passed/early", Mixed::Passed, Order::Early),
    ];
    let mut unraised = 0_u32;
    for (label, mixed, order) in cells {
        let name = format!("e2/{label}");
        let tally = tally(cx, &name, Load::Apart(Apart { mixed, order }), TRIALS)?;
        unraised = unraised.saturating_add(u32::from(unanswered(tally)));
    }
    cx.measure("e2/future-without-alarm", unraised.into());
    check(
        unraised == 0,
        format!("e2: {unraised} trial(s) left without an alarm at the new target"),
    )
}

fn hart_1_beside(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let rate = rate(cx)?;
    match cx.call(Concurrent::Start, ANSWER)? {
        Looped::Started => {}
        other => return Err(Failure::Incomplete(format!("f: start answered {other:?}"))),
    }
    let a = case_a(cx, "f/a/");
    let d = sweep(cx, rate, "f/d2/");
    let stopped = cx.call(Concurrent::Stop, ANSWER)?;
    cx.measure(
        "f/hart-1",
        serde_json::to_value(stopped).unwrap_or_default(),
    );
    a?;
    let unraised = d?;
    match stopped {
        Looped::Stopped {
            loads,
            alarms,
            missed,
        } => check(
            loads > 0 && alarms == loads && missed == 0 && unraised == 0,
            format!(
                "f: hart 1 loaded {loads}, saw {alarms} alarms, missed {missed}; \
                 d2 beside it left {unraised} future target(s) without an alarm"
            ),
        ),
        other => Err(Failure::Incomplete(format!("f: stop answered {other:?}"))),
    }
}

fn far_targets(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let half = 1_u64 << COUNTER_BITS.saturating_sub(1);
    let aheads = [
        ("2^51-16", half.saturating_sub(16)),
        ("2^51", half),
        ("2^51+16", half.saturating_add(16)),
        ("2^51+2^40", half.saturating_add(1 << 40)),
    ];
    let mut rows = Vec::new();
    for (label, ahead) in aheads {
        let reached = cx.call(Far { ahead }, ANSWER)?;
        if let Reached::Refused(refusal) = reached {
            return Err(Failure::Incomplete(format!(
                "g2 {label}: refused {refusal:?}"
            )));
        }
        rows.push(serde_json::json!({ "ahead": label, "reached": reached }));
    }
    cx.measure("g2", serde_json::Value::Array(rows));
    Ok(())
}

/// Whether an h trial refutes the driver: no alarm, and the counter read
/// after the strobe still had the target ahead.
const fn refutes(timed: &Timed) -> bool {
    match timed {
        Timed::Lost => false,
        Timed::Made { read, alarms, .. } => *alarms == 0 && *read < 0,
    }
}

fn short_timer(cx: &mut Cx<'_>) -> Result<(), Failure> {
    rate(cx)?;
    let mut refuting = 0_u32;
    for over in [Standing::Reached, Standing::Ahead] {
        let mut rows = Vec::new();
        for quarters in QUARTERS.saturating_neg()..=QUARTERS {
            let shots = match cx.call(
                Short {
                    quarters,
                    over,
                    trials: ATTEMPTS,
                },
                ANSWER,
            )? {
                Shot::Done(shots) => shots,
                Shot::Refused(refusal) => {
                    return Err(Failure::Incomplete(format!(
                        "h {} at {quarters}: refused {refusal:?}",
                        over_name(over)
                    )));
                }
            };
            let count = shots.iter().filter(|timed| refutes(timed)).count();
            refuting = refuting.saturating_add(u32::try_from(count).unwrap_or(u32::MAX));
            rows.push(serde_json::json!({ "quarters": quarters, "shots": shots }));
        }
        cx.measure(
            format!("h/{}", over_name(over)),
            serde_json::Value::Array(rows),
        );
    }
    cx.measure("h/refuting", refuting.into());
    check(
        refuting == 0,
        format!("h: {refuting} trial(s) without an alarm while the read had the target ahead"),
    )
}

fn enables_at_the_end(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let words: WordsRead = cx.call(Words, ANSWER)?;
    cx.measure("enables", serde_json::to_value(words).unwrap_or_default());
    // Comparator 0's enable is the executor's, which disables it while no
    // alarm is due: only comparators 1 and 2 are the probe's.
    check(
        words.work[1] && words.work[2] && words.interrupt[1] && !words.interrupt[2],
        format!("the enables at the end: {words:?}"),
    )
}

#[cfg(test)]
mod tests;
