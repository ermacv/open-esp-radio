use std::vec::Vec;

use super::{
    AnalogI2cPowerAccess, complete_open_i2c_power_and_reset_sequence,
    prepare_open_i2c_pre_delay_sequence,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    SetRfCircuitPower(bool),
    SetBbI2cPowerTie(bool),
    SetAnalogI2cPower(bool),
    SetAnalogI2cReset(bool),
}

struct FakePmu {
    powered: bool,
    reset_released: bool,
    operations: Vec<Operation>,
}

impl FakePmu {
    fn new(powered: bool, reset_released: bool) -> Self {
        Self {
            powered,
            reset_released,
            operations: Vec::new(),
        }
    }
}

impl AnalogI2cPowerAccess for FakePmu {
    fn set_rf_circuit_power(&mut self, enabled: bool) {
        self.operations.push(Operation::SetRfCircuitPower(enabled));
    }

    fn set_bb_i2c_power_tie(&mut self, enabled: bool) {
        self.operations.push(Operation::SetBbI2cPowerTie(enabled));
    }

    fn analog_i2c_is_powered(&self) -> bool {
        self.powered
    }

    fn set_analog_i2c_power(&mut self, enabled: bool) {
        self.operations.push(Operation::SetAnalogI2cPower(enabled));
        self.powered = enabled;
    }

    fn analog_i2c_reset_is_released(&self) -> bool {
        self.reset_released
    }

    fn set_analog_i2c_reset_released(&mut self, released: bool) {
        self.operations.push(Operation::SetAnalogI2cReset(released));
        self.reset_released = released;
    }
}

#[test]
fn pre_delay_retains_both_blob_operations() {
    let mut pmu = FakePmu::new(true, true);
    prepare_open_i2c_pre_delay_sequence(&mut pmu);
    assert_eq!(
        pmu.operations,
        [
            Operation::SetRfCircuitPower(false),
            Operation::SetBbI2cPowerTie(false),
        ]
    );
}

#[test]
fn powered_down_i2c_gets_the_complete_assert_release_edge() {
    let mut pmu = FakePmu::new(false, false);
    complete_open_i2c_power_and_reset_sequence(&mut pmu);
    assert_eq!(
        pmu.operations,
        [
            Operation::SetRfCircuitPower(true),
            Operation::SetBbI2cPowerTie(true),
            Operation::SetAnalogI2cPower(true),
            Operation::SetAnalogI2cReset(false),
            Operation::SetAnalogI2cReset(true),
        ]
    );
    assert!(pmu.powered);
    assert!(pmu.reset_released);
}

#[test]
fn already_powered_i2c_only_releases_reset_when_needed() {
    let mut pmu = FakePmu::new(true, false);
    complete_open_i2c_power_and_reset_sequence(&mut pmu);
    assert_eq!(
        pmu.operations,
        [
            Operation::SetRfCircuitPower(true),
            Operation::SetBbI2cPowerTie(true),
            Operation::SetAnalogI2cReset(true),
        ]
    );
}
