use std::vec::Vec;

use super::{
    BleBaseStackOnTaskEnableHardwareTransaction, BluetoothPhyEnvironmentAddress,
    BluetoothPhyEnvironmentAddressError, ENVIRONMENT_LAST_OFFSET,
    execute_base_stack_on_task_enable_hardware,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    EnableAccessAddressLowCorrelation,
    InitializeBlePhyRegisters,
}

#[derive(Default)]
struct Recorder {
    operations: Vec<Operation>,
}

impl BleBaseStackOnTaskEnableHardwareTransaction for Recorder {
    fn enable_access_address_low_correlation(&mut self) {
        self.operations
            .push(Operation::EnableAccessAddressLowCorrelation);
    }

    fn initialize_ble_phy_registers(&mut self) {
        self.operations.push(Operation::InitializeBlePhyRegisters);
    }
}

#[test]
fn base_stack_on_task_enable_orders_baseband_before_phy_initialization() {
    let mut recorder = Recorder::default();

    execute_base_stack_on_task_enable_hardware(&mut recorder);

    assert_eq!(
        recorder.operations,
        [
            Operation::EnableAccessAddressLowCorrelation,
            Operation::InitializeBlePhyRegisters,
        ]
    );
}

#[test]
fn an_environment_address_must_represent_its_last_published_member() {
    let highest = (u32::MAX - ENVIRONMENT_LAST_OFFSET) & !3;
    assert!(BluetoothPhyEnvironmentAddress::new(highest).is_ok());
    assert_eq!(
        BluetoothPhyEnvironmentAddress::new(highest + 4),
        Err(BluetoothPhyEnvironmentAddressError::ExtentOverflow)
    );
    assert_eq!(
        BluetoothPhyEnvironmentAddress::new(0x2f00_0102),
        Err(BluetoothPhyEnvironmentAddressError::Unaligned)
    );
}
