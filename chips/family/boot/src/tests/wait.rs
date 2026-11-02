use core::cell::Cell;

use oer_time::{Clock, Duration};
use oer_time_virtual::VirtualClock;

use super::Ticking;
use crate::wait::{BootBudget, BootDeadline, BootWait, WaitFailure};

const WAIT: BootWait = const {
    match BootWait::new("test-wait", Duration::from_micros(50)) {
        Ok(wait) => wait,
        Err(_) => panic!("a boot wait has a nonzero bound"),
    }
};

fn budget(micros: u64) -> BootBudget {
    BootBudget::below(Duration::from_micros(micros), Duration::from_secs(9))
        .expect("the budget is below the watchdog")
}

#[test]
fn a_budget_lies_strictly_below_the_armed_watchdog() {
    let watchdog = Duration::from_secs(9);
    assert!(BootBudget::below(Duration::from_secs(9), watchdog).is_none());
    assert!(BootBudget::below(Duration::from_micros(0), watchdog).is_none());
    let budget = BootBudget::below(Duration::from_secs(2), watchdog).expect("below");
    assert!(budget.admits(WAIT));
    assert!(!budget.admits(BootWait::new("long", Duration::from_secs(3)).expect("nonzero")));
}

#[test]
fn a_wait_without_a_bound_is_refused() {
    let failure = BootWait::new("zero", Duration::from_micros(0)).expect_err("zero bound");
    assert_eq!(failure, WaitFailure::ZeroBound("zero"));
    let failure = crate::BootFailure::from(failure);
    assert_eq!(failure.reason(), "boot-wait-zero-bound");
    assert_eq!(failure.wait(), Some("zero"));
}

#[test]
fn a_ready_wait_returns_at_its_first_attempt() {
    let virtual_clock = VirtualClock::new();
    let clock = Ticking::new(&virtual_clock);
    let deadline = BootDeadline::start(&clock, budget(1_000));
    let attempts = Cell::new(0);
    let value = deadline.poll(WAIT, || {
        attempts.set(attempts.get() + 1);
        Some(7)
    });
    assert_eq!(value, Ok(7));
    assert_eq!(attempts.get(), 1);
}

#[test]
fn a_wait_past_its_bound_fails_with_its_name() {
    let virtual_clock = VirtualClock::new();
    let clock = Ticking::new(&virtual_clock);
    let deadline = BootDeadline::start(&clock, budget(1_000));
    let start = virtual_clock.now();
    let value = deadline.poll(WAIT, || None::<()>);
    assert_eq!(value, Err(WaitFailure::TimedOut("test-wait")));
    let waited = virtual_clock.now().saturating_duration_since(start);
    assert!(waited >= WAIT.bound(), "{waited:?}");
    // The wait ends within a few reads of its bound, not at the deadline.
    assert!(waited < Duration::from_micros(60), "{waited:?}");
}

#[test]
fn hardware_that_completes_at_the_bound_is_seen_by_the_last_attempt() {
    let virtual_clock = VirtualClock::new();
    let clock = Ticking::new(&virtual_clock);
    let deadline = BootDeadline::start(&clock, budget(1_000));
    let ready_at = virtual_clock
        .now()
        .saturating_add(WAIT.bound())
        .saturating_add(Duration::from_micros(1));
    let value = deadline.poll(WAIT, || (virtual_clock.now() >= ready_at).then_some(()));
    assert_eq!(value, Ok(()));
}

#[test]
fn the_boot_deadline_ends_a_wait_before_its_own_bound() {
    let virtual_clock = VirtualClock::new();
    let clock = Ticking::new(&virtual_clock);
    let deadline = BootDeadline::start(&clock, budget(20));
    let value = deadline.poll(WAIT, || None::<()>);
    assert_eq!(value, Err(WaitFailure::DeadlineExceeded("test-wait")));
    assert_eq!(deadline.remaining(), Duration::from_micros(0));
}

#[test]
fn a_hold_lasts_its_bound() {
    let virtual_clock = VirtualClock::new();
    let clock = Ticking::new(&virtual_clock);
    let deadline = BootDeadline::start(&clock, budget(1_000));
    let start = virtual_clock.now();
    assert_eq!(deadline.hold(WAIT), Ok(()));
    let held = virtual_clock.now().saturating_duration_since(start);
    assert!(
        held >= WAIT.bound() && held < Duration::from_micros(55),
        "{held:?}"
    );
}

#[test]
fn a_hold_past_the_deadline_fails_without_waiting() {
    let virtual_clock = VirtualClock::new();
    let clock = Ticking::new(&virtual_clock);
    let deadline = BootDeadline::start(&clock, budget(10));
    let start = virtual_clock.now();
    assert_eq!(
        deadline.hold(WAIT),
        Err(WaitFailure::DeadlineExceeded("test-wait"))
    );
    assert!(virtual_clock.now().saturating_duration_since(start) < Duration::from_micros(5));
}

#[test]
fn a_failure_names_its_reason_and_wait() {
    let failure = crate::BootFailure::from(WaitFailure::TimedOut("cpll-calibration"));
    assert_eq!(failure.reason(), "boot-wait-timeout");
    assert_eq!(failure.wait(), Some("cpll-calibration"));
    let deadline = crate::BootFailure::from(WaitFailure::DeadlineExceeded("x"));
    assert_eq!(deadline.reason(), "boot-deadline");
}

#[test]
fn a_hold_that_overflows_time_is_refused_without_shortening_it() {
    let clock: VirtualClock = VirtualClock::starting_at(oer_time::Instant::from_micros(u64::MAX));
    let deadline = BootDeadline::start(&clock, budget(1_000));
    assert_eq!(
        deadline.hold(WAIT),
        Err(WaitFailure::DeadlineExceeded("test-wait"))
    );
    assert_eq!(clock.now().as_micros(), u64::MAX);
}

#[test]
fn an_exhausted_time_domain_still_runs_the_final_poll_attempt() {
    let clock: VirtualClock = VirtualClock::starting_at(oer_time::Instant::from_micros(u64::MAX));
    let deadline = BootDeadline::start(&clock, budget(1_000));
    assert_eq!(deadline.poll(WAIT, || Some(7)), Ok(7));
    assert_eq!(
        deadline.poll(WAIT, || None::<()>),
        Err(WaitFailure::DeadlineExceeded("test-wait"))
    );
}
