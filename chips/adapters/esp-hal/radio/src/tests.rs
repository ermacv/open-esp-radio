use std::{cell::RefCell, vec::Vec};

use super::*;

extern crate std;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Event {
    Acquire(PlatformClock),
    Release(PlatformClock),
    Abandon(PlatformClock, PlatformClockAbandonment),
}

std::thread_local! {
    static EVENTS: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
}

/// Records the platform requests of this test thread.
struct Recording;

impl ClockBackend for Recording {
    fn acquire(clock: PlatformClock) {
        EVENTS.with(|events| events.borrow_mut().push(Event::Acquire(clock)));
    }
    fn release(clock: PlatformClock) {
        EVENTS.with(|events| events.borrow_mut().push(Event::Release(clock)));
    }
    fn abandon(clock: PlatformClock, how: PlatformClockAbandonment) {
        EVENTS.with(|events| events.borrow_mut().push(Event::Abandon(clock, how)));
    }
}

fn events() -> Vec<Event> {
    EVENTS.with(|events| core::mem::take(&mut *events.borrow_mut()))
}

#[test]
fn every_release_follows_its_own_request() {
    let clocks = PlatformClocks::<Recording>::new();
    for clock in PlatformClock::ALL {
        let guard = clocks.acquire(clock).unwrap();
        assert_eq!(guard.clock(), clock);
        assert_eq!(events(), [Event::Acquire(clock)]);
        guard.release();
        assert_eq!(events(), [Event::Release(clock)]);
    }
}

#[test]
fn nested_references_are_given_back_one_each() {
    let clocks = PlatformClocks::<Recording>::new();
    let outer = clocks.acquire(PlatformClock::Mpll).unwrap();
    let inner = clocks.acquire(PlatformClock::Mpll).unwrap();
    inner.release();
    outer.release();
    assert_eq!(
        events(),
        [
            Event::Acquire(PlatformClock::Mpll),
            Event::Acquire(PlatformClock::Mpll),
            Event::Release(PlatformClock::Mpll),
            Event::Release(PlatformClock::Mpll),
        ]
    );
}

#[test]
fn an_abandoned_reference_is_never_released() {
    let clocks = PlatformClocks::<Recording>::new();
    clocks
        .acquire(PlatformClock::AnalogI2cMaster)
        .unwrap()
        .abandon();
    assert_eq!(
        events(),
        [
            Event::Acquire(PlatformClock::AnalogI2cMaster),
            Event::Abandon(
                PlatformClock::AnalogI2cMaster,
                PlatformClockAbandonment::Abandoned
            ),
        ]
    );
}

#[test]
fn a_dropped_guard_keeps_its_clock_and_is_recorded() {
    let clocks = PlatformClocks::<Recording>::new();
    let guard = clocks.acquire(PlatformClock::ModemCoexistence).unwrap();
    // A dropped guard is a defect, which debug builds assert on.
    let dropped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(guard)));
    assert_eq!(dropped.is_err(), cfg!(debug_assertions));
    assert_eq!(
        events(),
        [
            Event::Acquire(PlatformClock::ModemCoexistence),
            Event::Abandon(
                PlatformClock::ModemCoexistence,
                PlatformClockAbandonment::Dropped
            ),
        ]
    );
}
