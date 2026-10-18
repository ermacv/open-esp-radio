use std::{cell::Cell, rc::Rc, vec::Vec};

use super::*;

/// A MAC whose two timers fire once per start, `threshold` microseconds of
/// the shared clock after it, and whose latched bits either absorb a second
/// arrival or keep it pending across the acknowledgement.
struct Fake {
    clock: Rc<Cell<u64>>,
    threshold: [u32; 2],
    started: [Option<u64>; 2],
    latched: [bool; 2],
    pending: [bool; 2],
    retains_second_arrival: bool,
}

impl Fake {
    fn new(clock: &Rc<Cell<u64>>, retains_second_arrival: bool) -> Self {
        Self {
            clock: clock.clone(),
            threshold: [0; 2],
            started: [None; 2],
            latched: [false; 2],
            pending: [false; 2],
            retains_second_arrival,
        }
    }

    fn index(timer: Ieee802154Timer) -> usize {
        match timer {
            Ieee802154Timer::Timer0 => 0,
            Ieee802154Timer::Timer1 => 1,
        }
    }

    fn update(&mut self) {
        let now = self.clock.get();
        for index in 0..2 {
            if let Some(start) = self.started[index]
                && now >= start + u64::from(self.threshold[index])
            {
                self.started[index] = None;
                if self.latched[index] {
                    self.pending[index] = self.retains_second_arrival;
                }
                self.latched[index] = true;
            }
        }
    }
}

const EVENTS: [Ieee802154Event; 2] = [
    Ieee802154Event::Timer0Overflow,
    Ieee802154Event::Timer1Overflow,
];

impl Ieee802154RouteProbeRegisters for Fake {
    fn events(&mut self) -> Ieee802154EventObservation {
        self.update();
        let mut mask = Ieee802154EventMask::NONE;
        for (event, latched) in EVENTS.into_iter().zip(self.latched) {
            if latched {
                mask = mask.with(event);
            }
        }
        Ieee802154EventObservation::from_named(mask)
    }

    fn clear_events(&mut self, mask: Ieee802154EventMask) {
        self.update();
        for ((event, latched), pending) in EVENTS
            .into_iter()
            .zip(&mut self.latched)
            .zip(&mut self.pending)
        {
            if mask.contains(event) {
                *latched = core::mem::take(pending);
            }
        }
    }

    fn set_timer_threshold(&mut self, timer: Ieee802154Timer, microseconds: u32) {
        self.threshold[Self::index(timer)] = microseconds;
    }

    fn start_timer(&mut self, timer: Ieee802154Timer) {
        self.started[Self::index(timer)] = Some(self.clock.get());
    }

    fn stop_timer(&mut self, timer: Ieee802154Timer) {
        self.update();
        self.started[Self::index(timer)] = None;
    }
}

fn clock() -> (Rc<Cell<u64>>, impl FnMut() -> u64) {
    let clock = Rc::new(Cell::new(0));
    let ticking = clock.clone();
    (clock, move || {
        ticking.set(ticking.get() + 1);
        ticking.get()
    })
}

const CONFIG: Ieee802154RouteProbeConfig = match Ieee802154RouteProbeConfig::new(100, 1_000) {
    Some(config) => config,
    None => panic!("valid"),
};

#[test]
fn the_polled_phase_tells_coalescing_from_retention() {
    for (retains, outcome) in [
        (false, Ieee802154SameBitOutcome::Coalesced),
        (true, Ieee802154SameBitOutcome::Retained),
    ] {
        let (shared, mut now) = clock();
        let mut fake = Fake::new(&shared, retains);
        let (phase, result) = run_polled_same_bit(&mut fake, CONFIG, &mut now);
        result.unwrap();
        assert_eq!(phase.snapshot, Ieee802154TimerEvents::Timer0);
        assert_eq!(phase.outcome, outcome);
        assert_eq!(phase.control_rearmed, Ieee802154TimerEvents::Timer0);
        assert!(fake.events().is_clear());
    }
}

/// A MAC whose timers never fire.
struct Silent(Fake);

impl Ieee802154RouteProbeRegisters for Silent {
    fn events(&mut self) -> Ieee802154EventObservation {
        self.0.events()
    }

    fn clear_events(&mut self, mask: Ieee802154EventMask) {
        self.0.clear_events(mask);
    }

