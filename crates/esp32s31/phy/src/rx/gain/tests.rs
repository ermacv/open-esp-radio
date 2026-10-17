use super::*;
use std::vec::Vec;

fn parameters() -> PhyRxGainMemoryParameters {
    PhyRxGainMemoryParameters {
        parameter_002: 0xbf,
        wifi_index_dc: [[0x100; 2]; 8],
        wifi_fine_dc: [[0x100; 2]; crate::rx::gain_calibration::FINE_CODES],
        shared_index_dc: [[0x100; 2]; 11],
        wifi_auxiliary: 0,
    }
}

fn init_parameters() -> PhyRxGainInitParameters {
    PhyRxGainInitParameters {
        dc_calibrated: false,
        tables_initialized: false,
        dc: PhyRxGainDcParameters {
            crystal_selector: 0,
            pbus_rx_path_value: 0,
            rx_saturation_detected: false,
        },
        memory: parameters(),
    }
}

fn complete(action: PhyRxGainPublishAction) -> PhyRxGainPublishCompletion {
    match action {
        PhyRxGainPublishAction::ConfigurePbusDebugMode { bank } => {
            PhyRxGainPublishCompletion::PbusDebugModeConfigured { bank }
        }
        PhyRxGainPublishAction::ForcePbus { bank, transaction } => {
            PhyRxGainPublishCompletion::PbusCompleted { bank, transaction }
        }
        PhyRxGainPublishAction::ConfigureClock {
            bank,
            clock,
            enabled,
        } => PhyRxGainPublishCompletion::ClockConfigured {
            bank,
            clock,
            enabled,
        },
        PhyRxGainPublishAction::ProgramEntry { bank, entry } => {
            PhyRxGainPublishCompletion::EntryProgrammed { bank, entry }
        }
        PhyRxGainPublishAction::ConfigurePbusWorkMode { bank } => {
            PhyRxGainPublishCompletion::PbusWorkModeConfigured {
                bank,
                settle_required: false,
            }
        }
        PhyRxGainPublishAction::ConfigurePbusWorkModePulse { bank } => {
            PhyRxGainPublishCompletion::PbusWorkModePulseConfigured { bank }
        }
        PhyRxGainPublishAction::ClearPbusWorkModePulse { bank } => {
            PhyRxGainPublishCompletion::PbusWorkModePulseCleared { bank }
        }
        PhyRxGainPublishAction::DelayMicros { phase, micros } => {
            PhyRxGainPublishCompletion::DelayElapsed { phase, micros }
        }
        PhyRxGainPublishAction::Complete(_) | PhyRxGainPublishAction::Failed(_) => {
            panic!("terminal action")
        }
    }
}

