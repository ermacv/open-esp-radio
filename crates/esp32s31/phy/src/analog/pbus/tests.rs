use super::{
    PHY_PBUS_CLEAR_TRANSACTIONS, PhyForceTxRxAction, PhyForceTxRxCompletion,
    PhyForceTxRxExternalBinding, PhyForceTxRxTransition, PhyForceTxRxTransitionError,
    PhyPbusClearAction, PhyPbusClearCompletion, PhyPbusClearOutcome, PhyPbusClearTransition,
    PhyPbusClearTransitionError, PhyPbusForceTest, PhyPbusHardwareAction, PhyPbusHardwareBinding,
    PhyPbusHardwareBindingError, PhyPbusHardwareObservation,
};

fn reach_work_mode(transition: &mut PhyPbusClearTransition) {
    transition
        .advance(PhyPbusClearCompletion::DebugModeConfigured)
        .unwrap();
    for transaction in PHY_PBUS_CLEAR_TRANSACTIONS {
        assert_eq!(
            transition.action(),
            PhyPbusClearAction::ForceTest(transaction)
        );
        transition
            .advance(PhyPbusClearCompletion::ForceTestCompleted(transaction))
            .unwrap();
    }
    assert_eq!(transition.action(), PhyPbusClearAction::ConfigureWorkMode);
}

#[test]
fn clear_sequence_matches_all_twelve_rom_transactions() {
    assert_eq!(
        PHY_PBUS_CLEAR_TRANSACTIONS,
        [
            PhyPbusForceTest::new(4, 1, 0),
            PhyPbusForceTest::new(4, 2, 0),
            PhyPbusForceTest::new(5, 1, 0),
            PhyPbusForceTest::new(5, 2, 0),
            PhyPbusForceTest::new(0, 1, 0),
            PhyPbusForceTest::new(0, 2, 0),
            PhyPbusForceTest::new(1, 1, 0),
            PhyPbusForceTest::new(1, 2, 0),
            PhyPbusForceTest::new(2, 1, 0x100),
            PhyPbusForceTest::new(3, 1, 0x100),
            PhyPbusForceTest::new(2, 2, 0x100),
            PhyPbusForceTest::new(3, 2, 0x100),
        ]
    );
}

#[test]
fn force_test_completion_is_bound_to_the_current_transaction() {
    let mut transition = PhyPbusClearTransition::new();
    transition
        .advance(PhyPbusClearCompletion::DebugModeConfigured)
        .unwrap();
    let wrong = PHY_PBUS_CLEAR_TRANSACTIONS[1];
    assert_eq!(
        transition.advance(PhyPbusClearCompletion::ForceTestCompleted(wrong)),
        Err(PhyPbusClearTransitionError::WrongCompletion)
    );
    assert_eq!(
        transition.action(),
        PhyPbusClearAction::ForceTest(PHY_PBUS_CLEAR_TRANSACTIONS[0])
    );
}

#[test]
fn work_mode_without_settle_path_finishes_immediately() {
    let mut transition = PhyPbusClearTransition::new();
    reach_work_mode(&mut transition);
    transition
        .advance(PhyPbusClearCompletion::WorkModeConfigured {
            settle_required: false,
        })
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyPbusClearAction::Complete(PhyPbusClearOutcome::Cleared)
    );
}

#[test]
fn work_mode_settle_path_requires_both_async_timer_edges() {
    let mut transition = PhyPbusClearTransition::new();
    reach_work_mode(&mut transition);
    transition
        .advance(PhyPbusClearCompletion::WorkModeConfigured {
            settle_required: true,
        })
        .unwrap();
    assert_eq!(transition.action(), PhyPbusClearAction::DelayMicros(1));
    assert_eq!(
        transition.advance(PhyPbusClearCompletion::WorkModePulseConfigured),
        Err(PhyPbusClearTransitionError::WrongCompletion)
    );
    transition
        .advance(PhyPbusClearCompletion::DelayElapsed)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyPbusClearAction::ConfigureWorkModePulse
    );
    transition
        .advance(PhyPbusClearCompletion::WorkModePulseConfigured)
        .unwrap();
    assert_eq!(transition.action(), PhyPbusClearAction::DelayMicros(2));
    transition
        .advance(PhyPbusClearCompletion::DelayElapsed)
        .unwrap();
    assert_eq!(transition.action(), PhyPbusClearAction::ClearWorkModePulse);
    transition
        .advance(PhyPbusClearCompletion::WorkModePulseCleared)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyPbusClearAction::Complete(PhyPbusClearOutcome::Cleared)
    );
    assert_eq!(
        transition.advance(PhyPbusClearCompletion::WorkModePulseCleared),
        Err(PhyPbusClearTransitionError::AlreadyComplete)
    );
}

