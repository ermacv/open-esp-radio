use super::*;
use oer_time::{Clock, Duration, Instant};
use oer_time_virtual::VirtualClock;
use std::vec::Vec;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Edge {
    ColdReadout,
    SarResetReleased(bool),
    SarPower(bool),
    SensorClock(bool),
    SensorReset(bool),
    SensorPower(bool),
}

struct Port<'a> {
    clock: &'a VirtualClock<4>,
    edges: Vec<(Instant, Edge)>,
}

impl<'a> Port<'a> {
    fn new(clock: &'a VirtualClock<4>) -> Self {
        Self {
            clock,
            edges: Vec::new(),
        }
    }
    fn record(&mut self, edge: Edge) {
        self.edges.push((self.clock.now(), edge));
    }
}

impl TemperaturePort for Port<'_> {
    fn cold_readout_prefix(&mut self) {
        self.record(Edge::ColdReadout);
    }
    fn sar_reset_released(&mut self, released: bool) {
        self.record(Edge::SarResetReleased(released));
    }
    fn sar_power(&mut self, enabled: bool) {
        self.record(Edge::SarPower(enabled));
    }
    fn sensor_clock(&mut self, enabled: bool) {
        self.record(Edge::SensorClock(enabled));
    }
    fn sensor_reset(&mut self, asserted: bool) {
        self.record(Edge::SensorReset(asserted));
    }
    fn sensor_power(&mut self, enabled: bool) {
        self.record(Edge::SensorPower(enabled));
    }
}

fn settle(clock: &VirtualClock<4>) -> bool {
    clock
        .advance(Duration::from_micros(u64::from(
            SAR_I2C_RESET_SETTLE_MICROS,
        )))
        .expect("test reset settle fits virtual time");
    true
}

fn expected_acquire(start: Instant) -> Vec<(Instant, Edge)> {
    let ready = start.saturating_add(Duration::from_micros(1));
    std::vec![
        (start, Edge::SarResetReleased(false)),
        (ready, Edge::SarPower(true)),
        (ready, Edge::SarResetReleased(true)),
        (ready, Edge::SensorClock(true)),
        (ready, Edge::SensorReset(true)),
        (ready, Edge::SensorReset(false)),
        (ready, Edge::SensorPower(true)),
    ]
}

#[test]
fn cold_prefix_precedes_the_linked_override_resource_acquire() {
    let clock = VirtualClock::new();
    let mut port = Port::new(&clock);
    let mut resources = TemperatureResources::new();
    let start = clock.now();
    assert_eq!(
        resources.acquire_with(&mut port, || settle(&clock), true),
        Ok(())
    );
    let mut expected = std::vec![(start, Edge::ColdReadout)];
    expected.extend(expected_acquire(start));
    assert_eq!(port.edges, expected);
    assert!(resources.blocks_release());
}

#[test]
fn close_releases_sensor_then_clock_then_the_last_sar_i2c_reference() {
    let clock = VirtualClock::new();
    let mut port = Port::new(&clock);
    let mut resources = TemperatureResources::new();
    resources
        .acquire_with(&mut port, || settle(&clock), false)
        .expect("first acquire");
    port.edges.clear();
    let close = clock.now();
    assert_eq!(resources.release_with(&mut port), Ok(()));
    assert_eq!(
        port.edges,
        [
            (close, Edge::SensorPower(false)),
            (close, Edge::SensorClock(false)),
            (close, Edge::SarPower(false)),
        ]
    );
    assert!(!resources.blocks_release());
}

#[test]
fn wake_reacquires_in_source_order_after_the_full_named_reset_settle() {
    let clock = VirtualClock::new();
    let mut port = Port::new(&clock);
    let mut resources = TemperatureResources::new();
    resources
        .acquire_with(&mut port, || settle(&clock), true)
        .expect("cold acquire");
    resources.release_with(&mut port).expect("RF close");
    port.edges.clear();
    let start = clock.now();
    resources
        .acquire_with(&mut port, || settle(&clock), false)
        .expect("RF wake");
    assert_eq!(port.edges, expected_acquire(start));
}

#[test]
fn repeated_close_and_wake_cycles_do_not_retain_an_unreleased_power_reference() {
    let clock = VirtualClock::new();
    let mut port = Port::new(&clock);
    let mut resources = TemperatureResources::new();
    for _ in 0..3 {
        let start = clock.now();
        resources
            .acquire_with(&mut port, || settle(&clock), false)
            .expect("cycle acquire");
        resources.release_with(&mut port).expect("cycle release");
        let mut expected = expected_acquire(start);
        expected.extend([
            (clock.now(), Edge::SensorPower(false)),
            (clock.now(), Edge::SensorClock(false)),
            (clock.now(), Edge::SarPower(false)),
        ]);
        assert_eq!(port.edges, expected);
        assert!(!resources.blocks_release());
        port.edges.clear();
    }
}

#[test]
fn failed_reset_settle_never_enables_power_and_prevents_reuse_or_root_release() {
    let clock = VirtualClock::new();
    let mut port = Port::new(&clock);
    let mut route = crate::phy::restore::PhyRouteState::for_host_test();
    let start = clock.now();
    assert_eq!(
        route.temperature.acquire_with(&mut port, || false, false),
        Err(TemperaturePowerError::ResetSettleFailed)
    );
    assert_eq!(port.edges, [(start, Edge::SarResetReleased(false))]);
    assert_eq!(
        route
            .temperature
            .acquire_with(&mut port, || settle(&clock), false),
        Err(TemperaturePowerError::Poisoned)
    );
    assert_eq!(
        route.temperature.release_with(&mut port),
        Err(TemperaturePowerError::Poisoned)
    );
    assert_eq!(
        crate::root::check_phy_restore_complete(&route),
        Err(crate::root::RadioPhyReleaseError::TemperatureResourcesHeld)
    );
    assert_eq!(port.edges, [(start, Edge::SarResetReleased(false))]);
    assert_eq!(clock.now(), start);
}

#[test]
fn duplicate_acquire_and_unowned_release_do_not_write_hardware() {
    let clock = VirtualClock::new();
    let mut port = Port::new(&clock);
    let mut resources = TemperatureResources::new();
    assert_eq!(
        resources.release_with(&mut port),
        Err(TemperaturePowerError::NotAcquired)
    );
    assert!(port.edges.is_empty());
    resources
        .acquire_with(&mut port, || settle(&clock), false)
        .expect("acquire");
    port.edges.clear();
    let now = clock.now();
    assert_eq!(
        resources.acquire_with(&mut port, || false, true),
        Err(TemperaturePowerError::AlreadyAcquired)
    );
    assert!(port.edges.is_empty());
    assert_eq!(clock.now(), now);
}

#[test]
fn root_release_requires_the_temperature_resources_to_have_been_released() {
    let clock = VirtualClock::new();
    let mut port = Port::new(&clock);
    let mut route = crate::phy::restore::PhyRouteState::for_host_test();
    route
        .temperature
        .acquire_with(&mut port, || settle(&clock), false)
        .expect("acquire");
    assert_eq!(
        crate::root::check_phy_restore_complete(&route),
        Err(crate::root::RadioPhyReleaseError::TemperatureResourcesHeld)
    );
    route.temperature.release_with(&mut port).expect("release");
    assert_eq!(crate::root::check_phy_restore_complete(&route), Ok(()));
}
