use std::vec::Vec;

use super::{
    BluetoothSchedulerLockModifyControl, BluetoothSchedulerLockModifyObservation,
    BluetoothSchedulerLockModifyRequest, execute_scheduler_lock_modify_publication,
};
use crate::{BluetoothControllerSramAddress, BluetoothSchedulerHardwareListIndex};

#[test]
fn wait_requires_busy_and_start_simultaneously() {
    for (busy, start, expected) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (true, true, true),
    ] {
        assert_eq!(
            BluetoothSchedulerLockModifyObservation::from_fields_for_validation(busy, start, 0,)
                .wait_active(),
            expected
        );
    }
}

#[test]
fn result_projection_uses_idle_zero_or_busy_result_nibble() {
    assert_eq!(
        BluetoothSchedulerLockModifyObservation::from_fields_for_validation(false, true, 15)
            .result_code_after_publication(),
        0
    );
    assert_eq!(
        BluetoothSchedulerLockModifyObservation::from_fields_for_validation(true, false, 11)
            .result_code_after_publication(),
        0x0b
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    ClearHardwareListIndex,
    PublishHardwareListIndex(BluetoothSchedulerHardwareListIndex),
    PublishRequest(BluetoothControllerSramAddress),
    DeviceFence,
}

struct Recorder {
    operations: Vec<Operation>,
}

impl BluetoothSchedulerLockModifyControl for Recorder {
    fn clear_hardware_list_index(&mut self) {
        self.operations.push(Operation::ClearHardwareListIndex);
    }

    fn publish_hardware_list_index(&mut self, index: BluetoothSchedulerHardwareListIndex) {
        self.operations
            .push(Operation::PublishHardwareListIndex(index));
    }

    fn publish_request(&mut self, address: BluetoothControllerSramAddress) {
        self.operations.push(Operation::PublishRequest(address));
    }

    fn order_after_publication(&mut self) {
        self.operations.push(Operation::DeviceFence);
    }
}

#[test]
fn publication_uses_two_fresh_rmw_edges_before_request_and_fence() {
    let address =
        BluetoothControllerSramAddress::new(0x2f00_0040).expect("test address is representable");
    let index =
        BluetoothSchedulerHardwareListIndex::new(6).expect("test list index is representable");
    let request = BluetoothSchedulerLockModifyRequest::new(address, index);
    let mut recorder = Recorder {
        operations: Vec::new(),
    };

    let _published = execute_scheduler_lock_modify_publication(&mut recorder, request);

    assert_eq!(
        recorder.operations,
        [
            Operation::ClearHardwareListIndex,
            Operation::PublishHardwareListIndex(index),
            Operation::PublishRequest(address),
            Operation::DeviceFence,
        ]
    );
}
