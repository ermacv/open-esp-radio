use super::*;

#[test]
fn external_lowering_covers_each_txdc_operation_class_and_rejects_terminals() {
    assert!(matches!(
        PhyTxDcExternalBinding::lower(PhyTxDcAction::ConfigurePbusDebugMode),
        Ok(PhyTxDcExternalBinding::Mmio(_))
    ));
    assert!(matches!(
        PhyTxDcExternalBinding::lower(PhyTxDcAction::ReadPbus {
            selector: 1,
            path: 1,
        }),
        Ok(PhyTxDcExternalBinding::Mmio(_))
    ));
    assert!(matches!(
        PhyTxDcExternalBinding::lower(PhyTxDcAction::ForcePbus(PhyPbusForceTest::new(4, 1, 0))),
        Ok(PhyTxDcExternalBinding::Pbus(_))
    ));
    assert!(matches!(
        PhyTxDcExternalBinding::lower(PhyTxDcAction::DelayMicros {
            phase: PhyTxDcDelayPhase::ComparatorSettle {
                gain_index: 0,
                iteration: 0,
            },
            micros: 1,
        }),
        Ok(PhyTxDcExternalBinding::Timer(_))
    ));
    assert!(matches!(
        PhyTxDcExternalBinding::lower(PhyTxDcAction::PollReady {
            gain_index: 0,
            iteration: 0,
        }),
        Ok(PhyTxDcExternalBinding::Ready(_))
    ));
    assert_eq!(
        PhyTxDcExternalBinding::lower(PhyTxDcAction::Complete(PhyTxDcOutcome {
            dco: [[0; 4]; PHY_TX_DC_GAIN_COUNT as usize],
        })),
        Err(PhyTxDcExternalBindingError::UnsupportedAction)
    );
}

#[test]
fn gain_table_matches_complete_rom_object() {
    assert_eq!(
        (0..PHY_TX_DC_GAIN_COUNT)
            .map(tx_bb_gain)
            .collect::<std::vec::Vec<_>>(),
        [0, 128, 256, 32, 160]
    );
    assert_eq!(tx_bb_gain(5), 128);
}

#[test]
fn dco_adjustment_saturates_at_exact_nine_bit_limits() {
    assert_eq!(adjusted_dco(10, 20, true), 0);
    assert_eq!(adjusted_dco(500, 20, false), 511);
    assert_eq!(adjusted_dco(256, 124, true), 132);
    assert_eq!(adjusted_dco(256, 124, false), 380);
}

#[test]
fn twelve_step_search_averages_only_last_four_adjusted_values() {
    let mut search = Search::new(0);
    let mut expected_step = [124, 63, 32, 17, 9, 5, 3, 2, 1, 1, 1, 1].into_iter();
    while search.iteration != PHY_TX_DC_ITERATION_COUNT {
        assert_eq!(search.step, expected_step.next().unwrap());
        search.apply_comparators([false, false]);
    }
    assert_eq!(search.average(), [511, 511]);
}

#[test]
fn readiness_false_sample_preserves_exact_poll_action() {
    let parameters = PhyTxDcParameters {
        pbus_rx_path_value: 0xbf,
    };
    let search = Search::new(2);
    let mut transition = PhyTxDcTransition {
        parameters,
        mode: PhyTxDcMode::Wifi,
        dco: [[INITIAL_DCO; 4]; 5],
        step: Step::PollReady(search),
    };
    let action = transition.action();
    assert!(PhyTxDcReadyBinding::new(action).is_ok());
    transition
        .advance(PhyTxDcCompletion::ReadySampled {
            gain_index: 2,
            iteration: 0,
            ready: false,
        })
        .unwrap();
    assert_eq!(transition.action(), action);
}

#[test]
fn deadline_enters_tone_cleanup_before_pbus_restore() {
    let search = Search::new(1);
    let mut transition = PhyTxDcTransition {
        parameters: PhyTxDcParameters {
            pbus_rx_path_value: 0xbf,
        },
        mode: PhyTxDcMode::Wifi,
        dco: [[INITIAL_DCO; 4]; 5],
        step: Step::PollReady(search),
    };
    transition
        .advance(PhyTxDcCompletion::ReadyDeadlineElapsed {
            gain_index: 1,
            iteration: 0,
        })
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyTxDcAction::ConfigureTone {
            enabled: false,
            selector: 600,
            step: 120,
        }
    );
}

