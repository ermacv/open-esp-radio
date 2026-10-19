use super::*;
use crate::calibration::estimator::{
    PhyDcIqAccumulatorSnapshot, PhyDcIqAction, PhyDcIqCompletion, PhyDcIqEstimateOutcome,
    PhyDcIqReadinessSnapshot,
};

fn complete_dc_iq_action(action: PhyDcIqAction, estimate: PhyDcIqEstimate) -> PhyDcIqCompletion {
    match action {
        PhyDcIqAction::Configure(request) => PhyDcIqCompletion::Configured(request),
        PhyDcIqAction::SetEnable {
            request,
            phase,
            enabled,
        } => PhyDcIqCompletion::EnableSet {
            request,
            phase,
            enabled,
        },
        PhyDcIqAction::DelayMicros {
            request,
            phase,
            micros,
        } => PhyDcIqCompletion::DelayElapsed {
            request,
            phase,
            micros,
        },
        PhyDcIqAction::AwaitReadinessEdge { request, .. } => PhyDcIqCompletion::ReadinessObserved {
            request,
            snapshot: PhyDcIqReadinessSnapshot {
                ready: true,
                activity: false,
            },
        },
        PhyDcIqAction::ReadAccumulators(request) => {
            let divisor = i32::from(request.control) + 1;
            PhyDcIqCompletion::AccumulatorsRead {
                request,
                snapshot: PhyDcIqAccumulatorSnapshot {
                    i: estimate.i.wrapping_mul(divisor).wrapping_shl(6),
                    q: estimate.q.wrapping_mul(divisor).wrapping_shl(6),
                    power: 0,
                },
            }
        }
        action => panic!("unexpected terminal DC/IQ action: {action:?}"),
    }
}

fn complete_until_measurement(transition: &mut PhyRxDcoTransition) {
    transition
        .advance(PhyRxDcoCompletion::RxDcoControlRestorePrepared)
        .unwrap();
    transition
        .advance(PhyRxDcoCompletion::PbusRead {
            selector: 1,
            path: 2,
            value: 0,
        })
        .unwrap();
    loop {
        match transition.action() {
            PhyRxDcoAction::ForcePbus(transaction) => transition
                .advance(PhyRxDcoCompletion::PbusForceCompleted(transaction))
                .unwrap(),
            PhyRxDcoAction::DelayMicros { iteration, micros } => transition
                .advance(PhyRxDcoCompletion::DelayElapsed { iteration, micros })
                .unwrap(),
            PhyRxDcoAction::DcIq(_) => return,
            action => panic!("unexpected RX-DCO setup action: {action:?}"),
        }
    }
}

fn complete_one_measurement(transition: &mut PhyRxDcoTransition, estimate: PhyDcIqEstimate) {
    loop {
        let PhyRxDcoAction::DcIq(action) = transition.action() else {
            return;
        };
        transition
            .advance(PhyRxDcoCompletion::DcIq(complete_dc_iq_action(
                action, estimate,
            )))
            .unwrap();
    }
}

fn advance_to_next_measurement(transition: &mut PhyRxDcoTransition) {
    loop {
        match transition.action() {
            PhyRxDcoAction::ForcePbus(transaction) => transition
                .advance(PhyRxDcoCompletion::PbusForceCompleted(transaction))
                .unwrap(),
            PhyRxDcoAction::DelayMicros { iteration, micros } => transition
                .advance(PhyRxDcoCompletion::DelayElapsed { iteration, micros })
                .unwrap(),
            PhyRxDcoAction::DcIq(_) => return,
            action => panic!("unexpected RX-DCO loop action: {action:?}"),
        }
    }
}

#[test]
fn compensation_matches_reachable_rom_branches() {
    assert_eq!(rx_dco_compensation_step(16, 16, 0, 0), 16);
    assert_eq!(rx_dco_compensation_step(16, 16, 2, 4), 2);
    assert_eq!(rx_dco_compensation_step(16, -16, 0, 0), 8);
    assert_eq!(rx_dco_compensation_step(0, 0, 6, 0), -1);
    assert_eq!(rx_dco_compensation_step(1, 1, 6, 4), 1);
}

#[test]
fn early_success_restores_control_field_before_completion() {
    let mut transition = PhyRxDcoTransition::new(PhyRxDcoRequest::XTAL_DUTY);
    complete_until_measurement(&mut transition);
    complete_one_measurement(
        &mut transition,
        PhyDcIqEstimate {
            i: 2,
            q: -2,
            power: 0,
        },
    );

    let PhyRxDcoAction::RestoreRxDcoControl = transition.action() else {
        panic!("RX-DCO control restoration was not requested");
    };
    transition
        .advance(PhyRxDcoCompletion::RxDcoControlRestored)
        .unwrap();

    let PhyRxDcoAction::Complete(outcome) = transition.action() else {
        panic!("RX-DCO did not complete");
    };
    assert!(outcome.converged);
    assert_eq!(outcome.iterations, 1);
    assert_eq!(outcome.configuration, [0x0100_0100; 2]);
}

