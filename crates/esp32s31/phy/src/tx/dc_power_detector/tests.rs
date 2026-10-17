use super::*;

#[test]
fn final_tx_enable_command_must_complete_before_sar_or_bluetooth_setup() {
    let parameters = PhyTxDcPwdetParameters {
        dco: [[0x100; 4]; 3],
        clear_tone_after_ready: false,
    };
    for (mut transition, next) in [
        (
            PhyTxDcPwdetTransition::new(parameters),
            PhyTxDcPwdetAction::ConfigureSarCalibration,
        ),
        (
            PhyTxDcPwdetTransition::new_bluetooth(parameters, 7),
            PhyTxDcPwdetAction::ReadPbus {
                selector: 1,
                path: 1,
            },
        ),
    ] {
        transition.step = RootStep::TxOn { index: 9 };
        let previous = PhyPbusForceTest::new(4, 1, 0x0b);
        transition
            .advance(PhyTxDcPwdetCompletion::PbusCompleted(previous))
            .unwrap();
        let final_command = PhyPbusForceTest::new(5, 1, 0x1cf);
        assert_eq!(
            transition.action(),
            PhyTxDcPwdetAction::ForcePbus(final_command)
        );
        assert_eq!(
            transition.advance(PhyTxDcPwdetCompletion::SarCalibrationConfigured),
            Err(PhyTxDcPwdetTransitionError::WrongCompletion)
        );
        assert_eq!(
            transition.action(),
            PhyTxDcPwdetAction::ForcePbus(final_command)
        );
        transition
            .advance(PhyTxDcPwdetCompletion::PbusCompleted(final_command))
            .unwrap();
        assert_eq!(transition.action(), next);
    }
}

#[test]
fn unsupported_pbus_read_fails_closed() {
    assert_eq!(
        require_pbus_result(None),
        Err(PhyTxDcPwdetHardwareInvariant::UnsupportedPbusRead)
    );
}

fn tone_sar_completion(action: PhyToneSarAction, value: u16) -> PhyToneSarCompletion {
    match action {
        PhyToneSarAction::ArmTone {
            measurement,
            sample,
        } => PhyToneSarCompletion::ToneArmed {
            measurement,
            sample,
        },
        PhyToneSarAction::DelayMicros {
            measurement,
            sample,
            phase,
            micros,
        } => PhyToneSarCompletion::DelayElapsed {
            measurement,
            sample,
            phase,
            micros,
        },
        PhyToneSarAction::TriggerSar {
            measurement,
            sample,
        } => PhyToneSarCompletion::SarTriggered {
            measurement,
            sample,
        },
        PhyToneSarAction::PollReady {
            measurement,
            sample,
        } => PhyToneSarCompletion::ReadySampled {
            measurement,
            sample,
            ready: true,
        },
        PhyToneSarAction::ClearTone {
            measurement,
            sample,
        } => PhyToneSarCompletion::ToneCleared {
            measurement,
            sample,
        },
        PhyToneSarAction::ReadSar {
            measurement,
            sample,
        } => PhyToneSarCompletion::SarRead {
            measurement,
            sample,
            value,
        },
        terminal => panic!("unexpected terminal action {terminal:?}"),
    }
}

#[test]
fn child_scan_is_finite_and_commits_only_selected_dco_pair() {
    let initial = [1, 2, 0x100, 0x100];
    let mut transition = PhyTxDcPwdetSearchTransition::new(PhyTxDcPwdetSearchRequest {
        identity: 0,
        initial,
        clear_tone_after_ready: false,
    });
    let mut samples = 0_u16;
    loop {
        let completion = match transition.action() {
            PhyTxDcPwdetSearchAction::ForcePbus(transaction) => {
                PhyTxDcPwdetSearchCompletion::PbusCompleted(transaction)
            }
            PhyTxDcPwdetSearchAction::DelayMicros {
                identity,
                component,
                measurement,
                micros,
            } => PhyTxDcPwdetSearchCompletion::DelayElapsed {
                identity,
                component,
                measurement,
                micros,
            },
            PhyTxDcPwdetSearchAction::ToneSar(action) => {
                if matches!(action, PhyToneSarAction::ReadSar { .. }) {
                    samples += 1;
                }
                PhyTxDcPwdetSearchCompletion::ToneSar(tone_sar_completion(action, 100))
            }
            PhyTxDcPwdetSearchAction::Complete(outcome) => {
                assert_eq!(outcome.dco[0..2], initial[0..2]);
                assert!(outcome.measurements <= 208);
                break;
            }
            PhyTxDcPwdetSearchAction::Failed(failure) => {
                panic!("unexpected failure {failure:?}")
            }
        };
        transition.advance(completion).unwrap();
    }
    assert!(samples <= 416);
}

