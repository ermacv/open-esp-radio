use std::vec::Vec;

use super::{
    BluetoothInterruptControl, BluetoothPrimaryBank0Status, BluetoothPrimaryBank1Status,
    BluetoothPrimaryInterruptControl, BluetoothSchedulerRunInterruptControl,
    execute_primary_interrupt_epoch, execute_primary_prepare, execute_primary_release,
    execute_scheduler_run_interrupt_prepare,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    ClearBank0,
    ClearBank1,
    EnableBank0,
    EnableBank1,
    PrepareOutput,
    ReleaseOutput0,
    ReleaseOutput1,
    MaskBank0,
    MaskBank1,
}

#[derive(Default)]
struct SetupRecorder {
    operations: Vec<Operation>,
}

impl BluetoothInterruptControl for SetupRecorder {
    fn clear_primary_baseline_bank_0(&mut self) {
        self.operations.push(Operation::ClearBank0);
    }

    fn clear_primary_baseline_bank_1(&mut self) {
        self.operations.push(Operation::ClearBank1);
    }

    fn enable_primary_baseline_bank_0(&mut self) {
        self.operations.push(Operation::EnableBank0);
    }

    fn enable_primary_baseline_bank_1(&mut self) {
        self.operations.push(Operation::EnableBank1);
    }

    fn prepare_output(&mut self) {
        self.operations.push(Operation::PrepareOutput);
    }

    fn release_output_0(&mut self) {
        self.operations.push(Operation::ReleaseOutput0);
    }

    fn release_output_1(&mut self) {
        self.operations.push(Operation::ReleaseOutput1);
    }

    fn mask_primary_baseline_bank_0(&mut self) {
        self.operations.push(Operation::MaskBank0);
    }

    fn mask_primary_baseline_bank_1(&mut self) {
        self.operations.push(Operation::MaskBank1);
    }
}

impl BluetoothSchedulerRunInterruptControl for SetupRecorder {
    fn clear_scheduler_run_bank_0(&mut self) {
        self.operations.push(Operation::ClearBank0);
    }

    fn clear_scheduler_run_bank_1(&mut self) {
        self.operations.push(Operation::ClearBank1);
    }

    fn enable_scheduler_run_bank_0(&mut self) {
        self.operations.push(Operation::EnableBank0);
    }