#[test]
fn non_converging_measurement_stops_after_twelve_iterations() {
    let mut transition = PhyRxDcoTransition::new(PhyRxDcoRequest::XTAL_DUTY);
    complete_until_measurement(&mut transition);
    for iteration in 0..MAX_ITERATIONS {
        complete_one_measurement(
            &mut transition,
            PhyDcIqEstimate {
                i: 100,
                q: -100,
                power: 0,
            },
        );
        if iteration + 1 != MAX_ITERATIONS {
            advance_to_next_measurement(&mut transition);
        }
    }
    let PhyRxDcoAction::RestoreRxDcoControl = transition.action() else {
        panic!("bounded RX-DCO loop did not request restoration");
    };
    transition
        .advance(PhyRxDcoCompletion::RxDcoControlRestored)
        .unwrap();
    let PhyRxDcoAction::Complete(outcome) = transition.action() else {
        panic!("bounded RX-DCO loop did not complete");
    };
    assert_eq!(outcome.iterations, MAX_ITERATIONS);
    assert!(!outcome.converged);
}

#[test]
fn pbus_timeout_restores_control_field_before_typed_failure() {
    let mut transition = PhyRxDcoTransition::new(PhyRxDcoRequest::XTAL_DUTY);
    transition
        .advance(PhyRxDcoCompletion::RxDcoControlRestorePrepared)
        .unwrap();
    transition
        .advance(PhyRxDcoCompletion::PbusRead {
            selector: 1,
            path: 2,
            value: 0,
        })
        .unwrap();
    let PhyRxDcoAction::ForcePbus(transaction) = transition.action() else {
        panic!("setup PBus command was not requested");
    };
    transition
        .advance(PhyRxDcoCompletion::PbusForceTimedOut(transaction))
        .unwrap();
    assert!(matches!(
        transition.action(),
        PhyRxDcoAction::RestoreRxDcoControl
    ));
    transition
        .advance(PhyRxDcoCompletion::RxDcoControlRestored)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRxDcoAction::Failed(PhyRxDcoFailure::PbusForceTimedOut(transaction))
    );
}

const MINIMUM_REQUEST: PhyRxDcMinimumRequest = PhyRxDcMinimumRequest {
    measurement: 4,
    control: 0x0fa0,
    mode: 0,
    rx_saturation_detected: false,
    previous: PhyDcIqEstimate {
        i: 0,
        q: 0,
        power: 0,
    },
};

fn minimum_outcome(
    attempt: u8,
    power: i32,
    readiness_activity_edges: u16,
) -> PhyDcIqEstimateOutcome {
    PhyDcIqEstimateOutcome {
        request: PhyDcIqEstimateRequest {
            iteration: attempt,
            chain: 1,
            control: MINIMUM_REQUEST.control,
            mode: MINIMUM_REQUEST.mode,
        },
        estimate: PhyDcIqEstimate {
            i: i32::from(attempt),
            q: -i32::from(attempt),
            power,
        },
        readiness_activity_edges,
    }
}

#[test]
fn rx_dc_minimum_completes_immediately_below_36() {
    let mut transition = PhyRxDcMinimumTransition::new(MINIMUM_REQUEST);
    transition.accept_outcome(minimum_outcome(0, 35, 0));
    assert_eq!(
        transition.action(),
        PhyRxDcMinimumAction::Complete(PhyRxDcMinimumOutcome {
            request: MINIMUM_REQUEST,
            estimate: PhyDcIqEstimate {
                i: 0,
                q: 0,
                power: 35,
            },
            attempts: 1,
            readiness_activity_edges: 0,
        })
    );
}

#[test]
fn unadmitted_minimum_returns_the_previous_slot_with_the_sentinel_power() {
    let previous = PhyDcIqEstimate {
        i: 17,
        q: -9,
        power: 3,
    };
    let request = PhyRxDcMinimumRequest {
        previous,
        ..MINIMUM_REQUEST
    };
    let mut transition = PhyRxDcMinimumTransition::new(request);
    // Readiness activity without a detected saturation admits no estimate,
    // however low its power.
    for attempt in 0..8 {
        transition.accept_outcome(minimum_outcome(attempt, 1, 1));
    }
    let PhyRxDcMinimumAction::Complete(outcome) = transition.action() else {
        panic!("RX-DC minimum did not exhaust its attempts");
    };
    assert_eq!(outcome.attempts, 8);
    assert_eq!(
        outcome.estimate,
        PhyDcIqEstimate {
            i: 17,
            q: -9,
            power: 0x38,
        }
    );
}

#[test]
fn rx_dc_minimum_requires_three_attempts_between_36_and_47() {
    let mut transition = PhyRxDcMinimumTransition::new(MINIMUM_REQUEST);
    transition.accept_outcome(minimum_outcome(0, 40, 0));
    transition.accept_outcome(minimum_outcome(1, 42, 0));
    transition.accept_outcome(minimum_outcome(2, 41, 0));
    let PhyRxDcMinimumAction::Complete(outcome) = transition.action() else {
        panic!("RX-DC minimum did not complete after three attempts");
    };
    assert_eq!(outcome.attempts, 3);
    assert_eq!(outcome.estimate.power, 40);
}

