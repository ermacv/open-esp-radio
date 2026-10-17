use super::*;

fn tone_completion(action: PhyToneSarAction, sample_value: u16) -> PhyToneSarCompletion {
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
            value: sample_value,
        },
        terminal => panic!("unexpected tone terminal: {terminal:?}"),
    }
}

fn linear_completion(
    action: PhyTxIqLinearPowerAction,
    sample: u16,
) -> PhyTxIqLinearPowerCompletion {
    match action {
        PhyTxIqLinearPowerAction::ToneSar(action) => {
            PhyTxIqLinearPowerCompletion::ToneSar(tone_completion(action, sample))
        }
        terminal => panic!("unexpected linear terminal: {terminal:?}"),
    }
}

fn mis_completion(action: PhyTxIqMisPowerAction, sample: u16) -> PhyTxIqMisPowerCompletion {
    match action {
        PhyTxIqMisPowerAction::Configure {
            identity,
            first,
            polarity,
            attenuation,
            selector,
        } => PhyTxIqMisPowerCompletion::Configured {
            identity,
            first,
            polarity,
            attenuation,
            selector,
        },
        PhyTxIqMisPowerAction::DelayMicros {
            identity,
            phase,
            micros,
        } => PhyTxIqMisPowerCompletion::DelayElapsed {
            identity,
            phase,
            micros,
        },
        PhyTxIqMisPowerAction::LinearPower(action) => {
            PhyTxIqMisPowerCompletion::LinearPower(linear_completion(action, sample))
        }
        terminal => panic!("unexpected mis-power terminal: {terminal:?}"),
    }
}

fn cover_completion(action: PhyTxIqCoverAction, sample: u16) -> PhyTxIqCoverCompletion {
    match action {
        PhyTxIqCoverAction::ConfigureCoefficient {
            identity,
            iteration,
            kind,
            value,
        } => PhyTxIqCoverCompletion::CoefficientConfigured {
            identity,
            iteration,
            kind,
            value,
        },
        PhyTxIqCoverAction::MisPower(action) => {
            PhyTxIqCoverCompletion::MisPower(mis_completion(action, sample))
        }
        terminal => panic!("unexpected cover terminal: {terminal:?}"),
    }
}

fn environment_completion(
    action: PhyTxCalibrationEnvironmentAction,
) -> PhyTxCalibrationEnvironmentCompletion {
    match action {
        PhyTxCalibrationEnvironmentAction::ConfigurePbusDebugMode => {
            PhyTxCalibrationEnvironmentCompletion::PbusDebugModeConfigured
        }
        PhyTxCalibrationEnvironmentAction::ForcePbus(transaction) => {
            PhyTxCalibrationEnvironmentCompletion::PbusCompleted(transaction)
        }
        PhyTxCalibrationEnvironmentAction::ConfigureTxClock { enabled } => {
            PhyTxCalibrationEnvironmentCompletion::TxClockConfigured { enabled }
        }
        PhyTxCalibrationEnvironmentAction::ConfigurePowerDetector => {
            PhyTxCalibrationEnvironmentCompletion::PowerDetectorConfigured
        }
        PhyTxCalibrationEnvironmentAction::ConfigureCalibrationMode => {
            PhyTxCalibrationEnvironmentCompletion::CalibrationModeConfigured
        }
        PhyTxCalibrationEnvironmentAction::StopTone => {
            PhyTxCalibrationEnvironmentCompletion::ToneStopped
        }
        PhyTxCalibrationEnvironmentAction::ConfigurePbusWorkMode => {
            PhyTxCalibrationEnvironmentCompletion::PbusWorkModeConfigured {
                settle_required: false,
            }
        }
        PhyTxCalibrationEnvironmentAction::DelayMicros { phase, micros } => {
            PhyTxCalibrationEnvironmentCompletion::DelayElapsed { phase, micros }
        }
        PhyTxCalibrationEnvironmentAction::ConfigurePbusWorkModePulse => {
            PhyTxCalibrationEnvironmentCompletion::PbusWorkModePulseConfigured
        }
        PhyTxCalibrationEnvironmentAction::ClearPbusWorkModePulse => {
            PhyTxCalibrationEnvironmentCompletion::PbusWorkModePulseCleared
        }
        terminal => panic!("unexpected environment terminal: {terminal:?}"),
    }
}

