use oer_esp32s31_hal::bluetooth::BluetoothPrimaryInterruptEpoch;

use super::{
    BluetoothSchedulerReferenceGateObservation, BluetoothSchedulerWorkObservation,
    PrimaryInterruptClassification, PrimarySchedulerTrigger, SchedulerReferenceAction,
    SchedulerReferenceGate, SchedulerWorkClassifier, SchedulerWorkerWake, SchedulerWorkerWakeClass,
};

const fn trigger(
    source_21_pending: bool,
    sources_27_or_28_pending: bool,
    source_3_pending: bool,
) -> PrimarySchedulerTrigger {
    PrimarySchedulerTrigger::from_dynamic_fields_for_validation(
        source_21_pending,
        sources_27_or_28_pending,
        source_3_pending,
    )
}

const fn classify_work(
    trigger: PrimarySchedulerTrigger,
    busy: bool,
    state_29: bool,
) -> Option<SchedulerWorkerWake> {
    match trigger.work_inputs() {
        Some((mark_candidate, state_publication_requested)) => Some(
            SchedulerWorkClassifier {
                mark_candidate,
                state_publication_requested,
            }
            .classify(
                &BluetoothSchedulerWorkObservation::from_fields_for_validation(busy, state_29, 0),
            ),
        ),
        None => None,
    }
}

#[test]
fn baseline_fault_preempts_dynamic_scheduler_work() {
    let epoch = BluetoothPrimaryInterruptEpoch::for_fault_validation();
    let fault = PrimaryInterruptClassification::from_epoch(epoch)
        .expect_err("a baseline assertion source must preempt scheduler work");

    assert!(fault.sources().is_fault());
    assert!(fault.sources().bank_1_source_8_pending());
}

#[test]
fn fault_free_epoch_reaches_dynamic_scheduler_classifier() {
    let epoch = BluetoothPrimaryInterruptEpoch::for_dynamic_validation(false, true, true);
    let classification = PrimaryInterruptClassification::from_epoch(epoch)
        .expect("dynamic sources are not fault lanes");

    assert_eq!(
        classification.scheduler_trigger(),
        PrimarySchedulerTrigger::Bank1Source3 {
            bank_0_sources_27_or_28_pending: true,
        }
    );
}

#[test]
fn bank_zero_trigger_table_preserves_source_precedence_and_pairing() {
    assert_eq!(trigger(false, false, false), PrimarySchedulerTrigger::None);
    assert_eq!(
        trigger(true, false, false),
        PrimarySchedulerTrigger::Bank0Source21
    );
    assert_eq!(
        trigger(false, true, false),
        PrimarySchedulerTrigger::Bank0Sources27Or28 {
            source_21_pending: false,
        }
    );
    assert_eq!(
        trigger(true, true, false),
        PrimarySchedulerTrigger::Bank0Sources27Or28 {
            source_21_pending: true,
        }
    );
}

#[test]
fn bank_one_source_three_has_precedence_and_retains_mark_candidate() {
    assert_eq!(
        trigger(true, false, true),
        PrimarySchedulerTrigger::Bank1Source3 {
            bank_0_sources_27_or_28_pending: false,
        }
    );
    assert_eq!(
        trigger(false, true, true),
        PrimarySchedulerTrigger::Bank1Source3 {
            bank_0_sources_27_or_28_pending: true,
        }
    );
}

#[test]
fn reference_gate_requires_post_clear_action_only_when_not_busy() {
    let gate = SchedulerReferenceGate;

    assert_eq!(
        gate.classify(BluetoothSchedulerReferenceGateObservation::from_busy_for_validation(false)),
        SchedulerReferenceAction::ClearReferenceAndContinue
    );
    assert_eq!(
        gate.classify(BluetoothSchedulerReferenceGateObservation::from_busy_for_validation(true)),
        SchedulerReferenceAction::PreserveReference
    );
}

#[test]
fn source_twenty_one_requests_ordinary_work_and_state_publication() {
    for (busy, state_29, expected_publication) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (true, true, true),
    ] {
        let wake = classify_work(PrimarySchedulerTrigger::Bank0Source21, busy, state_29)
            .expect("source 21 must request work");
        assert_eq!(wake.class(), SchedulerWorkerWakeClass::Ordinary);
        assert_eq!(wake.deferred_work_publication(), Some(expected_publication));
    }
}

#[test]
fn sources_twenty_seven_or_twenty_eight_mark_only_requested_deferred_work() {
    let unmarked = classify_work(
        PrimarySchedulerTrigger::Bank0Sources27Or28 {
            source_21_pending: false,
        },
        true,
        false,
    )
    .expect("high source group must request work");
    let marked = classify_work(
        PrimarySchedulerTrigger::Bank0Sources27Or28 {
            source_21_pending: false,
        },
        true,
        true,
    )
    .expect("high source group must request work");

    assert_eq!(unmarked.class(), SchedulerWorkerWakeClass::Ordinary);
    assert_eq!(marked.class(), SchedulerWorkerWakeClass::Marked);
    assert_eq!(marked.deferred_work_publication(), None);
}

#[test]
fn combined_bank_zero_trigger_marks_and_publishes_the_same_second_read() {
    let wake = classify_work(
        PrimarySchedulerTrigger::Bank0Sources27Or28 {
            source_21_pending: true,
        },
        true,
        true,
    )
    .expect("combined bank-zero trigger must request work");

    assert_eq!(wake.class(), SchedulerWorkerWakeClass::Marked);
    assert_eq!(wake.deferred_work_publication(), Some(true));
}

#[test]
fn bank_one_trigger_always_publishes_and_marks_only_with_the_high_bank_zero_group() {
    let ordinary = classify_work(
        PrimarySchedulerTrigger::Bank1Source3 {
            bank_0_sources_27_or_28_pending: false,
        },
        true,
        true,
    )
    .expect("bank-one trigger must request work");
    let marked = classify_work(
        PrimarySchedulerTrigger::Bank1Source3 {
            bank_0_sources_27_or_28_pending: true,
        },
        true,
        true,
    )
    .expect("bank-one trigger must request work");

    assert_eq!(ordinary.class(), SchedulerWorkerWakeClass::Ordinary);
    assert_eq!(marked.class(), SchedulerWorkerWakeClass::Marked);
    assert_eq!(ordinary.deferred_work_publication(), Some(true));
    assert_eq!(marked.deferred_work_publication(), Some(true));
}

#[test]
fn no_dynamic_trigger_produces_no_scheduler_work() {
    assert_eq!(
        classify_work(PrimarySchedulerTrigger::None, true, true),
        None
    );
}
