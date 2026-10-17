use super::*;

const REQUEST: PhyDcIqEstimateRequest = PhyDcIqEstimateRequest {
    iteration: 3,
    chain: 1,
    control: 0x0fa0,
    mode: 0,
};

fn reach_readiness(transition: &mut PhyDcIqEstimateTransition) {
    transition
        .advance(PhyDcIqCompletion::Configured(REQUEST))
        .unwrap();
    transition
        .advance(PhyDcIqCompletion::EnableSet {
            request: REQUEST,
            phase: PhyDcIqEnablePhase::Start,
            enabled: true,
        })
        .unwrap();
    transition
        .advance(PhyDcIqCompletion::DelayElapsed {
            request: REQUEST,
            phase: PhyDcIqDelayPhase::Start,
            micros: 1,
        })
        .unwrap();
    transition
        .advance(PhyDcIqCompletion::EnableSet {
            request: REQUEST,
            phase: PhyDcIqEnablePhase::Measurement,
            enabled: true,
        })
        .unwrap();
}

fn finish_disable_tail(transition: &mut PhyDcIqEstimateTransition) {
    transition
        .advance(PhyDcIqCompletion::EnableSet {
            request: REQUEST,
            phase: PhyDcIqEnablePhase::Measurement,
            enabled: false,
        })
        .unwrap();
    transition
        .advance(PhyDcIqCompletion::DelayElapsed {
            request: REQUEST,
            phase: PhyDcIqDelayPhase::Stop,
            micros: 1,
        })
        .unwrap();
    transition
        .advance(PhyDcIqCompletion::EnableSet {
            request: REQUEST,
            phase: PhyDcIqEnablePhase::Start,
            enabled: false,
        })
        .unwrap();
}

#[test]
fn linear_to_db_matches_rom_table_boundaries() {
    assert_eq!(phy_linear_to_db(0, 0), 0);
    assert_eq!(phy_linear_to_db(1, 0), 28);
    assert_eq!(phy_linear_to_db(2, 0), 48);
    assert_eq!(phy_linear_to_db(3, 0), 76);
    assert_eq!(phy_linear_to_db(1, 3), 4);
}

#[test]
fn accumulator_transform_matches_reachable_mode_zero_equations() {
    assert_eq!(
        calculate_dc_iq_estimate(
            REQUEST,
            PhyDcIqAccumulatorSnapshot {
                i: 4001 * 64 * 2,
                q: -(4001 * 64 * 3),
                power: 4001 * 32,
            },
        ),
        PhyDcIqEstimate {
            i: 2,
            q: -3,
            power: 24,
        }
    );
}