fn loopback_completion(action: PhyTxIqLoopbackAction) -> PhyTxIqLoopbackCompletion {
    match action {
        PhyTxIqLoopbackAction::I2c(oer_radio_analog::Action::Read { address }) => {
            PhyTxIqLoopbackCompletion::I2c(oer_radio_analog::Completion::Read { address, value: 0 })
        }
        PhyTxIqLoopbackAction::I2c(oer_radio_analog::Action::Write { address, .. }) => {
            PhyTxIqLoopbackCompletion::I2c(oer_radio_analog::Completion::Written { address })
        }
        PhyTxIqLoopbackAction::ConfigureTxClock { enabled } => {
            PhyTxIqLoopbackCompletion::TxClockConfigured { enabled }
        }
        PhyTxIqLoopbackAction::ConfigureRxClock { enabled } => {
            PhyTxIqLoopbackCompletion::RxClockConfigured { enabled }
        }
        terminal => panic!("unexpected loopback terminal: {terminal:?}"),
    }
}

fn power_attenuation_completion(
    action: PhyPowerAttenuationAction,
    sample: u16,
) -> PhyPowerAttenuationCompletion {
    match action {
        PhyPowerAttenuationAction::ConfigureTone {
            iteration,
            selector,
            attenuation,
        } => PhyPowerAttenuationCompletion::ToneConfigured {
            iteration,
            selector,
            attenuation,
        },
        PhyPowerAttenuationAction::ToneSar(action) => {
            PhyPowerAttenuationCompletion::ToneSar(tone_completion(action, sample))
        }
        terminal => panic!("unexpected attenuation terminal: {terminal:?}"),
    }
}

fn calibration_completion(
    action: PhyTxIqCalibrationAction,
    sample: u16,
) -> PhyTxIqCalibrationCompletion {
    match action {
        PhyTxIqCalibrationAction::ConfigureCorrection { begin } => {
            PhyTxIqCalibrationCompletion::CorrectionConfigured { begin }
        }
        PhyTxIqCalibrationAction::ConfigurePbusDebugMode => {
            PhyTxIqCalibrationCompletion::PbusDebugModeConfigured
        }
        PhyTxIqCalibrationAction::Loopback(action) => {
            PhyTxIqCalibrationCompletion::Loopback(loopback_completion(action))
        }
        PhyTxIqCalibrationAction::ForcePbus(transaction) => {
            PhyTxIqCalibrationCompletion::PbusCompleted(transaction)
        }
        PhyTxIqCalibrationAction::Environment(action) => {
            PhyTxIqCalibrationCompletion::Environment(environment_completion(action))
        }
        PhyTxIqCalibrationAction::PrepareToneControlRestore => {
            PhyTxIqCalibrationCompletion::ToneControlRestorePrepared
        }
        PhyTxIqCalibrationAction::PowerAttenuation(action) => {
            PhyTxIqCalibrationCompletion::PowerAttenuation(power_attenuation_completion(
                action, sample,
            ))
        }
        PhyTxIqCalibrationAction::Cover(action) => {
            PhyTxIqCalibrationCompletion::Cover(cover_completion(action, sample))
        }
        PhyTxIqCalibrationAction::RestoreToneControl => {
            PhyTxIqCalibrationCompletion::ToneControlRestored
        }
        terminal => panic!("unexpected calibration terminal: {terminal:?}"),
    }
}

#[test]
fn linear_power_has_exact_four_sar_samples_and_wrapping_sum() {
    let request = PhyTxIqLinearPowerRequest {
        identity: 2,
        reference_codes: [80, 120],
        clear_tone_after_ready: false,
    };
    let mut transition = PhyTxIqLinearPowerTransition::new(request);
    let mut reads = 0;
    loop {
        let action = transition.action();
        if let PhyTxIqLinearPowerAction::Complete(outcome) = action {
            assert_eq!(outcome.identity, 2);
            assert!(outcome.power > 0);
            break;
        }
        if matches!(
            action,
            PhyTxIqLinearPowerAction::ToneSar(PhyToneSarAction::ReadSar { .. })
        ) {
            reads += 1;
        }
        transition.advance(linear_completion(action, 100)).unwrap();
    }
    assert_eq!(reads, 4);
}

