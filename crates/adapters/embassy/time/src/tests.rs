extern crate std;

use core::{
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};

use oer_time::{Clock, Duration, Instant, Timer};

use super::{EmbassyClock, now_micros};

fn poll<F: Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

#[test]
fn the_clock_reads_the_driver_and_never_decreases() {
    let first = EmbassyClock.now();
    let driver = now_micros();
    let second = EmbassyClock.now();
    assert!(first.as_micros() <= driver && driver <= second.as_micros());
}

#[test]
fn a_wait_ends_once_its_deadline_has_passed() {
    let start = EmbassyClock.now();
    let mut wait = pin!(EmbassyClock.wait_for(Duration::from_millis(20)));
    assert_eq!(poll(wait.as_mut()), Poll::Pending);
    std::thread::sleep(std::time::Duration::from_millis(25));
    assert_eq!(poll(wait.as_mut()), Poll::Ready(Ok(())));
    assert!(EmbassyClock.now().saturating_duration_since(start) >= Duration::from_millis(20));

    let mut reached = pin!(EmbassyClock.wait_until(start));
    assert_eq!(poll(reached.as_mut()), Poll::Ready(()));
}

#[test]
fn a_deadline_past_the_driver_range_is_never_reached() {
    let mut never = pin!(EmbassyClock.wait_until(Instant::from_micros(u64::MAX)));
    assert_eq!(poll(never.as_mut()), Poll::Pending);
}
