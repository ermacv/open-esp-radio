//! The named settle on a stepping clock and on virtual time.

use core::cell::Cell;

use oer_time::settle::{Settle, SettleLimit, Unsettled, settle};
use oer_time::{Clock, Duration, Instant};

/// A clock that advances one microsecond at every read.
struct Stepping(Cell<u64>);

impl Clock for Stepping {
    fn now(&self) -> Instant {
        let now = self.0.get();
        self.0.set(now + 1);
        Instant::from_micros(now)
    }
}

fn limit(micros: u64) -> SettleLimit {
    SettleLimit::new(Duration::from_micros(micros)).expect("a test limit is a valid settle limit")
}

#[test]
fn a_limit_is_nonzero_and_at_most_twenty_microseconds() {
    assert_eq!(SettleLimit::new(Duration::from_micros(0)), None);
    assert_eq!(SettleLimit::new(Duration::from_micros(21)), None);
    assert_eq!(limit(20).limit(), Duration::from_micros(20));
}

#[test]
fn first_agreeing_attempt_is_the_sample() {
    let clock = Stepping(Cell::new(0));
    let mut attempts = 0;
    let sample = settle(Settle::new(limit(4), &clock), || {
        attempts += 1;
        (attempts == 2).then_some(true)
    });
    assert_eq!(sample, Ok(true));
    assert_eq!(attempts, 2);
}

#[test]
fn a_pair_that_never_agrees_stops_when_the_limit_has_passed() {
    let clock = Stepping(Cell::new(100));
    let mut attempts = 0;
    let sample: Result<bool, _> = settle(Settle::new(limit(5), &clock), || {
        attempts += 1;
        None
    });
    assert_eq!(sample, Err(Unsettled));
    // The start read is 100; the checks read 101..=105, and 105 ends it.
    assert_eq!(attempts, 5);
}

#[test]
fn concrete_and_erased_clocks_keep_the_same_result_and_attempt_count() {
    use oer_time_virtual::VirtualClock;

    fn run<C: Clock + ?Sized>(
        wait: Settle<'_, C>,
        clock: &VirtualClock,
        ready_on: Option<usize>,
    ) -> (Result<usize, Unsettled>, usize) {
        let mut attempts = 0;
        let result = settle(wait, || {
            attempts += 1;
            clock
                .advance(Duration::from_micros(5))
                .expect("the virtual clock advances");
            (Some(attempts) == ready_on).then_some(attempts)
        });
        (result, attempts)
    }

    // Each attempt takes 5 us of a 10-us limit: the third attempt follows
    // the read that saw the limit pass, and it is the last.
    for (ready_on, expected) in [
        (Some(1), (Ok(1), 1)),
        (Some(2), (Ok(2), 2)),
        (Some(3), (Ok(3), 3)),
        (Some(4), (Err(Unsettled), 3)),
        (None, (Err(Unsettled), 3)),
    ] {
        let concrete_clock = VirtualClock::default();
        let concrete: Settle<'_, VirtualClock> = Settle::new(limit(10), &concrete_clock);
        let concrete_copy = concrete;
        let erased_clock = VirtualClock::default();
        let erased: Settle<'_, dyn Clock> = Settle::new(limit(10), &erased_clock);
        let erased_copy = erased;

        assert_eq!(run(concrete, &concrete_clock, ready_on), expected);
        assert_eq!(run(erased, &erased_clock, ready_on), expected);
        assert_eq!(concrete_copy.limit(), erased_copy.limit());
        assert_eq!(concrete_clock.now(), erased_clock.now());
    }
}

#[test]
fn an_interrupt_after_a_failed_attempt_still_gets_a_final_attempt() {
    use oer_time_virtual::VirtualClock;

    // The first attempt finds the hardware busy; an interrupt then holds the
    // CPU beyond the limit while the hardware completes. The settle samples
    // once more and takes the value instead of reporting it unsettled.
    let clock: VirtualClock = VirtualClock::default();
    let mut attempts = 0;
    let result = settle(Settle::new(limit(20), &clock), || {
        attempts += 1;
        if attempts == 1 {
            clock
                .advance(Duration::from_micros(30))
                .expect("the virtual clock advances past the limit");
            None
        } else {
            Some(attempts)
        }
    });
    assert_eq!(result, Ok(2));

    // A still-busy final attempt ends it.
    let clock: VirtualClock = VirtualClock::default();
    let mut attempts = 0;
    let result: Result<(), _> = settle(Settle::new(limit(20), &clock), || {
        attempts += 1;
        clock
            .advance(Duration::from_micros(30))
            .expect("the virtual clock advances past the limit");
        None
    });
    assert_eq!((result, attempts), (Err(Unsettled), 2));
}