    fn set_timer_threshold(&mut self, _: Ieee802154Timer, _: u32) {}

    fn start_timer(&mut self, _: Ieee802154Timer) {}

    fn stop_timer(&mut self, _: Ieee802154Timer) {}
}

#[test]
fn a_dirty_field_or_a_silent_timer_stops_the_polled_phase() {
    let (shared, mut now) = clock();
    let mut dirty = Fake::new(&shared, false);
    dirty.latched[1] = true;
    assert_eq!(
        run_polled_same_bit(&mut dirty, CONFIG, &mut now).1,
        Err(Ieee802154RouteProbeStop::NotClear)
    );
    assert_eq!(
        run_polled_same_bit(&mut Silent(Fake::new(&shared, false)), CONFIG, &mut now).1,
        Err(Ieee802154RouteProbeStop::LatchTimeout)
    );
}

/// Deliver the level line: enter the ISR while any event is latched, with
/// `actions` for the successive entries; returns the entries.
fn deliver(
    fake: &mut Fake,
    now: &mut impl FnMut() -> u64,
    actions: &[Ieee802154RouteProbeAction],
) -> Vec<Ieee802154RouteProbeEntry> {
    let mut entries = Vec::new();
    for _ in 0..10_000 {
        now();
        if !fake.events().is_clear() {
            let action = actions
                .get(entries.len())
                .copied()
                .unwrap_or(Ieee802154RouteProbeAction::Acknowledge);
            entries.push(route_probe_entry(fake, action, CONFIG, now));
        }
    }
    entries
}

#[test]
fn a_different_event_after_the_snapshot_retriggers_the_line() {
    let (shared, mut now) = clock();
    let mut fake = Fake::new(&shared, false);
    start_route_probe_phase(&mut fake, CONFIG).unwrap();
    let entries = deliver(
        &mut fake,
        &mut now,
        &[Ieee802154RouteProbeAction::RaiseTimer1],
    );
    assert_eq!(
        entries,
        [
            Ieee802154RouteProbeEntry {
                snapshot: Ieee802154TimerEvents::Timer0,
                before_acknowledgement: Ieee802154TimerEvents::Both,
            },
            Ieee802154RouteProbeEntry {
                snapshot: Ieee802154TimerEvents::Timer1,
                before_acknowledgement: Ieee802154TimerEvents::Timer1,
            },
        ]
    );
    assert_eq!(
        finish_route_probe(&mut fake),
        (
            Ieee802154TimerEvents::Clear,
            Ieee802154RouteProbeStop::Complete
        )
    );
}

#[test]
fn a_routed_same_bit_arrival_enters_once_or_twice_as_the_field_keeps_it() {
    for (retains, count) in [(false, 1), (true, 2)] {
        let (shared, mut now) = clock();
        let mut fake = Fake::new(&shared, retains);
        start_route_probe_phase(&mut fake, CONFIG).unwrap();
        let entries = deliver(
            &mut fake,
            &mut now,
            &[Ieee802154RouteProbeAction::RefireTimer0],
        );
        assert_eq!(entries.len(), count, "retains={retains}");
        assert_eq!(entries[0].snapshot, Ieee802154TimerEvents::Timer0);
        assert_eq!(
            finish_route_probe(&mut fake).1,
            Ieee802154RouteProbeStop::Complete
        );
    }
    // Whatever stays latched is reported and consumed.
    let (shared, _) = clock();
    let mut fake = Fake::new(&shared, false);
    fake.latched[0] = true;
    assert_eq!(
        finish_route_probe(&mut fake),
        (
            Ieee802154TimerEvents::Timer0,
            Ieee802154RouteProbeStop::NotClear
        )
    );
    assert!(fake.events().is_clear());
}

#[test]
fn the_configuration_bounds_the_waits() {
    assert!(Ieee802154RouteProbeConfig::new(0, 1_000).is_none());
    assert!(Ieee802154RouteProbeConfig::new(100, 399).is_none());
    assert!(Ieee802154RouteProbeConfig::new(10_001, 100_000).is_none());
    assert!(Ieee802154RouteProbeConfig::new(100, 100_001).is_none());
    assert!(Ieee802154RouteProbeConfig::new(100, 400).is_some());
}