#[test]
fn cover_has_seven_iterations_and_exact_112_sar_samples() {
    let request = PhyTxIqCoverRequest {
        identity: 3,
        attenuation: 80,
        selector: 0x80,
        reference_codes: [80, 120],
        clear_tone_after_ready: false,
    };
    let mut transition = PhyTxIqCoverTransition::new(request);
    let mut reads = 0;
    loop {
        let action = transition.action();
        let completion = match action {
            PhyTxIqCoverAction::ConfigureCoefficient {
                identity,
                iteration,
                kind,
                value,
            } => PhyTxIqCoverCompletion::CoefficientConfigured {
                identity,
                iteration,
                kind,
                value,
            },
            PhyTxIqCoverAction::MisPower(action) => {
                if matches!(
                    action,
                    PhyTxIqMisPowerAction::LinearPower(PhyTxIqLinearPowerAction::ToneSar(
                        PhyToneSarAction::ReadSar { .. }
                    ))
                ) {
                    reads += 1;
                }
                PhyTxIqCoverCompletion::MisPower(mis_completion(action, 100))
            }
            PhyTxIqCoverAction::Complete(outcome) => {
                assert_eq!(outcome.iterations, 7);
                break;
            }
            PhyTxIqCoverAction::Failed(failure) => {
                panic!("unexpected failure: {failure:?}")
            }
        };
        transition.advance(completion).unwrap();
    }
    assert_eq!(reads, 112);
}

#[test]
fn both_rfcal_variants_traverse_cleanup_and_finish_with_bounded_coefficients() {
    for variant in [
        PhyTxIqCalibrationVariant::Initial,
        PhyTxIqCalibrationVariant::Loopback,
    ] {
        let mut transition = PhyTxIqCalibrationTransition::new(PhyTxIqCalibrationRequest {
            identity: variant as u8,
            variant,
            environment: PhyTxCalibrationParameters {
                pbus_tx_path_value: 0x2f,
                pbus_rx_path_value: 0xbf,
                dco: [0x100; 4],
            },
            attenuation: 80,
            selector: 0x80,
            power_offset: 0,
            reference_codes: [80, 120],
            clear_tone_after_ready: false,
        });
        let mut edges = 0_u16;
        let mut prepare_edge = None;
        let mut restore_edge = None;
        let mut loopback_teardown_edge = None;
        let mut correction_off_edge = None;
        loop {
            let action = transition.action();
            match action {
                PhyTxIqCalibrationAction::PrepareToneControlRestore => {
                    assert!(prepare_edge.replace(edges).is_none());
                }
                PhyTxIqCalibrationAction::RestoreToneControl => {
                    assert!(restore_edge.replace(edges).is_none());
                }
                PhyTxIqCalibrationAction::Loopback(_)
                    if restore_edge.is_some() && loopback_teardown_edge.is_none() =>
                {
                    loopback_teardown_edge = Some(edges);
                }
                PhyTxIqCalibrationAction::ConfigureCorrection { begin: false } => {
                    assert!(correction_off_edge.replace(edges).is_none());
                }
                _ => {}
            }
            match action {
                PhyTxIqCalibrationAction::Complete(outcome) => {
                    assert!((-31..=31).contains(&outcome.gain));
                    assert!((-63..=63).contains(&outcome.phase));
                    assert!(edges < 2_000);
                    let prepare_edge = prepare_edge.expect("TX-IQ must prepare one restore");
                    let restore_edge = restore_edge.expect("TX-IQ must restore exactly once");
                    let correction_off_edge =
                        correction_off_edge.expect("TX-IQ must leave correction mode");
                    assert!(prepare_edge < restore_edge);
                    assert!(restore_edge < correction_off_edge);
                    match variant {
                        PhyTxIqCalibrationVariant::Initial => {
                            assert_eq!(loopback_teardown_edge, None);
                        }
                        PhyTxIqCalibrationVariant::Loopback => {
                            let teardown_edge = loopback_teardown_edge
                                .expect("loopback teardown must follow restore");
                            assert!(restore_edge < teardown_edge);
                            assert!(teardown_edge < correction_off_edge);
                        }
                    }
                    break;
                }
                PhyTxIqCalibrationAction::Failed(failure) => {
                    panic!("unexpected calibration failure: {failure:?}")
                }
                _ => {
                    transition
                        .advance(calibration_completion(action, 100))
                        .unwrap();
                    edges += 1;
                }
            }
        }
    }
}

