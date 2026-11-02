//! The sleep sequence over a model of the PMU and the LP timer.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

extern crate std;
use std::vec::Vec;

use super::{
    Outcome, POLLS_PER_TIMER_READ, RawOutcome, Registers, RtcTicks, SleepClocks, SleepError,
    SleepRequest, Slept, check_timer, sleep_until,
};
use crate::Access;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Op {
    State(SleepClocks),
    Waits,
    Capture,
    AckRtc,
    Arm(u64),
    Wake(bool),
    NoReject,
    AckOutcome,
    Request,
    Outcome,
    Pending,
}

/// What the model PMU answers to a request.
#[derive(Clone, Copy)]
enum Answer {
    /// The LP timer reaches the target after `polls` outcome polls.
    WakeAfter {
        polls: u32,
    },
    Reject,
    Never,
}

struct Model {
    ops: Vec<Op>,
    /// The counter every capture advances by one.
    counter: u64,
    answer: Answer,
    requested: bool,
    polls: u32,
    /// The armed target.
    armed: Option<u64>,
    /// The target never raises its status.
    dead_timer: bool,
}

const WAKE_CAUSES: u32 = 1 << 13;
const REJECT_CAUSES: u32 = 1 << 22;

impl Model {
    fn new(counter: u64, answer: Answer) -> Self {
        Self {
            ops: Vec::new(),
            counter,
            answer,
            requested: false,
            polls: 0,
            armed: None,
            dead_timer: false,
        }
    }
}

impl Registers for Model {
    const MIN_SLEEP: u64 = 10;
    fn configure_state(&mut self, _: Access<'_>, clocks: SleepClocks) {
        self.ops.push(Op::State(clocks));
    }
    fn configure_waits(&mut self, _: Access<'_>) {
        self.ops.push(Op::Waits);
    }
    fn capture_rtc(&mut self, _: Access<'_>) {
        self.ops.push(Op::Capture);
        self.counter += 1;
    }
    fn captured_rtc(&self, _: Access<'_>) -> u64 {
        self.counter
    }
    fn acknowledge_rtc_wake(&mut self, _: Access<'_>) {
        self.ops.push(Op::AckRtc);
    }
    fn rtc_wake_pending(&mut self, _: Access<'_>) -> bool {
        self.ops.push(Op::Pending);
        !self.dead_timer && self.armed.is_some_and(|at| self.counter >= at)
    }
    fn arm_rtc_target(&mut self, _: Access<'_>, at: RtcTicks) {
        self.ops.push(Op::Arm(at.get()));
        self.armed = Some(at.get());
    }
    fn select_wake(&mut self, _: Access<'_>, rtc_timer: bool) {
        self.ops.push(Op::Wake(rtc_timer));
    }
    fn disable_reject(&mut self, _: Access<'_>) {
        self.ops.push(Op::NoReject);
    }
    fn acknowledge_outcome(&mut self, _: Access<'_>) {
        self.ops.push(Op::AckOutcome);
    }
    fn request_sleep(&mut self, _: Access<'_>) {
        self.ops.push(Op::Request);
        self.requested = true;
    }
    fn outcome(&mut self, _: Access<'_>) -> RawOutcome {
        self.ops.push(Op::Outcome);
        assert!(self.requested, "the outcome is read only after the request");
        self.polls += 1;
        match self.answer {
            Answer::WakeAfter { polls } if self.polls >= polls => RawOutcome {
                woke: true,
                rejected: false,
            },
            Answer::Reject => RawOutcome {
                woke: true,
                rejected: true,
            },
            _ => RawOutcome::default(),
        }
    }
    fn wake_causes(&self, _: Access<'_>) -> u32 {
        WAKE_CAUSES
    }
    fn reject_causes(&self, _: Access<'_>) -> u32 {
        REJECT_CAUSES
    }
}

fn request(wake_at: u64, give_up_after: u64) -> SleepRequest {
    SleepRequest {
        wake_at: RtcTicks::new(wake_at).unwrap(),
        give_up_after: RtcTicks::new(give_up_after).unwrap(),
        clocks: SleepClocks::Running,
    }
}

#[test]
fn a_sleep_configures_arms_requests_and_cleans_up_in_order() {
    let mut model = Model::new(100, Answer::WakeAfter { polls: 3 });
    let slept = sleep_until(&mut model, request(1_000, 2_000)).unwrap();
    assert_eq!(
        slept,
        Slept {
            outcome: Outcome::Woke {
                causes: WAKE_CAUSES
            },
            requested: RtcTicks::new(101).unwrap(),
            resumed: RtcTicks::new(102).unwrap(),
        }
    );
    assert_eq!(
        model.ops,
        [
            Op::Capture,
            Op::State(SleepClocks::Running),
            Op::Waits,
            Op::NoReject,
            Op::AckOutcome,
            Op::AckRtc,
            Op::Arm(1_000),
            Op::Wake(true),
            Op::Request,
            Op::Outcome,
            Op::Outcome,
            Op::Outcome,
            Op::Capture,
            Op::Wake(false),
            Op::AckOutcome,
            Op::AckRtc,
        ]
    );
}

