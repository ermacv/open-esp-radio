//! SYSTIMER facts on the stand (`tasks/380`): a target loaded into a
//! comparator whose enable and interrupt enable were written once at boot,
//! over the probe `esp32s31-timer-facts`. One case a row of the task's
//! table; every number the image answers is a measure of the results file.
//!
//! Cases a, b, c, e and f pass when every trial gave the expected outcome;
//! d and g pass when answered, and their tables are the result.
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
    MOST_ATTEMPTS, MOST_TRIALS, New, Ran, Reached, Request, Run, SWEEP_NEW, Spread, Sweep, Swept,
    Tally, Words, WordsRead,
};
use oer_protocols_hil_agent::base::Service as _;
use oer_tools_hil_harness::{Case, Cx, Failure, Scenario, check};

const SCENARIO: Scenario = Scenario::new("timer-facts", "esp32s31-timer-facts", &[Request::KEYS]);

const CASES: [Case; 8] = [
    Case {
        name: "a-loaded-over-a-standing-target",
        run: loaded_over_a_standing_target,
    },
    Case {
        name: "b-loaded-after-an-alarm",
        run: loaded_after_an_alarm,
    },
    Case {
        name: "c-behind-the-counter",
        run: behind_the_counter,
    },
    Case {
        name: "d-across-the-standing-expiry",
        run: across_the_standing_expiry,
    },
    Case {
        name: "e-words-written-apart",
        run: words_written_apart,
    },
    Case {
        name: "f-hart-1-beside",
        run: hart_1_beside,
    },
    Case {
        name: "g-far-targets",
        run: far_targets,
    },
    Case {
        name: "enables-at-the-end",
        run: enables_at_the_end,
    },
];

/// Trials of each case of a to c, e and f.
const TRIALS: u32 = 1_000;
/// Case d's window on each side of the standing target's expiry, in ticks.
const SWEEP_TICKS: i32 = 5;
/// Case d's steps of one tick.
const STEPS_PER_TICK: i32 = 4;
/// Case d's trials at each offset.
const ATTEMPTS: u8 = 4;
/// How far an alarm may lie from its expected cycle and still be at it:
/// one microsecond of the timer.
const TOLERANCE_TICKS: u64 = 16;
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

/// Two tallies as one.
fn merged(a: Tally, b: Tally) -> Tally {
    let spread = |a: Spread, b: Spread| Spread {
        count: a.count.saturating_add(b.count),
        least: a.least.min(b.least),
        most: a.most.max(b.most),
        sum: a.sum.saturating_add(b.sum),
    };
    Tally {
        trials: a.trials.saturating_add(b.trials),
        lost: a.lost.saturating_add(b.lost),
        after_load: {
            let mut bins = a.after_load;
            for (bin, more) in bins.iter_mut().zip(b.after_load) {
                *bin = bin.saturating_add(more);
            }
            bins
        },
        before_load: a.before_load.saturating_add(b.before_load),
        at: spread(a.at, b.at),
        load: spread(a.load, b.load),
    }
}

