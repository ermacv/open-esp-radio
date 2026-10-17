use std::vec::Vec;

use super::{
    BluetoothSchedulerExecutionLockDisposition, BluetoothSchedulerExecutionLockRequest,
    BluetoothSchedulerInsertionExecutionControl,
    BluetoothSchedulerInsertionExecutionObservationControl, execute_execution_lock_observation,
    execute_execution_lock_publication, execute_execution_modify_list_deletion_publication,
    execute_execution_modify_publication,
};
use crate::{
    BluetoothControllerSramAddress, BluetoothSchedulerBusyObservation,
    BluetoothSchedulerHardwareListIndex,
};

fn scheduler(busy: bool) -> BluetoothSchedulerBusyObservation {
    BluetoothSchedulerBusyObservation::from_busy_for_validation(busy)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ObservationOperation {
    LockReady,
    LockResult,
    ModifyReady,
    ModifyRejected,
}

struct ObservationRecorder {
    operations: Vec<ObservationOperation>,
    lock_ready: bool,
    lock_result: u8,
    modify_ready: bool,
    modify_rejected: bool,
}

impl BluetoothSchedulerInsertionExecutionObservationControl for ObservationRecorder {
    fn observe_execution_lock_ready(&mut self) -> bool {
        self.operations.push(ObservationOperation::LockReady);
        self.lock_ready
    }

    fn observe_execution_lock_result(&mut self) -> u8 {
        self.operations.push(ObservationOperation::LockResult);
        self.lock_result
    }

    fn observe_execution_modify_ready(&mut self) -> bool {
        self.operations.push(ObservationOperation::ModifyReady);
        self.modify_ready
    }

    fn observe_execution_modify_rejected(&mut self) -> bool {
        self.operations.push(ObservationOperation::ModifyRejected);
        self.modify_rejected
    }
}

impl ObservationRecorder {
    fn new() -> Self {
        Self {
            operations: Vec::new(),
            lock_ready: false,
            lock_result: 0,
            modify_ready: false,
            modify_rejected: false,
        }
    }
}

#[test]
fn execution_lock_preserves_idle_ready_and_result_short_circuits() {
    let mut recorder = ObservationRecorder::new();
    assert_eq!(
        execute_execution_lock_observation(&mut recorder, scheduler(false)),
        BluetoothSchedulerExecutionLockDisposition::ReconcileCurrentHead
    );
    assert!(recorder.operations.is_empty());

    assert_eq!(
        execute_execution_lock_observation(&mut recorder, scheduler(true)),
        BluetoothSchedulerExecutionLockDisposition::Pending
    );
    assert_eq!(recorder.operations, [ObservationOperation::LockReady]);

    recorder.operations.clear();
    recorder.lock_ready = true;
    recorder.lock_result = 0;
    assert_eq!(
        execute_execution_lock_observation(&mut recorder, scheduler(true)),
        BluetoothSchedulerExecutionLockDisposition::ExecutionLockRetained
    );
    assert_eq!(
        recorder.operations,
        [
            ObservationOperation::LockReady,
            ObservationOperation::LockResult,
        ]
    );

    // The vendor body asserts on results one, three and four and treats the
    // other nonzero results as a failed lock followed by reconciliation.
    for (result, expected) in [
        (
            1,
            BluetoothSchedulerExecutionLockDisposition::UnsupportedHardwareResult,
        ),
        (
            2,
            BluetoothSchedulerExecutionLockDisposition::ReconcileCurrentHead,
        ),
        (
            3,
            BluetoothSchedulerExecutionLockDisposition::UnsupportedHardwareResult,
        ),
        (
            4,
            BluetoothSchedulerExecutionLockDisposition::UnsupportedHardwareResult,
        ),
        (
            5,
            BluetoothSchedulerExecutionLockDisposition::ReconcileCurrentHead,
        ),
        (
            6,
            BluetoothSchedulerExecutionLockDisposition::ReconcileCurrentHead,
        ),
        (
            7,
            BluetoothSchedulerExecutionLockDisposition::ReconcileCurrentHead,
        ),
    ] {
        recorder.lock_result = result;
        assert_eq!(
            execute_execution_lock_observation(&mut recorder, scheduler(true)),
            expected,
            "result {result}"
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    PublishLock(BluetoothSchedulerExecutionLockRequest),
    PublishModify(BluetoothSchedulerHardwareListIndex),
    PublishModifyListDeletion(BluetoothSchedulerHardwareListIndex),
    DeviceFence,
}

struct Recorder {
    operations: Vec<Operation>,
}

impl BluetoothSchedulerInsertionExecutionControl for Recorder {
    fn publish_execution_lock(&mut self, request: BluetoothSchedulerExecutionLockRequest) {
        self.operations.push(Operation::PublishLock(request));
    }

    fn publish_execution_modify(&mut self, index: BluetoothSchedulerHardwareListIndex) {
        self.operations.push(Operation::PublishModify(index));
    }

    fn publish_execution_modify_list_deletion(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
    ) {
        self.operations
            .push(Operation::PublishModifyListDeletion(index));
    }

    fn order_after_publication(&mut self) {
        self.operations.push(Operation::DeviceFence);
    }
}

#[test]
fn each_execution_command_is_followed_by_its_device_fence() {
    let address =
        BluetoothControllerSramAddress::new(0x2f00_0040).expect("test address is representable");
    let index = BluetoothSchedulerHardwareListIndex::new(6).expect("test list is representable");
    let request = BluetoothSchedulerExecutionLockRequest::new(address, index);
    let mut recorder = Recorder {
        operations: Vec::new(),
    };

    let _lock = execute_execution_lock_publication(&mut recorder, request);
    let _modify = execute_execution_modify_publication(&mut recorder, index);
    let _deletion = execute_execution_modify_list_deletion_publication(&mut recorder, index);

    assert_eq!(
        recorder.operations,
        [
            Operation::PublishLock(request),
            Operation::DeviceFence,
            Operation::PublishModify(index),
            Operation::DeviceFence,
            Operation::PublishModifyListDeletion(index),
            Operation::DeviceFence,
        ]
    );
}