#[test]
fn recalibration_publishes_fresh_fine_dc_only_in_wifi_bank() {
    fn publish(old: u16, fresh: u16) -> Vec<(PhyRxGainBank, PhyGainMemoryEntry)> {
        let mut parameters = init_parameters();
        parameters.memory.wifi_fine_dc = [[old; 2]; crate::rx::gain_calibration::FINE_CODES];
        let outcome = PhyRxGainDcOutcome {
            quality: Default::default(),
            wifi_index_dc: [[0x100; 2]; 8],
            wifi_fine_dc: [[fresh, fresh.wrapping_neg()]; crate::rx::gain_calibration::FINE_CODES],
            shared_index_dc: [[0x100; 2]; 11],
        };
        let mut root = PhyRxGainInitTransition::new(parameters);
        // Exercise the real handoff after a completed measurement, including
        // outer control restoration and every table publication action.
        root.step = InitStep::RestoreDcControl { outcome };
        root.advance(PhyRxGainInitCompletion::DcControlRestored)
            .unwrap();
        let mut entries = Vec::new();
        loop {
            match root.action() {
                PhyRxGainInitAction::Publish(action) => {
                    if let PhyRxGainPublishAction::ProgramEntry { bank, entry } = action {
                        entries.push((bank, entry));
                    }
                    root.advance(PhyRxGainInitCompletion::Publish(complete(action)))
                        .unwrap();
                }
                PhyRxGainInitAction::ConfigureLimits { wifi_last_index } => root
                    .advance(PhyRxGainInitCompletion::LimitsConfigured { wifi_last_index })
                    .unwrap(),
                PhyRxGainInitAction::EnableIqCorrection => root
                    .advance(PhyRxGainInitCompletion::IqCorrectionEnabled)
                    .unwrap(),
                PhyRxGainInitAction::Complete => {
                    let Some(Ok(result)) = root.terminal() else {
                        panic!("complete root did not retain its outcome");
                    };
                    assert_eq!(result.dc, Some(outcome));
                    return entries;
                }
                other => panic!("unexpected publication action: {other:?}"),
            }
        }
    }

    let fresh = publish(0, 3);
    assert_eq!(
        fresh,
        publish(19, 3),
        "old fine DC pairs must not leak into publication"
    );
    let changed = publish(0, 7);
    for bank in [PhyRxGainBank::Wifi, PhyRxGainBank::Shared] {
        let entries = |values: &[(PhyRxGainBank, PhyGainMemoryEntry)]| {
            values
                .iter()
                .filter(|(entry_bank, _)| *entry_bank == bank)
                .map(|(_, entry)| *entry)
                .collect::<Vec<_>>()
        };
        assert!(!entries(&fresh).is_empty());
        match bank {
            PhyRxGainBank::Wifi => assert_ne!(entries(&fresh), entries(&changed)),
            PhyRxGainBank::Shared => assert_eq!(entries(&fresh), entries(&changed)),
        }
    }
}

#[test]
fn complete_publisher_emits_72_wifi_and_76_shared_entries() {
    let mut transition = PhyRxGainPublishTransition::new(parameters());
    let mut wifi_entries = 0;
    let mut shared_entries = 0;
    loop {
        let action = transition.action();
        match action {
            PhyRxGainPublishAction::ProgramEntry {
                bank: PhyRxGainBank::Wifi,
                ..
            } => wifi_entries += 1,
            PhyRxGainPublishAction::ProgramEntry {
                bank: PhyRxGainBank::Shared,
                ..
            } => shared_entries += 1,
            PhyRxGainPublishAction::Complete(outcome) => {
                assert_eq!(outcome.wifi_entries, 72);
                assert_eq!(outcome.shared_entries, 76);
                break;
            }
            _ => {}
        }
        transition.advance(complete(action)).unwrap();
    }
    assert_eq!(wifi_entries, 72);
    assert_eq!(shared_entries, 76);
}

#[test]
fn pbus_failure_is_terminal_and_preserves_operation_identity() {
    let mut transition = PhyRxGainPublishTransition::new(parameters());
    transition
        .advance(PhyRxGainPublishCompletion::PbusDebugModeConfigured {
            bank: PhyRxGainBank::Wifi,
        })
        .unwrap();
    let PhyRxGainPublishAction::ForcePbus { bank, transaction } = transition.action() else {
        panic!("expected PBus");
    };
    transition
        .advance(PhyRxGainPublishCompletion::PbusTimedOut { bank, transaction })
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRxGainPublishAction::ConfigurePbusWorkMode { bank }
    );
    transition
        .advance(PhyRxGainPublishCompletion::PbusWorkModeConfigured {
            bank,
            settle_required: false,
        })
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRxGainPublishAction::Failed(PhyRxGainPublishFailure::PbusTimedOut { bank, transaction })
    );
}

#[test]
fn nested_dc_failure_restores_outer_control_before_becoming_terminal() {
    let failure = PhyRxGainDcFailure::Pbus {
        bank: crate::rx::gain_calibration::PhyRxGainDcBank::Wifi,
        transaction: crate::analog::pbus::PhyPbusForceTest::new(2, 1, 0x100),
    };
    let mut transition = PhyRxGainInitTransition::new(init_parameters());
    transition.step = InitStep::RestoreDcControlAfterFailure(failure);

    assert_eq!(transition.action(), PhyRxGainInitAction::RestoreDcControl);
    transition
        .advance(PhyRxGainInitCompletion::DcControlRestored)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRxGainInitAction::Failed(PhyRxGainInitFailure::Dc(failure))
    );
}

