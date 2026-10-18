use super::*;

fn sent(bytes: u64, elapsed: u64) -> HostTransmission {
    HostTransmission {
        source: std::net::Ipv4Addr::LOCALHOST,
        bytes,
        datagrams: bytes / 1000,
        elapsed: Duration::from_secs(elapsed),
        maximum_lateness: Duration::ZERO,
        maximum_catch_up_datagrams: 1,
        deadline_resets: 0,
    }
}

#[test]
fn under_offer_is_independent_of_delivery_and_retains_both_time_bases() {
    let assessment = Assessment::new(
        1_000_000,
        Duration::from_secs(12),
        Some(sent(750_000, 12)),
        Some(95),
    );
    assert!(matches!(assessment.status, Status::UnderOffered));
    assert_eq!(assessment.accepted_bps_over_window, Some(500_000));
    assert!(
        assessment
            .validate(1)
            .unwrap_err()
            .to_string()
            .contains("flow 1 offered load not met")
    );
    let assessment = Assessment::new(
        1_000_000,
        Duration::from_secs(12),
        Some(sent(1_500_000, 24)),
        Some(95),
    );
    assert_eq!(assessment.accepted_bps_over_window, Some(1_000_000));
    assert_eq!(assessment.accepted_bps_over_sender_elapsed, Some(500_000));
    assert!(
        assessment.validate(0).is_err(),
        "late completion cannot prove the requested rate"
    );
}

#[test]
fn optional_criterion_does_not_claim_an_unassessed_offer_passed() {
    let duration = Duration::from_secs(12);
    let adequate = Assessment::new(1_000_000, duration, Some(sent(1_500_000, 12)), Some(95));
    assert!(matches!(adequate.status, Status::Met));
    adequate.validate(0).unwrap();
    let unassessed = Assessment::new(1_000_000, duration, Some(sent(0, 12)), None);
    assert!(matches!(unassessed.status, Status::NotAssessed));
    unassessed.validate(0).unwrap();
    let missing = Assessment::new(1_000_000, duration, None, Some(95));
    assert!(matches!(missing.status, Status::Unavailable));
    assert!(missing.validate(0).is_err());
}
