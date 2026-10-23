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
fn minimum_accepts_a_first_low_power_or_a_third_mid_power_estimate() {
    let mut policy = PhyRxDcMinimumPolicyState::new(MINIMUM_REQUEST.previous);
    let PhyRxDcMinimumDecision::Complete(outcome) =
        policy.accept(MINIMUM_REQUEST, minimum_outcome(0, 35, 0))
    else {
        panic!("low power must complete")
    };
    assert_eq!(outcome.attempts, 1);
    assert_eq!(outcome.estimate.power, 35);
    let mut policy = PhyRxDcMinimumPolicyState::new(MINIMUM_REQUEST.previous);
    assert!(matches!(
        policy.accept(MINIMUM_REQUEST, minimum_outcome(0, 40, 0)),
        PhyRxDcMinimumDecision::Continue
    ));
    assert!(matches!(
        policy.accept(MINIMUM_REQUEST, minimum_outcome(1, 42, 0)),
        PhyRxDcMinimumDecision::Continue
    ));
    let PhyRxDcMinimumDecision::Complete(outcome) =
        policy.accept(MINIMUM_REQUEST, minimum_outcome(2, 41, 0))
    else {
        panic!("third mid power must complete")
    };
    assert_eq!(outcome.attempts, 3);
    assert_eq!(outcome.estimate.power, 40);
}
#[test]
fn rejected_searches_preserve_the_previous_slot_and_exhaust_after_eight() {
    let previous = PhyDcIqEstimate {
        i: 17,
        q: -9,
        power: 3,
    };
    let request = PhyRxDcMinimumRequest {
        previous,
        ..MINIMUM_REQUEST
    };
    let mut policy = PhyRxDcMinimumPolicyState::new(previous);
    for attempt in 0..RX_DC_MINIMUM_MAX_ATTEMPTS {
        let result = policy.accept(request, minimum_outcome(attempt, 1, 1));
        if attempt + 1 < RX_DC_MINIMUM_MAX_ATTEMPTS {
            assert!(matches!(result, PhyRxDcMinimumDecision::Continue));
        } else {
            let PhyRxDcMinimumDecision::Complete(outcome) = result else {
                panic!("finite search must complete")
            };
            assert_eq!(outcome.attempts, 8);
            assert_eq!(outcome.readiness_activity_edges, 8);
            assert_eq!(
                outcome.estimate,
                PhyDcIqEstimate {
                    i: 17,
                    q: -9,
                    power: 0x38
                }
            );
        }
    }
}
#[test]
fn a_clean_attempt_after_activity_is_admitted_and_saturation_can_admit_activity() {
    let mut policy = PhyRxDcMinimumPolicyState::new(MINIMUM_REQUEST.previous);
    assert!(matches!(
        policy.accept(MINIMUM_REQUEST, minimum_outcome(0, 20, 1)),
        PhyRxDcMinimumDecision::Continue
    ));
    let PhyRxDcMinimumDecision::Complete(outcome) =
        policy.accept(MINIMUM_REQUEST, minimum_outcome(1, 35, 0))
    else {
        panic!("clean sample must be admitted")
    };
    assert_eq!(outcome.attempts, 2);
    assert_eq!(
        outcome.estimate,
        PhyDcIqEstimate {
            i: 1,
            q: -1,
            power: 35
        }
    );
    assert_eq!(outcome.readiness_activity_edges, 1);
    let request = PhyRxDcMinimumRequest {
        rx_saturation_detected: true,
        ..MINIMUM_REQUEST
    };
    let mut policy = PhyRxDcMinimumPolicyState::new(request.previous);
    assert!(matches!(
        policy.accept(request, minimum_outcome(0, 20, 1)),
        PhyRxDcMinimumDecision::Complete(_)
    ));
}
