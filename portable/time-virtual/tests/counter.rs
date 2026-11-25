#![allow(clippy::disallowed_macros, reason = "test code: a test may panic")]
//! The virtual settle counter: one cycle a microsecond, each sample
//! advancing it by its step.

use oer_portable_settle::{SettleCounter, SettleLimit};
use oer_portable_time::Duration;
use oer_portable_time_virtual::VirtualCounter;

#[test]
fn a_sample_gives_the_value_then_steps_and_now_does_not_step() {
    let counter = VirtualCounter::new(7, 3);
    assert_eq!(counter.now(), 7);
    assert_eq!(counter.now(), 7);
    assert_eq!(counter.sample(), 7);
    assert_eq!(counter.sample(), 10);
    assert_eq!(counter.now(), 13);
}

#[test]
fn advance_moves_the_counter_and_wraps() {
    let counter = VirtualCounter::new(u32::MAX - 1, 0);
    counter.advance(5);
    assert_eq!(counter.now(), 3);
    assert_eq!(counter.sample(), 3);
    assert_eq!(counter.sample(), 3);
}

#[test]
fn a_limit_counts_one_cycle_a_microsecond() {
    let counter = VirtualCounter::new(0, 0);
    for micros in [1, 2, 20] {
        let limit = SettleLimit::new(Duration::from_micros(micros));
        assert_eq!(
            limit.map(|limit| counter.cycles(limit)),
            u32::try_from(micros).ok()
        );
    }
}
