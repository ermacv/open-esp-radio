use std::vec::Vec;

use super::{
    BluetoothControllerLatchedTime, BluetoothControllerTimeLatchBeginError,
    BluetoothControllerTimeLatchControl, BluetoothControllerTimeLatchRequest,
    BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError, ControllerTimeLatch,
    execute_latch_publication, execute_latch_step,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Publish,
    ReadControl,
    Fence,
    ReadLatchedTime0,
}

struct Recorder {
    latch_request_pending: bool,
    latched_time_0: u32,
    operations: Vec<Operation>,
}

impl BluetoothControllerTimeLatchControl for Recorder {
    fn publish_latch_request(&mut self, _request: BluetoothControllerTimeLatchRequest) {
        self.latch_request_pending = true;
        self.operations.push(Operation::Publish);
    }

    fn order_after_publication(&mut self) {
        self.operations.push(Operation::Fence);
    }

    fn latch_request_pending(&mut self) -> bool {
        self.operations.push(Operation::ReadControl);
        self.latch_request_pending
    }

    fn order_after_clear_observation(&mut self) {
        self.operations.push(Operation::Fence);
    }

    fn read_latched_time_0(&mut self) -> u32 {
        self.operations.push(Operation::ReadLatchedTime0);
        self.latched_time_0
    }
}

#[test]
fn publication_is_one_accessor_write_followed_by_one_fence() {
    let mut ownership = ControllerTimeLatch::new();
    let mut recorder = Recorder {
        latch_request_pending: false,
        latched_time_0: 0,
        operations: Vec::new(),
    };

    assert_eq!(
        execute_latch_publication(
            &mut ownership,
            &mut recorder,
            BluetoothControllerTimeLatchRequest::new(),
        ),
        Ok(())
    );

    assert!(recorder.latch_request_pending);
    assert_eq!(recorder.operations, [Operation::Publish, Operation::Fence]);
    assert!(ownership.in_flight());

    assert_eq!(
        execute_latch_publication(
            &mut ownership,
            &mut recorder,
            BluetoothControllerTimeLatchRequest::new(),
        ),
        Err(BluetoothControllerTimeLatchBeginError::AlreadyInFlight)
    );
    assert_eq!(recorder.operations, [Operation::Publish, Operation::Fence]);
}

#[test]
fn pending_step_reads_control_once_and_never_reads_latched_time() {
    let mut ownership = ControllerTimeLatch::new();
    assert_eq!(ownership.begin(), Ok(()));
    let mut recorder = Recorder {
        latch_request_pending: true,
        latched_time_0: 0xdead_beef,
        operations: Vec::new(),
    };

    assert_eq!(
        execute_latch_step(&mut ownership, &mut recorder),
        Ok(BluetoothControllerTimeLatchStep::Waiting)
    );
    assert_eq!(recorder.operations, [Operation::ReadControl]);
    assert!(ownership.in_flight());
}

#[test]
fn ready_step_reads_control_then_latched_time_exactly_once() {
    let mut ownership = ControllerTimeLatch::new();
    assert_eq!(ownership.begin(), Ok(()));
    let mut recorder = Recorder {
        latch_request_pending: false,
        latched_time_0: 0xffff_fffe,
        operations: Vec::new(),
    };

    assert_eq!(
        execute_latch_step(&mut ownership, &mut recorder),
        Ok(BluetoothControllerTimeLatchStep::Ready(
            BluetoothControllerLatchedTime::from_bits(0xffff_fffe,)
        ))
    );
    assert_eq!(
        recorder.operations,
        [
            Operation::ReadControl,
            Operation::Fence,
            Operation::ReadLatchedTime0
        ]
    );
    assert!(!ownership.in_flight());

    let operation_count = recorder.operations.len();
    assert_eq!(
        execute_latch_step(&mut ownership, &mut recorder),
        Err(BluetoothControllerTimeLatchStepError::NotInFlight)
    );
    assert_eq!(recorder.operations.len(), operation_count);
}

#[test]
fn cancelled_worker_resumes_the_same_request_and_drains_it_once() {
    let mut ownership = ControllerTimeLatch::new();
    let mut publication = Recorder {
        latch_request_pending: false,
        latched_time_0: 0,
        operations: Vec::new(),
    };
    assert_eq!(
        execute_latch_publication(
            &mut ownership,
            &mut publication,
            BluetoothControllerTimeLatchRequest::new(),
        ),
        Ok(())
    );

    let mut first_worker = Recorder {
        latch_request_pending: true,
        latched_time_0: 0,
        operations: Vec::new(),
    };
    assert_eq!(
        execute_latch_step(&mut ownership, &mut first_worker),
        Ok(BluetoothControllerTimeLatchStep::Waiting)
    );
    drop(first_worker);

    let mut replacement_worker = Recorder {
        latch_request_pending: false,
        latched_time_0: 0x1234_5678,
        operations: Vec::new(),
    };
    assert_eq!(
        execute_latch_publication(
            &mut ownership,
            &mut replacement_worker,
            BluetoothControllerTimeLatchRequest::new(),
        ),
        Err(BluetoothControllerTimeLatchBeginError::AlreadyInFlight)
    );
    assert!(replacement_worker.operations.is_empty());
    assert_eq!(
        execute_latch_step(&mut ownership, &mut replacement_worker),
        Ok(BluetoothControllerTimeLatchStep::Ready(
            BluetoothControllerLatchedTime::from_bits(0x1234_5678)
        ))
    );
    assert_eq!(
        replacement_worker.operations,
        [
            Operation::ReadControl,
            Operation::Fence,
            Operation::ReadLatchedTime0
        ]
    );

    let operation_count = replacement_worker.operations.len();
    assert_eq!(
        execute_latch_step(&mut ownership, &mut replacement_worker),
        Err(BluetoothControllerTimeLatchStepError::NotInFlight)
    );
    assert_eq!(replacement_worker.operations.len(), operation_count);
}

#[test]
fn idle_step_fails_without_any_register_access() {
    let mut ownership = ControllerTimeLatch::new();
    let mut recorder = Recorder {
        latch_request_pending: false,
        latched_time_0: 0,
        operations: Vec::new(),
    };

    assert_eq!(
        execute_latch_step(&mut ownership, &mut recorder),
        Err(BluetoothControllerTimeLatchStepError::NotInFlight)
    );
    assert!(recorder.operations.is_empty());
}
