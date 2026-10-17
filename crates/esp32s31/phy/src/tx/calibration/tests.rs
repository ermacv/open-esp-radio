use super::*;

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
        action => panic!("unexpected terminal tone/SAR action: {action:?}"),
    }
}

fn complete_tone_sar(transition: &mut PhyPowerAttenuationTransition, value: u16) {
    while let PhyPowerAttenuationAction::ToneSar(action) = transition.action() {
        transition
            .advance(PhyPowerAttenuationCompletion::ToneSar(tone_sar_completion(
                action, value,
            )))
            .unwrap();
    }
}

#[test]
fn power_db_uses_explicit_reference_codes() {
    assert_eq!(phy_tx_power_db(100, [90, 110], 4), 40);
}

#[test]
fn tx_calibration_exit_uses_the_complete_rom_work_mode_pulse() {
    let parameters = PhyTxCalibrationParameters {
        pbus_tx_path_value: 0,
        pbus_rx_path_value: 0,
        dco: [0; 4],
    };
    let mut transition = PhyTxCalibrationEnvironmentTransition::exit(parameters);
    transition
        .advance(PhyTxCalibrationEnvironmentCompletion::ToneStopped)
        .unwrap();
    transition
        .advance(PhyTxCalibrationEnvironmentCompletion::TxClockConfigured { enabled: false })
        .unwrap();
    for index in 0..=6 {
        let transaction = tx_work_pbus(index, parameters);
        assert_eq!(
            transition.action(),
            PhyTxCalibrationEnvironmentAction::ForcePbus(transaction)
        );
        transition
            .advance(PhyTxCalibrationEnvironmentCompletion::PbusCompleted(
                transaction,
            ))
            .unwrap();
    }
    transition
        .advance(
            PhyTxCalibrationEnvironmentCompletion::PbusWorkModeConfigured {
                settle_required: true,
            },
        )
        .unwrap();
    transition
        .advance(PhyTxCalibrationEnvironmentCompletion::DelayElapsed {
            phase: PhyTxCalibrationEnvironmentDelayPhase::PbusWorkMode,
            micros: 1,
        })
        .unwrap();
    transition
        .advance(PhyTxCalibrationEnvironmentCompletion::PbusWorkModePulseConfigured)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyTxCalibrationEnvironmentAction::DelayMicros {
            phase: PhyTxCalibrationEnvironmentDelayPhase::PbusWorkModePulse,
            micros: 2,
        }
    );
}

#[test]
fn search_completes_without_polling_when_error_is_in_window() {
    let request = PhyPowerAttenuationRequest {
        tone_selector: 0x80,
        initial_attenuation: 80,
        target_power: 13,
        power_offset: 4,
        reference_codes: [90, 110],
    };
    let mut transition = PhyPowerAttenuationTransition::new(request);
    transition
        .advance(PhyPowerAttenuationCompletion::ToneConfigured {
            iteration: 0,
            selector: 0x80,
            attenuation: 80,
        })
        .unwrap();
    complete_tone_sar(&mut transition, 100);
    assert_eq!(
        transition.action(),
        PhyPowerAttenuationAction::Complete(PhyPowerAttenuationOutcome {
            attenuation: 80,
            iterations: 1,
        })
    );
}

#[test]
fn search_has_exact_six_sample_bound() {
    let request = PhyPowerAttenuationRequest {
        tone_selector: 0x80,
        initial_attenuation: 80,
        target_power: -100,
        power_offset: 4,
        reference_codes: [90, 110],
    };
    let mut transition = PhyPowerAttenuationTransition::new(request);
    let mut samples = 0;
    loop {
        let completion = match transition.action() {
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
                if matches!(action, PhyToneSarAction::ReadSar { .. }) {
                    samples += 1;
                }
                PhyPowerAttenuationCompletion::ToneSar(tone_sar_completion(action, 100))
            }
            PhyPowerAttenuationAction::Complete(outcome) => {
                assert_eq!(outcome.iterations, 6);
                break;
            }
            PhyPowerAttenuationAction::Failed(failure) => {
                panic!("unexpected tone/SAR failure: {failure:?}")
            }
        };
        transition.advance(completion).unwrap();
    }
    assert_eq!(samples, 12);
}

#[test]
fn tone_sar_uses_two_samples_and_one_poll_per_external_edge() {
    let request = PhyToneSarRequest {
        measurement: 7,
        samples: 2,
        clear_tone_after_ready: true,
    };
    let mut transition = PhyToneSarTransition::new(request).unwrap();
    for value in [100, 104] {
        loop {
            let action = transition.action();
            if matches!(
                action,
                PhyToneSarAction::Complete(_) | PhyToneSarAction::Failed(_)
            ) {
                break;
            }
            transition
                .advance(tone_sar_completion(action, value))
                .unwrap();
            if matches!(action, PhyToneSarAction::ReadSar { .. }) {
                break;
            }
        }
    }
    assert_eq!(
        transition.action(),
        PhyToneSarAction::Complete(PhyToneSarOutcome {
            request,
            sample: 102,
        })
    );
}