/// `trials` of `load`, in runs of the most an answer takes, as one tally,
/// recorded as `name`.
fn tally(cx: &mut Cx<'_>, name: &str, load: Load, trials: u32) -> Result<Tally, Failure> {
    let mut total = Tally {
        trials: 0,
        lost: 0,
        after_load: [0; 3],
        before_load: 0,
        at: Spread::EMPTY,
        load: Spread::EMPTY,
    };
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

/// Every trial acted, saw no alarm before its load and exactly one after
/// it, at the expected cycle within `low..=high` cycles.
fn exactly_one(name: &str, tally: Tally, low: i64, high: i64) -> Result<(), Failure> {
    check(
        tally.lost == 0
            && tally.before_load == 0
            && tally.after_load == [0, tally.trials, 0]
            && tally.at.count == tally.trials
            && i64::from(tally.at.least) >= low
            && i64::from(tally.at.most) <= high,
        format!("{name}: {tally:?}, the first alarm expected within {low}..={high} cycles"),
    )
}

/// a, its two loads: the new target later and earlier than the standing one.
fn case_a(cx: &mut Cx<'_>, rate: Rate, prefix: &str) -> Result<(), Failure> {
    let tolerance = rate.cycles_of(TOLERANCE_TICKS);
    for (name, load) in [("later", Load::Later), ("earlier", Load::Earlier)] {
        let name = format!("{prefix}{name}");
        let tally = tally(cx, &name, load, TRIALS)?;
        exactly_one(&name, tally, tolerance.saturating_neg(), tolerance)?;
    }
    Ok(())
}

fn loaded_over_a_standing_target(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let rate = rate(cx)?;
    case_a(cx, rate, "a/")
}

fn loaded_after_an_alarm(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let rate = rate(cx)?;
    let tolerance = rate.cycles_of(TOLERANCE_TICKS);
    let tally = tally(cx, "b/after-alarm", Load::AfterAlarm, TRIALS)?;
    exactly_one(
        "b/after-alarm",
        tally,
        tolerance.saturating_neg(),
        tolerance,
    )
}

fn behind_the_counter(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let rate = rate(cx)?;
    // At once: within the tolerance of the load's strobe.
    let tolerance = rate.cycles_of(TOLERANCE_TICKS);
    for ticks in [1, 16, 1_000, 1_000_000] {
        let name = format!("c/behind-{ticks}");
        let tally = tally(cx, &name, Load::Behind { ticks }, TRIALS)?;
        exactly_one(&name, tally, 0, tolerance)?;
    }
    Ok(())
}

/// d's sweep: every offset, both new targets, each attempt recorded as it
/// happened. Answers the count of future targets that never raised.
fn sweep(cx: &mut Cx<'_>, rate: Rate, prefix: &str) -> Result<u32, Failure> {
    let tick = rate.cycles_of(1);
    let step = tick
        .checked_div(i64::from(STEPS_PER_TICK))
        .unwrap_or(1)
        .max(1);
    let span = tick.saturating_mul(i64::from(SWEEP_TICKS));
    let new_at = rate.cycles_of(SWEEP_NEW);
    let tolerance = rate.cycles_of(TOLERANCE_TICKS);
    let mut unraised = 0_u32;
    for (label, new) in [("future", New::Future), ("behind", New::Behind)] {
        let mut rows = Vec::new();
        let mut asked = span.saturating_neg();
        while asked <= span {
            let offset = i32::try_from(asked).unwrap_or(i32::MAX);
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
            for attempt in &attempts {
                if let (New::Future, Attempt::Made { first, second, .. }) = (new, attempt) {
                    // The new target raised: an alarm at its time, past the
                    // standing one's.
                    let raised = [*first, *second]
                        .into_iter()
                        .flatten()
                        .any(|cycle| i64::from(cycle) >= new_at.saturating_sub(tolerance));
                    if !raised {
                        unraised = unraised.saturating_add(1);
                    }
                }
            }
            rows.push(serde_json::json!({ "asked": offset, "attempts": attempts }));
            asked = asked.saturating_add(step);
        }
        cx.measure(format!("{prefix}{label}"), serde_json::Value::Array(rows));
    }
    cx.measure(format!("{prefix}future-never-raised"), unraised.into());
    Ok(unraised)
}

fn across_the_standing_expiry(cx: &mut Cx<'_>) -> Result<(), Failure> {
    const {
        if ATTEMPTS as usize > MOST_ATTEMPTS {
            panic!("a sweep's attempts fit one answer");
        }
    };
    let rate = rate(cx)?;
    let unraised = sweep(cx, rate, "d/")?;
    if unraised > 0 {
        // The one outcome that refutes the design: stated, the case still
        // passes as answered.
        eprintln!("d: {unraised} future target(s) never raised");
    }
    Ok(())
}

fn words_written_apart(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let rate = rate(cx)?;
    let tolerance = rate.cycles_of(TOLERANCE_TICKS);
    for (name, order) in [
        ("e/low-first", Apart::LowFirst),
        ("e/high-first", Apart::HighFirst),
    ] {
        let tally = tally(cx, name, Load::Apart(order), TRIALS)?;
        exactly_one(name, tally, tolerance.saturating_neg(), tolerance)?;
    }
    Ok(())
}

fn hart_1_beside(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let rate = rate(cx)?;
    match cx.call(Concurrent::Start, ANSWER)? {
        Looped::Started => {}
        other => return Err(Failure::Incomplete(format!("f: start answered {other:?}"))),
    }
    let a = case_a(cx, rate, "f/a/");
    let d = sweep(cx, rate, "f/d/");
    let stopped = cx.call(Concurrent::Stop, ANSWER)?;
    cx.measure(
        "f/hart-1",
        serde_json::to_value(stopped).unwrap_or_default(),
    );
    a?;
    d?;
    match stopped {
        Looped::Stopped {
            loads,
            alarms,
            missed,
        } => check(
            loads > 0 && alarms == loads && missed == 0,
            format!("f: hart 1 loaded {loads}, saw {alarms} alarms, missed {missed}"),
        ),
        other => Err(Failure::Incomplete(format!("f: stop answered {other:?}"))),
    }
}

fn far_targets(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let whole = 1_u64 << COUNTER_BITS;
    let aheads = (30..=51)
        .map(|k| (format!("2^{k}"), 1_u64 << k))
        .chain([("2^52-16000".to_owned(), whole.saturating_sub(16_000))]);
    let mut rows = Vec::new();
    for (label, ahead) in aheads {
        let reached = cx.call(Far { ahead }, ANSWER)?;
        if let Reached::Refused(refusal) = reached {
            return Err(Failure::Incomplete(format!(
                "g {label}: refused {refusal:?}"
            )));
        }
        rows.push(serde_json::json!({ "ahead": label, "reached": reached }));
    }
    cx.measure("g", serde_json::Value::Array(rows));
    Ok(())
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
