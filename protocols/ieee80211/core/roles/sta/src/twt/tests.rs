use super::*;
use oer_protocols_ieee80211_wire::tsf::TsfInstant;
use oer_protocols_ieee80211_wire::twt::IndividualTwtFlowType;
use oer_protocols_ieee80211_wire::twt::IndividualTwtImplicit;

const CONFIG: IndividualTwtRequesterConfig = match IndividualTwtRequesterConfig::new(
    oer_portable_time::Duration::from_micros(1_000),
    oer_portable_time::Duration::from_micros(100),
    2,
    2,
) {
    Ok(config) => config,
    Err(_) => panic!("valid requester config"),
};

fn parameters(implicit: IndividualTwtImplicit) -> IndividualTwtParameterSet {
    IndividualTwtParameterSet {
        request: oer_protocols_ieee80211_wire::twt::TwtRequestFields::from_bytes([0; 2])
            .with_requesting_sta(true)
            .with_setup_command(IndividualTwtSetupCommand::Request)
            .with_implicit(implicit == IndividualTwtImplicit::Implicit)
            .with_flow_id(IndividualTwtFlowId::new(2).unwrap().field()),
        target_wake_time: oer_protocols_ieee80211_wire::tsf::TsfInstant::from_micros(10_000),
        nominal_minimum_wake_duration: 1,
        wake_interval_mantissa: 1_024,
        twt_channel: 0,
    }
}

fn proposal(implicit: IndividualTwtImplicit) -> IndividualTwtProposal {
    IndividualTwtProposal {
        control: TwtControlFields::REQUEST,
        parameters: parameters(implicit),
    }
}

#[test]
fn generation_exhaustion_never_reissues_a_stale_identity() {
    let mut requester = IndividualTwtRequester::new(CONFIG);
    requester.generation = u32::MAX;
    idle_flow(
        &mut requester,
        proposal(IndividualTwtImplicit::Implicit)
            .parameters
            .flow_id(),
    )
    .queue_setup(
        proposal(IndividualTwtImplicit::Implicit),
        oer_portable_time::Instant::from_micros(0),
    )
    .unwrap();

    assert_eq!(
        requester.service(oer_portable_time::Instant::from_micros(0)),
        Err(IndividualTwtRequesterError::GenerationExhausted)
    );
    assert_eq!(
        requester.status(IndividualTwtFlowId::new(2).unwrap()),
        IndividualTwtFlowStatus::SetupQueued
    );
    assert_eq!(requester.next_dialog_token, DialogToken::new(1));

    requester.reset_for_reconnect();
    assert_eq!(requester.generation, u32::MAX);
}

#[test]
fn explicit_proposal_reports_the_exact_information_frontier() {
    assert_eq!(
        proposal(IndividualTwtImplicit::Explicit).validate(),
        Err(
            IndividualTwtRequesterError::ExplicitTwtInformationUnsupported(
                IndividualTwtInformationFrontier {
                    flow_id: IndividualTwtFlowId::new(2).unwrap(),
                    initial_target_wake_time: TsfInstant::from_micros(10_000),
                    information_frames_disabled: false,
                }
            )
        )
    );
}