#[test]
fn direct_minimum_does_not_start_an_estimator_without_a_complete_budget() {
    let mut called = false;
    let result: Result<Option<PhyRxDcMinimumTargetCompletion>, ()> =
        PhyRxDcMinimumTargetTransaction::new(MINIMUM_REQUEST).execute_with(
            u32::from(crate::HARDWARE_EDGE_LIMIT) + 7,
            |_, _| {
                called = true;
                panic!("an inadmissible estimator touched hardware")
            },
        );

    assert_eq!(result, Ok(None));
    assert!(!called);
}

#[test]
fn direct_minimum_admits_the_exact_complete_estimator_budget() {
    let mut allowance = 0;
    let result: Result<Option<PhyRxDcMinimumTargetCompletion>, ()> =
        PhyRxDcMinimumTargetTransaction::new(MINIMUM_REQUEST).execute_with(
            u32::from(crate::HARDWARE_EDGE_LIMIT) + 8,
            |estimator, maximum_samples| {
                allowance = maximum_samples;
                estimator.execute_with(
                    maximum_samples,
                    &mut (),
                    |_, _| {},
                    |_, _, _| {},
                    |_, _| Ok(()),
                    |_| PhyDcIqReadinessSnapshot {
                        ready: true,
                        activity: false,
                    },
                    |_| PhyDcIqAccumulatorSnapshot {
                        i: 0,
                        q: 0,
                        power: 0,
                    },
                )
            },
        );

    let completion = result.unwrap().expect("exact budget admits estimator");
    assert_eq!(allowance, crate::HARDWARE_EDGE_LIMIT);
    assert_eq!(completion.operations(), 9);
    assert_eq!(completion.estimators(), 1);
    assert!(completion.into_terminal().is_ok());
}

#[test]
fn rx_dc_minimum_accepts_a_clean_attempt_after_prior_activity() {
    let mut transition = PhyRxDcMinimumTransition::new(MINIMUM_REQUEST);
    transition.accept_outcome(minimum_outcome(0, 20, 1));
    transition.accept_outcome(minimum_outcome(1, 35, 0));
    let PhyRxDcMinimumAction::Complete(outcome) = transition.action() else {
        panic!("clean second attempt was not accepted");
    };
    assert_eq!(outcome.attempts, 2);
    assert_eq!(
        outcome.estimate,
        PhyDcIqEstimate {
            i: 1,
            q: -1,
            power: 35,
        }
    );
    assert_eq!(outcome.readiness_activity_edges, 1);
}

#[test]
fn rx_dc_minimum_uses_rom_power_sentinel_after_eight_rejected_samples() {
    let mut transition = PhyRxDcMinimumTransition::new(MINIMUM_REQUEST);
    for attempt in 0..RX_DC_MINIMUM_MAX_ATTEMPTS {
        transition.accept_outcome(minimum_outcome(attempt, 20, 1));
    }
    let PhyRxDcMinimumAction::Complete(outcome) = transition.action() else {
        panic!("bounded RX-DC minimum did not complete");
    };
    assert_eq!(outcome.attempts, RX_DC_MINIMUM_MAX_ATTEMPTS);
    assert_eq!(
        outcome.estimate,
        PhyDcIqEstimate {
            i: 0,
            q: 0,
            power: 0x38
        }
    );
    assert_eq!(
        outcome.readiness_activity_edges,
        u16::from(RX_DC_MINIMUM_MAX_ATTEMPTS)
    );
}

#[test]
fn rx_dc_minimum_propagates_child_timeout_after_disable_tail() {
    let mut transition = PhyRxDcMinimumTransition::new(MINIMUM_REQUEST);
    while let PhyRxDcMinimumAction::DcIq(action) = transition.action() {
        let completion = match action {
            PhyDcIqAction::Configure(request) => PhyDcIqCompletion::Configured(request),
            PhyDcIqAction::SetEnable {
                request,
                phase,
                enabled,
            } => PhyDcIqCompletion::EnableSet {
                request,
                phase,
                enabled,
            },
            PhyDcIqAction::DelayMicros {
                request,
                phase,
                micros,
            } => PhyDcIqCompletion::DelayElapsed {
                request,
                phase,
                micros,
            },
            PhyDcIqAction::AwaitReadinessEdge { request, .. } => {
                PhyDcIqCompletion::ReadinessTimedOut(request)
            }
            action => panic!("unexpected child action: {action:?}"),
        };
        transition
            .advance(PhyRxDcMinimumCompletion::DcIq(completion))
            .unwrap();
    }
    let PhyRxDcMinimumAction::Failed(PhyRxDcMinimumFailure::DcIq(
        PhyDcIqFailure::ReadinessTimedOut {
            request,
            readiness_activity_edges: 0,
        },
    )) = transition.action()
    else {
        panic!("typed child timeout was not propagated");
    };
    assert_eq!(
        request,
        PhyDcIqEstimateRequest {
            iteration: 0,
            chain: 1,
            control: MINIMUM_REQUEST.control,
            mode: MINIMUM_REQUEST.mode,
        }
    );
}
