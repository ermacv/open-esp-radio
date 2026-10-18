use super::*;

fn nominal() -> Ieee802154EventStatusProbeEvidence {
    Ieee802154EventStatusProbeEvidence {
        stop: Ieee802154EventStatusProbeStop::Complete,
        event_enable_before: Ieee802154ValidationEventEnableState::AllMasked,
        event_enable_active: Ieee802154ValidationEventEnableState::TimerPairOnly,
        event_enable_after: Ieee802154ValidationEventEnableState::AllMasked,
        post_enable_events: Ieee802154ObservedEventState::Clear,
        timer0_value_before_start: 0,
        timer1_value_before_start: 0,
        timer0_value_min: 0,
        timer0_value_max: 1,
        timer1_value_min: 0,
        timer1_value_max: 1,
        timer0_value_after_stop: 1,
        timer1_value_after_stop: 1,
        reset_events: Ieee802154ObservedEventState::Clear,
        dual_observed_events: Ieee802154ObservedEventState::Timer0AndTimer1,
        dual_latched_events: Ieee802154ObservedEventState::Timer0AndTimer1,
        after_timer0_ack_events: Ieee802154ObservedEventState::Timer1Only,
        after_timer1_ack_events: Ieee802154ObservedEventState::Clear,
        distinct_snapshot_events: Ieee802154ObservedEventState::Timer0Only,
        distinct_before_ack_events: Ieee802154ObservedEventState::Timer0AndTimer1,
        distinct_after_ack_events: Ieee802154ObservedEventState::Timer1Only,
        cleanup_pending_events: Ieee802154ObservedEventState::Clear,
        final_events: Ieee802154ObservedEventState::Clear,
    }
}

#[test]
fn accepts_only_the_bounded_selective_ack_observation() {
    assert!(validate(nominal()).is_ok());
}

#[test]
fn the_claim_names_the_narrow_result_and_excluded_properties() {
    assert_eq!(
        RESULT,
        "route-detached-enabled-selective-ack-and-distinct-arrival-observed"
    );
    assert_eq!(
        serde_json::json!(NOT_PROVEN),
        serde_json::json!([
            "full-w1c-semantics",
            "event-enable-generation-vs-visibility-semantics",
            "concurrent-same-bit-arrival",
            "level-triggered-route-behavior",
            "masked-final-status-means-physical-cleanup",
        ])
    );
}

#[test]
fn rejects_every_non_complete_stop() {
    for stop in [
        Ieee802154EventStatusProbeStop::UnsupportedSetup,
        Ieee802154EventStatusProbeStop::RouteNotQuiesced,
        Ieee802154EventStatusProbeStop::ResetNotClear,
        Ieee802154EventStatusProbeStop::EventEnableReadbackMismatch,
        Ieee802154EventStatusProbeStop::PostEnableStatusNotClear,
        Ieee802154EventStatusProbeStop::TimerActivityTimeout,
        Ieee802154EventStatusProbeStop::DualLatchTimeout,
        Ieee802154EventStatusProbeStop::SelectiveAcknowledgeMismatch,
        Ieee802154EventStatusProbeStop::DistinctFirstLatchTimeout,
        Ieee802154EventStatusProbeStop::DistinctSecondLatchTimeout,
        Ieee802154EventStatusProbeStop::CleanupNotClear,
    ] {
        let mut evidence = nominal();
        evidence.stop = stop;
        assert!(validate(evidence).is_err(), "accepted stop {stop:?}");
    }
}

#[test]
fn a_failed_boot_keeps_its_semantic_target_evidence() {
    let mut evidence = nominal();
    evidence.stop = Ieee802154EventStatusProbeStop::TimerActivityTimeout;
    evidence.timer0_value_max = 0;
    let results = oer_hil_workload::results::Results::default();
    results.observe("boot-001", &evidence);
    assert!(validate(evidence).is_err());
    let observed = results.snapshot().observations[0].value.clone();
    assert_eq!(observed["timer0_value_max"], 0);
    assert_eq!(observed["stop"], "TimerActivityTimeout");
}