#[test]
fn peer_accepted_explicit_agreement_is_torn_down_not_installed() {
    let mut requester = IndividualTwtRequester::new(CONFIG);
    idle_flow(
        &mut requester,
        proposal(IndividualTwtImplicit::Implicit)
            .parameters
            .flow_id(),
    )
    .queue_setup(
        proposal(IndividualTwtImplicit::Implicit),
        oer_portable_time::Instant::from_micros(0),
    )
    .unwrap();
    let IndividualTwtService::Transmit(transmission) = requester
        .service(oer_portable_time::Instant::from_micros(0))
        .unwrap()
    else {
        panic!("setup must be ready");
    };
    requester
        .complete_transmission(
            transmission,
            oer_protocols_ieee80211_core_upper_mac::rate_control::MpduOutcome::Acknowledged,
            oer_portable_time::Instant::from_micros(10),
        )
        .unwrap();

    let mut response_parameters = parameters(IndividualTwtImplicit::Explicit);
    response_parameters.request = response_parameters
        .request
        .with_requesting_sta(false)
        .with_setup_command(IndividualTwtSetupCommand::Accept);
    let disposition = requester
        .on_setup_response(IndividualTwtSetup {
            dialog_token: DialogToken::new(1),
            control: TwtControlFields::REQUEST,
            parameters: response_parameters,
        })
        .unwrap();
    assert_eq!(
        disposition,
        IndividualTwtSetupDisposition::ExplicitInformationUnsupported {
            flow_id: IndividualTwtFlowId::new(2).unwrap(),
            frontier: IndividualTwtInformationFrontier {
                flow_id: IndividualTwtFlowId::new(2).unwrap(),
                initial_target_wake_time: TsfInstant::from_micros(10_000),
                information_frames_disabled: false,
            },
        }
    );
    assert_eq!(
        requester.next_deadline(),
        Some(oer_portable_time::Instant::from_micros(0))
    );
    let IndividualTwtService::Transmit(teardown) = requester
        .service(oer_portable_time::Instant::from_micros(10))
        .unwrap()
    else {
        panic!("rollback teardown must be ready immediately");
    };
    assert_eq!(teardown.kind, IndividualTwtTxKind::Teardown);
}

fn agreement(flow: u8, target: u64, interval: u64, duration: u64) -> IndividualTwtAgreement {
    IndividualTwtAgreement {
        flow_id: IndividualTwtFlowId::new(flow).unwrap(),
        control: TwtControlFields::REQUEST,
        trigger: oer_protocols_ieee80211_wire::twt::IndividualTwtTrigger::NonTriggerEnabled,
        implicit: IndividualTwtImplicit::Implicit,
        flow_type: IndividualTwtFlowType::Announced,
        protection: oer_protocols_ieee80211_wire::twt::IndividualTwtProtection::Unprotected,
        target_wake_time: TsfInstant::from_micros(target),
        wake_interval: TsfDuration::from_micros(interval),
        wake_duration: TsfDuration::from_micros(duration),
    }
}

fn tsf(micros: u64) -> TsfInstant {
    TsfInstant::from_micros(micros)
}

#[test]
fn wake_plan_refuses_a_window_past_the_tsf_range() {
    // The current window has closed and the next one starts past 2^64: beyond the TSF
    // generation the plan is computed in, so it is refused, not wrapped.
    let late = agreement(0, u64::MAX - 600, 1_000, 256);
    assert_eq!(
        plan_agreement_wake(late, tsf(u64::MAX - 100), TsfDuration::from_micros(10)),
        Err(IndividualTwtWakePlanError::BeyondTsfRange)
    );
    // A target past now is a window ahead, never one after a wrap.
    let ahead = agreement(0, 50, 1_000, 256);
    assert_eq!(
        plan_agreement_wake(ahead, tsf(0), TsfDuration::from_micros(10)),
        Ok(IndividualTwtWakePlan {
            flow_bitmap: 1,
            wake: tsf(40),
            service_start: tsf(50),
            service_end: tsf(306),
            service_open: false,
        })
    );
}

#[test]
fn wake_plan_follows_the_periodic_schedule_from_the_target() {
    let periodic = agreement(1, 1_000, 1_000, 256);
    // Inside the third window: open, wake now.
    assert_eq!(
        plan_agreement_wake(periodic, tsf(3_100), TsfDuration::from_micros(10)),
        Ok(IndividualTwtWakePlan {
            flow_bitmap: 2,
            wake: tsf(3_100),
            service_start: tsf(3_000),
            service_end: tsf(3_256),
            service_open: true,
        })
    );
    // After it: the next window, waking the guard before it, not before now.
    assert_eq!(
        plan_agreement_wake(periodic, tsf(3_995), TsfDuration::from_micros(10)),
        Ok(IndividualTwtWakePlan {
            flow_bitmap: 2,
            wake: tsf(3_995),
            service_start: tsf(4_000),
            service_end: tsf(4_256),
            service_open: false,
        })
    );
}

