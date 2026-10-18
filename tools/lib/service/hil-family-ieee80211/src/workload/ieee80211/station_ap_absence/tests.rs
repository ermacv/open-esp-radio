use super::*;

#[test]
fn another_attempt_cannot_qualify_retry_exhaustion() {
    let error = validate_event(
        StationLifecycleEvent::RetryExhausted {
            generation: 1,
            attempts: 2,
            stage: StationFailureStage::CandidateSelection,
            reason: StationAttemptFailureReason::NoCandidate,
        },
        StationLifecycleEvent::RetryExhausted {
            generation: 1,
            attempts: QUALIFIED_ATTEMPTS,
            stage: StationFailureStage::CandidateSelection,
            reason: StationAttemptFailureReason::NoCandidate,
        },
        "retry exhaustion",
    )
    .unwrap_err();
    assert!(error.to_string().contains("attempts: 2"));
}

#[test]
fn typed_configuration_preserves_workload_bounds() {
    assert!(
        Config {
            timeout: Duration::from_secs(301),
            ..Config::default()
        }
        .validate()
        .is_err()
    );
}

#[test]
fn initial_absence_cannot_be_satisfied_by_a_recovery_epoch_or_another_role() {
    let initial = StationLifecycleEvent::RetryExhausted {
        generation: 0,
        attempts: QUALIFIED_ATTEMPTS,
        stage: StationFailureStage::CandidateSelection,
        reason: StationAttemptFailureReason::NoCandidate,
    };
    let recovery = StationLifecycleEvent::RetryExhausted {
        generation: 1,
        attempts: QUALIFIED_ATTEMPTS,
        stage: StationFailureStage::CandidateSelection,
        reason: StationAttemptFailureReason::NoCandidate,
    };
    assert!(validate_event(initial, initial, "initial absence").is_ok());
    assert!(validate_event(recovery, initial, "initial absence").is_err());
    use oer_hil_protocol::{wifi::WifiRole, wifi::WifiRoleTransitionEvidence};
    let admitted = WifiRoleTransitionEvidence {
        previous: WifiRole::Idle,
        current: WifiRole::Station,
        generation: 1,
    };
    assert!(validate_service_admission(admitted).is_ok());
    assert!(
        validate_service_admission(WifiRoleTransitionEvidence {
            current: WifiRole::AccessPoint,
            ..admitted
        })
        .is_err()
    );
}