#[test]
fn work_mode_pulse_uses_the_vendor_two_microsecond_delay() {
    let mut transition = PhyRxGainPublishTransition::new(parameters());
    transition.step = Step::WorkModePulseDelay {
        bank: PhyRxGainBank::Wifi,
        failure: None,
    };
    assert_eq!(
        transition.action(),
        PhyRxGainPublishAction::DelayMicros {
            phase: PhyRxGainDelayPhase::PbusWorkModePulse {
                bank: PhyRxGainBank::Wifi,
            },
            micros: 2,
        }
    );
    assert_eq!(
        transition.advance(PhyRxGainPublishCompletion::DelayElapsed {
            phase: PhyRxGainDelayPhase::PbusWorkModePulse {
                bank: PhyRxGainBank::Wifi,
            },
            micros: 1,
        }),
        Err(PhyRxGainPublishTransitionError::WrongCompletion)
    );
}

fn publish_root(parameters: PhyRxGainInitParameters) -> (PhyRxGainInitOutcome, usize) {
    let mut root = PhyRxGainInitTransition::new(parameters);
    let mut steps = 0;
    loop {
        let action = core::hint::black_box(&root).action();
        let completion = match action {
            PhyRxGainInitAction::Publish(action) => {
                PhyRxGainInitCompletion::Publish(complete(action))
            }
            PhyRxGainInitAction::ConfigureLimits { wifi_last_index } => {
                PhyRxGainInitCompletion::LimitsConfigured { wifi_last_index }
            }
            PhyRxGainInitAction::EnableIqCorrection => PhyRxGainInitCompletion::IqCorrectionEnabled,
            PhyRxGainInitAction::Complete => {
                let Some(Ok(outcome)) = root.terminal() else {
                    panic!("complete root did not retain its outcome");
                };
                return (outcome, steps);
            }
            other => panic!("unexpected publish-only action: {other:?}"),
        };
        core::hint::black_box(&mut root)
            .advance(core::hint::black_box(completion))
            .unwrap();
        steps += 1;
    }
}

#[test]
fn root_publishes_both_banks_and_retains_state_on_rejected_completion() {
    let mut parameters = init_parameters();
    parameters.dc_calibrated = true;
    let mut root = PhyRxGainInitTransition::new(parameters);
    loop {
        let action = root.action();
        let PhyRxGainInitAction::Publish(action) = action else {
            break;
        };
        let before = root;
        assert_eq!(
            root.advance(PhyRxGainInitCompletion::Publish(
                PhyRxGainPublishCompletion::DelayElapsed {
                    phase: PhyRxGainDelayPhase::PbusWorkMode {
                        bank: PhyRxGainBank::Wifi
                    },
                    micros: 0,
                }
            )),
            Err(PhyRxGainInitTransitionError::WrongCompletion)
        );
        assert_eq!(root, before);
        if let PhyRxGainPublishAction::ProgramEntry { bank, entry } = action {
            let wrong_bank = match bank {
                PhyRxGainBank::Wifi => PhyRxGainBank::Shared,
                PhyRxGainBank::Shared => PhyRxGainBank::Wifi,
            };
            assert_eq!(
                root.advance(PhyRxGainInitCompletion::Publish(
                    PhyRxGainPublishCompletion::EntryProgrammed {
                        bank: wrong_bank,
                        entry
                    }
                )),
                Err(PhyRxGainInitTransitionError::WrongCompletion)
            );
            assert_eq!(root, before);
        }

        root.advance(PhyRxGainInitCompletion::Publish(complete(action)))
            .unwrap();
    }
    assert!(matches!(
        root.action(),
        PhyRxGainInitAction::ConfigureLimits { .. }
    ));
    let (outcome, _) = publish_root(parameters);
    assert!(outcome.generated_tables);
    assert_eq!(
        (outcome.wifi_last_index, outcome.shared_last_index),
        (71, 75)
    );
}

