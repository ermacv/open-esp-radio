extern crate std;

use core::{
    cell::Cell,
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};

use super::{Clock, Duration, Instant, TimeOverflow, Timer};

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
fn instant_arithmetic_is_checked_at_both_ends() {
    let instant = Instant::from_micros(100);
    assert_eq!(
        instant.checked_add(Duration::from_micros(50)),
        Some(Instant::from_micros(150))
    );
    assert_eq!(
        Instant::from_micros(u64::MAX).checked_add(Duration::from_micros(1)),
        None
    );
    assert_eq!(instant.checked_sub(Duration::from_micros(101)), None);
    assert_eq!(
        instant.checked_duration_since(Instant::from_micros(40)),
        Some(Duration::from_micros(60))
    );
    assert_eq!(
        instant.checked_duration_since(Instant::from_micros(101)),
        None
    );
    assert_eq!(
        instant.saturating_duration_since(Instant::from_micros(101)),
        Duration::ZERO
    );
}

#[test]
fn durations_convert_every_unit_to_microseconds() {
    assert_eq!(Duration::from_millis(3).as_micros(), 3_000);
    assert_eq!(Duration::from_secs(2).as_micros(), 2_000_000);
    assert_eq!(
        Duration::from_millis(u32::MAX).as_micros(),
        u64::from(u32::MAX) * 1_000
    );
    assert_eq!(
        Duration::from_micros(u64::MAX).checked_add(Duration::from_micros(1)),
        None
    );
    assert_eq!(
        Duration::from_micros(5).saturating_sub(Duration::from_micros(9)),
        Duration::ZERO
    );
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
fn a_saturating_deadline_past_the_range_never_comes() {
    let late = Instant::from_micros(u64::MAX - 1);
    assert_eq!(
        late.saturating_add(Duration::from_secs(1)),
        Instant::from_micros(u64::MAX)
    );
    assert_eq!(
        Instant::from_micros(5).saturating_add(Duration::from_micros(10)),
        Instant::from_micros(15)
    );
}

/// Without its features `oer-time` stays a dependency-free contract
/// crate: every dependency is optional and only a feature enables it.
#[test]
fn without_features_the_crate_has_no_dependency() {
    let manifest: toml::Table = toml::from_str(include_str!("../Cargo.toml")).unwrap();
    let dependencies = manifest
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .unwrap();
    for (name, spec) in dependencies {
        let optional = spec.get("optional").and_then(toml::Value::as_bool) == Some(true);
        assert!(optional, "`{name}` is not optional");
    }
    for table in ["build-dependencies", "target"] {
        assert!(
            manifest.get(table).is_none(),
            "`[{table}]` adds dependencies"
        );
    }
    let defaults = manifest
        .get("features")
        .and_then(|features| features.get("default"));
    assert!(
        defaults.is_none_or(|default| default.as_array().is_some_and(|list| list.is_empty())),
        "a default feature"
    );
}
