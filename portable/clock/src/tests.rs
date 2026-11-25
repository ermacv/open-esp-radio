extern crate std;

use core::{
    cell::Cell,
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};

use oer_portable_time::{Duration, Instant};

use super::{CLOCK_EPOCH_OFFSET, Clock, TimeOverflow, Timer};

/// A clock that stands still until the test moves it; a wait completes once
/// the clock has reached its deadline.
struct ManualClock {
    now: Cell<u64>,
}

impl Clock for ManualClock {
    fn now(&self) -> Instant {
        Instant::from_micros(self.now.get())
    }
}

impl Timer for ManualClock {
    fn wait_until(&self, deadline: Instant) -> impl Future<Output = ()> {
        core::future::poll_fn(move |_| {
            if self.now() >= deadline {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
    }
}

fn poll<F: Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

#[test]
fn a_wait_completes_once_the_clock_reaches_its_deadline() {
    let clock = ManualClock { now: Cell::new(10) };
    let mut wait = pin!(clock.wait_for(Duration::from_micros(5)));
    assert_eq!(poll(wait.as_mut()), Poll::Pending);
    clock.now.set(14);
    assert_eq!(poll(wait.as_mut()), Poll::Pending);
    clock.now.set(15);
    assert_eq!(poll(wait.as_mut()), Poll::Ready(Ok(())));
}

#[test]
fn an_overflowing_deadline_fails_without_waiting() {
    let clock = ManualClock {
        now: Cell::new(u64::MAX - 1),
    };
    assert_eq!(
        clock.deadline_after(Duration::from_micros(2)),
        Err(TimeOverflow)
    );
    let mut wait = pin!(clock.wait_for(Duration::from_micros(2)));
    assert_eq!(poll(wait.as_mut()), Poll::Ready(Err(TimeOverflow)));
}

#[test]
fn a_shared_reference_is_the_same_timer() {
    let clock = ManualClock { now: Cell::new(0) };
    let shared = &clock;
    let mut wait = pin!(Timer::wait_until(&shared, Instant::from_micros(1)));
    assert_eq!(poll(wait.as_mut()), Poll::Pending);
    clock.now.set(1);
    assert_eq!(shared.now(), Instant::from_micros(1));
    assert_eq!(poll(wait.as_mut()), Poll::Ready(()));
}

#[test]
fn the_clock_epoch_offset_sets_bit_31_of_the_low_word() {
    assert_eq!(CLOCK_EPOCH_OFFSET.as_micros(), 1 << 31);
    let start = Instant::EPOCH.checked_add(CLOCK_EPOCH_OFFSET).unwrap();
    let low = u32::try_from(start.as_micros() % (1 << 32)).unwrap();
    assert_eq!(low, 1 << 31);
    assert_eq!(low.cast_signed(), i32::MIN);
}

/// The ports stand on the values alone, with no feature that could add a
/// dependency.
#[test]
fn the_crate_depends_on_the_values_alone() {
    let manifest: toml::Table = toml::from_str(include_str!("../Cargo.toml")).unwrap();
    let dependencies = manifest["dependencies"].as_table().unwrap();
    assert_eq!(
        dependencies.keys().collect::<std::vec::Vec<_>>(),
        ["oer-portable-time"]
    );
    for table in ["build-dependencies", "target", "features"] {
        assert!(manifest.get(table).is_none(), "`[{table}]`");
    }
}
