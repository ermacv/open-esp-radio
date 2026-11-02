#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
use super::{domain::*, membership::*};
use oer_radio_coex::RadioClient::*;
use oer_time::{Clock as _, Duration};
use oer_time_virtual::VirtualClock;
use std::vec::Vec;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Event {
    Select,
    Reset,
    Power,
    Release(Clock),
    Restore(u8),
}
struct Power {
    time: VirtualClock,
    log: Vec<(u64, Event)>,
    baseline: u8,
    power_fails: bool,
    restore_fails: bool,
    release_fails: Option<Clock>,
    refs: [bool; 3],
}
impl Power {
    fn new() -> Self {
        Self {
            time: VirtualClock::starting_at(oer_time::Instant::from_micros(0)),
            log: Vec::new(),
            baseline: 7,
            power_fails: false,
            restore_fails: false,
            release_fails: None,
            refs: [false; 3],
        }
    }
    fn record(&mut self, event: Event) {
        self.log.push((self.time.now().as_micros(), event));
        self.time.advance(Duration::from_micros(1)).unwrap();
    }
    fn events(&self) -> Vec<Event> {
        self.log.iter().map(|(_, e)| *e).collect()
    }
}
impl PowerPort for Power {
    type Baseline = u8;
    type RestoreError = u8;
    fn capture(&self) -> u8 {
        self.baseline
    }
    fn restore(&mut self, baseline: u8) -> Result<(), u8> {
        self.record(Event::Restore(baseline));
        if self.restore_fails { Err(9) } else { Ok(()) }
    }
    fn select_wifi_low_power_clock(&mut self) {
        self.record(Event::Select);
    }
    fn release(&mut self, clock: Clock) -> Result<(), ReleaseRefused> {
        if self.release_fails == Some(clock) {
            return Err(ReleaseRefused);
        }
        let index = match clock {
            Clock::AnalogI2c => 0,
            Clock::Pll160m => 1,
            Clock::Mpll => 2,
        };
        if self.refs[index] {
            self.record(Event::Release(clock));
            self.refs[index] = false;
        }
        Ok(())
    }
}
impl PowerEntryPort for Power {
    fn power_up(&mut self) -> Result<(), crate::power::PowerError> {
        self.record(Event::Power);
        self.refs[1] = true;
        self.refs[2] = true;
        if self.power_fails {
            return Err(crate::power::PowerError {
                checkpoint: crate::power::PowerCheckpoint::ModemClockSource,
                expected: true,
                observed: false,
            });
        }
        self.refs[0] = true;
        Ok(())
    }
}
impl WifiPowerPort for Power {
    fn reset_wifi(&mut self) {
        self.record(Event::Reset);
    }
}

#[test]
fn failed_entry_retries_original_baseline_and_commits_reset_only_on_success() {
    let mut port = Power::new();
    let mut state = PowerMembership::default();
    port.power_fails = true;
    assert!(matches!(
        state.enter_wifi(&mut port),
        Err(PowerError::Power(_))
    ));
    assert!(!state.any_client());
    port.baseline = 88;
    port.power_fails = false;
    state.enter_wifi(&mut port).unwrap();
    let before = port.log.clone();
    assert_eq!(state.enter_wifi(&mut port), Err(PowerError::AlreadyEntered));
    assert_eq!(port.log, before);
    state.enter(Bluetooth, &mut port).unwrap();
    state.exit(Wifi, &mut port).unwrap();
    assert!(state.holds(Bluetooth));
    state.exit(Bluetooth, &mut port).unwrap();
    assert_eq!(
        port.events(),
        [
            Event::Select,
            Event::Reset,
            Event::Power,
            Event::Select,
            Event::Reset,
            Event::Power,
            Event::Release(Clock::AnalogI2c),
            Event::Restore(7),
            Event::Release(Clock::Pll160m),
            Event::Release(Clock::Mpll)
        ]
    );
    assert_eq!(port.time.now().as_micros(), 10);
    state.enter_wifi(&mut port).unwrap();
    assert_eq!(port.events().last(), Some(&Event::Power));
    assert_eq!(
        port.events().iter().filter(|e| **e == Event::Reset).count(),
        2
    );
}
#[test]
fn last_leave_retains_membership_baseline_and_only_unreleased_references() {
    let mut port = Power::new();
    let mut state = PowerMembership::default();
    state.enter(Ieee802154, &mut port).unwrap();
    port.restore_fails = true;
    assert_eq!(
        state.exit(Ieee802154, &mut port),
        Err(PowerError::Restore(9))
    );
    assert!(state.holds(Ieee802154));
    assert_eq!(port.refs, [false, true, true]);
    port.baseline = 88;
    port.restore_fails = false;
    port.release_fails = Some(Clock::Pll160m);
    assert_eq!(
        state.exit(Ieee802154, &mut port),
        Err(PowerError::Release(Clock::Pll160m))
    );
    assert!(state.holds(Ieee802154));
    port.release_fails = None;
    state.exit(Ieee802154, &mut port).unwrap();
    assert!(!state.any_client());
    assert_eq!(port.refs, [false; 3]);
    assert_eq!(
        port.events()
            .iter()
            .filter(|e| **e == Event::Restore(7))
            .count(),
        2
    );
    assert_eq!(
        port.events()
            .iter()
            .filter(|e| **e == Event::Release(Clock::AnalogI2c))
            .count(),
        1
    );
    state.enter(Ieee802154, &mut port).unwrap();
    state.exit(Ieee802154, &mut port).unwrap();
    assert!(port.events().contains(&Event::Restore(88)));
}

