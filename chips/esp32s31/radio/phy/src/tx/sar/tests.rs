use super::*;
use crate::executor::test_clock::VirtualClock;
use core::{
    pin::pin,
    task::{Context, Poll, Waker},
};
use oer_portable_clock::Clock;
use oer_portable_time::Instant;
use std::{
    cell::{Cell, RefCell},
    vec::Vec,
};

std::thread_local! {
    /// What happened, in order: the hardware's edges and the settles.
    static EVENTS: RefCell<Vec<(&'static str, u64)>> = const { RefCell::new(Vec::new()) };
}

fn record(name: &'static str, at: Instant) {
    EVENTS.with(|events| events.borrow_mut().push((name, at.as_micros())));
}

fn events() -> Vec<(&'static str, u64)> {
    EVENTS.with(|events| events.borrow().clone())
}

/// Short settles complete at once and say how long they were.
struct Short;

impl PhyShortDelay for Short {
    const MAX_MICROS: u32 = 20;
    fn settle<K: SettleCounter + ?Sized>(_: &K, limit: oer_portable_settle::SettleLimit) {
        let name = match limit.micros() {
            1 => "settle-1",
            2 => "settle-2",
            _ => "settle-other",
        };
        record(name, Instant::EPOCH);
    }
}

struct Hardware<'a> {
    clock: &'a VirtualClock,
    ready_at: Option<Instant>,
    /// A handler runs right after the first ready read, for this long.
    handler_after_ready: Cell<u64>,
}

impl Io for Hardware<'_> {
    fn arm_tone(&mut self) {
        record("arm", self.clock.now());
    }
    fn trigger(&mut self) {
        record("trigger", self.clock.now());
    }
    fn ready(&mut self) -> bool {
        record("ready", self.clock.now());
        let ready = self.ready_at.is_some_and(|at| self.clock.now() >= at);
        self.clock
            .advance(Duration::from_micros(self.handler_after_ready.take()))
            .unwrap();
        ready
    }
    fn clear_tone(&mut self) {
        record("clear", self.clock.now());
    }
    fn sample(&mut self) -> u16 {
        record("sample", self.clock.now());
        73
    }
}

fn hardware(clock: &VirtualClock, ready_at: Option<u64>) -> Hardware<'_> {
    EVENTS.with(|events| events.borrow_mut().clear());
    Hardware {
        clock,
        ready_at: ready_at.map(Instant::from_micros),
        handler_after_ready: Cell::new(0),
    }
}

/// Drive `convert` to its end, the clock moved by `step` each time it waits;
/// the result and the polls it was pending.
fn run(
    clock: &VirtualClock,
    io: &mut Hardware<'_>,
    clear_tone: bool,
    step: u64,
) -> (Result<Sample, Error>, u32) {
    let mut conversion = pin!(convert::<Short, _>(io, clock, clear_tone));
    let mut context = Context::from_waker(Waker::noop());
    let mut pending = 0;
    loop {
        match conversion.as_mut().poll(&mut context) {
            Poll::Ready(result) => return (result, pending),
            Poll::Pending => {
                pending += 1;
                clock.advance(Duration::from_micros(step)).unwrap();
            }
        }
    }
}

#[test]
fn the_vendor_order_with_named_settles_and_no_wait_when_ready_at_once() {
    let clock = VirtualClock::new();
    let mut io = hardware(&clock, Some(0));
    let (result, pending) = run(&clock, &mut io, true, 1);
    assert_eq!(
        result,
        Ok(Sample {
            value: 73,
            readiness: Readiness {
                elapsed_micros: 0,
                observations: 1,
                ready: true,
            }
        })
    );
    assert_eq!(pending, 0, "a ready first read never gives the CPU up");
    assert_eq!(
        events().iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        [
            "arm", "settle-1", "trigger", "settle-2", "ready", "clear", "sample"
        ]
    );
}