#[test]
fn tone_sar_failure_after_prepare_restores_before_terminal_failure() {
    for variant in [
        PhyTxIqCalibrationVariant::Initial,
        PhyTxIqCalibrationVariant::Loopback,
    ] {
        let mut transition = PhyTxIqCalibrationTransition::new(PhyTxIqCalibrationRequest {
            identity: variant as u8,
            variant,
            environment: PhyTxCalibrationParameters {
                pbus_tx_path_value: 0x2f,
                pbus_rx_path_value: 0xbf,
                dco: [0x100; 4],
            },
            attenuation: 80,
            selector: 0x80,
            power_offset: 0,
            reference_codes: [80, 120],
            clear_tone_after_ready: false,
        });
        let mut edges = 0_u16;
        let mut injected_failure = None;
        let mut prepare_edge = None;
        let mut restore_edge = None;
        let mut loopback_teardown_edge = None;
        let mut correction_off_edge = None;
        loop {
            let action = transition.action();
            match action {
                PhyTxIqCalibrationAction::PrepareToneControlRestore => {
                    assert!(prepare_edge.replace(edges).is_none());
                }
                PhyTxIqCalibrationAction::RestoreToneControl => {
                    assert!(restore_edge.replace(edges).is_none());
                }
                PhyTxIqCalibrationAction::Loopback(_)
                    if restore_edge.is_some() && loopback_teardown_edge.is_none() =>
                {
                    loopback_teardown_edge = Some(edges);
                }
                PhyTxIqCalibrationAction::ConfigureCorrection { begin: false } => {
                    assert!(correction_off_edge.replace(edges).is_none());
                }
                _ => {}
            }
            match action {
                PhyTxIqCalibrationAction::Failed(failure) => {
                    assert_eq!(Some(failure), injected_failure);
                    assert!(edges < 500);
                    let prepare_edge = prepare_edge.expect("TX-IQ must prepare one restore");
                    let restore_edge = restore_edge.expect("TX-IQ must restore exactly once");
                    let correction_off_edge =
                        correction_off_edge.expect("TX-IQ must leave correction mode");
                    assert!(prepare_edge < restore_edge);
                    assert!(restore_edge < correction_off_edge);
                    match variant {
                        PhyTxIqCalibrationVariant::Initial => {
                            assert_eq!(loopback_teardown_edge, None);
                        }
                        PhyTxIqCalibrationVariant::Loopback => {
                            let teardown_edge = loopback_teardown_edge
                                .expect("loopback teardown must follow restore");
                            assert!(restore_edge < teardown_edge);
                            assert!(teardown_edge < correction_off_edge);
                        }
                    }
                    break;
                }
                PhyTxIqCalibrationAction::PowerAttenuation(PhyPowerAttenuationAction::ToneSar(
                    PhyToneSarAction::PollReady {
                        measurement,
                        sample,
                    },
                )) if injected_failure.is_none() => {
                    let failure = PhyToneSarFailure::ReadyDeadlineElapsed {
                        measurement,
                        sample,
                    };
                    injected_failure = Some(PhyTxIqCalibrationFailure::ToneSar(failure));
                    transition
                        .advance(PhyTxIqCalibrationCompletion::PowerAttenuation(
                            PhyPowerAttenuationCompletion::ToneSar(
                                PhyToneSarCompletion::ReadyDeadlineElapsed {
                                    measurement,
                                    sample,
                                },
                            ),
                        ))
                        .unwrap();
                    edges += 1;
                }
                PhyTxIqCalibrationAction::Complete(outcome) => {
                    panic!("failure injection unexpectedly completed TX-IQ: {outcome:?}")
                }
                _ => {
                    transition
                        .advance(calibration_completion(action, 100))
                        .unwrap();
                    edges += 1;
                }
            }
        }
    }
}

