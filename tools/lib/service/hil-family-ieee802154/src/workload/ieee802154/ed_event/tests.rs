use super::*;

fn nominal() -> Ieee802154EdEventProbeEvidence {
    Ieee802154EdEventProbeEvidence {
        stop: Ieee802154EdEventProbeStop::Complete,
        production_ed_first: Ieee802154PolledEdOutcome::Complete {
            rss_code: -17,
            polls: 2,
        },
        production_ed_second: Some(Ieee802154PolledEdOutcome::Complete {
            rss_code: -18,
            polls: 3,
        }),
        event_enable_before: Ieee802154ValidationEventEnableState::AllMasked,
        event_enable_active: Ieee802154ValidationEventEnableState::EdDoneTimer0RxAbortOnly,
        event_enable_after: Ieee802154ValidationEventEnableState::AllMasked,
        rx_abort_enable_before: Ieee802154ValidationRxAbortEnableState::AllMasked,
        rx_abort_enable_active: Ieee802154ValidationRxAbortEnableState::EdOperationReasonsOnly,
        rx_abort_enable_after: Ieee802154ValidationRxAbortEnableState::AllMasked,
        ed_duration_before: Ieee802154ValidationEdDurationState::Other,
        ed_duration_active: Ieee802154ValidationEdDurationState::ValidationEight,
        ed_duration_after: Ieee802154ValidationEdDurationState::ValidationEight,
        timer0_value_before_start: 0,
        timer0_value_min: 0,
        timer0_value_max: 1,
        timer0_value_after_stop: 1,
        reset_events: Ieee802154ObservedEventState::Clear,
        post_enable_events: Ieee802154ObservedEventState::Clear,
        observed_events: Ieee802154ObservedEventState::EdDoneAndTimer0,
        terminal_events: Ieee802154ObservedEventState::EdDoneAndTimer0,
        after_ed_done_write_events: Ieee802154ObservedEventState::Timer0Only,
        after_timer0_write_events: Ieee802154ObservedEventState::Clear,
        cleanup_pending_events: Ieee802154ObservedEventState::Clear,
        final_events: Ieee802154ObservedEventState::Clear,
        rx_abort_reason: None,
        stop_command_issued: false,
        cleanup_clear: true,
    }
}

#[test]
fn accepts_only_the_exact_closed_selected_write_trace() {
    assert!(validate(nominal(), 100_000).is_ok());

    let mut unexpected = nominal();
    unexpected.observed_events = Ieee802154ObservedEventState::UnexpectedNamed;
    unexpected.rx_abort_reason = Some(Ieee802154RxAbortObservation::Named(
        Ieee802154RxAbortReason::EdStop,
    ));
    assert!(validate(unexpected, 100_000).is_err());

    let mut wrong_write = nominal();
    wrong_write.after_ed_done_write_events = Ieee802154ObservedEventState::EdDoneAndTimer0;
    assert!(validate(wrong_write, 100_000).is_err());

    let mut unexpected_abort_enable = nominal();
    unexpected_abort_enable.rx_abort_enable_before =
        Ieee802154ValidationRxAbortEnableState::EdOperationReasonsOnly;
    assert!(validate(unexpected_abort_enable, 100_000).is_err());

    let mut restored_duration = nominal();
    restored_duration.ed_duration_after = restored_duration.ed_duration_before;
    assert!(validate(restored_duration, 100_000).is_err());

    let mut inactive_timer = nominal();
    inactive_timer.timer0_value_max = inactive_timer.timer0_value_min;
    assert!(validate(inactive_timer, 100_000).is_err());

    let mut missing_second = nominal();
    missing_second.production_ed_second = None;
    assert!(validate(missing_second, 100_000).is_err());

    let mut failed_first = nominal();
    failed_first.production_ed_first = Ieee802154PolledEdOutcome::Timeout { polls: 100_000 };
    assert!(validate(failed_first, 100_000).is_err());
}

#[test]
fn rejects_every_non_complete_stop() {
    for stop in [
        Ieee802154EdEventProbeStop::ProductionEdFailed,
        Ieee802154EdEventProbeStop::UnsupportedSetup,
        Ieee802154EdEventProbeStop::RouteNotQuiesced,
        Ieee802154EdEventProbeStop::ResetNotClear,
        Ieee802154EdEventProbeStop::EdDurationReadbackMismatch,
        Ieee802154EdEventProbeStop::EventEnableReadbackMismatch,
        Ieee802154EdEventProbeStop::RxAbortEnableReadbackMismatch,
        Ieee802154EdEventProbeStop::PostEnableStatusNotClear,
        Ieee802154EdEventProbeStop::TimerActivityTimeout,
        Ieee802154EdEventProbeStop::PairLatchTimeout,
        Ieee802154EdEventProbeStop::EdAborted,
        Ieee802154EdEventProbeStop::UnexpectedEvent,
        Ieee802154EdEventProbeStop::SelectiveWriteMismatch,
        Ieee802154EdEventProbeStop::CleanupNotClear,
    ] {
        let mut evidence = nominal();
        evidence.stop = stop;
        assert!(
            validate(evidence, 100_000).is_err(),
            "accepted stop {stop:?}"
        );
    }
}

#[test]
fn a_failed_boot_keeps_its_semantic_abort_diagnostics() {
    let mut evidence = nominal();
    evidence.stop = Ieee802154EdEventProbeStop::UnexpectedEvent;
    evidence.observed_events = Ieee802154ObservedEventState::RxAbortOnly;
    evidence.terminal_events = Ieee802154ObservedEventState::RxAbortOnly;
    evidence.rx_abort_reason = Some(Ieee802154RxAbortObservation::Named(
        Ieee802154RxAbortReason::EdStop,
    ));
    let results = oer_hil_workload::results::Results::default();
    results.observe("boot-001", &evidence);
    assert!(validate(evidence, 10).is_err());
    let observed = results.snapshot().observations[0].value.clone();
    assert_eq!(observed["terminal_events"], "RxAbortOnly");
    assert_eq!(observed["rx_abort_reason"]["Named"], "EdStop");
}
