use super::*;

const REQUEST: PhySignalPowerRequest = PhySignalPowerRequest {
    measurement: 7,
    shift: 12,
};

fn reach_readiness(transition: &mut PhySignalPowerTransition) {
    for completion in [
        PhySignalPowerCompletion::ClockConfigured {
            request: REQUEST,
            clock: PhySignalPowerClock::Tx,
            enabled: true,
        },
        PhySignalPowerCompletion::ClockConfigured {
            request: REQUEST,
            clock: PhySignalPowerClock::Rx,
            enabled: true,
        },
        PhySignalPowerCompletion::EstimatorEnableSet {
            request: REQUEST,
            phase: PhyDcIqEnablePhase::Measurement,
            enabled: false,
        },
        PhySignalPowerCompletion::DelayElapsed {
            request: REQUEST,
            phase: PhyDcIqDelayPhase::Stop,
            micros: 1,
        },
        PhySignalPowerCompletion::EstimatorEnableSet {
            request: REQUEST,
            phase: PhyDcIqEnablePhase::Start,
            enabled: false,
        },
        PhySignalPowerCompletion::EstimatorConfigured {
            request: REQUEST,
            control: 0x1000,
        },
        PhySignalPowerCompletion::EstimatorEnableSet {
            request: REQUEST,
            phase: PhyDcIqEnablePhase::Start,
            enabled: true,
        },
        PhySignalPowerCompletion::DelayElapsed {
            request: REQUEST,
            phase: PhyDcIqDelayPhase::Start,
            micros: 1,
        },
        PhySignalPowerCompletion::EstimatorEnableSet {
            request: REQUEST,
            phase: PhyDcIqEnablePhase::Measurement,
            enabled: true,
        },
    ] {
        transition.advance(completion).unwrap();
    }
}

#[test]
fn control_preserves_rom_shift_and_halfword_truncation() {
    assert_eq!(signal_power_estimator_control(12), 0x1000);
    assert_eq!(signal_power_estimator_control(16), 0);
    assert_eq!(signal_power_estimator_control(32), 1);
}

#[test]
fn accumulator_suffix_matches_signed_rom_equations() {
    assert_eq!(
        calculate_signal_power(
            REQUEST,
            PhySignalPowerAccumulatorSnapshot {
                sum_i: 3 << 10,
                difference_i: -4 << 10,
                difference_q: 2 << 10,
                sum_q: 5 << 10,
            },
        ),
        100
    );
}

#[test]
fn success_leaves_estimator_enabled_after_one_external_ready_edge() {
    let mut transition = PhySignalPowerTransition::new(REQUEST);
    reach_readiness(&mut transition);
    transition
        .advance(PhySignalPowerCompletion::ReadinessObserved {
            request: REQUEST,
            snapshot: PhyDcIqReadinessSnapshot {
                ready: true,
                activity: false,
            },
        })
        .unwrap();
    transition
        .advance(PhySignalPowerCompletion::AccumulatorsRead {
            request: REQUEST,
            snapshot: PhySignalPowerAccumulatorSnapshot {
                sum_i: 0,
                difference_i: 0,
                difference_q: 0,
                sum_q: 0,
            },
        })
        .unwrap();
    assert!(matches!(
        transition.action(),
        PhySignalPowerAction::Complete(PhySignalPowerOutcome { value: 0, .. })
    ));
}

#[test]
fn readiness_counts_all_samples_separately_from_activity() {
    let mut transition = PhySignalPowerTransition::new(REQUEST);
    reach_readiness(&mut transition);
    for activity in [false, true, false] {
        transition
            .advance(PhySignalPowerCompletion::ReadinessObserved {
                request: REQUEST,
                snapshot: PhyDcIqReadinessSnapshot {
                    ready: false,
                    activity,
                },
            })
            .unwrap();
    }
    assert_eq!(
        transition.action(),
        PhySignalPowerAction::AwaitReadinessEdge {
            request: REQUEST,
            readiness_activity_edges: 1,
            readiness_samples: 3,
        }
    );
}

#[test]
fn timeout_cleans_up_before_typed_failure() {
    let mut transition = PhySignalPowerTransition::new(REQUEST);
    reach_readiness(&mut transition);
    transition
        .advance(PhySignalPowerCompletion::ReadinessTimedOut(REQUEST))
        .unwrap();
    for completion in [
        PhySignalPowerCompletion::EstimatorEnableSet {
            request: REQUEST,
            phase: PhyDcIqEnablePhase::Measurement,
            enabled: false,
        },
        PhySignalPowerCompletion::DelayElapsed {
            request: REQUEST,
            phase: PhyDcIqDelayPhase::Stop,
            micros: 1,
        },
        PhySignalPowerCompletion::EstimatorEnableSet {
            request: REQUEST,
            phase: PhyDcIqEnablePhase::Start,
            enabled: false,
        },
    ] {
        transition.advance(completion).unwrap();
    }
    assert_eq!(
        transition.action(),
        PhySignalPowerAction::Failed(PhySignalPowerFailure::ReadinessTimedOut {
            request: REQUEST,
            readiness_activity_edges: 0,
        })
    );
}