#[test]
fn readiness_requires_external_edges_and_owns_diagnostic_count() {
    let mut transition = PhyDcIqEstimateTransition::new(REQUEST);
    reach_readiness(&mut transition);
    for activity in [false, true, true] {
        transition
            .advance(PhyDcIqCompletion::ReadinessObserved {
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
        PhyDcIqAction::AwaitReadinessEdge {
            request: REQUEST,
            readiness_activity_edges: 2,
            readiness_samples: 3,
        }
    );
    transition
        .advance(PhyDcIqCompletion::ReadinessObserved {
            request: REQUEST,
            snapshot: PhyDcIqReadinessSnapshot {
                ready: true,
                activity: true,
            },
        })
        .unwrap();
    transition
        .advance(PhyDcIqCompletion::AccumulatorsRead {
            request: REQUEST,
            snapshot: PhyDcIqAccumulatorSnapshot {
                i: 0,
                q: 0,
                power: 0,
            },
        })
        .unwrap();
    finish_disable_tail(&mut transition);
    let PhyDcIqAction::Complete(outcome) = transition.action() else {
        panic!("DC/IQ transition did not complete");
    };
    assert_eq!(outcome.readiness_activity_edges, 2);
    assert_eq!(
        outcome.estimate,
        PhyDcIqEstimate {
            i: 0,
            q: 0,
            power: 0,
        }
    );
}

#[test]
fn timeout_runs_complete_disable_tail_before_failure() {
    let mut transition = PhyDcIqEstimateTransition::new(REQUEST);
    reach_readiness(&mut transition);
    transition
        .advance(PhyDcIqCompletion::ReadinessTimedOut(REQUEST))
        .unwrap();
    assert!(matches!(
        transition.action(),
        PhyDcIqAction::SetEnable {
            phase: PhyDcIqEnablePhase::Measurement,
            enabled: false,
            ..
        }
    ));
    finish_disable_tail(&mut transition);
    assert_eq!(
        transition.action(),
        PhyDcIqAction::Failed(PhyDcIqFailure::ReadinessTimedOut {
            request: REQUEST,
            readiness_activity_edges: 0,
        })
    );
}

#[test]
fn external_lowering_separates_mmio_timer_readiness_and_terminal() {
    assert!(matches!(
        PhyDcIqExternalBinding::lower(PhyDcIqAction::Configure(REQUEST)),
        Ok(PhyDcIqExternalBinding::Mmio(_))
    ));
    assert!(matches!(
        PhyDcIqExternalBinding::lower(PhyDcIqAction::DelayMicros {
            request: REQUEST,
            phase: PhyDcIqDelayPhase::Start,
            micros: 1,
        }),
        Ok(PhyDcIqExternalBinding::Timer(_))
    ));
    assert!(matches!(
        PhyDcIqExternalBinding::lower(PhyDcIqAction::AwaitReadinessEdge {
            request: REQUEST,
            readiness_activity_edges: 0,
            readiness_samples: 7,
        }),
        Ok(PhyDcIqExternalBinding::Readiness(binding)) if binding.samples() == 7
    ));
    assert!(matches!(
        PhyDcIqExternalBinding::lower(PhyDcIqAction::Failed(PhyDcIqFailure::ReadinessTimedOut {
            request: REQUEST,
            readiness_activity_edges: 0,
        })),
        Err(PhyDcIqBindingError::UnsupportedAction)
    ));
}

#[test]
fn readiness_samples_directly_on_first_retry_and_last_allowed_observation() {
    for samples in [0, 1, crate::HARDWARE_EDGE_LIMIT - 1] {
        for ready in [false, true] {
            let binding = PhyDcIqReadinessBinding::new(PhyDcIqAction::AwaitReadinessEdge {
                request: REQUEST,
                readiness_activity_edges: 0,
                readiness_samples: samples,
            })
            .unwrap();
            let mut reads = 0;
            let snapshot = PhyDcIqReadinessSnapshot {
                ready,
                activity: true,
            };
            let completion = binding.observe_with(crate::HARDWARE_EDGE_LIMIT, || {
                reads += 1;
                snapshot
            });
            assert_eq!(reads, 1);
            assert_eq!(
                completion,
                PhyDcIqCompletion::ReadinessObserved {
                    request: REQUEST,
                    snapshot,
                }
            );
        }
    }
}

#[test]
fn exhausted_readiness_does_not_touch_hardware() {
    for samples in [crate::HARDWARE_EDGE_LIMIT, crate::HARDWARE_EDGE_LIMIT + 1] {
        let binding = PhyDcIqReadinessBinding::new(PhyDcIqAction::AwaitReadinessEdge {
            request: REQUEST,
            readiness_activity_edges: 2,
            readiness_samples: samples,
        })
        .unwrap();
        assert_eq!(
            binding.observe_with(crate::HARDWARE_EDGE_LIMIT, || panic!(
                "exhausted probe must not read"
            )),
            PhyDcIqCompletion::ReadinessTimedOut(REQUEST),
        );
    }
}

#[test]
fn readiness_batch_runs_the_rom_shaped_poll_loop_and_commits_once() {
    let mut transition = PhyDcIqEstimateTransition::new(REQUEST);
    reach_readiness(&mut transition);
    let PhyDcIqExternalBinding::Readiness(binding) =
        PhyDcIqExternalBinding::lower(transition.action()).unwrap()
    else {
        panic!("expected readiness binding");
    };
    let mut snapshots = [
        PhyDcIqReadinessSnapshot {
            ready: false,
            activity: false,
        },
        PhyDcIqReadinessSnapshot {
            ready: false,
            activity: true,
        },
        PhyDcIqReadinessSnapshot {
            ready: true,
            activity: true,
        },
    ]
    .into_iter();
    let (completion, operations) = binding.observe_until_ready_with(
        crate::HARDWARE_EDGE_LIMIT,
        &mut snapshots,
        |snapshots| snapshots.next().expect("bounded sample"),
        |_| PhyDcIqAccumulatorSnapshot {
            i: 0,
            q: 0,
            power: 0,
        },
    );
    assert_eq!(operations, 4);
    assert!(snapshots.next().is_none());
    transition.advance(completion).unwrap();
    finish_disable_tail(&mut transition);
    let PhyDcIqAction::Complete(outcome) = transition.action() else {
        panic!("DC/IQ transition did not complete");
    };
    assert_eq!(outcome.readiness_activity_edges, 1);
}

#[test]
fn readiness_batch_stops_at_the_existing_total_sample_bound() {
    let binding = PhyDcIqReadinessBinding::new(PhyDcIqAction::AwaitReadinessEdge {
        request: REQUEST,
        readiness_activity_edges: 7,
        readiness_samples: crate::HARDWARE_EDGE_LIMIT - 2,
    })
    .unwrap();
    let mut reads = 0;
    let (completion, operations) = binding.observe_until_ready_with(
        crate::HARDWARE_EDGE_LIMIT,
        &mut reads,
        |reads| {
            *reads += 1;
            PhyDcIqReadinessSnapshot {
                ready: false,
                activity: *reads == 2,
            }
        },
        |_| panic!("accumulators must not be read before readiness"),
    );
    assert_eq!(reads, 2);
    assert_eq!(operations, 2);
    let PhyDcIqCompletion::ReadinessBatchObserved(batch) = completion else {
        panic!("expected batched readiness completion");
    };
    assert_eq!(batch.observations, 2);
    assert_eq!(batch.activity_edges, 1);
    assert!(!batch.ready);
    assert_eq!(batch.accumulators, None);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TargetEvent {
    Configure(u16),
    Enable(PhyDcIqEnablePhase, bool),
    Settle(u32),
    Readiness,
    Accumulators,
}

#[derive(Debug)]
struct TargetContext {
    events: std::vec::Vec<TargetEvent>,
    readiness: std::vec::Vec<PhyDcIqReadinessSnapshot>,
    next_readiness: usize,
    accumulators: PhyDcIqAccumulatorSnapshot,
}

fn execute_target_estimator(
    transition: &PhyDcIqEstimateTransition,
    maximum_samples: u16,
    context: &mut TargetContext,
) -> PhyDcIqTargetCompletion {
    transition
        .lower_target_transaction()
        .unwrap()
        .execute_with(
            maximum_samples,
            context,
            |context, control| context.events.push(TargetEvent::Configure(control)),
            |context, phase, enabled| {
                context.events.push(TargetEvent::Enable(phase, enabled));
            },
            |context, micros| {
                context.events.push(TargetEvent::Settle(micros));
                Ok::<_, ()>(())
            },
            |context| {
                context.events.push(TargetEvent::Readiness);
                let snapshot = context.readiness[context.next_readiness];
                context.next_readiness += 1;
                snapshot
            },
            |context| {
                context.events.push(TargetEvent::Accumulators);
                context.accumulators
            },
        )
        .unwrap()
}

#[test]
fn target_transaction_preserves_the_complete_rom_estimator_envelope() {
    let mut transition = PhyDcIqEstimateTransition::new(REQUEST);
    let accumulators = PhyDcIqAccumulatorSnapshot {
        i: 4001 * 64 * 2,
        q: -(4001 * 64 * 3),
        power: 4001 * 32,
    };
    let mut context = TargetContext {
        events: std::vec::Vec::new(),
        readiness: std::vec![
            PhyDcIqReadinessSnapshot {
                ready: false,
                activity: false,
            },
            PhyDcIqReadinessSnapshot {
                ready: false,
                activity: true,
            },
            PhyDcIqReadinessSnapshot {
                ready: true,
                activity: true,
            },
        ],
        next_readiness: 0,
        accumulators,
    };

    let completion = execute_target_estimator(&transition, 8, &mut context);
    assert_eq!(completion.operations(), 11);
    assert_eq!(
        context.events,
        std::vec![
            TargetEvent::Configure(REQUEST.control),
            TargetEvent::Enable(PhyDcIqEnablePhase::Start, true),
            TargetEvent::Settle(1),
            TargetEvent::Enable(PhyDcIqEnablePhase::Measurement, true),
            TargetEvent::Readiness,
            TargetEvent::Readiness,
            TargetEvent::Readiness,
            TargetEvent::Accumulators,
            TargetEvent::Enable(PhyDcIqEnablePhase::Measurement, false),
            TargetEvent::Settle(1),
            TargetEvent::Enable(PhyDcIqEnablePhase::Start, false),
        ]
    );

    transition.advance_target_transaction(completion).unwrap();
    assert_eq!(
        transition.action(),
        PhyDcIqAction::Complete(PhyDcIqEstimateOutcome {
            request: REQUEST,
            estimate: calculate_dc_iq_estimate(REQUEST, accumulators),
            readiness_activity_edges: 1,
        })
    );
    assert_eq!(
        transition.lower_target_transaction(),
        Err(PhyDcIqBindingError::UnsupportedAction)
    );
}

#[test]
fn target_transaction_timeout_still_executes_the_complete_disable_tail() {
    let mut transition = PhyDcIqEstimateTransition::new(REQUEST);
    let mut context = TargetContext {
        events: std::vec::Vec::new(),
        readiness: std::vec![
            PhyDcIqReadinessSnapshot {
                ready: false,
                activity: true,
            },
            PhyDcIqReadinessSnapshot {
                ready: false,
                activity: true,
            },
        ],
        next_readiness: 0,
        accumulators: PhyDcIqAccumulatorSnapshot {
            i: 0,
            q: 0,
            power: 0,
        },
    };

    let completion = execute_target_estimator(&transition, 2, &mut context);
    assert_eq!(completion.operations(), 9);
    assert!(!context.events.contains(&TargetEvent::Accumulators));
    assert_eq!(
        &context.events[context.events.len() - 3..],
        &[
            TargetEvent::Enable(PhyDcIqEnablePhase::Measurement, false),
            TargetEvent::Settle(1),
            TargetEvent::Enable(PhyDcIqEnablePhase::Start, false),
        ]
    );

    transition.advance_target_transaction(completion).unwrap();
    assert_eq!(
        transition.action(),
        PhyDcIqAction::Failed(PhyDcIqFailure::ReadinessTimedOut {
            request: REQUEST,
            readiness_activity_edges: 2,
        })
    );
}
