use std::vec::Vec;

use super::{
    BluetoothSchedulerCancellationControl, BluetoothSchedulerCancellationDisposition,
    BluetoothSchedulerCancellationSourceAcknowledged, BluetoothSchedulerSkipDisposition,
    BluetoothSchedulerSkipRequest, BluetoothSchedulerSkipResult, execute_cancellation_indexing,
    execute_cancellation_observation, execute_cancellation_release, execute_cancellation_request,
    execute_skip_clear, execute_skip_observation, execute_skip_publication,
};
use crate::{
    BluetoothControllerSramAddress, BluetoothSchedulerBusyObservation,
    BluetoothSchedulerHardwareListIndex,
};

fn scheduler(busy: bool) -> BluetoothSchedulerBusyObservation {
    BluetoothSchedulerBusyObservation::from_busy_for_validation(busy)
}

fn index(value: u8) -> BluetoothSchedulerHardwareListIndex {
    BluetoothSchedulerHardwareListIndex::new(value).expect("test list is representable")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    PublishSkip(BluetoothSchedulerSkipRequest),
    ObserveSkip,
    ClearSkip,
    ClearListIndex,
    PublishListIndex(BluetoothSchedulerHardwareListIndex),
    SetControl,
    ClearControl,
    ObserveStatus0324,
    ObserveStatus0208,
    DeviceFence,
}

struct Recorder {
    operations: Vec<Operation>,
    skip: (bool, u8),
    status_0324: bool,
    status_0208: bool,
}

impl Recorder {
    fn new() -> Self {
        Self {
            operations: Vec::new(),
            skip: (true, 0),
            status_0324: false,
            status_0208: true,
        }
    }
}

impl BluetoothSchedulerCancellationControl for Recorder {
    fn publish_skip(&mut self, request: BluetoothSchedulerSkipRequest) {
        self.operations.push(Operation::PublishSkip(request));
    }

    fn observe_skip(&mut self) -> (bool, u8) {
        self.operations.push(Operation::ObserveSkip);
        self.skip
    }

    fn clear_skip(&mut self) {
        self.operations.push(Operation::ClearSkip);
    }

    fn clear_list_index(&mut self) {
        self.operations.push(Operation::ClearListIndex);
    }

    fn publish_list_index(&mut self, index: BluetoothSchedulerHardwareListIndex) {
        self.operations.push(Operation::PublishListIndex(index));
    }

    fn set_control(&mut self) {
        self.operations.push(Operation::SetControl);
    }

    fn clear_control(&mut self) {
        self.operations.push(Operation::ClearControl);
    }

    fn observe_status_0324(&mut self) -> bool {
        self.operations.push(Operation::ObserveStatus0324);
        self.status_0324
    }

    fn observe_status_0208(&mut self) -> bool {
        self.operations.push(Operation::ObserveStatus0208);
        self.status_0208
    }

    fn order_after_publication(&mut self) {
        self.operations.push(Operation::DeviceFence);
    }
}

#[test]
fn a_skip_request_is_published_observed_and_cleared_in_order() {
    let address =
        BluetoothControllerSramAddress::new(0x2f00_0080).expect("test address is representable");
    let request = BluetoothSchedulerSkipRequest::new(address, index(0));
    let mut recorder = Recorder::new();

    let published = execute_skip_publication(&mut recorder, request);
    assert_eq!(
        execute_skip_observation(&mut recorder, scheduler(true)),
        BluetoothSchedulerSkipDisposition::Pending
    );
    recorder.skip = (false, 2);
    assert_eq!(
        execute_skip_observation(&mut recorder, scheduler(true)),
        BluetoothSchedulerSkipDisposition::Completed(BluetoothSchedulerSkipResult::Two)
    );
    let _cleared = execute_skip_clear(&mut recorder, published);

    assert_eq!(
        recorder.operations,
        [
            Operation::PublishSkip(request),
            Operation::DeviceFence,
            Operation::ObserveSkip,
            Operation::ObserveSkip,
            Operation::ClearSkip,
            Operation::DeviceFence,
        ]
    );
}

