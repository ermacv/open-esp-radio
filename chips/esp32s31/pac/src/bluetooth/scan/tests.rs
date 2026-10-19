use std::vec::Vec;

use super::{BluetoothScanStartTransaction, execute_scan_start_transaction};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScanStartStep {
    BackoffState1,
    BackoffState0,
    UpperLimitMax,
}

#[derive(Default)]
struct RecordingScanStartTransaction {
    steps: Vec<ScanStartStep>,
}

impl BluetoothScanStartTransaction for RecordingScanStartTransaction {
    fn initialize_backoff_state_1(&mut self) {
        self.steps.push(ScanStartStep::BackoffState1);
    }

    fn initialize_backoff_state_0(&mut self) {
        self.steps.push(ScanStartStep::BackoffState0);
    }

    fn publish_standard_upper_limit_max(&mut self) {
        self.steps.push(ScanStartStep::UpperLimitMax);
    }
}

#[test]
fn scanner_start_initializes_the_backoff_state_before_its_upper_limit() {
    let mut transaction = RecordingScanStartTransaction::default();
    execute_scan_start_transaction(&mut transaction);
    assert_eq!(
        transaction.steps,
        [
            ScanStartStep::BackoffState1,
            ScanStartStep::BackoffState0,
            ScanStartStep::UpperLimitMax,
        ]
    );
}
