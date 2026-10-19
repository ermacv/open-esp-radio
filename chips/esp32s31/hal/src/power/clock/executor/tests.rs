use std::{cell::RefCell, rc::Rc, vec::Vec};

use super::*;
use crate::power::clock::{ModemClockModule, ModemClockPlannerIdentity};
use crate::power::test_clocks::{ClockEvent, CountingPlatformClocks, abandoned, held, holding};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Device(ModemClockDevice, bool),
    AcquirePll,
    ReleasePll,
    AcquireAnalogI2c,
    ReleaseAnalogI2c,
    Acquire(PlatformClock),
    Release(PlatformClock),
    Abandon(PlatformClock, PlatformClockAbandonment),
}

/// One ordered log shared by the modem port and the platform provider.
type Log = Rc<RefCell<Vec<Operation>>>;

struct Port(Log);

impl ModemClockPort for Port {
    fn configure_device(&mut self, device: ModemClockDevice, enable: bool) {
        self.0.borrow_mut().push(Operation::Device(device, enable));
    }
}

fn setup() -> (Log, Port, CountingPlatformClocks) {
    let log = Log::default();
    let sink = log.clone();
    let platform = CountingPlatformClocks::logging(move |event| {
        sink.borrow_mut().push(match event {
            ClockEvent::Acquire(PlatformClock::Pll160m) => Operation::AcquirePll,
            ClockEvent::Acquire(PlatformClock::AnalogI2cMaster) => Operation::AcquireAnalogI2c,
            ClockEvent::Acquire(other) => Operation::Acquire(other),
            ClockEvent::Release(PlatformClock::Pll160m) => Operation::ReleasePll,
            ClockEvent::Release(PlatformClock::AnalogI2cMaster) => Operation::ReleaseAnalogI2c,
            ClockEvent::Release(other) => Operation::Release(other),
            ClockEvent::Abandon(clock, how) => Operation::Abandon(clock, how),
        });
    });
    (log.clone(), Port(log), platform)
}

#[test]
fn the_pll_source_brackets_the_modem_gate_and_the_analog_clock_goes_to_the_platform() {
    let (log, mut port, platform) = setup();
    let mut held_clocks = ModemPlatformClocks::default();
    let identity = ModemClockPlannerIdentity::new();
    let planner = ModemClockPlanner::managed(&identity);
    let prepared = planner
        .prepare_acquire(ModemClockModule::Phy.dependencies())
        .unwrap_or_else(|_| panic!("PHY"));
    let (planner, lease) = execute_acquire(prepared, &mut port, &mut held_clocks, &platform)
        .unwrap_or_else(|_| panic!("acquire"));
    assert_eq!(
        held(),
        holding(&[PlatformClock::Pll160m, PlatformClock::AnalogI2cMaster])
    );
    // The owner's own count is what a post-mortem snapshot reports.
    let mut counted = PlatformClockHolds::default();
    held_clocks.count_into(&mut counted);
    assert_eq!(counted, held());
    assert_eq!(
        *log.borrow(),
        [
            Operation::Device(ModemClockDevice::ModemAdcCommonFe, true),
            Operation::Device(ModemClockDevice::ModemPrivateFe, true),
            Operation::AcquirePll,
            Operation::Device(ModemClockDevice::PllSourceGate, true),
            Operation::AcquireAnalogI2c,
            Operation::Device(ModemClockDevice::WifiBaseband80x1, true),
        ]
    );

    log.borrow_mut().clear();
    let prepared = planner
        .prepare_release(lease)
        .unwrap_or_else(|_| panic!("release"));
    let _planner = execute_release(prepared, &mut port, &mut held_clocks)
        .unwrap_or_else(|_| panic!("release"));
    assert!(held().is_empty());
    assert_eq!(
        *log.borrow(),
        [
            Operation::Device(ModemClockDevice::ModemAdcCommonFe, false),
            Operation::Device(ModemClockDevice::ModemPrivateFe, false),
            Operation::Device(ModemClockDevice::PllSourceGate, false),
            Operation::ReleasePll,
            Operation::ReleaseAnalogI2c,
            Operation::Device(ModemClockDevice::WifiBaseband80x1, false),
        ]
    );
}