struct Baseband(Vec<u8>);
impl BtbbPort for Baseband {
    fn initialize(&mut self, gain: u8) {
        self.0.push(gain);
    }
}
#[test]
fn baseband_gain_checks_precede_first_init_and_later_join_has_no_effects() {
    let mut state = BtbbMembership::default();
    let mut port = Baseband(Vec::new());
    assert_eq!(state.publish(None, 1, 4), Err(BtbbError::Unregistered));
    assert_eq!(state.publish(Some(2), 1, 4), Err(BtbbError::Stale));
    let old = state.publish(Some(1), 1, 4).unwrap();
    let gain = state.publish(Some(1), 1, 5).unwrap();
    assert_eq!(
        state.acquire(Bluetooth, Some(1), &old, &mut port),
        Err(BtbbError::Stale)
    );
    assert_eq!(
        state.acquire(Bluetooth, Some(2), &gain, &mut port),
        Err(BtbbError::Stale)
    );
    assert_eq!(
        state.acquire(Bluetooth, None, &gain, &mut port),
        Err(BtbbError::Unregistered)
    );
    assert!(port.0.is_empty());
    assert_eq!(
        state.acquire(Bluetooth, Some(1), &gain, &mut port),
        Ok(BtbbAcquired::Initialized)
    );
    assert_eq!(
        state.acquire(Ieee802154, Some(1), &gain, &mut port),
        Ok(BtbbAcquired::Joined)
    );
    assert_eq!(
        state.acquire(Bluetooth, Some(1), &gain, &mut port),
        Err(BtbbError::AlreadyAcquired)
    );
    assert_eq!(port.0, [5]);
    state.release(Bluetooth).unwrap();
    state.release(Ieee802154).unwrap();
    assert_eq!(state.release(Bluetooth), Err(BtbbError::NotAcquired));
    assert!(!state.any_held());
}
struct LowPower {
    log: Vec<u8>,
    fail_acquire: bool,
    fail_release: bool,
}
impl LowPowerPort for LowPower {
    type Guard = u32;
    type Error = u8;
    fn deselect(&mut self) {
        self.log.push(3);
    }
    fn release(&mut self, guard: u32) -> Result<(), (u32, u8)> {
        self.log.push(4);
        if self.fail_release {
            Err((guard, 8))
        } else {
            Ok(())
        }
    }
}
impl LowPowerAcquirePort for LowPower {
    fn select(&mut self) {
        self.log.push(1);
    }
    fn acquire(&mut self) -> Result<u32, u8> {
        self.log.push(2);
        if self.fail_acquire { Err(9) } else { Ok(17) }
    }
}
#[test]
fn low_power_refusals_preserve_selector_order_and_the_exact_guard() {
    let mut held = None;
    let mut port = LowPower {
        log: Vec::new(),
        fail_acquire: true,
        fail_release: false,
    };
    assert_eq!(
        deselect_low_power(&mut held, &mut port),
        Err(LowPowerError::NotSelected)
    );
    assert_eq!(
        select_low_power(&mut held, &mut port),
        Err(LowPowerError::Platform(9))
    );
    assert_eq!(held, None);
    port.fail_acquire = false;
    select_low_power(&mut held, &mut port).unwrap();
    assert_eq!(
        select_low_power(&mut held, &mut port),
        Err(LowPowerError::AlreadySelected)
    );
    port.fail_release = true;
    assert_eq!(
        deselect_low_power(&mut held, &mut port),
        Err(LowPowerError::Platform(8))
    );
    assert_eq!(held, Some(17));
    port.fail_release = false;
    deselect_low_power(&mut held, &mut port).unwrap();
    assert_eq!(held, None);
    assert_eq!(port.log, [1, 2, 1, 2, 3, 4, 3, 4]);
}
#[test]
fn coexistence_membership_counts_external_and_each_client_independently() {
    let mut set = Participants::default();
    assert_eq!(set.enter(Wifi), 1);
    assert_eq!(set.enter(Bluetooth), 2);
    assert_eq!(set.enter_external(), 3);
    assert_eq!(set.leave(Wifi), 2);
    assert_eq!(set.leave_external(), 1);
    assert_eq!(set.leave(Bluetooth), 0);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExtError {
    Active,
    Delay,
    Clock,
}
struct External {
    time: VirtualClock,
    log: Vec<(u64, u8)>,
    fail_enable: bool,
    fail_disable: bool,
}
impl External {
    fn record(&mut self, event: u8) {
        self.log.push((self.time.now().as_micros(), event));
        self.time.advance(Duration::from_micros(1)).unwrap();
    }
}
impl ExternalPort<u8> for External {
    type Error = ExtError;
    fn enabled(&mut self, enabled: bool) {
        self.record(if enabled { 5 } else { 7 });
    }
    fn clear_priorities(&mut self, _config: u8) {
        self.record(6);
    }
    fn disable_clock(&mut self) -> Result<(), ExtError> {
        self.record(8);
        if self.fail_disable {
            Err(ExtError::Clock)
        } else {
            Ok(())
        }
    }
}
impl ExternalStartPort<u8> for External {
    type Delay = u8;
    fn already_active() -> ExtError {
        ExtError::Active
    }
    fn grant_delay(config: u8) -> Result<u8, ExtError> {
        if config > 15 {
            Err(ExtError::Delay)
        } else {
            Ok(config)
        }
    }
    fn enable_clock(&mut self) -> Result<(), ExtError> {
        self.record(1);
        if self.fail_enable {
            Err(ExtError::Clock)
        } else {
            Ok(())
        }
    }
    fn mode(&mut self, _config: u8) {
        self.record(2);
    }
    fn grant(&mut self, config: u8, delay: u8) {
        assert_eq!(config, delay);
        self.record(3);
    }
    fn priorities(&mut self, _config: u8) {
        self.record(4);
    }
}
#[test]
fn external_coexistence_validates_before_clocks_and_stops_before_release_failure() {
    let mut held = None;
    let mut port = External {
        time: VirtualClock::starting_at(oer_time::Instant::from_micros(0)),
        log: Vec::new(),
        fail_enable: false,
        fail_disable: false,
    };
    assert_eq!(
        start_external(&mut held, 16, &mut port),
        Err(ExtError::Delay)
    );
    assert!(port.log.is_empty());
    port.fail_enable = true;
    assert_eq!(
        start_external(&mut held, 3, &mut port),
        Err(ExtError::Clock)
    );
    assert_eq!(held, None);
    assert_eq!(port.log, [(0, 1)]);
    port.fail_enable = false;
    start_external(&mut held, 3, &mut port).unwrap();
    assert_eq!(
        start_external(&mut held, 16, &mut port),
        Err(ExtError::Active)
    );
    port.fail_disable = true;
    assert_eq!(stop_external(&mut held, &mut port), Err(ExtError::Clock));
    assert_eq!(held, None);
    assert_eq!(
        port.log,
        [
            (0, 1),
            (1, 1),
            (2, 2),
            (3, 3),
            (4, 4),
            (5, 5),
            (6, 6),
            (7, 7),
            (8, 8)
        ]
    );
}

use oer_espressif_modem_clock as clocks;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Dep {
    Source,
    Device,
}
impl clocks::ModemClockDependency for Dep {
    const LOW_BIT_FIRST: &'static [Self] = &[Self::Source, Self::Device];
    fn index(self) -> clocks::DependencyIndex {
        match self {
            Self::Source => clocks::DependencyIndex::constant::<0>(),
            Self::Device => clocks::DependencyIndex::constant::<1>(),
        }
    }
    fn refcounted(self) -> bool {
        true
    }
    fn retained_while_wifi_initialized(self) -> bool {
        false
    }
}
struct Modem {
    log: Vec<u8>,
    acquire_fails: bool,
    release_fails: bool,
}
impl clocks::execution::RegisterPort<Dep> for Modem {
    type Device = Dep;
    type Clock = ();
    type Error = ();
    fn action(dependency: Dep) -> clocks::execution::Action<Dep, ()> {
        match dependency {
            Dep::Source => clocks::execution::Action::Platform(()),
            Dep::Device => clocks::execution::Action::Device(Dep::Device),
        }
    }
    fn configure_device(&mut self, _device: Dep, enabled: bool) {
        self.log.push(if enabled { 3 } else { 4 });
    }
    fn release_clock(&mut self, _clock: ()) -> Result<(), ()> {
        self.log.push(5);
        if self.release_fails { Err(()) } else { Ok(()) }
    }
}
impl clocks::execution::AcquirePort<Dep> for Modem {
    fn prepare_power_state(&mut self) {
        self.log.push(1);
    }
    fn acquire_clock(&mut self, _clock: ()) -> Result<(), ()> {
        self.log.push(2);
        if self.acquire_fails { Err(()) } else { Ok(()) }
    }
}
fn modem() -> (super::modem::ModemClocks<Dep, 2>, Modem) {
    (
        super::modem::ModemClocks::new(clocks::ModemClockPlannerIdentity::new().unwrap()),
        Modem {
            log: Vec::new(),
            acquire_fails: false,
            release_fails: false,
        },
    )
}
#[test]
fn module_slots_hold_shared_edges_and_reject_duplicate_or_missing_leases_without_writes() {
    use super::modem::Error;
    let (mut state, mut port) = modem();
    let deps = clocks::DependencySet::of(&[Dep::Source, Dep::Device]);
    assert_eq!(state.disable(0, &mut port), Err(Error::NotEnabled));
    assert_eq!(state.enable(2, deps, &mut port), Err(Error::Rejected));
    assert!(port.log.is_empty());
    state.enable(0, deps, &mut port).unwrap();
    assert_eq!(state.enable(0, deps, &mut port), Err(Error::AlreadyEnabled));
    state.enable(1, deps, &mut port).unwrap();
    state.disable(0, &mut port).unwrap();
    assert!(state.any_held());
    assert_eq!(port.log, [1, 2, 3, 1]);
    state.disable(1, &mut port).unwrap();
    assert_eq!(port.log, [1, 2, 3, 1, 5, 4]);
    assert!(!state.any_held());
}
#[test]
fn acquire_and_release_poison_keep_the_failed_owner_and_refuse_every_later_change() {
    use super::modem::{Error, ModemClocks};
    for release in [false, true] {
        let (mut state, mut port) = modem();
        let deps = clocks::DependencySet::of(&[Dep::Source, Dep::Device]);
        if release {
            state.enable(0, deps, &mut port).unwrap();
            port.release_fails = true;
            assert_eq!(state.disable(0, &mut port), Err(Error::Poisoned));
        } else {
            port.acquire_fails = true;
            assert_eq!(state.enable(0, deps, &mut port), Err(Error::Poisoned));
        }
        let before = port.log.clone();
        assert!(state.any_held());
        assert_eq!(state.enable(1, deps, &mut port), Err(Error::Poisoned));
        assert_eq!(state.disable(0, &mut port), Err(Error::Poisoned));
        assert_eq!(state.set_wifi_initialized(true), Err(Error::Poisoned));
        assert_eq!(port.log, before);
        match &state {
            ModemClocks::AcquirePoisoned { owner } if !release => {
                assert_eq!(owner.dependency(), Dep::Source);
                assert_eq!(owner.completed_edges(), 0);
            }
            ModemClocks::ReleasePoisoned { owner } if release => {
                assert_eq!(owner.dependency(), Dep::Source);
                assert_eq!(owner.completed_edges(), 0);
            }
            _ => panic!("the exact failed transaction must stay retained"),
        }
    }
}