#[test]
fn tone_sar_bounds_not_ready_observations_without_consuming_a_sample() {
    let mut transition = PhyToneSarTransition::new(PhyToneSarRequest {
        measurement: 7,
        samples: 2,
        clear_tone_after_ready: false,
    })
    .unwrap();
    while !matches!(transition.action(), PhyToneSarAction::PollReady { .. }) {
        transition
            .advance(tone_sar_completion(transition.action(), 0))
            .unwrap();
    }
    for observation in 1..=crate::HARDWARE_EDGE_LIMIT {
        transition
            .advance(PhyToneSarCompletion::ReadySampled {
                measurement: 7,
                sample: 0,
                ready: false,
            })
            .unwrap();
        if observation < crate::HARDWARE_EDGE_LIMIT {
            assert!(matches!(
                transition.action(),
                PhyToneSarAction::PollReady { .. }
            ));
        }
    }
    assert_eq!(
        transition.action(),
        PhyToneSarAction::Failed(PhyToneSarFailure::ReadyObservationLimit {
            measurement: 7,
            sample: 0,
            observations: crate::HARDWARE_EDGE_LIMIT,
        })
    );
    assert_eq!(
        transition.advance(PhyToneSarCompletion::ReadySampled {
            measurement: 7,
            sample: 0,
            ready: true,
        }),
        Err(PhyToneSarTransitionError::AlreadyComplete)
    );
}

#[test]
fn tone_sar_observation_budget_restarts_for_each_conversion() {
    let mut transition = PhyToneSarTransition::new(PhyToneSarRequest {
        measurement: 9,
        samples: 2,
        clear_tone_after_ready: false,
    })
    .unwrap();
    for sample in 0..2 {
        while !matches!(transition.action(), PhyToneSarAction::PollReady { .. }) {
            transition
                .advance(tone_sar_completion(transition.action(), 123))
                .unwrap();
        }
        for _ in 1..crate::HARDWARE_EDGE_LIMIT {
            transition
                .advance(PhyToneSarCompletion::ReadySampled {
                    measurement: 9,
                    sample,
                    ready: false,
                })
                .unwrap();
        }
        transition
            .advance(PhyToneSarCompletion::ReadySampled {
                measurement: 9,
                sample,
                ready: true,
            })
            .unwrap();
        assert!(matches!(
            transition.action(),
            PhyToneSarAction::ReadSar { .. }
        ));
        transition
            .advance(tone_sar_completion(transition.action(), 123))
            .unwrap();
    }
    assert!(matches!(
        transition.action(),
        PhyToneSarAction::Complete(PhyToneSarOutcome { sample: 123, .. })
    ));
}

#[test]
fn tx_cap_lowering_covers_every_nested_operation_class() {
    let i2c = analog_registers::RFPLL_CAPACITOR_LOW;
    assert!(matches!(
        PhyTxCapExternalBinding::lower(PhyTxCapAction::Environment(
            PhyTxCalibrationEnvironmentAction::ConfigurePbusDebugMode
        )),
        Ok(PhyTxCapExternalBinding::Environment(
            PhyTxCalibrationEnvironmentExternalBinding::Mmio(_)
        ))
    ));
    assert!(matches!(
        PhyTxCalibrationEnvironmentExternalBinding::lower(
            PhyTxCalibrationEnvironmentAction::ForcePbus(PhyPbusForceTest::new(1, 1, 0))
        ),
        Ok(PhyTxCalibrationEnvironmentExternalBinding::Pbus(_))
    ));
    assert!(matches!(
        PhyTxCalibrationEnvironmentExternalBinding::lower(
            PhyTxCalibrationEnvironmentAction::DelayMicros {
                phase: PhyTxCalibrationEnvironmentDelayPhase::PbusWorkMode,
                micros: 1,
            }
        ),
        Ok(PhyTxCalibrationEnvironmentExternalBinding::Timer(_))
    ));
    assert!(matches!(
        PhyTxCapExternalBinding::lower(PhyTxCapAction::Rfpll(RfpllFrequencyAction::DelayMicros(
            20
        ))),
        Ok(PhyTxCapExternalBinding::Rfpll(
            crate::analog::rfpll::RfpllFrequencyExternalBinding::Timer(_)
        ))
    ));
    assert!(matches!(
        PhyTxCapExternalBinding::lower(PhyTxCapAction::I2c(oer_radio_analog::Action::Read {
            address: i2c
        })),
        Ok(PhyTxCapExternalBinding::I2c(_))
    ));
    assert!(matches!(
        PhyTxCapExternalBinding::lower(PhyTxCapAction::Attenuation(
            PhyPowerAttenuationAction::ConfigureTone {
                iteration: 0,
                selector: 0x80,
                attenuation: 1,
            }
        )),
        Ok(PhyTxCapExternalBinding::Attenuation(
            PhyPowerAttenuationExternalBinding::Mmio(_)
        ))
    ));
    assert!(matches!(
        PhyTxCapExternalBinding::lower(PhyTxCapAction::Search(PhyTxCapSearchAction::I2c(
            oer_radio_analog::Action::Write {
                address: i2c,
                value: 3,
            }
        ))),
        Ok(PhyTxCapExternalBinding::Search(
            PhyTxCapSearchExternalBinding::I2c(_)
        ))
    ));
    assert!(matches!(
        PhyToneSarExternalBinding::lower(PhyToneSarAction::PollReady {
            measurement: 0,
            sample: 0,
        }),
        Ok(PhyToneSarExternalBinding::Mmio(_))
    ));
    assert!(matches!(
        PhyToneSarExternalBinding::lower(PhyToneSarAction::DelayMicros {
            measurement: 0,
            sample: 0,
            phase: PhyToneSarDelayPhase::ToneArmed,
            micros: 2,
        }),
        Ok(PhyToneSarExternalBinding::Timer(_))
    ));
    assert!(matches!(
        PhyTxCapExternalBinding::lower(PhyTxCapAction::Complete(PhyTxCapOutcome {
            capacitance: [0; 6],
            attenuation: 0,
        })),
        Err(PhyTxCapExternalBindingError::UnsupportedAction)
    ));
}
