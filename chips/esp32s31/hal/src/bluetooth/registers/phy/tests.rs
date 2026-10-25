use std::vec::Vec;

use super::{
    BleBaseStackOnTaskEnableHardwareTransaction, execute_base_stack_on_task_enable_hardware,
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