#[test]
fn init_skip_does_not_emit_any_hardware_action() {
    let transition = PhyTxIqInitTransition::new(PhyTxIqInitParameters {
        already_calibrated: true,
        crystal_selector: 0,
        environment: PhyTxCalibrationParameters {
            pbus_tx_path_value: 0,
            pbus_rx_path_value: 0,
            dco: [0; 4],
        },
        capacitance: [0; 6],
        channel_6_dcode: [0; 2],
        initial_attenuation: 0,
        power_offset: 0,
        reference_codes: [0; 2],
        clear_tone_after_ready: false,
    });
    assert_eq!(
        transition.action(),
        PhyTxIqInitAction::Complete(PhyTxIqInitOutcome {
            coefficient: [0; 2],
            external_dcode: [0; 2],
            temperature: None,
            calibration_performed: false,
        })
    );
}

#[test]
fn external_lowering_covers_every_txiq_operation_layer() {
    let i2c = analog_registers::RFPLL_CAPACITOR_LOW;
    assert!(matches!(
        PhyTxIqInitExternalBinding::lower(PhyTxIqInitAction::Rfpll(
            RfpllFrequencyAction::DelayMicros(5)
        )),
        Ok(PhyTxIqInitExternalBinding::Rfpll(_))
    ));
    assert!(matches!(
        PhyTxIqInitExternalBinding::lower(PhyTxIqInitAction::WriteI2c {
            address: i2c,
            value: 1,
        }),
        Ok(PhyTxIqInitExternalBinding::I2c(_))
    ));
    assert!(matches!(
        PhyTxIqInitExternalBinding::lower(PhyTxIqInitAction::Temperature(
            PhyTemperatureTransition::new().action()
        )),
        Ok(PhyTxIqInitExternalBinding::Temperature(_))
    ));
    assert!(matches!(
        PhyTxIqCalibrationExternalBinding::lower(PhyTxIqCalibrationAction::ConfigureCorrection {
            begin: true
        }),
        Ok(PhyTxIqCalibrationExternalBinding::Mmio(_))
    ));
    assert!(matches!(
        PhyTxIqCalibrationExternalBinding::lower(
            PhyTxIqCalibrationAction::PrepareToneControlRestore
        ),
        Ok(PhyTxIqCalibrationExternalBinding::Mmio(_))
    ));
    assert!(matches!(
        PhyTxIqCalibrationExternalBinding::lower(PhyTxIqCalibrationAction::RestoreToneControl),
        Ok(PhyTxIqCalibrationExternalBinding::Mmio(_))
    ));
    assert!(matches!(
        PhyTxIqCalibrationExternalBinding::lower(PhyTxIqCalibrationAction::ForcePbus(
            PhyPbusForceTest::new(1, 2, 0)
        )),
        Ok(PhyTxIqCalibrationExternalBinding::Pbus(_))
    ));
    assert!(matches!(
        PhyTxIqCalibrationExternalBinding::lower(PhyTxIqCalibrationAction::Loopback(
            PhyTxIqLoopbackAction::I2c(oer_radio_analog::Action::Read {
                address: analog_registers::TX_IQ_LOOPBACK_ENABLE.address(),
            })
        )),
        Ok(PhyTxIqCalibrationExternalBinding::Loopback(
            PhyTxIqLoopbackExternalBinding::I2c(_)
        ))
    ));
    assert!(matches!(
        PhyTxIqMisPowerExternalBinding::lower(PhyTxIqMisPowerAction::DelayMicros {
            identity: 0,
            phase: PhyTxIqMisPowerDelayPhase::FirstPolarity,
            micros: 2,
        }),
        Ok(PhyTxIqMisPowerExternalBinding::Timer(_))
    ));
    assert!(matches!(
        PhyTxIqCoverExternalBinding::lower(PhyTxIqCoverAction::ConfigureCoefficient {
            identity: 0,
            iteration: 0,
            kind: PhyTxIqCoefficientKind::Gain,
            value: 0,
        }),
        Ok(PhyTxIqCoverExternalBinding::Mmio(_))
    ));
    assert!(matches!(
        PhyTxIqInitExternalBinding::lower(PhyTxIqInitAction::Complete(PhyTxIqInitOutcome {
            coefficient: [0; 2],
            external_dcode: [0; 2],
            temperature: None,
            calibration_performed: false,
        })),
        Err(PhyTxIqExternalBindingError::UnsupportedAction)
    ));
}
