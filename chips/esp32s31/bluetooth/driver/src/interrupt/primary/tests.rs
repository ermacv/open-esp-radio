use crate::interrupt::{
    PrimaryInterruptClassification, PrimaryWakeFault, SchedulerWakeBatch, SchedulerWakeCell,
    SchedulerWakePublication, SchedulerWorkerWakeClass,
};

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
    gate: Result<BluetoothSchedulerReferenceGateObservation, Unsettled>,
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
            epoch: Some(BluetoothPrimaryInterruptEpoch::for_dynamic_host_test(
                source_21_pending,
                sources_27_or_28_pending,
                source_3_pending,
            )),
            gate: Ok(
                BluetoothSchedulerReferenceGateObservation::from_busy_for_host_test(gate_busy),
            ),
            work: Some(
                BluetoothSchedulerWorkObservation::from_fields_for_host_test(
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
        backend.epoch = Some(BluetoothPrimaryInterruptEpoch::for_fault_host_test());
        backend
    }

    fn unclassified() -> Self {
        let mut backend = Self::dynamic(false, false, false, false, false, false);
        backend.epoch = Some(BluetoothPrimaryInterruptEpoch::for_unclassified_host_test());
        backend
    }
}

impl PrimaryInterruptBackend for Backend {
    fn capture_primary_and_acknowledge(&mut self) -> Option<BluetoothPrimaryInterruptEpoch> {
        self.operations.push(Operation::PrimaryEpoch);
        self.epoch.take()
    }

