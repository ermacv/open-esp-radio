use std::{cell::RefCell, rc::Rc, vec::Vec};

use super::*;
use crate::power::clock::{ModemClockModule, ModemClockPlanner, ModemClockPlannerIdentity};
use crate::power::test_gates::{GateEvent, RecordingGates};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    PreparePowerState,
    Device(ModemClockDevice, bool),
    AcquirePll,
    ReleasePll,
    AcquireAnalogI2c,
    ReleaseAnalogI2c,
    Acquire(PlatformClock),
    Release(PlatformClock),
    WifiPower(bool),
}

/// One ordered log shared by the modem port and the platform gates.
type Log = Rc<RefCell<Vec<Operation>>>;

struct Port(Log);

impl ModemClockPort for Port {
    fn prepare_power_state(&mut self) {
        self.0.borrow_mut().push(Operation::PreparePowerState);
    }
    fn enable_device(&mut self, device: ModemClockDevice) {
        self.0.borrow_mut().push(Operation::Device(device, true));
    }
    fn disable_device(&mut self, device: ModemClockDevice) {
        self.0.borrow_mut().push(Operation::Device(device, false));
    }
}

fn setup() -> (Log, Port, RecordingGates) {
    let log = Log::default();
    let sink = log.clone();
    let platform = RecordingGates::logging(move |event| {
        sink.borrow_mut().push(match event {
            GateEvent::Acquire(PlatformClock::Pll160m) => Operation::AcquirePll,
            GateEvent::Acquire(PlatformClock::AnalogI2cMaster) => Operation::AcquireAnalogI2c,
            GateEvent::Acquire(other) => Operation::Acquire(other),
            GateEvent::Release(PlatformClock::Pll160m) => Operation::ReleasePll,
            GateEvent::Release(PlatformClock::AnalogI2cMaster) => Operation::ReleaseAnalogI2c,
            GateEvent::Release(other) => Operation::Release(other),
            GateEvent::OpenWifiPower => Operation::WifiPower(true),
            GateEvent::CloseWifiPower => Operation::WifiPower(false),
        });
    });
    (log.clone(), Port(log), platform)
}

#[test]
fn the_pll_source_brackets_the_modem_gate_and_the_analog_clock_goes_to_the_platform() {
    let (log, mut port, mut platform) = setup();
    let mut held_clocks = ModemPlatformClocks::default();
    let identity = ModemClockPlannerIdentity::new().expect("identity capacity");
    let planner = ModemClockPlanner::managed(identity);
    let prepared = planner
        .prepare_acquire(crate::power::clock::module_dependencies(
            ModemClockModule::Phy,
        ))
        .unwrap_or_else(|_| panic!("PHY"));
    let (planner, lease) = execute_acquire(prepared, &mut port, &mut held_clocks, &mut platform)
        .unwrap_or_else(|_| panic!("acquire"));
    assert_eq!(
        platform.held(),
        [PlatformClock::Pll160m, PlatformClock::AnalogI2cMaster]
    );
    assert_eq!(
        *log.borrow(),
        [
            Operation::PreparePowerState,
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
    let _planner = execute_release(prepared, &mut port, &mut held_clocks, &mut platform)
        .unwrap_or_else(|_| panic!("release"));
    assert!(platform.held().is_empty());
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

/// The planner's release gives each platform hold back once.
#[test]
fn the_planner_releases_its_platform_holds_once() {
    let (_, _port, mut platform) = setup();
    let mut held_clocks = ModemPlatformClocks::default();
    for clock in [PlatformClock::Pll160m, PlatformClock::ModemCoexistence] {
        held_clocks.acquire(clock, &mut platform);
    }
    held_clocks.release(PlatformClock::ModemCoexistence, &mut platform);
    assert_eq!(platform.held(), [PlatformClock::Pll160m]);
    held_clocks.release(PlatformClock::ModemCoexistence, &mut platform);
    assert_eq!(
        platform.held(),
        [PlatformClock::Pll160m],
        "nothing left to release"
    );
    held_clocks.release(PlatformClock::Pll160m, &mut platform);
    assert!(platform.held().is_empty());
}

/// PHY registration runs before a protocol client can enter common power.
/// Its first baseband register access must already have a published modem
/// power state; the later client power sequence cannot supply this prerequisite.
#[test]
fn phy_and_calibration_publish_power_before_any_clock_edge() {
    for module in [ModemClockModule::Phy, ModemClockModule::PhyCalibration] {
        let (log, mut port, mut platform) = setup();
        let mut held_clocks = ModemPlatformClocks::default();
        let identity = ModemClockPlannerIdentity::new().expect("identity capacity");
        let planner = ModemClockPlanner::managed(identity);
        let prepared = planner
            .prepare_acquire(crate::power::clock::module_dependencies(module))
            .expect("module");
        let (planner, lease) =
            execute_acquire(prepared, &mut port, &mut held_clocks, &mut platform)
                .unwrap_or_else(|_| panic!("module clocks"));
        let operations = log.borrow();
        assert_eq!(operations.first(), Some(&Operation::PreparePowerState));
        assert!(operations.contains(&Operation::Device(ModemClockDevice::WifiBaseband80x1, true)));
        if module == ModemClockModule::PhyCalibration {
            assert!(operations.contains(&Operation::Device(ModemClockDevice::WifiBaseband, true)));
        }
        drop(operations);
        let prepared = planner.prepare_release(lease).expect("release");
        let _planner = execute_release(prepared, &mut port, &mut held_clocks, &mut platform)
            .unwrap_or_else(|_| panic!("release"));
    }
}

/// Exercise the real PAC writes as well as the mock-port ordering above.
#[test]
fn prepare_power_state_selects_active_modem_on_host_registers() {
    for fill in [0, u32::MAX] {
        crate::radio::phy::registers::host::with_registers(fill, |registers| {
            assert!(
                !registers
                    .platform_clock_power_observation()
                    .hp_active_icg_selected
            );
            registers.prepare_power_state();
            assert!(
                registers
                    .platform_clock_power_observation()
                    .hp_active_icg_selected
            );
            assert!(
                registers
                    .modem_syscon_power_observation()
                    .active_clock_map_configured
            );
            assert!(
                registers
                    .shared_modem_clock_observation()
                    .power_state_map_configured
            );
        });
    }
}

#[allow(clippy::result_large_err)]
fn execute_acquire(
    prepared: super::super::PreparedModemClockAcquire,
    port: &mut impl ModemClockPort,
    held: &mut ModemPlatformClocks,
    platform: &mut RecordingGates,
) -> Result<
    (
        super::super::ModemClockPlanner,
        super::super::ModemClockLease,
    ),
    super::super::PoisonedModemClockAcquire,
> {
    oer_chips_family_radio_modem_clock::execution::acquire(
        prepared,
        &mut ModemExecution {
            port,
            held,
            platform,
        },
    )
}
#[allow(clippy::result_large_err)]
fn execute_release(
    prepared: super::super::PreparedModemClockRelease,
    port: &mut impl ModemClockPort,
    held: &mut ModemPlatformClocks,
    platform: &mut RecordingGates,
) -> Result<super::super::ModemClockPlanner, super::super::PoisonedModemClockRelease> {
    oer_chips_family_radio_modem_clock::execution::release(
        prepared,
        &mut ModemExecution {
            port,
            held,
            platform,
        },
    )
}
