#![no_std]
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
//! The HIL timer facts service: targets loaded into a SYSTIMER comparator
//! whose enable (`CONF.TARGETn_WORK_EN`) and interrupt enable
//! (`INT_ENA.TARGETn_INT_ENA`) were written once at boot and never after.
//!
//! The image (`esp32s31-timer-facts`) runs every case on comparator 1 of
//! unit 0 on the boot hart, and, for the concurrent case, a loop on
//! comparator 2 on hart 1. An alarm is seen by polling `INT_RAW` against
//! the hart's cycle counter; every time is in that hart's cycles.
//!
//! Each trial of a case starts from a **reference alarm**: a target
//! [`REFERENCE_AHEAD`] ticks ahead of the counter, loaded once the previous
//! alarm was acknowledged, and seen at cycle `c_R`. The trial's targets are
//! counted from the reference's target `R`, and the cycle a target `X` is
//! expected at is `c_R + (X − R) × cycles / ticks`, with the rate the image
//! measured at boot ([`Calibrated`]). The reference and the trial's alarms
//! are seen by the same poll, so the poll's latency cancels.
//!
//! A trial writes its target's words before its act and, at the act, only
//! the load's strobe (`COMPn_LOAD`), except where a case writes words at
//! the act ([`Order`]); every offset is counted from the strobe, and the
//! first poll of `INT_RAW` follows the strobe with nothing between, except
//! in the short timer ([`Short`]), whose one read of the counter comes
//! first.
//!
//! The messages are shared by the image, which answers them, and the host
//! program, which sends them and decides.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

/// A trial's reference target, ahead of the counter: 10 µs.
pub const REFERENCE_AHEAD: u64 = 160;
/// The standing target still ahead, after the reference: 100 µs.
pub const STANDING_AHEAD: u64 = 1_600;
/// When a trial acts (its strobe, unless the case places it), after the
/// reference: 50 µs, half way to the standing target.
pub const ACT_AFTER: u64 = 800;
/// Case a's later target: past the standing one by 50 µs.
pub const LATER_PAST_STANDING: u64 = 800;
/// Case a's earlier target: before the standing one by 25 µs, still ahead
/// of the counter by 25 µs when loaded.
pub const EARLIER_BEFORE_STANDING: u64 = 400;
/// Case b's target, after the reference's alarm: 50 µs past the act.
pub const AFTER_ALARM_AHEAD: u64 = 800;
/// Case e′'s new target, past the act: 50 µs.
pub const APART_AHEAD: u64 = 800;
/// Case e′'s old low word, behind the counter's (a passed mixed value) or
/// past the new target's (one ahead): 1 000 ticks.
pub const APART_LOW: u64 = 1_000;
/// Case d′'s new target, from the standing one: 10 µs past it (future) or
/// 10 µs before it (behind the counter at the strobe).
pub const SWEEP_NEW: u64 = 160;
/// Hart 1's next target, from its previous one: 2 µs.
pub const LOOP_STEP: u64 = 32;
/// The window in which an alarm is counted, after the trial's last
/// target: 200 µs.
pub const WINDOW: u64 = 3_200;
/// How far an alarm may lie from its expected cycle and still be at it:
/// 1 µs.
pub const TOLERANCE: u64 = 16;
/// The ticks of the boot's rate measurement: 10 ms.
pub const RATE_TICKS: u64 = 160_000;
/// The counter's width, in bits: a target's bits above it are dropped.
pub const COUNTER_BITS: u32 = 52;

/// The most trials one [`Run`] takes, so that an answer comes within the
/// console's patience.
pub const MOST_TRIALS: u16 = 250;
/// The most trials one [`Sweep`] or [`Short`] takes: its answer lists each.
pub const MOST_ATTEMPTS: usize = 8;

oer_protocols_hil_protocol::messages! {
    endpoint Calibrate = "timer-facts/calibrate" => Calibrated;
    topic Calibrated = "timer-facts/calibrated";
    endpoint Run = "timer-facts/run" => Ran;
    topic Ran = "timer-facts/ran";
    endpoint Sweep = "timer-facts/sweep" => Swept;
    topic Swept = "timer-facts/swept";
    endpoint Short = "timer-facts/short" => Shot;
    topic Shot = "timer-facts/shot";
    endpoint Far = "timer-facts/far" => Reached;
    topic Reached = "timer-facts/reached";
    endpoint Concurrent = "timer-facts/concurrent" => Looped;
    topic Looped = "timer-facts/looped";
    endpoint Words = "timer-facts/words" => WordsRead;
    topic WordsRead = "timer-facts/words-read";
}