    fn capture_scheduler_reference_gate(
        &mut self,
    ) -> Result<BluetoothSchedulerReferenceGateObservation, Unsettled> {
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

fn classify(
    capture: PrimaryInterruptCapture,
) -> Option<Result<SchedulerWakeBatch, PrimaryWakeFault>> {
    let cell = SchedulerWakeCell::new(oer_interrupt_table::signal::host::deferred_signal());
    let _ = cell.publish_from_interrupt(capture);
    cell.take()
        .expect("host P2 context")
        .map(|batch| batch.classify())
}

#[test]
fn ordinary_dynamic_epoch_captures_then_p2_classifies_one_event() {
    let mut backend = Backend::dynamic(true, false, false, false, true, true);
    let capture = execute_primary_interrupt_capture(&mut backend);
    let event = classify(capture)
        .expect("work capture")
        .expect("fault-free inputs");
    assert!(!event.is_marked());
    assert_eq!(
        backend.operations,
        [Operation::PrimaryEpoch, Operation::Work]
    );
}

#[test]
fn idle_reference_gate_clears_before_fresh_work_read() {
    let mut backend = Backend::dynamic(false, true, true, false, true, true);
    let capture = execute_primary_interrupt_capture(&mut backend);
    assert!(
        classify(capture)
            .expect("capture")
            .expect("no fault")
            .is_marked()
    );
    assert_eq!(
        backend.operations,
        [
            Operation::PrimaryEpoch,
            Operation::ReferenceGate,
            Operation::ReferenceClear,
            Operation::Work
        ]
    );
}

#[test]
fn busy_reference_gate_preserves_reference_then_uses_a_fresh_work_read() {
    let mut backend = Backend::dynamic(false, true, true, true, false, false);
    let capture = execute_primary_interrupt_capture(&mut backend);
    assert!(
        !classify(capture)
            .expect("capture")
            .expect("no fault")
            .is_marked()
    );
    assert_eq!(
        backend.operations,
        [
            Operation::PrimaryEpoch,
            Operation::ReferenceGate,
            Operation::Work
        ]
    );
}

#[test]
fn a_clear_bank_captures_nothing_and_publishes_nothing() {
    let mut backend = Backend::dynamic(false, false, false, false, false, false);
    backend.epoch = None;
    let capture = execute_primary_interrupt_capture(&mut backend);
    assert_eq!(capture, PrimaryInterruptCapture(0));
    assert_eq!(backend.operations, [Operation::PrimaryEpoch]);
    let cell = SchedulerWakeCell::new(oer_interrupt_table::signal::host::deferred_signal());
    assert_eq!(cell.publish_from_interrupt(capture), Ok(None));
}

#[test]
fn fault_and_empty_epochs_do_not_touch_scheduler_state() {
    let mut fault = Backend::fault();
    let capture = execute_primary_interrupt_capture(&mut fault);
    assert!(
        classify(capture)
            .expect("fault capture")
            .expect_err("no work authority")
            .bank_1_source_8_pending()
    );
    assert_eq!(fault.operations, [Operation::PrimaryEpoch]);
    let mut empty = Backend::dynamic(false, false, false, false, false, false);
    assert_eq!(
        classify(execute_primary_interrupt_capture(&mut empty)),
        None
    );
    assert_eq!(empty.operations, [Operation::PrimaryEpoch]);
}

#[test]
fn unclassified_status_fails_closed_without_reading_scheduler_state() {
    let mut backend = Backend::unclassified();
    let capture = execute_primary_interrupt_capture(&mut backend);
    assert!(
        classify(capture)
            .expect("capture")
            .expect_err("unclassified source")
            .unclassified_pending()
    );
    assert_eq!(backend.operations, [Operation::PrimaryEpoch]);
}

#[test]
fn unsettled_reference_gate_preserves_reference_and_reaches_p2_as_fault() {
    let mut backend = Backend::dynamic(false, false, true, false, false, false);
    backend.gate = Err(Unsettled);
    let capture = execute_primary_interrupt_capture(&mut backend);
    assert!(
        classify(capture)
            .expect("capture")
            .expect_err("failed settle")
            .diagnostic_unsettled()
    );
    assert_eq!(
        backend.operations,
        [Operation::PrimaryEpoch, Operation::ReferenceGate]
    );
}

fn dynamic_inputs(key: u8) -> (bool, bool, bool, bool) {
    (key & 1 != 0, key & 2 != 0, key & 4 != 0, key & 8 != 0)
}
fn reference_marker(key: u8) -> Option<bool> {
    let (a, b, c, deferred) = dynamic_inputs(key);
    let epoch = BluetoothPrimaryInterruptEpoch::for_dynamic_host_test(a, b, c);
    let classification = PrimaryInterruptClassification::from_epoch(epoch).expect("no fault");
    classification.work_classifier().map(|classifier| {
        classifier
            .classify(
                &BluetoothSchedulerWorkObservation::from_fields_for_host_test(true, deferred, 7),
            )
            .class()
            == SchedulerWorkerWakeClass::Marked
    })
}

#[test]
fn every_pair_of_coalesced_inputs_preserves_the_reference_marker_without_cross_pairing() {
    // In particular source-27/sample=false followed by source-21/sample=true
    // must remain ordinary: independently OR-ing sources and samples is wrong.
    for first in 0..16 {
        for second in 0..16 {
            let cell = SchedulerWakeCell::new(oer_interrupt_table::signal::host::deferred_signal());
            let mut pending = false;
            for key in [first, second] {
                let (a, b, c, deferred) = dynamic_inputs(key);
                let mut backend = Backend::dynamic(a, b, c, false, true, deferred);
                let published = cell
                    .publish_from_interrupt(execute_primary_interrupt_capture(&mut backend))
                    .unwrap();
                if reference_marker(key).is_some() {
                    assert_eq!(
                        published,
                        Some(if pending {
                            SchedulerWakePublication::Coalesced
                        } else {
                            SchedulerWakePublication::Pended
                        })
                    );
                    pending = true;
                } else {
                    assert_eq!(published, None);
                }
            }
            let batch = cell.take().expect("host P2 context");
            assert_eq!(batch.is_some(), pending);
            if let Some(batch) = batch {
                let marked = batch.classify().expect("no fault").is_marked();
                assert_eq!(
                    marked,
                    reference_marker(first) == Some(true) || reference_marker(second) == Some(true),
                    "inputs {first}, {second}"
                );
            }
            assert_eq!(cell.take(), Ok(None));
        }
    }
}

#[test]
fn coalesced_faults_preempt_work_and_keep_each_source_until_p2_takes_them() {
    let cell = SchedulerWakeCell::new(oer_interrupt_table::signal::host::deferred_signal());
    let mut unsettled = Backend::dynamic(false, false, true, false, false, false);
    unsettled.gate = Err(Unsettled);
    for mut backend in [
        Backend::dynamic(false, true, false, false, false, true),
        Backend::fault(),
        Backend::unclassified(),
        unsettled,
    ] {
        let _ = cell.publish_from_interrupt(execute_primary_interrupt_capture(&mut backend));
    }
    let fault = cell
        .take()
        .expect("host P2")
        .expect("captured inputs")
        .classify()
        .expect_err("faults must not issue a scheduler-work authority");
    assert!(fault.bank_1_source_8_pending());
    assert!(fault.unclassified_pending());
    assert!(fault.diagnostic_unsettled());
    assert!(!fault.bank_0_source_15_pending());
    assert!(!fault.bank_1_source_9_pending());
    assert!(!fault.bank_1_source_12_pending());
    assert_eq!(cell.take(), Ok(None));
}

#[test]
fn overflow_is_counted_without_a_scheduler_suffix_and_survives_fault_coincidence() {
    for fault in [false, true] {
        let mut backend = if fault {
            Backend::fault()
        } else {
            Backend::dynamic(false, false, false, false, false, false)
        };
        backend.epoch = backend
            .epoch
            .take()
            .map(BluetoothPrimaryInterruptEpoch::with_controller_time_overflow_for_host_test);
        let cell = SchedulerWakeCell::new(oer_interrupt_table::signal::host::deferred_signal());
        cell.publish_from_interrupt(execute_primary_interrupt_capture(&mut backend))
            .unwrap();
        assert_eq!(cell.controller_time_overflows().0, 1);
        assert_eq!(backend.operations, [Operation::PrimaryEpoch]);
        let result = cell.take().unwrap().unwrap().classify();
        if fault {
            assert!(result.is_err());
        } else {
            let batch = result.unwrap();
            assert_eq!(
                batch.controller_time_overflows(),
                Some(cell.controller_time_overflows())
            );
            assert!(!batch.has_scheduler_work());
        }
    }
}

#[test]
fn overflow_and_marked_scheduler_work_remain_independent_when_captured_together() {
    let mut backend = Backend::dynamic(false, true, false, false, true, true);
    backend.epoch = backend
        .epoch
        .take()
        .map(BluetoothPrimaryInterruptEpoch::with_controller_time_overflow_for_host_test);
    let cell = SchedulerWakeCell::new(oer_interrupt_table::signal::host::deferred_signal());
    cell.publish_from_interrupt(execute_primary_interrupt_capture(&mut backend))
        .unwrap();
    let batch = cell.take().unwrap().unwrap().classify().unwrap();
    assert!(batch.is_marked());
    assert!(batch.has_scheduler_work());
    assert_eq!(batch.controller_time_overflows().unwrap().0, 1);
    assert_eq!(
        backend.operations,
        [Operation::PrimaryEpoch, Operation::Work]
    );
}