#[test]
fn the_earliest_of_two_flows_wins_and_open_windows_merge() {
    let guard = TsfDuration::from_micros(10);
    let early = plan_agreement_wake(agreement(0, 500, 10_000, 100), tsf(0), guard).unwrap();
    let late = plan_agreement_wake(agreement(1, 900, 10_000, 100), tsf(0), guard).unwrap();
    assert_eq!(late.merge_or_earlier(early), early);
    let first = plan_agreement_wake(agreement(0, 0, 10_000, 600), tsf(550), guard).unwrap();
    let second = plan_agreement_wake(agreement(1, 500, 10_000, 600), tsf(550), guard).unwrap();
    let merged = first.merge_or_earlier(second);
    assert_eq!(merged.flow_bitmap, 0b11);
    assert_eq!(
        (merged.service_start, merged.service_end),
        (tsf(0), tsf(1_100))
    );
    assert!(merged.service_open);
}

#[test]
fn wake_guard_before_epoch_is_an_error_even_when_now_would_clip_it() {
    let early = agreement(0, 5, 1_000, 256);
    assert_eq!(
        plan_agreement_wake(early, tsf(0), TsfDuration::from_micros(10)),
        Err(IndividualTwtWakePlanError::BeforeTsfEpoch)
    );
    // An already open service period wakes now and needs no future guard.
    assert_eq!(
        plan_agreement_wake(early, tsf(5), TsfDuration::from_micros(10))
            .unwrap()
            .wake,
        tsf(5)
    );
    let endpoint = agreement(0, u64::MAX - 256, 1_000, 256);
    assert_eq!(
        plan_agreement_wake(endpoint, tsf(u64::MAX - 256), TsfDuration::from_micros(0))
            .unwrap()
            .service_end,
        tsf(u64::MAX)
    );
    let beyond = agreement(0, u64::MAX - 255, 1_000, 256);
    assert_eq!(
        plan_agreement_wake(beyond, tsf(u64::MAX - 255), TsfDuration::from_micros(0)),
        Err(IndividualTwtWakePlanError::BeyondTsfRange)
    );
}

fn idle_flow(
    requester: &mut IndividualTwtRequester,
    flow_id: IndividualTwtFlowId,
) -> IndividualTwtIdleFlow<'_> {
    let IndividualTwtFlowAccess::Idle(idle) = requester.flow(flow_id) else {
        panic!("fixture owns the idle flow");
    };
    idle
}

#[test]
fn queue_authority_follows_the_selected_flow_through_transmission() {
    use oer_portable_clock::Clock as _;
    let clock = oer_portable_time_virtual::VirtualClock::<1>::new();
    let mut requester = IndividualTwtRequester::new(CONFIG);
    let flow = proposal(IndividualTwtImplicit::Implicit)
        .parameters
        .flow_id();
    idle_flow(&mut requester, flow)
        .queue_setup(proposal(IndividualTwtImplicit::Implicit), clock.now())
        .unwrap();
    assert!(matches!(
        requester.flow(flow),
        IndividualTwtFlowAccess::TeardownReady(_)
    ));
    let IndividualTwtService::Transmit(transmission) = requester.service(clock.now()).unwrap()
    else {
        panic!("queued setup transmits");
    };
    assert!(matches!(
        requester.flow(flow),
        IndividualTwtFlowAccess::Transmitting(_)
    ));
    clock
        .advance(oer_portable_time::Duration::from_micros(10))
        .unwrap();
    requester
        .complete_transmission(
            transmission,
            oer_protocols_ieee80211_core_upper_mac::rate_control::MpduOutcome::Acknowledged,
            clock.now(),
        )
        .unwrap();
    let IndividualTwtFlowAccess::TeardownReady(teardown) = requester.flow(flow) else {
        panic!("completed transmission permits teardown");
    };
    teardown.queue_teardown(clock.now());
    let IndividualTwtService::Transmit(transmission) = requester.service(clock.now()).unwrap()
    else {
        panic!("queued teardown transmits");
    };
    assert_eq!(transmission.kind, IndividualTwtTxKind::Teardown);
    requester
        .complete_transmission(
            transmission,
            oer_protocols_ieee80211_core_upper_mac::rate_control::MpduOutcome::Acknowledged,
            clock.now(),
        )
        .unwrap();
    assert!(matches!(
        requester.flow(flow),
        IndividualTwtFlowAccess::Idle(_)
    ));
}