#[test]
fn bluetooth_mode_reads_and_forces_the_bluetooth_tx_path_before_sar_setup() {
    let mut transition = PhyTxDcPwdetTransition::new_bluetooth(
        PhyTxDcPwdetParameters {
            dco: [[0; 4]; 3],
            clear_tone_after_ready: false,
        },
        0x12,
    );

    loop {
        let completion = match transition.action() {
            PhyTxDcPwdetAction::PrepareRegisters => PhyTxDcPwdetCompletion::RegistersPrepared,
            PhyTxDcPwdetAction::ConfigureTxClock { enabled } => {
                PhyTxDcPwdetCompletion::TxClockConfigured { enabled }
            }
            PhyTxDcPwdetAction::ConfigurePowerDetector => {
                PhyTxDcPwdetCompletion::PowerDetectorConfigured
            }
            PhyTxDcPwdetAction::ConfigurePbusDebugMode => {
                PhyTxDcPwdetCompletion::PbusDebugModeConfigured
            }
            PhyTxDcPwdetAction::ForcePbus(transaction) => {
                PhyTxDcPwdetCompletion::PbusCompleted(transaction)
            }
            PhyTxDcPwdetAction::ConfigureTone {
                enabled,
                selector,
                attenuation,
            } => PhyTxDcPwdetCompletion::ToneConfigured {
                enabled,
                selector,
                attenuation,
            },
            PhyTxDcPwdetAction::DelayMicros { phase, micros } => {
                PhyTxDcPwdetCompletion::DelayElapsed { phase, micros }
            }
            PhyTxDcPwdetAction::ReadPbus { selector, path } => {
                assert_eq!((selector, path), (1, 1));
                transition
                    .advance(PhyTxDcPwdetCompletion::PbusRead {
                        selector,
                        path,
                        value: 0x34,
                    })
                    .unwrap();
                break;
            }
            action => panic!("unexpected Bluetooth prefix action {action:?}"),
        };
        transition.advance(completion).unwrap();
    }

    let forced_path = PhyPbusForceTest::new(1, 1, 0x36);
    assert_eq!(
        transition.action(),
        PhyTxDcPwdetAction::ForcePbus(forced_path)
    );
    transition
        .advance(PhyTxDcPwdetCompletion::PbusCompleted(forced_path))
        .unwrap();

    let forced_tx_path = PhyPbusForceTest::new(4, 2, 0x90);
    assert_eq!(
        transition.action(),
        PhyTxDcPwdetAction::ForcePbus(forced_tx_path)
    );
    transition
        .advance(PhyTxDcPwdetCompletion::PbusCompleted(forced_tx_path))
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyTxDcPwdetAction::ConfigureSarCalibration
    );
}