/// Host CPU cost only: no MMIO, waits or radio-quality claim.
#[test]
#[ignore = "manual release-mode CPU benchmark of the production publisher"]
fn benchmark_rx_gain_publication() {
    for sample in 0..3 {
        let start = std::time::Instant::now();
        let mut steps = 0;
        for iteration in 0..20_000 {
            let mut parameters = init_parameters();
            parameters.dc_calibrated = true;
            parameters.memory.parameter_002 ^= iteration as u8;
            let (outcome, count) = publish_root(core::hint::black_box(parameters));
            core::hint::black_box(outcome);
            steps += count;
        }
        std::println!(
            "sample={sample} iterations=20000 steps={steps} elapsed_ns={}",
            start.elapsed().as_nanos()
        );
    }
}

#[test]
fn root_preserves_publish_failure_through_cleanup() {
    let mut parameters = init_parameters();
    parameters.dc_calibrated = true;
    let mut root = PhyRxGainInitTransition::new(parameters);
    let PhyRxGainInitAction::Publish(action) = root.action() else {
        panic!("publisher")
    };
    root.advance(PhyRxGainInitCompletion::Publish(complete(action)))
        .unwrap();
    let PhyRxGainInitAction::Publish(PhyRxGainPublishAction::ForcePbus { bank, transaction }) =
        root.action()
    else {
        panic!("PBus transaction")
    };
    root.advance(PhyRxGainInitCompletion::Publish(
        PhyRxGainPublishCompletion::PbusTimedOut { bank, transaction },
    ))
    .unwrap();
    assert_eq!(
        root.action(),
        PhyRxGainInitAction::Publish(PhyRxGainPublishAction::ConfigurePbusWorkMode { bank })
    );
    root.advance(PhyRxGainInitCompletion::Publish(complete(
        PhyRxGainPublishAction::ConfigurePbusWorkMode { bank },
    )))
    .unwrap();
    assert_eq!(
        root.action(),
        PhyRxGainInitAction::Failed(PhyRxGainInitFailure::Publish(
            PhyRxGainPublishFailure::PbusTimedOut { bank, transaction }
        ))
    );
}

#[test]
fn root_dc_rejection_preserves_state_and_accepted_nested_progress() {
    use crate::analog::rfpll::{RfpllFrequencyAction, RfpllFrequencyCompletion};
    let mut root = PhyRxGainInitTransition::new(init_parameters());
    root.advance(PhyRxGainInitCompletion::DcControlRestorePrepared)
        .unwrap();
    let before = root;
    assert_eq!(
        root.advance(PhyRxGainInitCompletion::Dc(
            PhyRxGainDcCompletion::RegistersConfigured { enabled: false }
        )),
        Err(PhyRxGainInitTransitionError::WrongCompletion)
    );
    assert_eq!(root, before);
    root.advance(PhyRxGainInitCompletion::Dc(
        PhyRxGainDcCompletion::RegistersConfigured { enabled: true },
    ))
    .unwrap();
    let PhyRxGainInitAction::Dc(PhyRxGainDcAction::Rfpll(
        RfpllFrequencyAction::StartChannelSwitch {
            frequency_index,
            crystal_selector,
        },
    )) = root.action()
    else {
        panic!("nested PLL start");
    };
    let completion = PhyRxGainInitCompletion::Dc(PhyRxGainDcCompletion::Rfpll(
        RfpllFrequencyCompletion::ChannelSwitchStarted {
            frequency_index,
            crystal_selector,
        },
    ));
    root.advance(completion).unwrap();
    let accepted = root;
    assert_eq!(
        root.advance(completion),
        Err(PhyRxGainInitTransitionError::WrongCompletion)
    );
    assert_eq!(root, accepted);
    assert!(matches!(
        root.action(),
        PhyRxGainInitAction::Dc(PhyRxGainDcAction::Rfpll(RfpllFrequencyAction::DelayMicros(
            1
        )))
    ));
}

