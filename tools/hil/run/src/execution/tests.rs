use super::*;

#[test]
fn io_failure_is_broken_but_a_scenario_assertion_is_failed() {
    let io = std::io::Error::from(std::io::ErrorKind::NotConnected);
    let evidence = ExecutionEvidence {
        quarantined: false,
        measurements: Vec::new(),
        interrupted: false,
        failure: Some(classify(&io)),
    };
    assert_eq!(evidence.outcome(), Outcome::Broken);
    assert_eq!(evidence.failure.unwrap().kind, FailureKind::Infrastructure);
    let assertion: Box<dyn std::error::Error + Send + Sync> =
        "target failed the throughput criterion".into();
    let evidence = ExecutionEvidence {
        quarantined: false,
        measurements: Vec::new(),
        interrupted: false,
        failure: Some(classify(&*assertion)),
    };
    assert_eq!(evidence.outcome(), Outcome::Failed);
    assert_eq!(evidence.failure.unwrap().kind, FailureKind::Scenario);
}

#[test]
fn operation_context_preserves_the_infrastructure_classification() {
    let source: Box<dyn std::error::Error + Send + Sync> =
        std::io::Error::from(std::io::ErrorKind::BrokenPipe).into();
    let error = oer_hil_link::error::context("session configuration", source);
    let failure = classify(&*error);
    assert_eq!(failure.kind, FailureKind::Infrastructure);
    assert!(failure.message.starts_with("session configuration:"));
}

#[test]
fn fixture_setup_failure_is_broken_even_through_operation_context() {
    let error = oer_hil_link::error::context(
        "prepare AP client",
        oer_hil_lab::Error::new("OpenWrt radio does not match the laboratory configuration").into(),
    );
    let evidence = ExecutionEvidence {
        quarantined: false,
        failure: Some(classify(&*error)),
        ..Default::default()
    };
    assert_eq!(evidence.outcome(), Outcome::Broken);
    assert_eq!(evidence.failure.unwrap().kind, FailureKind::Infrastructure);
}

#[test]
fn a_quarantined_board_is_its_own_outcome() {
    let evidence = ExecutionEvidence {
        quarantined: true,
        measurements: Vec::new(),
        interrupted: false,
        failure: Some(Failure::new(FailureKind::Infrastructure, "unreachable")),
    };
    assert_eq!(evidence.outcome(), Outcome::BoardQuarantined);
}
