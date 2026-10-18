use super::*;

#[test]
fn missing_typed_completion_cannot_qualify_a_cycle() {
    assert!(!validate_cycle_completion(None, 2).unwrap());
}

#[test]
fn typed_completion_requires_every_owner_edge() {
    let mut incomplete = StationEpochEvidence::COMPLETE;
    incomplete.scan_owners_returned = false;
    assert!(validate_cycle_completion(Some(incomplete), 2).is_err());
    assert!(validate_cycle_completion(Some(StationEpochEvidence::COMPLETE), 2).unwrap());
}

#[test]
fn retryable_peer_failure_does_not_preempt_lifecycle_policy() {
    let retrying = StationLifecycleEvent::AttemptFailed {
        generation: 1,
        attempt: 1,
        stage: oer_hil_protocol::wifi::StationFailureStage::Association,
        reason: StationAttemptFailureReason::PeerProtocol,
    };
    assert!(validate_cycle_event(retrying, 2).is_ok());
}

#[test]
fn fatal_or_exhausted_lifecycle_fails_immediately() {
    let fatal = StationLifecycleEvent::AttemptFailed {
        generation: 1,
        attempt: 1,
        stage: oer_hil_protocol::wifi::StationFailureStage::Hardware,
        reason: StationAttemptFailureReason::ContractViolation,
    };
    assert!(validate_cycle_event(fatal, 2).is_err());
    let exhausted = StationLifecycleEvent::RetryExhausted {
        generation: 1,
        attempts: 3,
        stage: oer_hil_protocol::wifi::StationFailureStage::CandidateSelection,
        reason: StationAttemptFailureReason::NoCandidate,
    };
    assert!(validate_cycle_event(exhausted, 2).is_err());
}

#[test]
fn cycle_requires_ordered_disconnect_and_next_generation_connect() {
    let mut progress = CycleProgress::default();
    assert!(
        progress
            .observe_lifecycle(
                StationLifecycleEvent::Connected {
                    generation: 2,
                    association_bandwidth_mhz: None,
                    security: None,
                },
                1,
                1,
            )
            .is_err()
    );

    progress
        .observe_lifecycle(
            StationLifecycleEvent::Disconnected {
                generation: 1,
                reason: oer_hil_protocol::wifi::StationDisconnectReason::ReconnectRequested,
            },
            1,
            1,
        )
        .unwrap();
    progress
        .observe_lifecycle(
            StationLifecycleEvent::Connected {
                generation: 2,
                association_bandwidth_mhz: None,
                security: None,
            },
            1,
            1,
        )
        .unwrap();
    assert!(!progress.complete());
    progress.owners_complete = true;
    assert!(progress.complete());
}

#[test]
fn typed_configuration_preserves_workload_bounds() {
    assert!(
        Config {
            cycles: 0,
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        Config {
            boots: 101,
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        Config {
            initial_hold: Duration::from_secs(31),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        Config {
            timeout: Duration::from_secs(301),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
}

#[test]
fn first_boot_may_create_or_replace_the_startup_artifact() {
    for disposition in [
        StartupArtifactDisposition::Created,
        StartupArtifactDisposition::Replaced,
        StartupArtifactDisposition::Restored,
    ] {
        assert!(
            validate_startup_artifact_replay(
                1,
                Some(StartupArtifactStatus {
                    disposition,
                    total_length: 252,
                    initialization_elapsed_micros: 1,
                }),
            )
            .is_ok()
        );
    }
}

#[test]
fn later_boots_require_restoration_of_the_persisted_artifact() {
    let status = |disposition| {
        Some(StartupArtifactStatus {
            disposition,
            total_length: 252,
            initialization_elapsed_micros: 1,
        })
    };
    assert!(
        validate_startup_artifact_replay(2, status(StartupArtifactDisposition::Restored)).is_ok()
    );
    assert!(
        validate_startup_artifact_replay(2, status(StartupArtifactDisposition::Created)).is_err()
    );
    assert!(
        validate_startup_artifact_replay(2, status(StartupArtifactDisposition::Replaced)).is_err()
    );
    assert!(validate_startup_artifact_replay(2, None).is_err());
}