#[test]
fn cursor_lowering_keeps_terminal_products_out_of_hardware_admission() {
    let mut root = PhyRxGainInitTransition::new(init_parameters());
    assert_eq!(
        root.prepare_external(),
        PhyRxGainInitExternalBinding::lower(root.action()).map(Some)
    );
    root.advance(PhyRxGainInitCompletion::DcControlRestorePrepared)
        .unwrap();
    assert_eq!(
        root.prepare_external(),
        PhyRxGainInitExternalBinding::lower(root.action()).map(Some)
    );
    root.step = InitStep::Complete;
    let before = root;
    assert_eq!(root.prepare_external(), Ok(None));
    assert_eq!(root, before);
    root.step = InitStep::Failed(PhyRxGainInitFailure::Dc(PhyRxGainDcFailure::Pbus {
        bank: crate::rx::gain_calibration::PhyRxGainDcBank::Wifi,
        transaction: crate::analog::pbus::PhyPbusForceTest::new(2, 1, 0x100),
    }));
    assert_eq!(root.prepare_external(), Ok(None));
}

/// Complete one non-estimator action of a one-step RX-DC child.
fn complete_calibration(
    action: crate::rx::gain_calibration::PhyRxDcCalibrationAction,
) -> crate::rx::gain_calibration::PhyRxDcCalibrationCompletion {
    use crate::rx::gain_calibration::{
        PhyRxDcCalibrationAction as A, PhyRxDcCalibrationCompletion as C,
    };
    match action {
        A::PrepareControlRestore => C::ControlRestorePrepared,
        A::ReadPbus { selector, path } => C::PbusRead {
            selector,
            path,
            value: 0,
        },
        A::ForcePbus(transaction) => C::PbusForceCompleted(transaction),
        A::DelayMicros {
            measurement,
            micros,
        } => C::DelayElapsed {
            measurement,
            micros,
        },
        A::RestoreControl => C::ControlRestored,
        other => panic!("unexpected calibration action {other:?}"),
    }
}

/// Drive the real cold prefix with successful hardware observations, stopping
/// at the first borrowed estimator. No test-only state construction bypasses
/// the parent's preparation or RF/clock sequence.
fn root_at_minimum() -> PhyRxGainInitTransition {
    use crate::analog::{
        i2c,
        rfpll::{RfpllFrequencyAction as R, RfpllFrequencyCompletion as RC},
    };
    use oer_radio_analog::Action as I;
    use oer_radio_analog::Completion as IC;
    let mut root = PhyRxGainInitTransition::new(init_parameters());
    for _ in 0..100 {
        if root.minimum_mut().is_some() {
            return root;
        }
        let before = root;
        assert_eq!(
            root.finish_minimum(),
            Err(PhyRxGainInitTransitionError::WrongCompletion)
        );
        assert_eq!(root, before);
        let completion = match root.action() {
            PhyRxGainInitAction::PrepareDcControlRestore => {
                PhyRxGainInitCompletion::DcControlRestorePrepared
            }
            PhyRxGainInitAction::Dc(action) => PhyRxGainInitCompletion::Dc(match action {
                PhyRxGainDcAction::ConfigureRegisters { enabled } => {
                    PhyRxGainDcCompletion::RegistersConfigured { enabled }
                }
                PhyRxGainDcAction::Rfpll(action) => PhyRxGainDcCompletion::Rfpll(match action {
                    R::StartChannelSwitch {
                        frequency_index,
                        crystal_selector,
                    } => RC::ChannelSwitchStarted {
                        frequency_index,
                        crystal_selector,
                    },
                    R::ClearChannelSwitch => RC::ChannelSwitchCleared,
                    R::ReadChannelReady { .. } => RC::ChannelReadyObserved { ready: true },
                    R::ConfigureNrx { frequency_mhz } => RC::NrxConfigured { frequency_mhz },
                    R::DelayMicros(micros) => RC::DelayElapsed(micros),
                    other => panic!("unexpected channel prefix {other:?}"),
                }),
                PhyRxGainDcAction::ConfigurePbusDebugMode => {
                    PhyRxGainDcCompletion::PbusDebugModeConfigured
                }
                PhyRxGainDcAction::ForcePbus { bank, transaction } => {
                    PhyRxGainDcCompletion::PbusCompleted { bank, transaction }
                }
                PhyRxGainDcAction::ConfigureClock { clock, enabled } => {
                    PhyRxGainDcCompletion::ClockConfigured { clock, enabled }
                }
                PhyRxGainDcAction::ReadPbus { selector, path } => PhyRxGainDcCompletion::PbusRead {
                    selector,
                    path,
                    value: 0,
                },
                PhyRxGainDcAction::I2c(action) => PhyRxGainDcCompletion::I2c(match action {
                    I::Read { address } => IC::Read { address, value: 0 },
                    I::Write { address, .. } => IC::Written { address },
                    other => panic!("unexpected I2C prefix {other:?}"),
                }),
                PhyRxGainDcAction::DelayMicros { phase, micros } => {
                    PhyRxGainDcCompletion::DelayElapsed { phase, micros }
                }
                PhyRxGainDcAction::Calibration(action) => {
                    PhyRxGainDcCompletion::Calibration(complete_calibration(action))
                }
                other => panic!("unexpected DC prefix {other:?}"),
            }),
            other => panic!("unexpected root prefix {other:?}"),
        };
        root.advance(completion).unwrap();
    }
    panic!("prefix did not reach minimum");
}