#[test]
fn not_ready_waits_a_microsecond_on_the_timer_and_stops_at_the_bound() {
    let clock = VirtualClock::new();
    let mut io = hardware(&clock, None);
    let (result, pending) = run(&clock, &mut io, true, 1);
    assert_eq!(
        result,
        Err(Error::NotReady(Readiness {
            elapsed_micros: 40,
            observations: 41,
            ready: false
        }))
    );
    assert_eq!(pending, 40, "each unready read before the bound yields");
    assert!(
        !events()
            .iter()
            .any(|(name, _)| *name == "sample" || *name == "clear")
    );
}

#[test]
fn ready_exactly_at_the_bound_succeeds() {
    let clock = VirtualClock::new();
    let mut io = hardware(&clock, Some(40));
    let (result, _) = run(&clock, &mut io, false, 1);
    assert_eq!(
        result.map(|sample| sample.readiness),
        Ok(Readiness {
            elapsed_micros: 40,
            observations: 41,
            ready: true
        })
    );
}

#[test]
fn a_late_wake_reads_a_finished_conversion_and_an_unfinished_one_reads_no_result() {
    for ready in [true, false] {
        let clock = VirtualClock::new();
        let mut io = hardware(&clock, ready.then_some(50));
        let (result, pending) = run(&clock, &mut io, false, 100);
        let readiness = Readiness {
            elapsed_micros: 100,
            observations: 2,
            ready,
        };
        let expected = if ready {
            Ok(Sample {
                value: 73,
                readiness,
            })
        } else {
            Err(Error::NotReady(readiness))
        };
        assert_eq!(result, expected);
        assert_eq!(pending, 1);
        assert_eq!(
            events().iter().any(|(name, _)| *name == "sample"),
            ready,
            "only a ready conversion's result is read"
        );
    }
}

#[test]
fn a_bound_past_the_timer_range_issues_no_edge() {
    let clock = VirtualClock::starting_at(Instant::from_micros(u64::MAX - 10));
    let mut io = hardware(&clock, Some(0));
    let (result, _) = run(&clock, &mut io, true, 1);
    assert_eq!(result, Err(Error::DeadlineOverflow));
    assert!(events().is_empty());
}

#[test]
fn a_handler_after_a_ready_read_does_not_expire_a_finished_conversion() {
    let clock = VirtualClock::new();
    let mut io = hardware(&clock, Some(10));
    io.handler_after_ready.set(100);
    let (result, _) = run(&clock, &mut io, false, 1);
    assert!(
        matches!(result, Ok(Sample { value: 73, .. })),
        "the next read finds the conversion finished: {result:?}"
    );
}

/// A tally that keeps every wait it is told of.
#[derive(Default)]
struct Kept(RefCell<Vec<(crate::executor::wait::Waited, u64, u64)>>);

impl WaitTally for Kept {
    const COUNTS: bool = true;
    fn waited(&self, kind: crate::executor::wait::Waited, asked: u64, taken: u64) {
        self.0.borrow_mut().push((kind, asked, taken));
    }
}

#[test]
fn a_counting_timer_sees_the_two_settles_spun_and_the_readiness_waits() {
    use crate::executor::wait::{PhyTimer, Waited};
    let clock = VirtualClock::new();
    let kept = Kept::default();
    let timer = PhyTimer::counted(&clock, &clock, &kept);
    let mut io = hardware(&clock, Some(2));
    let mut conversion = pin!(convert::<Short, _>(&mut io, &timer, false));
    let mut context = Context::from_waker(Waker::noop());
    while conversion.as_mut().poll(&mut context).is_pending() {
        clock.advance(Duration::from_micros(1)).unwrap();
    }
    assert_eq!(
        *kept.0.borrow(),
        [
            (Waited::SettleSpun, 1, 0),
            (Waited::SettleSpun, 2, 0),
            (Waited::Completion, 1, 1),
            (Waited::Completion, 1, 1),
        ]
    );
}
