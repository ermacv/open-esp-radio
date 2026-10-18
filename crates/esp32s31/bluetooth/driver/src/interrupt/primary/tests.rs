use crate::interrupt::SchedulerWorkerWakeClass;

use oer_esp32s31_hal::bluetooth::BluetoothPrimaryInterruptEpoch;

use std::vec::Vec;

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    PrimaryEpoch,
    ReferenceGate,
    ReferenceClear,
    Work,
}

struct Backend {
    epoch: Option<BluetoothPrimaryInterruptEpoch>,
    gate: Result<BluetoothSchedulerReferenceGateObservation, BluetoothDiagnosticUnsettled>,
    work: Option<BluetoothSchedulerWorkObservation>,
    operations: Vec<Operation>,
}

impl Backend {
    fn dynamic(
        source_21_pending: bool,
        sources_27_or_28_pending: bool,
        source_3_pending: bool,
        gate_busy: bool,
        work_busy: bool,
        work_state_29: bool,
    ) -> Self {
        Self {
            epoch: Some(BluetoothPrimaryInterruptEpoch::for_dynamic_validation(
                source_21_pending,
                sources_27_or_28_pending,
                source_3_pending,
            )),
            gate: Ok(
                BluetoothSchedulerReferenceGateObservation::from_busy_for_validation(gate_busy),
            ),
            work: Some(
                BluetoothSchedulerWorkObservation::from_fields_for_validation(
                    work_busy,
                    work_state_29,
                    7,
                ),
            ),
            operations: Vec::new(),
        }
    }

    fn fault() -> Self {
        let mut backend = Self::dynamic(false, false, false, false, false, false);
        backend.epoch = Some(BluetoothPrimaryInterruptEpoch::for_fault_validation());
        backend
    }

    fn unclassified() -> Self {
        let mut backend = Self::dynamic(false, false, false, false, false, false);
        backend.epoch = Some(BluetoothPrimaryInterruptEpoch::for_unclassified_validation());
        backend
    }
}

impl PrimaryInterruptBackend for Backend {
    fn capture_primary_and_acknowledge(&mut self) -> BluetoothPrimaryInterruptEpoch {
        self.operations.push(Operation::PrimaryEpoch);
        self.epoch.take().expect("one primary epoch")
    }

    fn capture_scheduler_reference_gate(
        &mut self,
    ) -> Result<BluetoothSchedulerReferenceGateObservation, BluetoothDiagnosticUnsettled> {
        self.operations.push(Operation::ReferenceGate);
        self.gate
    }

    fn clear_scheduler_reference(&mut self) {
        self.operations.push(Operation::ReferenceClear);
    }

    fn capture_scheduler_work(&mut self) -> BluetoothSchedulerWorkObservation {
        self.operations.push(Operation::Work);
        self.work.take().expect("one scheduler work observation")
    }
}

#[test]
fn ordinary_dynamic_epoch_produces_one_paired_scheduler_event() {
    let mut backend = Backend::dynamic(true, false, false, false, true, true);

    let PrimaryInterruptStep::Scheduler(event) = execute_primary_interrupt_step(&mut backend)
    else {
        panic!("dynamic source must produce scheduler work");
    };

    assert_eq!(event.wake().class(), SchedulerWorkerWakeClass::Ordinary);
    assert_eq!(event.wake().deferred_work_publication(), Some(true));
    assert!(event.scheduler_busy());
    assert_eq!(event.current_hardware_list().get(), 7);
    assert_eq!(
        backend.operations,
        [Operation::PrimaryEpoch, Operation::Work]
    );
}

#[test]
fn idle_reference_gate_clears_before_fresh_work_read() {
    let mut backend = Backend::dynamic(false, true, true, false, true, true);

    let PrimaryInterruptStep::Scheduler(event) = execute_primary_interrupt_step(&mut backend)
    else {
        panic!("idle reference gate must continue after the ordered clear");
    };

    assert!(event.scheduler_busy());
    assert_eq!(
        backend.operations,
        [
            Operation::PrimaryEpoch,
            Operation::ReferenceGate,
            Operation::ReferenceClear,
            Operation::Work,
        ]
    );
}

#[test]
fn busy_reference_gate_preserves_reference_then_uses_a_fresh_work_read() {
    let mut backend = Backend::dynamic(false, true, true, true, false, false);

    let PrimaryInterruptStep::Scheduler(event) = execute_primary_interrupt_step(&mut backend)
    else {
        panic!("busy reference gate must continue to deferred work");
    };

    assert_eq!(event.wake().class(), SchedulerWorkerWakeClass::Ordinary);
    assert!(!event.scheduler_busy());
    assert_eq!(
        backend.operations,
        [
            Operation::PrimaryEpoch,
            Operation::ReferenceGate,
            Operation::Work,
        ]
    );
}

#[test]
fn fault_and_empty_epochs_do_not_touch_scheduler_state() {
    let mut fault = Backend::fault();
    assert!(matches!(
        execute_primary_interrupt_step(&mut fault),
        PrimaryInterruptStep::Fault(_)
    ));
    assert_eq!(fault.operations, [Operation::PrimaryEpoch]);

    let mut empty = Backend::dynamic(false, false, false, false, false, false);
    assert!(matches!(
        execute_primary_interrupt_step(&mut empty),
        PrimaryInterruptStep::NoSchedulerWork(_)
    ));
    assert_eq!(empty.operations, [Operation::PrimaryEpoch]);
}

#[test]
fn unclassified_status_fails_closed_without_reading_scheduler_state() {
    let mut backend = Backend::unclassified();

    let PrimaryInterruptStep::Fault(fault) = execute_primary_interrupt_step(&mut backend) else {
        panic!("unclassified status must fail closed");
    };

    assert!(fault.sources().unclassified_pending());
    assert_eq!(backend.operations, [Operation::PrimaryEpoch]);
}

#[test]
fn one_primary_scheduler_event_updates_the_durable_handoff() {
    let mut backend = Backend::dynamic(false, true, true, true, true, true);
    let scheduler_wake = SchedulerWakeCell::new();

    let published = execute_primary_interrupt_step(&mut backend).publish(&scheduler_wake);

    assert!(matches!(
        published,
        PrimaryPublishedInterruptStep::Scheduler {
            scheduler: SchedulerWakePublication::WakeWorker,
            ..
        }
    ));
    assert!(
        scheduler_wake
            .take()
            .expect("scheduler work must be durable")
            .is_marked()
    );

    let empty = execute_primary_interrupt_step(&mut Backend::dynamic(
        false, false, false, false, false, false,
    ))
    .publish(&scheduler_wake);
    assert!(matches!(
        empty,
        PrimaryPublishedInterruptStep::NoSchedulerWork(_)
    ));
    assert!(!scheduler_wake.is_pending());
}

#[test]
fn unsettled_reference_gate_preserves_the_reference_and_publishes_no_work() {
    let mut backend = Backend::dynamic(false, false, true, false, false, false);
    backend.gate = Err(BluetoothDiagnosticUnsettled);

    assert!(matches!(
        execute_primary_interrupt_step(&mut backend),
        PrimaryInterruptStep::DiagnosticUnsettled(_)
    ));
    assert_eq!(
        backend.operations,
        [Operation::PrimaryEpoch, Operation::ReferenceGate]
    );
}
