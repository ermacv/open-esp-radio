use core::cell::Cell;

use oer_time::{Clock, Duration, Instant};

use super::*;

/// A clock that advances one microsecond at every read.
struct Stepping(Cell<u64>);

impl Clock for Stepping {
    fn now(&self) -> Instant {
        let now = self.0.get();
        self.0.set(now + 1);
        Instant::from_micros(now)
    }
}

fn limit(micros: u64) -> BluetoothDiagnosticSettleLimit {
    BluetoothDiagnosticSettleLimit::new(Duration::from_micros(micros)).unwrap()
}

#[test]
fn a_limit_is_nonzero_and_at_most_twenty_microseconds() {
    assert_eq!(
        BluetoothDiagnosticSettleLimit::new(Duration::from_micros(0)),
        None
    );
    assert_eq!(
        BluetoothDiagnosticSettleLimit::new(Duration::from_micros(21)),
        None
    );
    assert_eq!(limit(20).limit(), Duration::from_micros(20));
}

#[test]
fn first_agreeing_attempt_is_the_sample() {
    let clock = Stepping(Cell::new(0));
    let mut attempts = 0;
    let sample = settle(BluetoothDiagnosticSettle::new(limit(4), &clock), || {
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
    let sample: Result<bool, _> = settle(BluetoothDiagnosticSettle::new(limit(5), &clock), || {
        attempts += 1;
        None
    });
    assert_eq!(sample, Err(BluetoothDiagnosticUnsettled));
    // The start read is 100; the checks read 101..=105, and 105 ends it.
    assert_eq!(attempts, 5);
}
