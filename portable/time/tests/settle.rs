#![allow(clippy::disallowed_methods)]
#![allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::expect_used,
    reason = "test code: a test may panic"
)]

//! The named settle on a virtual counter: one cycle a microsecond, each
//! sample advancing it by its step.

use oer_portable_time::Duration;
use oer_portable_time::settle::{Settle, SettleCounter, SettleLimit, Unsettled, hold, settle};
use oer_portable_time_virtual::VirtualCounter;

fn limit(micros: u64) -> SettleLimit {
    SettleLimit::new(Duration::from_micros(micros)).expect("a test limit is a valid settle limit")
}

#[test]
fn a_limit_is_nonzero_and_at_most_twenty_microseconds() {
    assert_eq!(SettleLimit::new(Duration::from_micros(0)), None);
    assert_eq!(SettleLimit::new(Duration::from_micros(21)), None);
    assert_eq!(limit(20).limit(), Duration::from_micros(20));
    assert_eq!(limit(20).micros(), 20);
    assert_eq!(limit(1).micros(), 1);
}

#[test]
fn first_agreeing_attempt_is_the_sample() {
    let counter = VirtualCounter::new(0, 1);
    let mut attempts = 0;
    let sample = settle(Settle::new(limit(4), &counter), || {
        attempts += 1;
        (attempts == 2).then_some(true)
    });
    assert_eq!(sample, Ok(true));
    assert_eq!(attempts, 2);
}

#[test]
fn a_pair_that_never_agrees_stops_when_the_limit_has_passed() {
    let counter = VirtualCounter::new(100, 1);
    let mut attempts = 0;
    let sample: Result<bool, _> = settle(Settle::new(limit(5), &counter), || {
        attempts += 1;
        None
    });
    assert_eq!(sample, Err(Unsettled));
    // The start sample is 100; the checks sample 101..=105, and 105 ends it.
    assert_eq!(attempts, 5);
}

#[test]
fn a_settle_across_the_counter_s_wrap_keeps_its_length() {
    let counter = VirtualCounter::new(u32::MAX - 2, 1);
    let mut attempts = 0;
    let sample: Result<bool, _> = settle(Settle::new(limit(5), &counter), || {
        attempts += 1;
        None
    });
    assert_eq!((sample, attempts), (Err(Unsettled), 5));
}

#[test]
fn concrete_and_erased_counters_keep_the_same_result_and_attempt_count() {
    fn run<K: SettleCounter + ?Sized>(
        wait: Settle<'_, K>,
        counter: &VirtualCounter,
        ready_on: Option<usize>,
    ) -> (Result<usize, Unsettled>, usize) {
        let mut attempts = 0;
        let result = settle(wait, || {
            attempts += 1;
            counter.advance(5);
            (Some(attempts) == ready_on).then_some(attempts)
        });
        (result, attempts)
    }

    // Each attempt takes 5 cycles of a 10-cycle limit: the third attempt
    // follows the sample that saw the limit pass, and it is the last.
    for (ready_on, expected) in [
        (Some(1), (Ok(1), 1)),
        (Some(2), (Ok(2), 2)),
        (Some(3), (Ok(3), 3)),
        (Some(4), (Err(Unsettled), 3)),
        (None, (Err(Unsettled), 3)),
    ] {
        let concrete_counter = VirtualCounter::new(0, 0);
        let concrete: Settle<'_, VirtualCounter> = Settle::new(limit(10), &concrete_counter);
        let concrete_copy = concrete;
        let erased_counter = VirtualCounter::new(0, 0);
        let erased: Settle<'_, dyn SettleCounter> = Settle::new(limit(10), &erased_counter);
        let erased_copy = erased;

        assert_eq!(run(concrete, &concrete_counter, ready_on), expected);
        assert_eq!(run(erased, &erased_counter, ready_on), expected);
        assert_eq!(concrete_copy.limit(), erased_copy.limit());
        assert_eq!(concrete_counter.now(), erased_counter.now());
    }
}

#[test]
fn an_interrupt_after_a_failed_attempt_still_gets_a_final_attempt() {
    // The first attempt finds the hardware busy; an interrupt then holds the
    // CPU beyond the limit while the hardware completes. The settle samples
    // once more and takes the value instead of reporting it unsettled.
    let counter = VirtualCounter::new(0, 0);
    let mut attempts = 0;
    let result = settle(Settle::new(limit(20), &counter), || {
        attempts += 1;
        if attempts == 1 {
            counter.advance(30);
            None
        } else {
            Some(attempts)
        }
    });
    assert_eq!(result, Ok(2));

    // A still-busy final attempt ends it.
    let counter = VirtualCounter::new(0, 0);
    let mut attempts = 0;
    let result: Result<(), _> = settle(Settle::new(limit(20), &counter), || {
        attempts += 1;
        counter.advance(30);
        None
    });
    assert_eq!((result, attempts), (Err(Unsettled), 2));
}

#[test]
fn a_hold_returns_once_its_whole_length_has_passed() {
    let counter = VirtualCounter::new(100, 1);
    hold(Settle::new(limit(20), &counter));
    let after = counter.now();
    assert!(
        after - 100 >= 20,
        "returned at {after}, before the hold's length"
    );
    assert!(
        after - 100 <= 24,
        "held past its length and the samples around it: {after}"
    );
}

#[test]
fn the_counter_s_cycles_decide_the_length() {
    /// Three cycles a microsecond.
    struct Triple(VirtualCounter);
    impl SettleCounter for Triple {
        fn cycles(&self, limit: SettleLimit) -> u32 {
            limit.micros() * 3
        }
        fn sample(&self) -> u32 {
            self.0.sample()
        }
    }
    let counter = Triple(VirtualCounter::new(0, 1));
    let mut attempts = 0;
    let result: Result<(), _> = settle(Settle::new(limit(2), &counter), || {
        attempts += 1;
        None
    });
    assert_eq!((result, attempts), (Err(Unsettled), 6));
}

#[test]
fn a_lent_counter_counts_as_its_owner() {
    fn run<K: SettleCounter>(
        counter: &K,
        now: impl Fn() -> u32,
    ) -> (Result<(), Unsettled>, u32, u32) {
        let mut attempts = 0;
        let result = settle(Settle::new(limit(5), counter), || {
            attempts += 1;
            None
        });
        (result, attempts, now())
    }
    let owned = VirtualCounter::new(10, 1);
    let owned = run(&owned, || owned.now());
    let counter = VirtualCounter::new(10, 1);
    // `K` is `&VirtualCounter`: the settle counts on the lent counter.
    let lent = run(&&counter, || counter.now());
    assert_eq!(lent, owned);
    assert_eq!(owned.0, Err(Unsettled));
}
