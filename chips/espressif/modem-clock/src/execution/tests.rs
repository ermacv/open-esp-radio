#![allow(clippy::disallowed_macros)]

use super::*;
use crate::{DependencyIndex, DependencySet, ModemClockPlannerIdentity};
use oer_time::{Clock, Duration, Instant};
use oer_time_virtual::VirtualClock;
use std::vec::Vec;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Dependency {
    Source,
    Analog,
    Gate,
}
impl ModemClockDependency for Dependency {
    const LOW_BIT_FIRST: &'static [Self] = &[Self::Source, Self::Analog, Self::Gate];
    fn index(self) -> DependencyIndex {
        match self {
            Self::Source => DependencyIndex::constant::<0>(),
            Self::Analog => DependencyIndex::constant::<1>(),
            Self::Gate => DependencyIndex::constant::<2>(),
        }
    }
    fn refcounted(self) -> bool {
        true
    }
    fn retained_while_wifi_initialized(self) -> bool {
        false
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Event {
    Prepare,
    Acquire(u8),
    Device(u8, bool),
    Release(u8),
}
#[derive(Debug)]
struct Refused;
#[derive(Default)]
struct Port {
    events: Vec<(Instant, Event)>,
    time: VirtualClock,
    refuse: Option<u8>,
    held: [bool; 2],
}
impl Port {
    fn record(&mut self, event: Event) {
        self.events.push((self.time.now(), event));
        self.time.advance(Duration::from_micros(1)).unwrap();
    }
    fn events(&self) -> Vec<Event> {
        self.events.iter().map(|(_, e)| *e).collect()
    }
}
impl RegisterPort<Dependency> for Port {
    type Device = u8;
    type Clock = u8;
    type Error = Refused;
    fn action(d: Dependency) -> Action<u8, u8> {
        match d {
            Dependency::Source => Action::Source {
                clock: 0,
                device: 0,
            },
            Dependency::Analog => Action::Platform(1),
            Dependency::Gate => Action::Device(1),
        }
    }
    fn configure_device(&mut self, device: u8, on: bool) {
        self.record(Event::Device(device, on));
    }
    fn release_clock(&mut self, clock: u8) -> Result<(), Refused> {
        self.record(Event::Release(clock));
        if self.refuse == Some(clock) {
            return Err(Refused);
        }
        self.held[clock as usize] = false;
        Ok(())
    }
}
impl AcquirePort<Dependency> for Port {
    fn prepare_power_state(&mut self) {
        self.record(Event::Prepare);
    }
    fn acquire_clock(&mut self, clock: u8) -> Result<(), Refused> {
        self.record(Event::Acquire(clock));
        if self.refuse == Some(clock) {
            return Err(Refused);
        }
        self.held[clock as usize] = true;
        Ok(())
    }
}
fn prepared() -> PreparedModemClockAcquire<Dependency> {
    ModemClockPlanner::managed(ModemClockPlannerIdentity::new().unwrap())
        .prepare_acquire(DependencySet::of_iter(
            Dependency::LOW_BIT_FIRST.iter().copied(),
        ))
        .unwrap()
}
#[test]
fn upstream_brackets_device_and_both_directions_commit_in_order() {
    let mut port = Port::default();
    let begin = port.time.now();
    let (planner, lease) = acquire(prepared(), &mut port).unwrap();
    assert_eq!(
        port.events(),
        [
            Event::Prepare,
            Event::Acquire(0),
            Event::Device(0, true),
            Event::Acquire(1),
            Event::Device(1, true)
        ]
    );
    assert_eq!(port.held, [true, true]);
    port.events.clear();
    let _planner = release(planner.prepare_release(lease).unwrap(), &mut port).unwrap();
    assert_eq!(
        port.events(),
        [
            Event::Device(0, false),
            Event::Release(0),
            Event::Release(1),
            Event::Device(1, false)
        ]
    );
    assert_eq!(port.held, [false, false]);
    assert_eq!(
        port.time.now().checked_duration_since(begin),
        Some(Duration::from_micros(9))
    );
}
#[test]
fn acquire_refusal_poison_keeps_completed_sources_and_skips_the_suffix() {
    for (clock, dependency, held) in [
        (0, Dependency::Source, [false, false]),
        (1, Dependency::Analog, [true, false]),
    ] {
        let mut port = Port {
            refuse: Some(clock),
            ..Port::default()
        };
        let Err(poisoned) = acquire(prepared(), &mut port) else {
            panic!("refused acquisition");
        };
        assert_eq!(poisoned.dependency(), dependency);
        assert_eq!(
            port.events.last().map(|(_, e)| *e),
            Some(Event::Acquire(clock))
        );
        assert_eq!(port.held, held);
        assert!(!port.events().contains(&Event::Device(1, true)));
    }
}
#[test]
fn release_refusal_closes_device_once_and_keeps_source_with_poison() {
    let mut port = Port::default();
    let (planner, lease) = acquire(prepared(), &mut port).unwrap();
    port.events.clear();
    port.refuse = Some(0);
    let Err(poisoned) = release(planner.prepare_release(lease).unwrap(), &mut port) else {
        panic!("refused release");
    };
    assert_eq!(poisoned.dependency(), Dependency::Source);
    assert_eq!(port.events(), [Event::Device(0, false), Event::Release(0)]);
    assert_eq!(port.held, [true, true]);
}