#[test]
fn skip_results_keep_their_positional_values() {
    let mut recorder = Recorder::new();
    for (result, expected) in [
        (
            0,
            BluetoothSchedulerSkipDisposition::UnsupportedHardwareResult,
        ),
        (
            1,
            BluetoothSchedulerSkipDisposition::Completed(BluetoothSchedulerSkipResult::One),
        ),
        (
            2,
            BluetoothSchedulerSkipDisposition::Completed(BluetoothSchedulerSkipResult::Two),
        ),
        (
            3,
            BluetoothSchedulerSkipDisposition::Completed(BluetoothSchedulerSkipResult::Three),
        ),
    ] {
        recorder.skip = (false, result);
        assert_eq!(
            execute_skip_observation(&mut recorder, scheduler(true)),
            expected,
            "result {result}"
        );
    }

    recorder.operations.clear();
    assert_eq!(
        execute_skip_observation(&mut recorder, scheduler(false)),
        BluetoothSchedulerSkipDisposition::SchedulerIdle
    );
    assert!(recorder.operations.is_empty());
}

#[test]
fn cancellation_control_orders_index_request_and_release() {
    let mut recorder = Recorder::new();

    let indexed = execute_cancellation_indexing(&mut recorder, index(3));
    assert_eq!(indexed.hardware_list_index(), index(3));
    let acknowledged = BluetoothSchedulerCancellationSourceAcknowledged { _private: () };
    let requested = execute_cancellation_request(&mut recorder, indexed, acknowledged);
    let _released = execute_cancellation_release(&mut recorder, requested);

    assert_eq!(
        recorder.operations,
        [
            Operation::ClearListIndex,
            Operation::PublishListIndex(index(3)),
            Operation::SetControl,
            Operation::DeviceFence,
            Operation::ClearControl,
            Operation::DeviceFence,
        ]
    );
}

#[test]
fn cancellation_waits_are_sequential_and_end_when_the_scheduler_is_idle() {
    let mut recorder = Recorder::new();
    let indexed = execute_cancellation_indexing(&mut recorder, index(0));
    let acknowledged = BluetoothSchedulerCancellationSourceAcknowledged { _private: () };
    let mut requested = execute_cancellation_request(&mut recorder, indexed, acknowledged);
    recorder.operations.clear();

    // First wait: status 0x324 bit zero still clear.
    assert_eq!(
        execute_cancellation_observation(&mut recorder, &mut requested, scheduler(true)),
        BluetoothSchedulerCancellationDisposition::Pending
    );
    assert_eq!(recorder.operations, [Operation::ObserveStatus0324]);

    // The first wait ends; the second still holds.
    recorder.operations.clear();
    recorder.status_0324 = true;
    assert_eq!(
        execute_cancellation_observation(&mut recorder, &mut requested, scheduler(true)),
        BluetoothSchedulerCancellationDisposition::Pending
    );
    assert_eq!(
        recorder.operations,
        [Operation::ObserveStatus0324, Operation::ObserveStatus0208]
    );

    // The first predicate is not sampled again once it has ended.
    recorder.operations.clear();
    recorder.status_0324 = false;
    recorder.status_0208 = false;
    assert_eq!(
        execute_cancellation_observation(&mut recorder, &mut requested, scheduler(true)),
        BluetoothSchedulerCancellationDisposition::Settled
    );
    assert_eq!(recorder.operations, [Operation::ObserveStatus0208]);

    recorder.operations.clear();
    assert_eq!(
        execute_cancellation_observation(&mut recorder, &mut requested, scheduler(false)),
        BluetoothSchedulerCancellationDisposition::SchedulerIdle
    );
    assert!(recorder.operations.is_empty());
}
