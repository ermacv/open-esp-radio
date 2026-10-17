use std::{cell::RefCell, rc::Rc, vec::Vec};

use oer_esp32s31_pac::{PlatformClockPowerObservation, SharedModemClockObservation};

use super::{
    PlatformClock, PlatformClockError, PowerCheckpoint, PowerEntry, PowerError,
    PowerSequenceBackend, execute_owned,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    SelectWifiLowPowerClock,
    ResetWifi(bool),
    SelectHpActiveIcg,
    ApplyModemIcg,
    ApplySleepIcg,
    EnableModemBus,
    ConfigureHpActiveMap,
    PrepareSharedMap,
    Acquire(PlatformClock),
    ConfigureModemSource,
    ResetBaseband(bool),
    EnablePhyClocks,
    SelectI2c160Mhz,
}

struct FakeShared {
    operations: Rc<RefCell<Vec<Operation>>>,
    prepare_calls: u8,
    /// The platform clock owner refuses this reference.
    refused: Option<PowerCheckpoint>,
    platform: PlatformClockPowerObservation,
    modem: oer_esp32s31_pac::ModemSysconPowerObservation,
    observation: SharedModemClockObservation,
}

impl FakeShared {
    fn ready(operations: Rc<RefCell<Vec<Operation>>>) -> Self {
        Self {
            operations,
            prepare_calls: 0,
            refused: None,
            platform: PlatformClockPowerObservation {
                hp_active_icg_selected: true,
                modem_register_bus_clock_enabled: true,
                modem_source_clocks_configured: true,
            },
            modem: oer_esp32s31_pac::ModemSysconPowerObservation {
                wifi_reset_released: true,
                active_clock_map_configured: true,
                phy_calibration_clocks_enabled: true,
                phy_i2c_160mhz_selected: true,
            },
            observation: SharedModemClockObservation {
                power_state_map_configured: true,
            },
        }
    }
}

impl PowerSequenceBackend for FakeShared {
    fn select_hp_active_modem_icg(&mut self) {
        self.operations
            .borrow_mut()
            .push(Operation::SelectHpActiveIcg);
    }

    fn apply_modem_icg_selection(&mut self) {
        self.operations.borrow_mut().push(Operation::ApplyModemIcg);
    }

    fn apply_sleep_icg_selection(&mut self) {
        self.operations.borrow_mut().push(Operation::ApplySleepIcg);
    }

    fn enable_modem_register_bus_clock(&mut self) {
        self.operations.borrow_mut().push(Operation::EnableModemBus);
    }

    fn acquire_platform_clock(&mut self, clock: PlatformClock) -> Result<(), PlatformClockError> {
        self.operations.borrow_mut().push(Operation::Acquire(clock));
        let refused = match clock {
            PlatformClock::Mpll => PowerCheckpoint::Mpll,
            PlatformClock::Pll160m => PowerCheckpoint::Reference160m,
            _ => PowerCheckpoint::I2cClock,
        };
        if self.refused == Some(refused) {
            Err(PlatformClockError)
        } else {
            Ok(())
        }
    }

    fn configure_modem_source_clocks(&mut self) {
        self.operations
            .borrow_mut()
            .push(Operation::ConfigureModemSource);
    }

    fn platform_clock_power_observation(&self) -> PlatformClockPowerObservation {
        self.platform
    }

    fn set_wifi_baseband_and_mac_reset(&mut self, asserted: bool) {
        self.operations
            .borrow_mut()
            .push(Operation::ResetWifi(asserted));
    }

    fn set_wifi_baseband_reset(&mut self, asserted: bool) {
        self.operations
            .borrow_mut()
            .push(Operation::ResetBaseband(asserted));
    }

    fn configure_wifi_power_clock_map(&mut self) {
        self.operations
            .borrow_mut()
            .push(Operation::ConfigureHpActiveMap);
    }

    fn enable_phy_calibration_clocks(&mut self) {
        self.operations
            .borrow_mut()
            .push(Operation::EnablePhyClocks);
    }

    fn select_phy_i2c_160mhz_source(&mut self) {
        self.operations
            .borrow_mut()
            .push(Operation::SelectI2c160Mhz);
    }

    fn modem_syscon_power_observation(&self) -> oer_esp32s31_pac::ModemSysconPowerObservation {
        self.modem
    }

    fn prepare_shared_modem_clock_map(&mut self) {
        self.operations
            .borrow_mut()
            .push(Operation::PrepareSharedMap);
        self.prepare_calls += 1;
    }

    fn select_wifi_low_power_clock(&mut self) {
        self.operations
            .borrow_mut()
            .push(Operation::SelectWifiLowPowerClock);
    }

    fn shared_modem_clock_observation(&self) -> SharedModemClockObservation {
        self.observation
    }
}

#[test]
fn exact_semantic_sequence_is_finite_and_ordered() {
    let operations = Rc::new(RefCell::new(Vec::new()));
    let mut shared = FakeShared::ready(operations.clone());
    assert_eq!(
        execute_owned(&mut shared, PowerEntry::FirstSinceBoot),
        Ok(())
    );
    assert_eq!(
        operations.borrow().as_slice(),
        [
            Operation::SelectWifiLowPowerClock,
            Operation::ResetWifi(true),
            Operation::ResetWifi(false),
            Operation::Acquire(PlatformClock::Mpll),
            Operation::SelectHpActiveIcg,
            Operation::ApplyModemIcg,
            Operation::ApplySleepIcg,
            Operation::EnableModemBus,
            Operation::ConfigureHpActiveMap,
            Operation::PrepareSharedMap,
            Operation::Acquire(PlatformClock::Pll160m),
            Operation::ConfigureModemSource,
            Operation::ResetBaseband(true),
            Operation::ResetBaseband(false),
            Operation::EnablePhyClocks,
            Operation::SelectI2c160Mhz,
            Operation::Acquire(PlatformClock::AnalogI2cMaster),
        ]
    );
    assert_eq!(shared.prepare_calls, 1);
}