#[test]
fn a_foreign_proposal_does_not_mutate_either_flow() {
    use oer_portable_clock::Clock as _;
    let clock = oer_portable_time_virtual::VirtualClock::<1>::new();
    let mut requester = IndividualTwtRequester::new(CONFIG);
    let proposal = proposal(IndividualTwtImplicit::Implicit);
    let selected = IndividualTwtFlowId::new(1).unwrap();
    assert_eq!(
        idle_flow(&mut requester, selected).queue_setup(proposal, clock.now()),
        Err(IndividualTwtRequesterError::ProposalFlowMismatch {
            selected,
            proposed: proposal.parameters.flow_id(),
        })
    );
    assert_eq!(requester.status(selected), IndividualTwtFlowStatus::Idle);
    assert_eq!(
        requester.status(proposal.parameters.flow_id()),
        IndividualTwtFlowStatus::Idle
    );
}

#[test]
fn every_previous_nontransmitting_occupied_phase_still_permits_teardown() {
    use oer_portable_clock::Clock as _;
    let clock = oer_portable_time_virtual::VirtualClock::<1>::new();
    let flow = proposal(IndividualTwtImplicit::Implicit)
        .parameters
        .flow_id();
    for phase in [
        FlowPhase::SetupQueued {
            proposal: proposal(IndividualTwtImplicit::Implicit),
            attempts_remaining: 2,
            ready_at: clock.now(),
        },
        FlowPhase::AwaitingResponse {
            proposal: proposal(IndividualTwtImplicit::Implicit),
            dialog_token: DialogToken::new(1),
            generation: 1,
            attempts_remaining: 1,
            deadline: clock.now(),
        },
        FlowPhase::AwaitingHardwareInstall {
            generation: 1,
            agreement: agreement(2, 10_000, 1_024, 256),
        },
        FlowPhase::Active(agreement(2, 10_000, 1_024, 256)),
        FlowPhase::TeardownQueued {
            attempts_remaining: 1,
            ready_at: clock.now(),
        },
    ] {
        let mut requester = IndividualTwtRequester::new(CONFIG);
        *flow.select_mut(&mut requester.flows) = phase;
        let IndividualTwtFlowAccess::TeardownReady(teardown) = requester.flow(flow) else {
            panic!("occupied nontransmitting phase permits teardown");
        };
        teardown.queue_teardown(clock.now());
        assert_eq!(
            requester.status(flow),
            IndividualTwtFlowStatus::TeardownQueued
        );
    }
}