#[test]
fn a_refused_platform_request_poisons_the_transaction_before_the_gate() {
    let (log, mut port, platform) = setup();
    platform.set_refused(PlatformClock::Pll160m, true);
    let mut held_clocks = ModemPlatformClocks::default();
    let identity = ModemClockPlannerIdentity::new();
    let planner = ModemClockPlanner::managed(&identity);
    let prepared = planner
        .prepare_acquire(ModemClockModule::Coexistence.dependencies())
        .unwrap_or_else(|_| panic!("coexistence"));
    let Err(poisoned) = execute_acquire(prepared, &mut port, &mut held_clocks, &platform) else {
        panic!("a refused PLL request must poison the acquisition");
    };
    assert_eq!(poisoned.dependency(), Dependency::Pll160AndModemSource);
    // The modem gate was not opened without its upstream source.
    assert!(log.borrow().is_empty());
    assert!(held().is_empty());
}

#[test]
fn a_poisoned_acquisition_is_abandoned_with_its_references_on() {
    let (_, mut port, platform) = setup();
    platform.set_refused(PlatformClock::AnalogI2cMaster, true);
    let mut held_clocks = ModemPlatformClocks::default();
    let identity = ModemClockPlannerIdentity::new();
    let planner = ModemClockPlanner::managed(&identity);
    let prepared = planner
        .prepare_acquire(ModemClockModule::Phy.dependencies())
        .unwrap_or_else(|_| panic!("PHY"));
    let Err(poisoned) = execute_acquire(prepared, &mut port, &mut held_clocks, &platform) else {
        panic!("a refused analog-I2C request must poison the acquisition");
    };
    assert_eq!(poisoned.dependency(), Dependency::AnalogI2cMaster);
    // The 160 MHz source taken before the refusal stays with the poisoned
    // owner; abandoning the owner keeps it on and records it, as a failed
    // transaction stays fail-stop.
    assert_eq!(held(), holding(&[PlatformClock::Pll160m]));
    drop(poisoned);
    held_clocks.abandon();
    assert_eq!(held(), holding(&[PlatformClock::Pll160m]));
    let (abandoned, dropped) = abandoned();
    assert_eq!(abandoned, holding(&[PlatformClock::Pll160m]));
    assert!(dropped.is_empty());
}

/// A guard ends explicitly: `release` gives the reference back, and
/// `abandon` keeps it held and records it.
#[test]
fn a_guard_ends_with_release_or_abandon() {
    let platform = CountingPlatformClocks::new();
    let kept = platform
        .acquire(PlatformClock::ModemCoexistence)
        .unwrap_or_else(|_| panic!("granted"));
    kept.abandon();
    let released = platform
        .acquire(PlatformClock::AnalogI2cMaster)
        .unwrap_or_else(|_| panic!("granted"));
    released.release();
    assert_eq!(held(), holding(&[PlatformClock::ModemCoexistence]));
    let (abandoned, dropped) = abandoned();
    assert_eq!(abandoned, holding(&[PlatformClock::ModemCoexistence]));
    assert!(dropped.is_empty());
}

/// Dropping a guard without `release` or `abandon` is a defect: the clock
/// stays on and the provider records the drop, so destroying the radio's
/// bookkeeping never closes a clock under a user that still runs on it
/// (an IEEE 802.15.4 ED probe once lost the coexistence clock when its
/// shared radio was dropped); a debug build also asserts.
#[test]
fn a_dropped_guard_keeps_its_clock_and_is_recorded() {
    let platform = CountingPlatformClocks::new();
    let guard = platform
        .acquire(PlatformClock::ModemCoexistence)
        .unwrap_or_else(|_| panic!("granted"));
    let dropped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(guard)));
    assert_eq!(dropped.is_err(), cfg!(debug_assertions));
    assert_eq!(held(), holding(&[PlatformClock::ModemCoexistence]));
    let (abandoned, dropped) = abandoned();
    assert!(abandoned.is_empty());
    assert_eq!(dropped, holding(&[PlatformClock::ModemCoexistence]));
}

/// The planner's explicit release gives each platform clock back once.
#[test]
fn the_planner_releases_its_platform_clocks_explicitly() {
    let (_, _port, platform) = setup();
    let mut held_clocks = ModemPlatformClocks::default();
    for clock in [PlatformClock::Pll160m, PlatformClock::ModemCoexistence] {
        held_clocks
            .acquire(clock, &platform)
            .unwrap_or_else(|_| panic!("granted"));
    }
    held_clocks.release(PlatformClock::ModemCoexistence);
    assert_eq!(held(), holding(&[PlatformClock::Pll160m]));
    held_clocks.release(PlatformClock::ModemCoexistence);
    assert_eq!(held(), holding(&[PlatformClock::Pll160m]));
    held_clocks.release(PlatformClock::Pll160m);
    assert!(held().is_empty());
}