#[test]
fn root_borrowed_minimum_commits_only_terminal_results_and_preserves_failure_cleanup() {
    use crate::{
        calibration::estimator::{
            PhyDcIqAccumulatorSnapshot, PhyDcIqAction as A, PhyDcIqCompletion as C,
            PhyDcIqReadinessSnapshot,
        },
        rx::dc_offset::{PhyRxDcMinimumAction, PhyRxDcMinimumCompletion},
    };
    for timeout in [false, true] {
        let mut fused = root_at_minimum();
        let mut stepwise = fused;
        for _ in 0..1000 {
            let before = fused;
            assert_eq!(
                fused.finish_minimum(),
                Err(PhyRxGainInitTransitionError::WrongCompletion)
            );
            assert_eq!(fused, before);
            let PhyRxDcMinimumAction::DcIq(action) = fused.minimum_mut().unwrap().action() else {
                panic!("pending estimator expected")
            };
            let completion = PhyRxDcMinimumCompletion::DcIq(match action {
                A::Configure(request) => C::Configured(request),
                A::SetEnable {
                    request,
                    phase,
                    enabled,
                } => C::EnableSet {
                    request,
                    phase,
                    enabled,
                },
                A::DelayMicros {
                    request,
                    phase,
                    micros,
                } => C::DelayElapsed {
                    request,
                    phase,
                    micros,
                },
                A::AwaitReadinessEdge { request, .. } if timeout => C::ReadinessTimedOut(request),
                A::AwaitReadinessEdge { request, .. } => C::ReadinessObserved {
                    request,
                    snapshot: PhyDcIqReadinessSnapshot {
                        ready: true,
                        activity: false,
                    },
                },
                A::ReadAccumulators(request) => C::AccumulatorsRead {
                    request,
                    snapshot: PhyDcIqAccumulatorSnapshot {
                        i: 0,
                        q: 0,
                        power: 0,
                    },
                },
                other => panic!("unexpected estimator action {other:?}"),
            });
            stepwise
                .advance(PhyRxGainInitCompletion::Dc(
                    PhyRxGainDcCompletion::Calibration(
                        crate::rx::gain_calibration::PhyRxDcCalibrationCompletion::Minimum(
                            completion,
                        ),
                    ),
                ))
                .unwrap();
            fused.minimum_mut().unwrap().advance(completion).unwrap();
            if fused.minimum_mut().unwrap().terminal().is_some() {
                fused.finish_minimum().unwrap();
                assert_eq!(fused, stepwise, "root must retain the same results/cleanup");
                let accepted = fused;
                assert_eq!(
                    fused.finish_minimum(),
                    Err(PhyRxGainInitTransitionError::WrongCompletion)
                );
                assert_eq!(fused, accepted);
                assert!(
                    fused.dc_outcome.is_none(),
                    "one minimum is not a complete gain calibration"
                );
                break;
            }
            assert_eq!(fused, stepwise);
        }
        assert!(
            fused.minimum_mut().is_none(),
            "bounded estimator must finish"
        );
        if timeout {
            // An estimator failure must still drain the DC clock/PBus cleanup
            // and outer control restoration before exposing the terminal error.
            for _ in 0..20 {
                let completion = match fused.action() {
                    PhyRxGainInitAction::Dc(PhyRxGainDcAction::ConfigureClock {
                        clock,
                        enabled,
                    }) => PhyRxGainInitCompletion::Dc(PhyRxGainDcCompletion::ClockConfigured {
                        clock,
                        enabled,
                    }),
                    PhyRxGainInitAction::Dc(PhyRxGainDcAction::ConfigurePbusWorkMode) => {
                        PhyRxGainInitCompletion::Dc(PhyRxGainDcCompletion::PbusWorkModeConfigured {
                            settle_required: false,
                        })
                    }
                    PhyRxGainInitAction::Dc(PhyRxGainDcAction::ForcePbus { bank, transaction }) => {
                        PhyRxGainInitCompletion::Dc(PhyRxGainDcCompletion::PbusCompleted {
                            bank,
                            transaction,
                        })
                    }
                    PhyRxGainInitAction::Dc(PhyRxGainDcAction::ConfigureRegisters { enabled }) => {
                        PhyRxGainInitCompletion::Dc(PhyRxGainDcCompletion::RegistersConfigured {
                            enabled,
                        })
                    }
                    PhyRxGainInitAction::RestoreDcControl => {
                        PhyRxGainInitCompletion::DcControlRestored
                    }
                    PhyRxGainInitAction::Dc(PhyRxGainDcAction::Calibration(action)) => {
                        PhyRxGainInitCompletion::Dc(PhyRxGainDcCompletion::Calibration(
                            complete_calibration(action),
                        ))
                    }
                    PhyRxGainInitAction::Failed(PhyRxGainInitFailure::Dc(_)) => break,
                    other => panic!("unexpected failure cleanup {other:?}"),
                };
                fused.advance(completion).unwrap();
                stepwise.advance(completion).unwrap();
                assert_eq!(fused, stepwise);
            }
            assert!(matches!(
                fused.action(),
                PhyRxGainInitAction::Failed(PhyRxGainInitFailure::Dc(_))
            ));
        }
    }
}