#[test]
fn a_reject_is_an_outcome_and_wins_over_a_wake() {
    let mut model = Model::new(0, Answer::Reject);
    let slept = sleep_until(&mut model, request(1_000, 2_000)).unwrap();
    assert_eq!(
        slept.outcome,
        Outcome::Rejected {
            causes: REJECT_CAUSES
        }
    );
    assert_eq!(model.ops.last(), Some(&Op::AckRtc));
}

#[test]
fn a_target_within_the_minimum_sleep_writes_nothing() {
    // The capture reads 101; the minimum sleep reaches 111.
    let mut model = Model::new(100, Answer::Never);
    assert_eq!(
        sleep_until(&mut model, request(111, 2_000)),
        Err(SleepError::TooSoon {
            now: RtcTicks::new(101).unwrap(),
            wake_at: RtcTicks::new(111).unwrap(),
        })
    );
    assert_eq!(model.ops, [Op::Capture]);
}

#[test]
fn a_bound_before_the_target_writes_nothing() {
    let mut model = Model::new(0, Answer::Never);
    assert_eq!(
        sleep_until(&mut model, request(1_000, 1_000)),
        Err(SleepError::Bound)
    );
    assert!(model.ops.is_empty());
}

#[test]
fn no_outcome_by_the_bound_is_an_error_and_disables_the_wake() {
    // Each timer read advances the model counter by one: the bound passes
    // after a few reads, each after `POLLS_PER_TIMER_READ` polls.
    let mut model = Model::new(0, Answer::Never);
    assert_eq!(
        sleep_until(&mut model, request(20, 22)),
        Err(SleepError::NoOutcome {
            now: RtcTicks::new(23).unwrap()
        })
    );
    let polls = model.ops.iter().filter(|op| **op == Op::Outcome).count();
    assert_eq!(polls, 22 * POLLS_PER_TIMER_READ as usize);
    assert_eq!(
        model.ops[model.ops.len() - 3..],
        [Op::Wake(false), Op::AckOutcome, Op::AckRtc]
    );
}

#[test]
fn a_counter_wider_than_the_timer_is_a_typed_error() {
    let mut model = Model::new(RtcTicks::MAX, Answer::Never);
    assert_eq!(
        sleep_until(&mut model, request(1_000, 2_000)),
        Err(SleepError::Counter(RtcTicks::MAX + 1))
    );
}

#[test]
fn ticks_hold_only_the_counter_width() {
    assert_eq!(
        RtcTicks::new(RtcTicks::MAX).map(RtcTicks::get),
        Some(RtcTicks::MAX)
    );
    assert_eq!(RtcTicks::new(RtcTicks::MAX + 1), None);
    assert_eq!(RtcTicks::new(RtcTicks::MAX).unwrap().checked_add(1), None);
    let later = RtcTicks::new(30).unwrap();
    let earlier = RtcTicks::new(10).unwrap();
    assert_eq!(later.saturating_since(earlier), 20);
    assert_eq!(earlier.saturating_since(later), 0);
}

fn ticks(count: u64) -> RtcTicks {
    RtcTicks::new(count).unwrap()
}

#[test]
fn a_timer_check_arms_the_target_and_reports_the_count_it_fired_at() {
    // Each timer read advances the counter by one: the target at 5 fires
    // after a few reads.
    let mut model = Model::new(0, Answer::Never);
    let fired = check_timer(&mut model, ticks(5), ticks(100)).unwrap();
    assert!(fired >= ticks(5));
    assert_eq!(model.ops[1..3], [Op::AckRtc, Op::Arm(5)]);
    assert_eq!(model.ops.last(), Some(&Op::AckRtc));
    assert!(!model.ops.contains(&Op::Request), "a check never sleeps");
}

#[test]
fn a_timer_that_never_fires_is_no_outcome() {
    let mut model = Model::new(0, Answer::Never);
    model.dead_timer = true;
    assert_eq!(
        check_timer(&mut model, ticks(3), ticks(4)),
        Err(SleepError::NoOutcome { now: ticks(5) })
    );
    assert_eq!(model.ops.last(), Some(&Op::AckRtc));
}

#[test]
fn a_timer_check_refuses_a_past_target_and_a_bound_before_it() {
    let mut model = Model::new(10, Answer::Never);
    assert_eq!(
        check_timer(&mut model, ticks(5), ticks(100)),
        Err(SleepError::TooSoon {
            now: ticks(11),
            wake_at: ticks(5)
        })
    );
    assert_eq!(
        check_timer(&mut model, ticks(50), ticks(50)),
        Err(SleepError::Bound)
    );
}
