//! Counting platform clock provider for host tests.
//!
//! A guard releases through its backend, so the counts and the optional
//! event sink live in thread-local state; each test runs on its own thread.
//! Creating a provider resets that state.

use core::cell::{Cell, RefCell};
use std::boxed::Box;

use super::{
    PlatformClock, PlatformClockAbandonment, PlatformClockBackend, PlatformClockError,
    PlatformClockGuard, PlatformClockHolds, PlatformClockProvider,
};

/// One platform clock edge the provider observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClockEvent {
    Acquire(PlatformClock),
    Release(PlatformClock),
    Abandon(PlatformClock, PlatformClockAbandonment),
}

type Sink = Box<dyn Fn(ClockEvent)>;

std::thread_local! {
    static HELD: Cell<PlatformClockHolds> = Cell::new(PlatformClockHolds::default());
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
    static ABANDONED: Cell<PlatformClockHolds> = Cell::new(PlatformClockHolds::default());
    static DROPPED: Cell<PlatformClockHolds> = Cell::new(PlatformClockHolds::default());
}

fn record(event: ClockEvent) {
    SINK.with_borrow(|sink| {
        if let Some(sink) = sink {
            sink(event);
        }
    });
}

/// The thread-local reference counts used by test providers and guards.
pub(crate) struct CountingBackend;

impl PlatformClockBackend for CountingBackend {
    fn acquire(clock: PlatformClock) {
        HELD.with(|held| {
            let mut holds = held.get();
            holds.set(clock, holds.get(clock) + 1);
            held.set(holds);
        });
        record(ClockEvent::Acquire(clock));
    }
    fn release(clock: PlatformClock) {
        HELD.with(|held| {
            let mut holds = held.get();
            let count = holds.get(clock);
            assert!(count > 0, "{clock:?} released without a reference");
            holds.set(clock, count - 1);
            held.set(holds);
        });
        record(ClockEvent::Release(clock));
    }

    fn abandon(clock: PlatformClock, how: PlatformClockAbandonment) {
        let counts = match how {
            PlatformClockAbandonment::Abandoned => &ABANDONED,
            PlatformClockAbandonment::Dropped => &DROPPED,
        };
        counts.with(|counts| {
            let mut holds = counts.get();
            holds.set(clock, holds.get(clock) + 1);
            counts.set(holds);
        });
        record(ClockEvent::Abandon(clock, how));
    }
}

/// The references this thread's owners abandoned deliberately, and those
/// whose guards were dropped; both stay counted in [`held`].
pub(crate) fn abandoned() -> (PlatformClockHolds, PlatformClockHolds) {
    (ABANDONED.with(Cell::get), DROPPED.with(Cell::get))
}

/// The references this thread's guards hold.
pub(crate) fn held() -> PlatformClockHolds {
    HELD.with(Cell::get)
}

/// A platform that grants every clock not marked refused and counts the
/// references its guards hold.
pub(crate) struct CountingPlatformClocks {
    refused: Cell<[bool; PlatformClock::COUNT]>,
}

impl CountingPlatformClocks {
    /// A granting platform with no event sink.
    pub(crate) fn new() -> Self {
        HELD.with(|held| held.set(PlatformClockHolds::default()));
        ABANDONED.with(|counts| counts.set(PlatformClockHolds::default()));
        DROPPED.with(|counts| counts.set(PlatformClockHolds::default()));
        SINK.with_borrow_mut(|sink| *sink = None);
        Self {
            refused: Cell::new([false; PlatformClock::COUNT]),
        }
    }

    /// A granting platform that reports every edge to `sink`.
    pub(crate) fn logging(sink: impl Fn(ClockEvent) + 'static) -> Self {
        let platform = Self::new();
        SINK.with_borrow_mut(|slot| *slot = Some(Box::new(sink)));
        platform
    }

    /// Refuse or grant later requests for `clock`.
    pub(crate) fn set_refused(&self, clock: PlatformClock, refused: bool) {
        let mut all = self.refused.get();
        all[clock.index()] = refused;
        self.refused.set(all);
    }
}

impl PlatformClockProvider for CountingPlatformClocks {
    type Backend = CountingBackend;

    fn acquire(
        &self,
        clock: PlatformClock,
    ) -> Result<PlatformClockGuard<Self::Backend>, PlatformClockError> {
        if self.refused.get()[clock.index()] {
            return Err(PlatformClockError);
        }
        CountingBackend::acquire(clock);
        Ok(PlatformClockGuard::new(clock))
    }
}

/// The references `expected` lists, each once.
pub(crate) fn holding(expected: &[PlatformClock]) -> PlatformClockHolds {
    let mut holds = PlatformClockHolds::default();
    for &clock in expected {
        holds.set(clock, holds.get(clock) + 1);
    }
    holds
}
