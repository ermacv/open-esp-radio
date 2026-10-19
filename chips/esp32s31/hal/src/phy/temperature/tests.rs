use std::vec::Vec;

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    RegisterBank,
    PeripheralClock,
    PhyReadout,
    PhyConversion,
    Power,
    PowerDown,
}

#[derive(Default)]
struct FakePlatform {
    code: u8,
    operations: Vec<Operation>,
}

impl PhyTemperatureSystemControl for FakePlatform {
    fn enable_temperature_sensor_register_bank(&mut self) {
        self.operations.push(Operation::RegisterBank);
    }

    fn enable_temperature_sensor_clock(&mut self) {
        self.operations.push(Operation::PeripheralClock);
    }

    fn enable_temperature_sensor_phy_readout(&mut self) {
        self.operations.push(Operation::PhyReadout);
    }

    fn enable_temperature_sensor_phy_conversion(&mut self) {
        self.operations.push(Operation::PhyConversion);
    }

    fn enable_temperature_sensor_power(&mut self) {
        self.operations.push(Operation::Power);
    }

    fn disable_temperature_sensor_power(&mut self) {
        self.operations.push(Operation::PowerDown);
    }

    fn read_temperature_sensor_code(&self) -> u8 {
        self.code
    }
}

#[test]
fn initialization_preserves_all_five_fresh_reads_and_their_order() {
    let mut platform = FakePlatform::default();
    initialize(&mut platform);
    assert_eq!(
        platform.operations,
        [
            Operation::RegisterBank,
            Operation::PeripheralClock,
            Operation::PhyReadout,
            Operation::PhyConversion,
            Operation::Power,
        ]
    );
}

#[test]
fn code_sample_reads_one_shared_word_and_extracts_only_the_low_byte() {
    let platform = FakePlatform {
        code: 0xfe,
        operations: Vec::new(),
    };
    assert_eq!(read_code(&platform), 0xfe);
}

#[test]
fn power_down_uses_the_distinct_lifecycle_edge() {
    let mut platform = FakePlatform::default();
    power_down(&mut platform);
    assert_eq!(platform.operations, [Operation::PowerDown]);
}

#[test]
fn retained_wake_reasserts_only_sensor_power() {
    let mut platform = FakePlatform::default();
    power_up(&mut platform);
    assert_eq!(platform.operations, [Operation::Power]);
}