#[test]
fn root_cleanup_restores_registers_before_terminal_failure() {
    let mut transition = PhyTxDcPwdetTransition::new(PhyTxDcPwdetParameters {
        dco: [[0; 4]; 3],
        clear_tone_after_ready: false,
    });
    let mut inject_initial_failure = true;
    let mut pulse_cleared = false;
    let mut clock_disabled = false;
    let mut registers_restored = false;
    loop {
        let completion = match transition.action() {
            PhyTxDcPwdetAction::PrepareRegisters => PhyTxDcPwdetCompletion::RegistersPrepared,
            PhyTxDcPwdetAction::ConfigureTxClock { enabled } => {
                if !enabled {
                    assert!(pulse_cleared);
                    clock_disabled = true;
                }
                PhyTxDcPwdetCompletion::TxClockConfigured { enabled }
            }
            PhyTxDcPwdetAction::ConfigurePowerDetector => {
                PhyTxDcPwdetCompletion::PowerDetectorConfigured
            }
            PhyTxDcPwdetAction::ConfigurePbusDebugMode => {
                PhyTxDcPwdetCompletion::PbusDebugModeConfigured
            }
            PhyTxDcPwdetAction::ForcePbus(transaction) if inject_initial_failure => {
                inject_initial_failure = false;
                PhyTxDcPwdetCompletion::PbusTimedOut(transaction)
            }
            PhyTxDcPwdetAction::ForcePbus(transaction) => {
                PhyTxDcPwdetCompletion::PbusCompleted(transaction)
            }
            PhyTxDcPwdetAction::ConfigureTone {
                enabled,
                selector,
                attenuation,
            } => PhyTxDcPwdetCompletion::ToneConfigured {
                enabled,
                selector,
                attenuation,
            },
            PhyTxDcPwdetAction::ConfigurePbusWorkMode => {
                PhyTxDcPwdetCompletion::PbusWorkModeConfigured {
                    settle_required: true,
                }
            }
            PhyTxDcPwdetAction::DelayMicros {
                phase: PhyTxDcPwdetDelayPhase::WorkMode,
                micros,
            } => PhyTxDcPwdetCompletion::DelayElapsed {
                phase: PhyTxDcPwdetDelayPhase::WorkMode,
                micros,
            },
            PhyTxDcPwdetAction::ConfigurePbusWorkModePulse => {
                PhyTxDcPwdetCompletion::PbusWorkModePulseConfigured
            }
            PhyTxDcPwdetAction::DelayMicros {
                phase: PhyTxDcPwdetDelayPhase::WorkModePulse,
                micros,
            } => {
                assert_eq!(micros, 2);
                PhyTxDcPwdetCompletion::DelayElapsed {
                    phase: PhyTxDcPwdetDelayPhase::WorkModePulse,
                    micros,
                }
            }
            PhyTxDcPwdetAction::ClearPbusWorkModePulse => {
                pulse_cleared = true;
                PhyTxDcPwdetCompletion::PbusWorkModePulseCleared
            }
            PhyTxDcPwdetAction::RestoreRegisters => {
                assert!(clock_disabled);
                registers_restored = true;
                PhyTxDcPwdetCompletion::RegistersRestored
            }
            PhyTxDcPwdetAction::Failed(PhyTxDcPwdetFailure::PbusTimedOut(_)) => {
                assert!(registers_restored);
                break;
            }
            action => panic!("unexpected cleanup action {action:?}"),
        };
        transition.advance(completion).unwrap();
    }
}

#[test]
fn external_lowering_covers_root_and_search_operation_classes() {
    let transaction = PhyPbusForceTest::new(1, 2, 0x80);
    assert!(matches!(
        PhyTxDcPwdetExternalBinding::lower(PhyTxDcPwdetAction::PrepareRegisters),
        Ok(PhyTxDcPwdetExternalBinding::Mmio(_))
    ));
    assert!(matches!(
        PhyTxDcPwdetExternalBinding::lower(PhyTxDcPwdetAction::ForcePbus(transaction)),
        Ok(PhyTxDcPwdetExternalBinding::Pbus(_))
    ));
    assert!(matches!(
        PhyTxDcPwdetExternalBinding::lower(PhyTxDcPwdetAction::DelayMicros {
            phase: PhyTxDcPwdetDelayPhase::InitialTone,
            micros: 1,
        }),
        Ok(PhyTxDcPwdetExternalBinding::Timer(_))
    ));
    assert!(matches!(
        PhyTxDcPwdetExternalBinding::lower(PhyTxDcPwdetAction::Search(
            PhyTxDcPwdetSearchAction::ForcePbus(transaction)
        )),
        Ok(PhyTxDcPwdetExternalBinding::Search(
            PhyTxDcPwdetSearchExternalBinding::Pbus(_)
        ))
    ));
    assert!(matches!(
        PhyTxDcPwdetSearchExternalBinding::lower(PhyTxDcPwdetSearchAction::DelayMicros {
            identity: 1,
            component: 2,
            measurement: 3,
            micros: 2,
        }),
        Ok(PhyTxDcPwdetSearchExternalBinding::Timer(_))
    ));
    assert!(matches!(
        PhyTxDcPwdetSearchExternalBinding::lower(PhyTxDcPwdetSearchAction::ToneSar(
            PhyToneSarAction::ClearTone {
                measurement: 0,
                sample: 0,
            }
        )),
        Ok(PhyTxDcPwdetSearchExternalBinding::ToneSar(_))
    ));
    assert!(matches!(
        PhyTxDcPwdetExternalBinding::lower(PhyTxDcPwdetAction::Complete(PhyTxDcPwdetOutcome {
            dco: [[0; 4]; 3],
            total_measurements: 0,
        })),
        Err(PhyTxDcPwdetExternalBindingError::UnsupportedAction)
    ));
}