#[test]
fn failed_semantic_readback_names_the_exact_checkpoint() {
    let operations = Rc::new(RefCell::new(Vec::new()));
    let mut shared = FakeShared::ready(operations);
    shared.platform.modem_source_clocks_configured = false;

    assert_eq!(
        execute_owned(&mut shared, PowerEntry::FirstSinceBoot),
        Err(PowerError {
            checkpoint: PowerCheckpoint::ModemClockSource,
            expected: true,
            observed: false,
        })
    );
}

#[test]
fn power_sequence_takes_the_i2c_reference_on_success_and_each_readback_failure() {
    for failed in [
        None,
        Some(PowerCheckpoint::ResetReleased),
        Some(PowerCheckpoint::HpActiveIcg),
        Some(PowerCheckpoint::ModemBusClock),
        Some(PowerCheckpoint::HpActiveClockMap),
        Some(PowerCheckpoint::SharedClockMap),
        Some(PowerCheckpoint::ModemClockSource),
        Some(PowerCheckpoint::PhyClocks),
        Some(PowerCheckpoint::I2cSource),
    ] {
        let operations = Rc::new(RefCell::new(Vec::new()));
        let mut shared = FakeShared::ready(operations.clone());
        if let Some(checkpoint) = failed {
            *match checkpoint {
                PowerCheckpoint::ResetReleased => &mut shared.modem.wifi_reset_released,
                PowerCheckpoint::HpActiveIcg => &mut shared.platform.hp_active_icg_selected,
                PowerCheckpoint::ModemBusClock => {
                    &mut shared.platform.modem_register_bus_clock_enabled
                }
                PowerCheckpoint::HpActiveClockMap => &mut shared.modem.active_clock_map_configured,
                PowerCheckpoint::SharedClockMap => {
                    &mut shared.observation.power_state_map_configured
                }
                PowerCheckpoint::ModemClockSource => {
                    &mut shared.platform.modem_source_clocks_configured
                }
                PowerCheckpoint::PhyClocks => &mut shared.modem.phy_calibration_clocks_enabled,
                PowerCheckpoint::I2cSource => &mut shared.modem.phy_i2c_160mhz_selected,
                PowerCheckpoint::Mpll
                | PowerCheckpoint::Reference160m
                | PowerCheckpoint::I2cClock => {
                    unreachable!("platform references are not read back")
                }
            } = false;
        }
        let result = super::execute_owned(&mut shared, PowerEntry::FirstSinceBoot);
        assert_eq!(
            result,
            failed.map_or(Ok(()), |checkpoint| Err(PowerError {
                checkpoint,
                expected: true,
                observed: false,
            }))
        );
        let operations = operations.borrow();
        assert_eq!(
            operations
                .iter()
                .filter(|op| **op == Operation::Acquire(PlatformClock::AnalogI2cMaster))
                .count(),
            1,
            "the I2C reference belongs to the retained epoch"
        );
        let reset = operations
            .iter()
            .rposition(|op| *op == Operation::ResetBaseband(false))
            .unwrap();
        let clocks = operations
            .iter()
            .position(|op| *op == Operation::EnablePhyClocks)
            .unwrap();
        let source = operations
            .iter()
            .position(|op| *op == Operation::SelectI2c160Mhz)
            .unwrap();
        assert!(reset < clocks && clocks < source);
    }
}

#[test]
fn repeated_power_up_leaves_out_only_the_wifi_mac_reset_pulse() {
    let operations = Rc::new(RefCell::new(Vec::new()));
    let mut shared = FakeShared::ready(operations.clone());
    assert_eq!(execute_owned(&mut shared, PowerEntry::Repeated), Ok(()));
    let operations = operations.borrow();
    assert!(!operations.contains(&Operation::ResetWifi(true)));
    assert!(!operations.contains(&Operation::SelectWifiLowPowerClock));
    assert_eq!(
        operations.as_slice()[..2],
        [
            Operation::Acquire(PlatformClock::Mpll),
            Operation::SelectHpActiveIcg
        ]
    );
    assert!(operations.contains(&Operation::ResetBaseband(true)));
    assert!(operations.contains(&Operation::ResetBaseband(false)));
}

#[test]
fn a_refused_platform_reference_stops_the_sequence_at_its_edge() {
    for (refused, last) in [
        (
            PowerCheckpoint::Mpll,
            Operation::Acquire(PlatformClock::Mpll),
        ),
        (
            PowerCheckpoint::Reference160m,
            Operation::Acquire(PlatformClock::Pll160m),
        ),
        (
            PowerCheckpoint::I2cClock,
            Operation::Acquire(PlatformClock::AnalogI2cMaster),
        ),
    ] {
        let operations = Rc::new(RefCell::new(Vec::new()));
        let mut shared = FakeShared::ready(operations.clone());
        shared.refused = Some(refused);
        assert_eq!(
            execute_owned(&mut shared, PowerEntry::FirstSinceBoot),
            Err(PowerError {
                checkpoint: refused,
                expected: true,
                observed: false,
            })
        );
        assert_eq!(operations.borrow().last(), Some(&last));
    }
}