/// Why an endpoint answered without acting.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Refusal {
    /// The boot's rate measurement saw no alarm: no expected time exists.
    NoRate,
    /// More trials than [`MOST_TRIALS`] or [`MOST_ATTEMPTS`], or none.
    Trials,
    /// Hart 1's loop already runs.
    Running,
    /// Hart 1's loop does not run.
    NotRunning,
    /// Hart 1 did not answer within its bound.
    Hart1Silent,
    /// The counter's read did not verify.
    Unverified,
}

/// The boot's rate: the cycles of [`RATE_TICKS`] between two alarms.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Calibrate;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Calibrated {
    Rate {
        ticks: u32,
        cycles: u32,
    },
    /// An alarm of the measurement did not come within its bound.
    Missed,
}

/// What a trial's load replaces.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Standing {
    /// The reference target, already reached and its alarm acknowledged.
    Reached,
    /// A target [`STANDING_AHEAD`] after the reference, still ahead.
    Ahead,
}

/// A case of single loads, `trials` times.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Run {
    pub case: Case,
    pub trials: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Case {
    /// a: over the standing target still ahead, at the act a later one.
    /// Expected: one alarm, at the later target.
    Later,
    /// a: as [`Self::Later`], the new target before the standing one.
    Earlier,
    /// b: over the reference, reached and acknowledged, a target
    /// [`AFTER_ALARM_AHEAD`] past the act. Expected: one alarm, at it.
    AfterAlarm,
    /// c′: a target `ticks` behind the counter at the strobe, over
    /// `over`. The expected cycle is the strobe's.
    Behind { ticks: u32, over: Standing },
    /// e′: a load that changes the high word.
    Apart(Apart),
}

/// e′: the standing target lies one high word ahead of the new target,
/// whose high word the load writes back. `mixed` is the value the new high
/// word makes with the old low one; `order` how the words are written.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Apart {
    pub mixed: Mixed,
    pub order: Order,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Mixed {
    /// The old low word [`APART_LOW`] behind the counter's.
    Passed,
    /// The old low word [`APART_LOW`] past the new target's.
    Ahead,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Order {
    /// At the act: `TARGET1_HI`, `TARGET1_LO`, the strobe.
    HighFirst,
    /// At the act: `TARGET1_LO`, `TARGET1_HI`, the strobe.
    LowFirst,
    /// Both words written right after the standing target's load; at the
    /// act the strobe alone.
    Early,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Ran {
    Done(Tally),
    Refused(Refusal),
}

/// What the trials of a [`Run`] saw. An alarm is *at* the expected cycle
/// within [`TOLERANCE`], *early* when it comes before that.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Tally {
    pub trials: u16,
    /// Trials that could not act: the reference alarm did not come within
    /// its bound or the counter's read did not verify.
    pub lost: u16,
    /// Trials with an alarm before the strobe.
    pub before_strobe: u16,
    /// Trials with no alarm, one and more than one after the strobe.
    pub after_strobe: [u16; 3],
    /// Trials with an alarm after the strobe and early.
    pub early: u16,
    /// Trials with an alarm at the expected cycle.
    pub on_time: u16,
    /// The first alarm after the strobe, less the expected cycle.
    pub first: Spread,
    /// The alarm at the expected cycle, less it.
    pub at: Spread,
}

impl Tally {
    pub const EMPTY: Self = Self {
        trials: 0,
        lost: 0,
        before_strobe: 0,
        after_strobe: [0; 3],
        early: 0,
        on_time: 0,
        first: Spread::EMPTY,
        at: Spread::EMPTY,
    };
}

/// The least, the greatest and the sum of a measure over `count` samples.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Spread {
    pub count: u16,
    pub least: i32,
    pub most: i32,
    pub sum: i64,
}

impl Spread {
    pub const EMPTY: Self = Self {
        count: 0,
        least: i32::MAX,
        most: i32::MIN,
        sum: 0,
    };