#[test]
fn busy_force_test_timeout_is_terminal_without_retry() {
    let mut transition = PhyPbusClearTransition::new();
    transition
        .advance(PhyPbusClearCompletion::DebugModeConfigured)
        .unwrap();
    let transaction = PHY_PBUS_CLEAR_TRANSACTIONS[0];
    transition
        .advance(PhyPbusClearCompletion::ForceTestTimedOut(transaction))
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyPbusClearAction::Complete(PhyPbusClearOutcome::ForceTestTimedOut(transaction))
    );
    assert_eq!(
        transition.advance(PhyPbusClearCompletion::ForceTestCompleted(transaction)),
        Err(PhyPbusClearTransitionError::AlreadyComplete)
    );
}

#[test]
fn generic_hardware_binding_never_advances_from_a_busy_observation() {
    let transaction = PhyPbusForceTest::new(4, 1, 0x123);
    let mut binding = PhyPbusHardwareBinding::new(transaction);
    assert_eq!(binding.action(), PhyPbusHardwareAction::Start(transaction));
    binding.started().unwrap();
    assert_eq!(
        binding.observe_completed(false).unwrap(),
        PhyPbusHardwareObservation::StillPending
    );
    assert_eq!(
        binding.action(),
        PhyPbusHardwareAction::AwaitCompletionEdge(transaction)
    );
    assert_eq!(
        binding.into_transaction(),
        Err(PhyPbusHardwareBindingError::Incomplete)
    );

    let mut binding = PhyPbusHardwareBinding::new(transaction);
    binding.started().unwrap();
    assert_eq!(
        binding.observe_completed(true).unwrap(),
        PhyPbusHardwareObservation::EdgeConsumed
    );
    assert_eq!(binding.into_transaction().unwrap(), transaction);
}

#[test]
fn force_txrx_requires_both_writes_and_both_async_delays_for_each_branch() {
    for enabled in [false, true] {
        let mut transition = PhyForceTxRxTransition::new(enabled);
        let mut actions = std::vec::Vec::new();
        loop {
            let action = transition.action();
            actions.push(action);
            let completion = match PhyForceTxRxExternalBinding::lower(action) {
                Ok(PhyForceTxRxExternalBinding::Mmio(binding)) => {
                    let PhyForceTxRxAction::Configure { enabled, phase } = binding.action() else {
                        panic!("MMIO binding lost its action identity")
                    };
                    PhyForceTxRxCompletion::Configured { enabled, phase }
                }
                Ok(PhyForceTxRxExternalBinding::Timer(binding)) => {
                    assert_eq!(binding.micros(), 1);
                    binding.into_completion()
                }
                Err(_) => break,
            };
            transition.advance(completion).unwrap();
        }
        assert_eq!(
            actions,
            [
                PhyForceTxRxAction::Configure { enabled, phase: 0 },
                PhyForceTxRxAction::DelayMicros {
                    enabled,
                    completed_phase: 0,
                    micros: 1,
                },
                PhyForceTxRxAction::Configure { enabled, phase: 1 },
                PhyForceTxRxAction::DelayMicros {
                    enabled,
                    completed_phase: 1,
                    micros: 1,
                },
                PhyForceTxRxAction::Complete { enabled },
            ]
        );
    }
}

#[test]
fn force_txrx_rejects_a_stale_or_early_timer_edge() {
    let mut transition = PhyForceTxRxTransition::new(true);
    assert_eq!(
        transition.advance(PhyForceTxRxCompletion::DelayElapsed {
            enabled: true,
            completed_phase: 0,
            micros: 1,
        }),
        Err(PhyForceTxRxTransitionError::WrongCompletion)
    );
    assert_eq!(
        transition.action(),
        PhyForceTxRxAction::Configure {
            enabled: true,
            phase: 0,
        }
    );
}