    fn enable_scheduler_run_bank_1(&mut self) {
        self.operations.push(Operation::EnableBank1);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EpochOperation {
    SampleBank0,
    SampleBank1,
    AcknowledgeBank0,
    AcknowledgeBank1,
    ReadDiagnosticDetail0,
    ReadDiagnosticDetail1,
    ReadDiagnosticState,
}

struct EpochRecorder {
    bank_0: BluetoothPrimaryBank0Status,
    bank_1: BluetoothPrimaryBank1Status,
    operations: Vec<EpochOperation>,
}

impl EpochRecorder {
    fn fault_sources() -> Self {
        Self {
            bank_0: BluetoothPrimaryBank0Status {
                source_15_pending: true,
                source_21_pending: false,
                sources_27_or_28_pending: false,
                unclassified_pending: false,
            },
            bank_1: BluetoothPrimaryBank1Status {
                source_3_pending: false,
                source_8_pending: true,
                source_9_pending: true,
                source_12_pending: true,
                unclassified_pending: false,
            },
            operations: Vec::new(),
        }
    }

    fn empty() -> Self {
        Self {
            bank_0: BluetoothPrimaryBank0Status {
                source_15_pending: false,
                source_21_pending: false,
                sources_27_or_28_pending: false,
                unclassified_pending: false,
            },
            bank_1: BluetoothPrimaryBank1Status {
                source_3_pending: false,
                source_8_pending: false,
                source_9_pending: false,
                source_12_pending: false,
                unclassified_pending: false,
            },
            operations: Vec::new(),
        }
    }
}

impl BluetoothPrimaryInterruptControl for EpochRecorder {
    type Bank0Snapshot = BluetoothPrimaryBank0Status;
    type Bank1Snapshot = BluetoothPrimaryBank1Status;

    fn sample_bank_0(&mut self) -> Self::Bank0Snapshot {
        self.operations.push(EpochOperation::SampleBank0);
        self.bank_0
    }

    fn sample_bank_1(&mut self) -> Self::Bank1Snapshot {
        self.operations.push(EpochOperation::SampleBank1);
        self.bank_1
    }

    fn bank_0_status(&self, snapshot: &Self::Bank0Snapshot) -> BluetoothPrimaryBank0Status {
        *snapshot
    }

    fn bank_1_status(&self, snapshot: &Self::Bank1Snapshot) -> BluetoothPrimaryBank1Status {
        *snapshot
    }

    fn acknowledge_bank_0(&mut self, _snapshot: Self::Bank0Snapshot) {
        self.operations.push(EpochOperation::AcknowledgeBank0);
    }

    fn acknowledge_bank_1(&mut self, _snapshot: Self::Bank1Snapshot) {
        self.operations.push(EpochOperation::AcknowledgeBank1);
    }

    fn capture_diagnostic_detail_0(&mut self) {
        self.operations.push(EpochOperation::ReadDiagnosticDetail0);
    }

    fn capture_diagnostic_detail_1(&mut self) {
        self.operations.push(EpochOperation::ReadDiagnosticDetail1);
    }

    fn capture_diagnostic_state(&mut self) {
        self.operations.push(EpochOperation::ReadDiagnosticState);
    }
}

#[test]
fn primary_epoch_acknowledges_before_conditional_fault_capture() {
    let mut recorder = EpochRecorder::fault_sources();
    let epoch = execute_primary_interrupt_epoch(&mut recorder);

    assert_eq!(
        recorder.operations,
        [
            EpochOperation::SampleBank0,
            EpochOperation::SampleBank1,
            EpochOperation::AcknowledgeBank0,
            EpochOperation::AcknowledgeBank1,
            EpochOperation::ReadDiagnosticDetail0,
            EpochOperation::ReadDiagnosticDetail1,
            EpochOperation::ReadDiagnosticState,
        ]
    );
    let faults = epoch.fault_sources();
    assert!(faults.bank_0_source_15_pending());
    assert!(faults.bank_1_source_8_pending());
    assert!(faults.bank_1_source_9_pending());
    assert!(faults.bank_1_source_12_pending());
}

#[test]
fn primary_epoch_skips_diagnostic_reads_without_matching_sources() {
    let mut recorder = EpochRecorder::empty();
    let epoch = execute_primary_interrupt_epoch(&mut recorder);

    assert_eq!(
        recorder.operations,
        [
            EpochOperation::SampleBank0,
            EpochOperation::SampleBank1,
            EpochOperation::AcknowledgeBank0,
            EpochOperation::AcknowledgeBank1,
        ]
    );
    assert!(!epoch.fault_sources().is_fault());
}

#[test]
fn scheduler_run_interrupts_clear_stale_sources_before_enabling_them() {
    let mut recorder = SetupRecorder::default();
    execute_scheduler_run_interrupt_prepare(&mut recorder);
    assert_eq!(
        recorder.operations,
        [
            Operation::ClearBank0,
            Operation::ClearBank1,
            Operation::EnableBank0,
            Operation::EnableBank1,
        ]
    );
}

#[test]
fn primary_prepare_preserves_clear_enable_strobe_order() {
    let mut recorder = SetupRecorder::default();
    execute_primary_prepare(&mut recorder);

    assert_eq!(
        recorder.operations,
        [
            Operation::ClearBank0,
            Operation::ClearBank1,
            Operation::EnableBank0,
            Operation::EnableBank1,
            Operation::PrepareOutput,
        ]
    );
}

#[test]
fn primary_release_preserves_strobe_then_mask_order() {
    let mut recorder = SetupRecorder::default();
    execute_primary_release(&mut recorder);

    assert_eq!(
        recorder.operations,
        [
            Operation::ReleaseOutput0,
            Operation::ReleaseOutput1,
            Operation::MaskBank0,
            Operation::MaskBank1,
        ]
    );
}