    /// The spread with `sample` among its samples.
    #[must_use]
    pub const fn with(self, sample: i32) -> Self {
        Self {
            count: self.count.saturating_add(1),
            least: if sample < self.least {
                sample
            } else {
                self.least
            },
            most: if sample > self.most {
                sample
            } else {
                self.most
            },
            sum: self.sum.saturating_add(sample as i64),
        }
    }
}

/// d′: the standing target ahead and a new one whose words are written
/// before the act; the strobe `offset` cycles from the standing target's
/// expected cycle, `trials` times.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Sweep {
    pub offset: i32,
    pub new: New,
    pub trials: u8,
}

/// Where case d′'s new target lies.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum New {
    /// [`SWEEP_NEW`] past the standing target.
    Future,
    /// [`SWEEP_NEW`] before the standing target: behind the counter.
    Behind,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[allow(
    clippy::large_enum_variant,
    reason = "a message: the image builds its answer once and publishes it"
)]
pub enum Swept {
    Done(heapless::Vec<Attempt, MOST_ATTEMPTS>),
    Refused(Refusal),
}

/// One trial of a [`Sweep`], as it happened. Cycles are counted from the
/// standing target's expected cycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Attempt {
    /// The reference alarm did not come.
    Lost,
    Made {
        /// The strobe: the offset the trial had.
        strobe: i32,
        /// Every alarm of the trial, before the strobe and after, at most
        /// the first two kept.
        alarms: u8,
        first: Option<i32>,
        second: Option<i32>,
    },
}

/// h: the short timer. Over `over`, a target that at the strobe lies
/// `quarters` quarter ticks ahead of the counter (behind when negative);
/// after the strobe one read of the counter, then the watch; `trials`
/// times.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Short {
    pub quarters: i16,
    pub over: Standing,
    pub trials: u8,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[allow(
    clippy::large_enum_variant,
    reason = "a message: the image builds its answer once and publishes it"
)]
pub enum Shot {
    Done(heapless::Vec<Timed, MOST_ATTEMPTS>),
    Refused(Refusal),
}

/// One trial of a [`Short`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Timed {
    /// The reference alarm did not come, or a counter read did not verify.
    Lost,
    Made {
        /// The strobe, less the target's expected cycle.
        strobe: i32,
        /// The counter read after the strobe, less the target, in ticks:
        /// negative while the target is ahead of the read.
        read: i32,
        /// The alarms after the strobe, and the first one's cycle after the
        /// strobe.
        alarms: u8,
        first: Option<u32>,
    },
}

/// g′: over a target still ahead, a target `ahead` ticks past the counter,
/// its bits above [`COUNTER_BITS`] dropped.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Far {
    pub ahead: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Reached {
    /// An alarm within [`WINDOW`]: cycles from the strobe.
    AtOnce {
        latency: u32,
    },
    /// No alarm within the window.
    Waiting,
    Refused(Refusal),
}

/// f: hart 1's loop on comparator 2, started or stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Concurrent {
    Start,
    Stop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Looped {
    Started,
    /// Hart 1's counts: its loads, the alarms it saw, and its loads whose
    /// alarm did not come within its bound.
    Stopped {
        loads: u32,
        alarms: u32,
        missed: u32,
    },
    Refused(Refusal),
}

/// The enables, read at the end of the run.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Words;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WordsRead {
    /// `CONF.TARGET0..2_WORK_EN`.
    pub work: [bool; 3],
    /// `INT_ENA.TARGET0..2_INT_ENA`.
    pub interrupt: [bool; 3],
}

oer_protocols_hil_agent::service! {
    /// The timer facts' requests: each is a command the image's comparator
    /// owner runs.
    pub enum Request (sessions = false, observer = Observer, commands = Command, commander = Commander) {
        Calibrate(Calibrate) => command calibrate,
        Run(Run) => command run,
        Sweep(Sweep) => command sweep,
        Short(Short) => command short,
        Far(Far) => command far,
        Concurrent(Concurrent) => command concurrent,
        Words(Words) => command words,
    }
}

/// The largest request payload the image takes.
pub const MAXIMUM_PAYLOAD_BYTES: u16 = 16;

#[cfg(test)]
mod tests;