#[test]
fn rejects_each_wrong_checkpoint() {
    let mut cases = Vec::new();

    let mut evidence = nominal();
    evidence.event_enable_before = Ieee802154ValidationEventEnableState::Unexpected;
    cases.push(("entry event enable", evidence));

    let mut evidence = nominal();
    evidence.event_enable_active = Ieee802154ValidationEventEnableState::AllMasked;
    cases.push(("active event enable", evidence));

    let mut evidence = nominal();
    evidence.event_enable_after = Ieee802154ValidationEventEnableState::TimerPairOnly;
    cases.push(("final event enable", evidence));

    let mut evidence = nominal();
    evidence.reset_events = Ieee802154ObservedEventState::Timer0Only;
    cases.push(("reset", evidence));

    let mut evidence = nominal();
    evidence.post_enable_events = Ieee802154ObservedEventState::Timer0Only;
    cases.push(("post enable", evidence));

    let mut evidence = nominal();
    evidence.dual_latched_events = Ieee802154ObservedEventState::Timer0Only;
    cases.push(("dual missing timer1", evidence));

    let mut evidence = nominal();
    evidence.dual_observed_events = Ieee802154ObservedEventState::Timer0Only;
    cases.push(("dual union omits terminal timer1", evidence));

    let mut evidence = nominal();
    evidence.after_timer0_ack_events = Ieee802154ObservedEventState::Timer0AndTimer1;
    cases.push(("timer0 not acknowledged", evidence));

    let mut evidence = nominal();
    evidence.after_timer0_ack_events = Ieee802154ObservedEventState::Clear;
    cases.push(("timer1 removed with timer0", evidence));

    let mut evidence = nominal();
    evidence.after_timer1_ack_events = Ieee802154ObservedEventState::Timer1Only;
    cases.push(("timer1 not acknowledged", evidence));

    let mut evidence = nominal();
    evidence.distinct_snapshot_events = Ieee802154ObservedEventState::Clear;
    cases.push(("distinct first timer missing", evidence));

    let mut evidence = nominal();
    evidence.distinct_snapshot_events = Ieee802154ObservedEventState::Timer0AndTimer1;
    cases.push(("distinct second timer arrived too early", evidence));

    let mut evidence = nominal();
    evidence.distinct_before_ack_events = Ieee802154ObservedEventState::Timer0Only;
    cases.push(("distinct second timer missing", evidence));

    let mut evidence = nominal();
    evidence.distinct_after_ack_events = Ieee802154ObservedEventState::Timer0AndTimer1;
    cases.push(("distinct timer0 not acknowledged", evidence));

    let mut evidence = nominal();
    evidence.distinct_after_ack_events = Ieee802154ObservedEventState::Clear;
    cases.push(("distinct timer1 removed with timer0", evidence));

    let mut evidence = nominal();
    evidence.cleanup_pending_events = Ieee802154ObservedEventState::Timer0AndTimer1;
    cases.push(("masked cleanup observation", evidence));

    let mut evidence = nominal();
    evidence.final_events = Ieee802154ObservedEventState::Timer1Only;
    cases.push(("final cleanup", evidence));

    for (checkpoint, evidence) in cases {
        assert!(
            validate(evidence).is_err(),
            "accepted wrong {checkpoint} checkpoint"
        );
    }
}

#[test]
fn accepts_either_masked_cleanup_visibility() {
    let mut visible = nominal();
    visible.cleanup_pending_events = Ieee802154ObservedEventState::Timer1Only;
    assert!(validate(visible).is_ok());
}

#[test]
fn requires_counter_activity_and_a_bounded_entry_sample() {
    let mut inactive = nominal();
    inactive.timer0_value_max = inactive.timer0_value_min;
    assert!(validate(inactive).is_err());

    let mut out_of_range = nominal();
    out_of_range.timer1_value_before_start = out_of_range.timer1_value_max + 1;
    assert!(validate(out_of_range).is_err());
}

#[test]
fn rejects_unexpected_or_unclassified_semantic_events() {
    let mut unexpected = nominal();
    unexpected.reset_events = Ieee802154ObservedEventState::UnexpectedNamed;
    assert!(validate(unexpected).is_err());

    let mut unclassified = nominal();
    unclassified.reset_events = Ieee802154ObservedEventState::Unclassified;
    assert!(validate(unclassified).is_err());
}