/// A transmission of an earlier generation of the flow ends nothing: the
/// setup and the teardown in flight wait for their own.
#[test]
fn a_stale_generation_ends_neither_the_setup_nor_the_teardown_in_flight() {
    use oer_portable_clock::Clock as _;
    use oer_protocols_ieee80211_core_upper_mac::rate_control::MpduOutcome;
    let clock = oer_portable_time_virtual::VirtualClock::<1>::new();
    let mut requester = IndividualTwtRequester::new(CONFIG);
    let flow = proposal(IndividualTwtImplicit::Implicit)
        .parameters
        .flow_id();
    idle_flow(&mut requester, flow)
        .queue_setup(proposal(IndividualTwtImplicit::Implicit), clock.now())
        .unwrap();
    let IndividualTwtService::Transmit(setup) = requester.service(clock.now()).unwrap() else {
        panic!("queued setup transmits");
    };
    for outcome in [MpduOutcome::Acknowledged, MpduOutcome::Unacknowledged] {
        let stale = IndividualTwtTransmission {
            generation: setup.generation.wrapping_add(1),
            ..setup
        };
        assert_eq!(
            requester.complete_transmission(stale, outcome, clock.now()),
            Err(IndividualTwtRequesterError::StaleTransmission)
        );
        assert!(matches!(
            requester.flow(flow),
            IndividualTwtFlowAccess::Transmitting(_)
        ));
    }
    requester
        .complete_transmission(setup, MpduOutcome::Acknowledged, clock.now())
        .unwrap();
    let IndividualTwtFlowAccess::TeardownReady(teardown) = requester.flow(flow) else {
        panic!("completed transmission permits teardown");
    };
    teardown.queue_teardown(clock.now());
    let IndividualTwtService::Transmit(down) = requester.service(clock.now()).unwrap() else {
        panic!("queued teardown transmits");
    };
    for outcome in [MpduOutcome::Acknowledged, MpduOutcome::Unacknowledged] {
        let stale = IndividualTwtTransmission {
            generation: down.generation.wrapping_add(1),
            ..down
        };
        assert_eq!(
            requester.complete_transmission(stale, outcome, clock.now()),
            Err(IndividualTwtRequesterError::StaleTransmission)
        );
        assert!(matches!(
            requester.flow(flow),
            IndividualTwtFlowAccess::Transmitting(_)
        ));
    }
    requester
        .complete_transmission(down, MpduOutcome::Acknowledged, clock.now())
        .unwrap();
    assert!(matches!(
        requester.flow(flow),
        IndividualTwtFlowAccess::Idle(_)
    ));
}

#[test]
fn a_reconnect_returns_every_installed_agreement_and_leaves_none() {
    let mut requester = IndividualTwtRequester::new(CONFIG);
    let installed = agreement(2, 10_000, 1_024, 256);
    *IndividualTwtFlowId::new(2)
        .unwrap()
        .select_mut(&mut requester.flows) = FlowPhase::Active(installed);
    requester.next_dialog_token = DialogToken::new(9);
    let removed = requester.reset_for_reconnect();
    assert_eq!(removed.iter().flatten().count(), 1);
    assert!(removed.contains(&Some(installed)));
    assert_eq!(requester.next_dialog_token, DialogToken::new(1));
    assert_eq!(
        requester.status(IndividualTwtFlowId::new(2).unwrap()),
        IndividualTwtFlowStatus::Idle
    );
}

/// A Demand is met only by a response that accepts every requested field
/// unchanged.
#[test]
fn a_demand_is_met_only_by_its_own_parameters() {
    let demand = proposal(IndividualTwtImplicit::Implicit);
    let response = |parameters: IndividualTwtParameterSet| IndividualTwtSetup {
        dialog_token: oer_protocols_ieee80211_wire::management::DialogToken::new(1),
        control: demand.control,
        parameters: IndividualTwtParameterSet {
            request: parameters
                .request
                .with_requesting_sta(false)
                .with_setup_command(IndividualTwtSetupCommand::Accept),
            ..parameters
        },
    };
    let same = demand.parameters;
    assert!(demand_response_matches(demand, response(same)));
    for changed in [
        IndividualTwtParameterSet {
            target_wake_time: tsf(10_001),
            ..same
        },
        IndividualTwtParameterSet {
            nominal_minimum_wake_duration: 2,
            ..same
        },
        IndividualTwtParameterSet {
            wake_interval_mantissa: 1_025,
            ..same
        },
        IndividualTwtParameterSet {
            twt_channel: 1,
            ..same
        },
        IndividualTwtParameterSet {
            request: same.request.with_trigger(true),
            ..same
        },
        IndividualTwtParameterSet {
            request: same.request.with_implicit(false),
            ..same
        },
    ] {
        assert!(
            !demand_response_matches(demand, response(changed)),
            "{changed:?}"
        );
    }
}