#[test]
fn coarse_regions_follow_owned_children_without_advancing_them() {
    let mut root = PhyRxGainInitTransition::new(init_parameters());
    assert_eq!(root.phase(), Some(Phase::Control));
    let before = root;
    assert!(
        root.advance(PhyRxGainInitCompletion::IqCorrectionEnabled)
            .is_err()
    );
    assert_eq!(root, before);
    root.advance(PhyRxGainInitCompletion::DcControlRestorePrepared)
        .unwrap();
    assert_eq!(root.phase(), Some(Phase::Dc));
    let minimum = root_at_minimum();
    assert_eq!(minimum.phase(), Some(Phase::Dc));
    let mut parameters = init_parameters();
    parameters.dc_calibrated = true;
    let root = PhyRxGainInitTransition::new(parameters);
    assert_eq!(root.phase(), Some(Phase::Publish));
}

#[test]
fn initialized_tables_enter_only_the_compact_hardware_tail() {
    let mut root = PhyRxGainInitTransition::with_initialized_tables();
    assert_eq!(root.phase(), Some(Phase::Control));
    assert_eq!(
        root.action(),
        PhyRxGainInitAction::ConfigureLimits {
            wifi_last_index: generated_table(PhyRxGainBank::Wifi).last_index,
        }
    );

    root.advance(PhyRxGainInitCompletion::LimitsConfigured {
        wifi_last_index: generated_table(PhyRxGainBank::Wifi).last_index,
    })
    .unwrap();
    assert_eq!(root.action(), PhyRxGainInitAction::EnableIqCorrection);
    root.advance(PhyRxGainInitCompletion::IqCorrectionEnabled)
        .unwrap();

    assert_eq!(
        root.terminal(),
        Some(Ok(PhyRxGainInitOutcome {
            dc: None,
            generated_tables: false,
            wifi_last_index: generated_table(PhyRxGainBank::Wifi).last_index,
            shared_last_index: generated_table(PhyRxGainBank::Shared).last_index,
        }))
    );
}