#[test]
fn wifi_calibration_forces_tx_path_per_row_and_cleans_it_up() {
    let mut transition = PhyTxDcPwdetTransition::new(PhyTxDcPwdetParameters {
        dco: [[0; 4]; 3],
        clear_tone_after_ready: false,
    });
    let mut path_values = std::vec::Vec::new();
    let mut sar_configured = false;
    let mut awaiting_gain = false;
    loop {
        let completion = match transition.action() {
            PhyTxDcPwdetAction::PrepareRegisters => PhyTxDcPwdetCompletion::RegistersPrepared,
            PhyTxDcPwdetAction::ConfigureTxClock { enabled } => {
                PhyTxDcPwdetCompletion::TxClockConfigured { enabled }
            }
            PhyTxDcPwdetAction::ConfigurePowerDetector => {
                PhyTxDcPwdetCompletion::PowerDetectorConfigured
            }
            PhyTxDcPwdetAction::ConfigurePbusDebugMode => {
                PhyTxDcPwdetCompletion::PbusDebugModeConfigured
            }
            PhyTxDcPwdetAction::ForcePbus(transaction) => {
                if awaiting_gain {
                    assert_eq!((transaction.selector(), transaction.path()), (1, 2));
                    awaiting_gain = false;
                }
                if (transaction.selector(), transaction.path()) == (5, 1) {
                    path_values.push(transaction.value());
                    if transaction.value() != 0 && sar_configured {
                        awaiting_gain = true;
                    } else if transaction.value() != 0 {
                        assert_eq!(
                            transaction.value(),
                            0x1cf,
                            "initial TX-on uses its own path value"
                        );
                    }
                }
                PhyTxDcPwdetCompletion::PbusCompleted(transaction)
            }
            PhyTxDcPwdetAction::ConfigureTone {
                enabled,
                selector,
                attenuation,
            } => PhyTxDcPwdetCompletion::ToneConfigured {
                enabled,
                selector,
                attenuation,
            },
            PhyTxDcPwdetAction::DelayMicros { phase, micros } => {
                PhyTxDcPwdetCompletion::DelayElapsed { phase, micros }
            }
            PhyTxDcPwdetAction::ConfigureSarCalibration => {
                assert_eq!(path_values.last(), Some(&0x1cf));
                sar_configured = true;
                PhyTxDcPwdetCompletion::SarCalibrationConfigured
            }
            PhyTxDcPwdetAction::Search(action) => {
                assert!(!awaiting_gain);
                let completion = match action {
                    PhyTxDcPwdetSearchAction::ForcePbus(transaction) => {
                        PhyTxDcPwdetSearchCompletion::PbusCompleted(transaction)
                    }
                    PhyTxDcPwdetSearchAction::DelayMicros {
                        identity,
                        component,
                        measurement,
                        micros,
                    } => PhyTxDcPwdetSearchCompletion::DelayElapsed {
                        identity,
                        component,
                        measurement,
                        micros,
                    },
                    PhyTxDcPwdetSearchAction::ToneSar(action) => {
                        PhyTxDcPwdetSearchCompletion::ToneSar(tone_sar_completion(action, 100))
                    }
                    action => panic!("unexpected search action {action:?}"),
                };
                PhyTxDcPwdetCompletion::Search(completion)
            }
            PhyTxDcPwdetAction::ConfigurePbusWorkMode => {
                PhyTxDcPwdetCompletion::PbusWorkModeConfigured {
                    settle_required: false,
                }
            }
            PhyTxDcPwdetAction::RestoreRegisters => PhyTxDcPwdetCompletion::RegistersRestored,
            PhyTxDcPwdetAction::Complete(_) => break,
            action => panic!("unexpected calibration action {action:?}"),
        };
        transition.advance(completion).unwrap();
    }
    assert_eq!(path_values, [0, 0x1cf, 0x1ef, 0x1ef, 0x1e7, 0]);
}