#[test]
fn bluetooth_variant_retains_the_complete_extra_pbus_prefix() {
    let mut transition = PhyTxDcTransition::new_bluetooth(
        PhyTxDcParameters {
            pbus_rx_path_value: 0xbf,
        },
        0x35,
    );
    assert_eq!(transition.action(), PhyTxDcAction::ConfigurePbusDebugMode);
    transition
        .advance(PhyTxDcCompletion::PbusDebugModeConfigured)
        .unwrap();

    for index in 0..ENTER_PBUS_COUNT {
        let transaction = enter_pbus_transaction(index);
        assert_eq!(transition.action(), PhyTxDcAction::ForcePbus(transaction));
        transition
            .advance(PhyTxDcCompletion::PbusCompleted(transaction))
            .unwrap();
    }

    assert_eq!(
        transition.action(),
        PhyTxDcAction::ReadPbus {
            selector: 1,
            path: 1,
        }
    );
    transition
        .advance(PhyTxDcCompletion::PbusRead {
            selector: 1,
            path: 1,
            value: 0x40,
        })
        .unwrap();

    let gain_control = PhyPbusForceTest::new(1, 1, 0x42);
    assert_eq!(transition.action(), PhyTxDcAction::ForcePbus(gain_control));
    transition
        .advance(PhyTxDcCompletion::PbusCompleted(gain_control))
        .unwrap();

    let tx_path = PhyPbusForceTest::new(4, 2, 0x35 << 3);
    assert_eq!(transition.action(), PhyTxDcAction::ForcePbus(tx_path));
    transition
        .advance(PhyTxDcCompletion::PbusCompleted(tx_path))
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyTxDcAction::ForcePbus(PhyPbusForceTest::new(1, 2, 0))
    );
    assert_eq!(transition.gain_count(), 3);
    assert_eq!(transition.selected_gain(0), 0);
    assert_eq!(transition.selected_gain(1), 0x80);
    assert_eq!(transition.selected_gain(2), 0x100);
}

#[test]
fn bluetooth_variant_completes_exactly_three_common_comparator_searches() {
    let mut transition = PhyTxDcTransition::new_bluetooth(
        PhyTxDcParameters {
            pbus_rx_path_value: 0xbf,
        },
        0x35,
    );
    let mut comparator_reads = [0_u8; 3];

    loop {
        let action = transition.action();
        let completion = match action {
            PhyTxDcAction::ConfigurePbusDebugMode => PhyTxDcCompletion::PbusDebugModeConfigured,
            PhyTxDcAction::ReadPbus { selector, path } => PhyTxDcCompletion::PbusRead {
                selector,
                path,
                value: 0x40,
            },
            PhyTxDcAction::ForcePbus(transaction) => PhyTxDcCompletion::PbusCompleted(transaction),
            PhyTxDcAction::ConfigureTxClock => PhyTxDcCompletion::TxClockConfigured,
            PhyTxDcAction::ConfigureTone {
                enabled,
                selector,
                step,
            } => PhyTxDcCompletion::ToneConfigured {
                enabled,
                selector,
                step,
            },
            PhyTxDcAction::DelayMicros { phase, micros } => {
                PhyTxDcCompletion::DelayElapsed { phase, micros }
            }
            PhyTxDcAction::TriggerMeasurement {
                gain_index,
                iteration,
            } => PhyTxDcCompletion::MeasurementTriggered {
                gain_index,
                iteration,
            },
            PhyTxDcAction::PollReady {
                gain_index,
                iteration,
            } => PhyTxDcCompletion::ReadySampled {
                gain_index,
                iteration,
                ready: true,
            },
            PhyTxDcAction::ReadComparators {
                gain_index,
                iteration,
            } => {
                comparator_reads[gain_index as usize] += 1;
                PhyTxDcCompletion::ComparatorsRead {
                    gain_index,
                    iteration,
                    comparator_high: [false, false],
                }
            }
            PhyTxDcAction::ClearMeasurement => PhyTxDcCompletion::MeasurementCleared,
            PhyTxDcAction::ConfigurePbusWorkMode => PhyTxDcCompletion::PbusWorkModeConfigured {
                settle_required: false,
            },
            PhyTxDcAction::ConfigurePbusWorkModePulse | PhyTxDcAction::ClearPbusWorkModePulse => {
                panic!("no pulse is emitted when work mode needs no settling")
            }
            PhyTxDcAction::Complete(outcome) => {
                assert_eq!(comparator_reads, [12, 12, 12]);
                assert_eq!(outcome.dco[0], [0x1ff, 0x1ff, 0x100, 0x100]);
                assert_eq!(outcome.dco[1], [0x1ff, 0x1ff, 0x100, 0x100]);
                assert_eq!(outcome.dco[2], [0x1ff, 0x1ff, 0x100, 0x100]);
                assert_eq!(outcome.dco[3], [0x100; 4]);
                assert_eq!(outcome.dco[4], [0x100; 4]);
                break;
            }
            PhyTxDcAction::Failed(failure) => panic!("unexpected failure: {failure:?}"),
        };
        transition.advance(completion).unwrap();
    }
}