fn search_completion(action: PhyTxDcPwdetSearchAction) -> PhyTxDcPwdetSearchCompletion {
    match action {
        PhyTxDcPwdetSearchAction::ForcePbus(transaction) => {
            PhyTxDcPwdetSearchCompletion::PbusCompleted(transaction)
        }
        PhyTxDcPwdetSearchAction::DelayMicros {
            identity,
            component,
            measurement,
            micros,
        } => PhyTxDcPwdetSearchCompletion::DelayElapsed {
            identity,
            component,
            measurement,
            micros,
        },
        PhyTxDcPwdetSearchAction::ToneSar(action) => {
            PhyTxDcPwdetSearchCompletion::ToneSar(tone_sar_completion(action, 100))
        }
        action => panic!("unexpected nested search action: {action:?}"),
    }
}

fn search_root(identity: u8) -> PhyTxDcPwdetTransition {
    let mut root = PhyTxDcPwdetTransition::new(PhyTxDcPwdetParameters {
        dco: [[1, 2, 0x100, 0x100]; 3],
        clear_tone_after_ready: false,
    });
    root.step = RootStep::Search(PhyTxDcPwdetSearchTransition::new(
        PhyTxDcPwdetSearchRequest {
            identity,
            initial: [1, 2, 0x100, 0x100],
            clear_tone_after_ready: false,
        },
    ));
    root
}

#[test]
fn nested_search_rejects_wrong_completions_without_changing_parent() {
    let mut root = search_root(0);
    let mut steps = 0;
    while let PhyTxDcPwdetAction::Search(action) = root.action() {
        let before = root;
        assert_eq!(
            root.advance(PhyTxDcPwdetCompletion::Search(
                PhyTxDcPwdetSearchCompletion::DelayElapsed {
                    identity: 0,
                    component: 0,
                    measurement: 0,
                    micros: 0,
                }
            )),
            Err(PhyTxDcPwdetTransitionError::WrongCompletion)
        );
        assert_eq!(root, before);
        // Exercise rejection inside ToneSar too, after borrowing the child.
        assert_eq!(
            root.advance(PhyTxDcPwdetCompletion::Search(
                PhyTxDcPwdetSearchCompletion::ToneSar(PhyToneSarCompletion::SarRead {
                    measurement: 0,
                    sample: u8::MAX,
                    value: 0,
                })
            )),
            Err(PhyTxDcPwdetTransitionError::WrongCompletion)
        );
        assert_eq!(root, before);
        root.advance(PhyTxDcPwdetCompletion::Search(search_completion(action)))
            .unwrap();
        steps += 1;
        assert!(steps < 10_000);
    }
    assert_eq!(root.row, 1);
    assert_eq!(root.dco[0][..2], [1, 2]);
    assert!(root.total_measurements > 0);
    assert!(matches!(root.action(), PhyTxDcPwdetAction::ForcePbus(_)));
}

#[test]
fn nested_search_failure_enters_root_cleanup() {
    let mut root = search_root(0);
    let PhyTxDcPwdetAction::Search(PhyTxDcPwdetSearchAction::ForcePbus(transaction)) =
        root.action()
    else {
        panic!("first DCO transaction")
    };
    root.advance(PhyTxDcPwdetCompletion::Search(
        PhyTxDcPwdetSearchCompletion::PbusTimedOut(transaction),
    ))
    .unwrap();
    assert!(matches!(
        root.step,
        RootStep::CleanupDco {
            terminal: RootTerminal::Failed(_),
            ..
        }
    ));
    assert_eq!(root.row, 0);
    assert_eq!(root.total_measurements, 0);
}

/// Host state-machine cost, with deterministic SAR completions and no MMIO.
#[test]
#[ignore = "manual release-mode CPU benchmark of the nested production TX search"]
fn benchmark_nested_tx_search() {
    for sample in 0..3 {
        let start = std::time::Instant::now();
        let mut steps = 0;
        for iteration in 0..2000 {
            let mut root = search_root(core::hint::black_box(iteration as u8));
            while let PhyTxDcPwdetAction::Search(action) = core::hint::black_box(&root).action() {
                let completion = PhyTxDcPwdetCompletion::Search(search_completion(action));
                core::hint::black_box(&mut root)
                    .advance(core::hint::black_box(completion))
                    .unwrap();
                steps += 1;
            }
            core::hint::black_box(root);
        }
        std::println!(
            "sample={sample} iterations=2000 steps={steps} elapsed_ns={}",
            start.elapsed().as_nanos()
        );
    }
}
