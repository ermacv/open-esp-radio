use super::{
    PHY_REGISTER_I2C_RESET_SAMPLE_LIMIT, PhyCalibrationIdentity, PhyCalibrationPath,
    PhyRegisterAction, PhyRegisterBindingError, PhyRegisterCompletion, PhyRegisterDelayPhase,
    PhyRegisterFailure, PhyRegisterFinalI2cBinding, PhyRegisterLocalStep, PhyRegisterMmioAction,
    PhyRegisterMmioCompletion, PhyRegisterOutcome, PhyRegisterResetSampleBinding,
    PhyRegisterTimerBinding, PhyRegisterTransition,
};

fn complete_mmio(transition: &mut PhyRegisterTransition, action: PhyRegisterMmioAction) {
    transition
        .advance_external(PhyRegisterCompletion::Mmio(PhyRegisterMmioCompletion {
            action,
        }))
        .unwrap();
}

fn complete_delay(
    transition: &mut PhyRegisterTransition,
    phase: PhyRegisterDelayPhase,
    micros: u32,
) {
    transition
        .advance_external(PhyRegisterCompletion::DelayElapsed { phase, micros })
        .unwrap();
}

fn complete_failure_cleanup(transition: &mut PhyRegisterTransition) {
    assert!(!transition.failure_cleanup_completed());
    complete_mmio(
        transition,
        PhyRegisterMmioAction::SetCalibrationClock { enabled: false },
    );
    complete_mmio(
        transition,
        PhyRegisterMmioAction::SetBbpllCalibration { enabled: false },
    );
    complete_mmio(
        transition,
        PhyRegisterMmioAction::SetHardwareFrequencyControl { enabled: true },
    );
    complete_mmio(
        transition,
        PhyRegisterMmioAction::ConfigureForceTxRx {
            enabled: false,
            phase: 0,
        },
    );
    complete_delay(
        transition,
        PhyRegisterDelayPhase::ForceTxRx {
            enabled: false,
            completed_phase: 0,
        },
        1,
    );
    assert!(!transition.failure_cleanup_completed());
    complete_mmio(
        transition,
        PhyRegisterMmioAction::ConfigureForceTxRx {
            enabled: false,
            phase: 1,
        },
    );
    assert!(!transition.failure_cleanup_completed());
    complete_delay(
        transition,
        PhyRegisterDelayPhase::ForceTxRx {
            enabled: false,
            completed_phase: 1,
        },
        1,
    );
}

#[test]
fn production_config_contains_only_the_qualified_tx_power_policy() {
    let state = crate::state::PhyState::new(crate::state::PhyConfig::production());
    let profile = state.tx_target_power_profile();
    assert_eq!(
        profile,
        crate::tx::power::PhyTxTargetPowerProfile::new(
            0x54,
            [
                0x50, 0x50, 0x50, 0x50, 0x4c, 0x48, 0x50, 0x50, 0x4c, 0x48, 0x40, 0x3c, 0x3c, 0x3c,
                0x4c, 0x4c, 0x48, 0x44,
            ],
            false,
        )
    );
}

#[test]
fn state_owner_cannot_escape_before_a_terminal_parent_outcome() {
    let transition = PhyRegisterTransition::with_production_config();
    assert!(!transition.failure_cleanup_completed());
    let transition = match transition.into_model_parts() {
        Ok(_) => panic!("an active cold initializer must retain its unique state owner"),
        Err(transition) => transition,
    };
    let transition = match transition.into_failed_parts() {
        Ok(_) => panic!("an active cold initializer is not a terminal failure"),
        Err(transition) => transition,
    };
    assert!(transition.state().is_some());
}

#[test]
fn lowering_failure_identifies_the_pending_action_without_calling_the_port() {
    use crate::executor::{PhyRegisterPort, PhyRegisterRunError, run_phy_register};
    use core::{
        future::Future,
        task::{Context, Poll, Waker},
    };

    struct UnreachablePort;
    impl PhyRegisterPort for UnreachablePort {
        type Error = ();
        async fn complete(
            &mut self,
            _binding: super::PhyRegisterExternalBinding,
        ) -> Result<PhyRegisterCompletion, Self::Error> {
            panic!("an unsupported action must never reach hardware");
        }
    }

    let mut transition = PhyRegisterTransition::with_production_config();
    transition.phase = Some(super::Phase::Prelude(
        super::PreludeStep::I2cInitialSample { index: 2 },
    ));
    let action = PhyRegisterAction::SampleI2cMasterReset {
        index: 2,
        sample: 0,
    };
    {
        let mut port = UnreachablePort;
        let mut future = std::pin::pin!(run_phy_register(&mut transition, &mut port));
        assert_eq!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Err(PhyRegisterRunError::Lowering {
                stage: super::PhyRegisterStage::I2cMasterResetSample,
                error: PhyRegisterBindingError::UnsupportedAction,
            })),
        );
    }
    assert_eq!(
        transition.step_local(),
        Ok(PhyRegisterLocalStep::External(action))
    );
    assert!(!transition.state().unwrap().phy_registered());
    assert!(transition.into_model_parts().is_err());
}

#[test]
fn apply_profile_invariant_errors_restore_the_exact_parent_phase() {
    let mut transition = PhyRegisterTransition::with_production_config();
    transition.phase = Some(super::Phase::Prelude(super::PreludeStep::ApplyProfile));
    let config = transition.config.take().unwrap();

    assert_eq!(
        transition.step_local(),
        Err(super::PhyRegisterTransitionError::MissingStateOwner)
    );
    assert!(matches!(
        transition.phase,
        Some(super::Phase::Prelude(super::PreludeStep::ApplyProfile))
    ));

    transition.config = Some(config);
    let state = transition.state.take().unwrap();
    assert_eq!(
        transition.step_local(),
        Err(super::PhyRegisterTransitionError::MissingStateOwner)
    );
    assert!(transition.config.is_some());
    assert!(matches!(
        transition.phase,
        Some(super::Phase::Prelude(super::PreludeStep::ApplyProfile))
    ));

    transition.state = Some(state);
    assert_eq!(
        transition.step_local().unwrap(),
        PhyRegisterLocalStep::StateAdvanced
    );
}

#[test]
fn mark_registered_invariant_error_restores_the_exact_tail_phase() {
    let mut transition = PhyRegisterTransition::with_production_config();
    let state = transition.state.take().unwrap();
    transition.phase = Some(super::Phase::Tail(super::TailStep::MarkRegistered));

    assert_eq!(
        transition.step_local(),
        Err(super::PhyRegisterTransitionError::MissingStateOwner)
    );
    assert!(matches!(
        transition.phase,
        Some(super::Phase::Tail(super::TailStep::MarkRegistered))
    ));

    transition.state = Some(state);
    assert_eq!(
        transition.step_local().unwrap(),
        PhyRegisterLocalStep::StateAdvanced
    );
    assert!(transition.state().unwrap().phy_registered());
}

#[test]
fn calibration_clock_invariant_error_restores_the_exact_prelude_phase() {
    let mut transition = PhyRegisterTransition::with_production_config();
    let state = transition.state.take().unwrap();
    transition.phase = Some(super::Phase::Prelude(
        super::PreludeStep::CalibrationClockOn,
    ));
    let completion = PhyRegisterCompletion::Mmio(PhyRegisterMmioCompletion {
        action: PhyRegisterMmioAction::SetCalibrationClock { enabled: true },
    });

    assert_eq!(
        transition.advance_external(completion),
        Err(super::PhyRegisterTransitionError::MissingStateOwner)
    );
    assert!(matches!(
        transition.phase,
        Some(super::Phase::Prelude(
            super::PreludeStep::CalibrationClockOn
        ))
    ));

    transition.state = Some(state);
    transition.advance_external(completion).unwrap();
    assert!(matches!(transition.phase, Some(super::Phase::Rf(_))));
    assert!(transition.state().is_some());
}

#[test]
fn complete_without_the_registered_marker_rejects_model_extraction() {
    let mut transition = PhyRegisterTransition::with_production_config();
    transition.phase = Some(super::Phase::Complete(PhyRegisterOutcome {
        #[cfg(feature = "registration-diagnostics")]
        rf_calibration: None,
        full_calibration_performed: true,
        calibration_path: PhyCalibrationPath::FullUncached,
    }));

    let transition = match transition.into_model_parts() {
        Ok(_) => panic!("terminal phase without its marker must fail closed"),
        Err(transition) => transition,
    };
    assert!(!transition.state().unwrap().phy_registered());
}

#[test]
fn wrong_rf_completion_preserves_the_exact_child_owner() {
    let mut transition = PhyRegisterTransition::with_production_config();
    let state = transition.state.take().unwrap();
    transition.phase = Some(super::Phase::Rf(
        crate::calibration::cold::PhyRfColdInit::new(state),
    ));

    assert_eq!(
        transition.advance_external(PhyRegisterCompletion::Rf(
            crate::analog::i2c::PhyRfInitPrefixCompletion::BbpllCalibrationConfigured,
        )),
        Err(super::PhyRegisterTransitionError::WrongCompletion)
    );
    assert!(transition.state().is_some());
    assert_eq!(
        transition.step_local().unwrap(),
        PhyRegisterLocalStep::External(PhyRegisterAction::Rf(
            crate::analog::i2c::PhyRfInitPrefixAction::ConfigureFeBbClock,
        ))
    );
}

#[test]
fn wrong_baseband_completion_preserves_the_exact_child_owner() {
    let mut transition = PhyRegisterTransition::with_production_config();
    let state = transition.state.take().unwrap();
    transition.phase = Some(super::Phase::Baseband(
        crate::calibration::baseband::PhyBbInitTransition::new(state),
    ));

    assert_eq!(
        transition.advance_external(PhyRegisterCompletion::Baseband(
            crate::calibration::baseband::PhyBbInitCompletion::Mmio(
                crate::calibration::baseband::PhyBbMmioCompletion {
                    action: crate::calibration::baseband::PhyBbMmioAction::SetBasebandMode {
                        mode: crate::calibration::baseband::PhyBbBasebandMode::Calibration,
                    },
                }
            ),
        )),
        Err(super::PhyRegisterTransitionError::WrongCompletion)
    );
    assert!(transition.state().is_some());
    assert_eq!(
        transition.step_local().unwrap(),
        PhyRegisterLocalStep::External(PhyRegisterAction::Baseband(
            crate::calibration::baseband::PhyBbInitAction::Mmio(
                crate::calibration::baseband::PhyBbMmioAction::EnableBasebandInitialization,
            ),
        ))
    );
}

#[test]
fn wrong_temperature_completion_preserves_the_exact_child_owner() {
    let mut transition = PhyRegisterTransition::with_production_config();
    transition.phase = Some(super::Phase::Temperature(
        crate::analog::temperature::PhyTemperatureTransition::new(),
    ));

    assert_eq!(
        transition.advance_external(PhyRegisterCompletion::Temperature(
            crate::analog::temperature::PhyTemperatureCompletion::CodeSampled { value: 0 },
        )),
        Err(super::PhyRegisterTransitionError::WrongCompletion)
    );
    assert!(transition.state().is_some());
    assert!(matches!(
        transition.step_local().unwrap(),
        PhyRegisterLocalStep::External(PhyRegisterAction::Temperature(
            crate::analog::temperature::PhyTemperatureAction::ReadMasked { .. }
        ))
    ));
}

#[test]
fn temperature_step_error_restores_the_terminal_child() {
    let mut temperature = crate::analog::temperature::PhyTemperatureTransition::new();
    let crate::analog::temperature::PhyTemperatureAction::ReadMasked { field } =
        temperature.action()
    else {
        unreachable!()
    };
    temperature
        .advance(
            crate::analog::temperature::PhyTemperatureCompletion::MaskedRead { field, value: 5 },
        )
        .unwrap();
    temperature
        .advance(crate::analog::temperature::PhyTemperatureCompletion::CodeSampled { value: 128 })
        .unwrap();

    let mut transition = PhyRegisterTransition::with_production_config();
    transition.phase = Some(super::Phase::Temperature(temperature));
    assert_eq!(
        transition.step_local(),
        Err(super::PhyRegisterTransitionError::MissingStateOwner)
    );
    assert!(transition.state().is_some());
    transition.temperature_control =
        Some(transition.state().unwrap().register_temperature_control());
    assert_eq!(
        transition.step_local().unwrap(),
        PhyRegisterLocalStep::StateAdvanced
    );
}

#[test]
fn parent_timer_and_final_i2c_bindings_preserve_action_identity() {
    let phase = PhyRegisterDelayPhase::HardwareFrequencyDisabled;
    let timer =
        PhyRegisterTimerBinding::new(PhyRegisterAction::DelayMicros { phase, micros: 2 }).unwrap();
    assert_eq!(timer.micros(), 2);
    assert_eq!(
        timer.into_completion(),
        PhyRegisterCompletion::DelayElapsed { phase, micros: 2 }
    );

    let address = super::PHY_REGISTER_FINAL_I2C_ADDRESS;
    let mut i2c =
        PhyRegisterFinalI2cBinding::new(PhyRegisterAction::ReadFinalI2c { address }).unwrap();
    assert_eq!(
        i2c.action(),
        crate::calibration::cold::PhyColdI2cAction::StartRead { address }
    );
    i2c.read_started().unwrap();
    i2c.observe_read_result(Ok(0x5a)).unwrap();
    assert_eq!(
        i2c.into_completion().unwrap(),
        PhyRegisterCompletion::FinalI2cRead {
            address,
            value: 0x5a,
        }
    );
    assert_eq!(
        PhyRegisterTimerBinding::new(PhyRegisterAction::ReadFinalI2c { address }),
        Err(PhyRegisterBindingError::UnsupportedAction)
    );
}

#[test]
fn prelude_has_async_delay_and_reset_sample_edges() {
    let mut transition = PhyRegisterTransition::with_production_config();
    assert_eq!(
        transition.step_local().unwrap(),
        PhyRegisterLocalStep::External(PhyRegisterAction::Mmio(
            PhyRegisterMmioAction::PrepareColdStart
        ))
    );
    complete_mmio(&mut transition, PhyRegisterMmioAction::PrepareColdStart);
    complete_mmio(
        &mut transition,
        PhyRegisterMmioAction::ConfigureForceTxRx {
            enabled: true,
            phase: 0,
        },
    );
    assert_eq!(
        transition.step_local().unwrap(),
        PhyRegisterLocalStep::External(PhyRegisterAction::DelayMicros {
            phase: PhyRegisterDelayPhase::ForceTxRx {
                enabled: true,
                completed_phase: 0,
            },
            micros: 1,
        })
    );
    complete_delay(
        &mut transition,
        PhyRegisterDelayPhase::ForceTxRx {
            enabled: true,
            completed_phase: 0,
        },
        1,
    );
    complete_mmio(
        &mut transition,
        PhyRegisterMmioAction::ConfigureForceTxRx {
            enabled: true,
            phase: 1,
        },
    );
    complete_delay(
        &mut transition,
        PhyRegisterDelayPhase::ForceTxRx {
            enabled: true,
            completed_phase: 1,
        },
        1,
    );
    complete_mmio(&mut transition, PhyRegisterMmioAction::ResetFrequencyModule);
    complete_mmio(
        &mut transition,
        PhyRegisterMmioAction::SetHardwareFrequencyControl { enabled: false },
    );
    complete_delay(
        &mut transition,
        PhyRegisterDelayPhase::HardwareFrequencyDisabled,
        2,
    );
    let action = transition.step_local().unwrap();
    assert_eq!(
        action,
        PhyRegisterLocalStep::External(PhyRegisterAction::SampleI2cMasterReset {
            index: 0,
            sample: 0,
        })
    );
    assert!(
        PhyRegisterResetSampleBinding::new(match action {
            PhyRegisterLocalStep::External(action) => action,
            _ => unreachable!(),
        })
        .is_some()
    );
}

#[test]
fn stuck_i2c_reset_fails_only_after_bounded_async_samples_and_cleans_up() {
    let mut transition = PhyRegisterTransition::with_production_config_and_calibration(
        CALIBRATION_IDENTITY,
        Some(retained_cache(CALIBRATION_IDENTITY)),
    );
    transition.phase = Some(super::Phase::Prelude(super::PreludeStep::I2cResetSample {
        index: 0,
        sample: PHY_REGISTER_I2C_RESET_SAMPLE_LIMIT - 1,
    }));
    transition
        .advance_external(PhyRegisterCompletion::I2cMasterResetSampled {
            index: 0,
            sample: PHY_REGISTER_I2C_RESET_SAMPLE_LIMIT - 1,
            busy: true,
        })
        .unwrap();
    assert_eq!(
        transition.step_local().unwrap(),
        PhyRegisterLocalStep::External(PhyRegisterAction::Mmio(
            PhyRegisterMmioAction::SetCalibrationClock { enabled: false }
        ))
    );
    complete_mmio(
        &mut transition,
        PhyRegisterMmioAction::SetCalibrationClock { enabled: false },
    );
    complete_mmio(
        &mut transition,
        PhyRegisterMmioAction::SetBbpllCalibration { enabled: false },
    );
    complete_mmio(
        &mut transition,
        PhyRegisterMmioAction::SetHardwareFrequencyControl { enabled: true },
    );
    complete_mmio(
        &mut transition,
        PhyRegisterMmioAction::ConfigureForceTxRx {
            enabled: false,
            phase: 0,
        },
    );
    complete_delay(
        &mut transition,
        PhyRegisterDelayPhase::ForceTxRx {
            enabled: false,
            completed_phase: 0,
        },
        1,
    );
    complete_mmio(
        &mut transition,
        PhyRegisterMmioAction::ConfigureForceTxRx {
            enabled: false,
            phase: 1,
        },
    );
    complete_delay(
        &mut transition,
        PhyRegisterDelayPhase::ForceTxRx {
            enabled: false,
            completed_phase: 1,
        },
        1,
    );
    assert_eq!(
        transition.step_local().unwrap(),
        PhyRegisterLocalStep::Failed(PhyRegisterFailure::I2cMasterResetTimedOut {
            index: 0,
            samples: PHY_REGISTER_I2C_RESET_SAMPLE_LIMIT,
        })
    );
    assert!(transition.calibration_cache().is_none());
    let transition = match transition.into_model_parts() {
        Ok(_) => panic!("a failed registration cannot yield completed model parts"),
        Err(transition) => transition,
    };
    assert!(transition.failure_cleanup_completed());
    let (state, retry_cache) = match transition.into_failed_parts() {
        Ok(parts) => parts,
        Err(_) => panic!("terminal failure must release its ordinary state owner"),
    };
    assert!(!state.phy_registered());
    assert!(retry_cache.unwrap().matches(CALIBRATION_IDENTITY));
}

#[test]
fn success_tail_marks_owned_state_before_releasing_radio() {
    let mut transition =
        PhyRegisterTransition::with_production_config_and_calibration(CALIBRATION_IDENTITY, None);
    #[cfg(feature = "registration-diagnostics")]
    let diagnostics = Some(super::RfCalibrationDiagnostics {
        charge_pump_locked: false,
        frequency: None,
    });
    #[cfg(feature = "registration-diagnostics")]
    {
        transition.rf_calibration = diagnostics;
    }
    transition.phase = Some(super::Phase::Tail(super::TailStep::BackupCalibration));
    assert_eq!(
        transition.step_local().unwrap(),
        PhyRegisterLocalStep::StateAdvanced
    );
    assert!(transition.calibration_cache().is_none());
    complete_mmio(
        &mut transition,
        PhyRegisterMmioAction::SetBbpllCalibration { enabled: false },
    );
    transition
        .advance_external(PhyRegisterCompletion::FinalI2cRead {
            address: super::PHY_REGISTER_FINAL_I2C_ADDRESS,
            value: 0,
        })
        .unwrap();
    assert_eq!(
        transition.step_local().unwrap(),
        PhyRegisterLocalStep::StateAdvanced
    );
    assert!(transition.state().unwrap().phy_registered());
    let mut transition = match transition.into_model_parts() {
        Ok(_) => panic!("the registered marker alone is not terminal success"),
        Err(transition) => transition,
    };
    complete_mmio(
        &mut transition,
        PhyRegisterMmioAction::SetHardwareFrequencyControl { enabled: true },
    );
    complete_mmio(
        &mut transition,
        PhyRegisterMmioAction::ConfigureForceTxRx {
            enabled: false,
            phase: 0,
        },
    );
    complete_delay(
        &mut transition,
        PhyRegisterDelayPhase::ForceTxRx {
            enabled: false,
            completed_phase: 0,
        },
        1,
    );
    complete_mmio(
        &mut transition,
        PhyRegisterMmioAction::ConfigureForceTxRx {
            enabled: false,
            phase: 1,
        },
    );
    complete_delay(
        &mut transition,
        PhyRegisterDelayPhase::ForceTxRx {
            enabled: false,
            completed_phase: 1,
        },
        1,
    );
    assert_eq!(
        transition.step_local().unwrap(),
        PhyRegisterLocalStep::Complete(PhyRegisterOutcome {
            #[cfg(feature = "registration-diagnostics")]
            rf_calibration: diagnostics,
            full_calibration_performed: true,
            calibration_path: PhyCalibrationPath::FullForCache,
        })
    );
    assert!(
        transition
            .calibration_cache()
            .unwrap()
            .matches(CALIBRATION_IDENTITY)
    );
    assert!(!transition.failure_cleanup_completed());
    let transition = match transition.into_failed_parts() {
        Ok(_) => panic!("a successful cold initializer is not a terminal failure"),
        Err(transition) => transition,
    };
    let (state, calibration_cache) = match transition.into_model_parts() {
        Ok(parts) => parts,
        Err(_) => panic!("a completed cold initializer must release its state owner"),
    };
    assert!(calibration_cache.unwrap().matches(CALIBRATION_IDENTITY));
    assert!(state.phy_registered());
}

const CALIBRATION_IDENTITY: PhyCalibrationIdentity = PhyCalibrationIdentity {
    rf_cal_version: 0x1234_5678,
    base_mac_address: [0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6],
    mac_extension: 0x0718,
};

fn retained_cache(identity: PhyCalibrationIdentity) -> crate::state::PhyCalibrationCache {
    let mut state = crate::state::PhyState::new(crate::state::PhyConfig::production());
    state.mark_baseband_calibration_complete();
    state.apply_tx_power_outcome(crate::tx::power::PhyTxPowerOutcome {
        reference_codes: [80, 120],
        power_curve: [-3, 4, 5],
        point_corrections: [6, -7, 8],
        power_adjustment: -9,
        final_attenuation: 13,
        current_channel: 11,
        calibration_performed: true,
    });
    let mut snapshot = state.calibration_cache(identity).into_snapshot();
    snapshot.common.rc_calibrated = true;
    snapshot.wifi.pwdet_calibrated = true;
    snapshot.wifi.tx_iq_calibrated = true;
    snapshot.wifi.rx_gain_dc_calibrated = true;
    snapshot.wifi.rx_gain_tables_initialized = true;
    snapshot.wifi.wifi_rx_table_last_index =
        crate::calibration::baseband::PHY_WIFI_RX_GAIN_LAST_INDEX;
    snapshot.wifi.shared_rx_table_last_index =
        crate::calibration::baseband::PHY_SHARED_RX_GAIN_LAST_INDEX;
    snapshot.bluetooth.tx_dc_calibrated = true;
    snapshot.bluetooth.tx_power_calibrated = true;
    crate::state::PhyCalibrationCache::from_snapshot(snapshot).unwrap()
}

#[test]
fn complete_cache_selects_partial_calibration_and_republishes_hardware_tables() {
    let cache = retained_cache(CALIBRATION_IDENTITY);
    let mut transition = PhyRegisterTransition::with_production_config_and_calibration(
        CALIBRATION_IDENTITY,
        Some(cache),
    );
    transition.phase = Some(super::Phase::Prelude(super::PreludeStep::ApplyProfile));

    assert_eq!(
        transition.step_local().unwrap(),
        PhyRegisterLocalStep::StateAdvanced
    );
    assert_eq!(
        transition.calibration_path,
        PhyCalibrationPath::PartialFromCache
    );
    assert!(transition.calibration_cache().is_none());
    assert!(transition.state().unwrap().baseband_calibration_complete());
    assert!(
        transition
            .state()
            .unwrap()
            .tx_power_parameters()
            .already_calibrated
    );
    assert!(transition.state().unwrap().rc_calibration_complete());
    assert!(
        transition
            .state()
            .unwrap()
            .rx_gain_init_parameters()
            .dc_calibrated
    );
    assert!(
        !transition
            .state()
            .unwrap()
            .rx_gain_init_parameters()
            .tables_initialized
    );
    assert!(
        !transition
            .state()
            .unwrap()
            .channel_frequency_control()
            .frequency_table_initialized
    );
    let temperature = transition.temperature_control.unwrap();
    assert!(temperature.updates_offset_130());
    assert!(!temperature.updates_reference_copies());
}

#[test]
fn rejected_caller_cache_falls_back_to_full_calibration() {
    let cache = retained_cache(PhyCalibrationIdentity {
        rf_cal_version: CALIBRATION_IDENTITY.rf_cal_version + 1,
        ..CALIBRATION_IDENTITY
    });
    let mut transition = PhyRegisterTransition::with_production_config_and_calibration(
        CALIBRATION_IDENTITY,
        Some(cache),
    );
    transition.phase = Some(super::Phase::Prelude(super::PreludeStep::ApplyProfile));

    assert_eq!(
        transition.step_local().unwrap(),
        PhyRegisterLocalStep::StateAdvanced
    );
    assert_eq!(
        transition.calibration_path,
        PhyCalibrationPath::FullAfterRejectedCache
    );
    assert!(transition.calibration_cache().is_none());
    assert!(!transition.state().unwrap().baseband_calibration_complete());
    assert!(
        !transition
            .state()
            .unwrap()
            .tx_power_parameters()
            .already_calibrated
    );
    let temperature = transition.temperature_control.unwrap();
    assert!(temperature.updates_offset_130());
    assert!(temperature.updates_reference_copies());
}

#[test]
fn incomplete_cache_falls_back_without_retaining_partial_state() {
    let cache = retained_cache(CALIBRATION_IDENTITY);
    let mut snapshot = cache.into_snapshot();
    snapshot.wifi.rx_gain_dc_calibrated = false;
    let cache = crate::state::PhyCalibrationCache::from_snapshot(snapshot).unwrap();
    let mut transition = PhyRegisterTransition::with_production_config_and_calibration(
        CALIBRATION_IDENTITY,
        Some(cache),
    );
    transition.phase = Some(super::Phase::Prelude(super::PreludeStep::ApplyProfile));

    assert_eq!(
        transition.step_local().unwrap(),
        PhyRegisterLocalStep::StateAdvanced
    );
    assert_eq!(
        transition.calibration_path,
        PhyCalibrationPath::FullAfterRejectedCache
    );
    let state = transition.state().unwrap();
    assert!(!state.baseband_calibration_complete());
    assert!(!state.rx_gain_init_parameters().dc_calibrated);
    assert!(!state.rx_gain_init_parameters().tables_initialized);
    assert!(!state.tx_power_parameters().already_calibrated);
}

#[test]
fn final_i2c_failure_returns_only_the_original_retry_cache() {
    let retry_cache = retained_cache(CALIBRATION_IDENTITY);
    let retry_snapshot = *retry_cache.snapshot();
    let mut transition = PhyRegisterTransition::with_production_config_and_calibration(
        CALIBRATION_IDENTITY,
        Some(retry_cache),
    );
    transition.phase = Some(super::Phase::Tail(super::TailStep::BackupCalibration));

    assert_eq!(
        transition.step_local().unwrap(),
        PhyRegisterLocalStep::StateAdvanced
    );
    assert!(!transition.calibration_cache_ready);
    assert!(transition.calibration_cache().is_none());
    complete_mmio(
        &mut transition,
        PhyRegisterMmioAction::SetBbpllCalibration { enabled: false },
    );
    transition
        .advance_external(PhyRegisterCompletion::FinalI2cDeadlineExceeded {
            address: super::PHY_REGISTER_FINAL_I2C_ADDRESS,
        })
        .unwrap();
    assert!(!transition.calibration_cache_ready);
    assert!(transition.calibration_cache().is_none());
    assert!(!transition.failure_cleanup_completed());
    complete_failure_cleanup(&mut transition);
    assert!(transition.failure_cleanup_completed());
    assert_eq!(
        transition.step_local().unwrap(),
        PhyRegisterLocalStep::Failed(PhyRegisterFailure::FinalI2cDeadlineExceeded)
    );
    let transition = match transition.into_model_parts() {
        Ok(_) => panic!("late failure must not yield completed model parts"),
        Err(transition) => transition,
    };
    let (state, recovered_retry_cache) = transition
        .into_failed_parts()
        .unwrap_or_else(|_| panic!("late failure must return its ordinary state and retry input"));
    assert!(!state.phy_registered());
    let recovered_retry_cache = recovered_retry_cache.expect("caller input must be preserved");
    assert_eq!(*recovered_retry_cache.snapshot(), retry_snapshot);
    assert_ne!(
        retry_snapshot,
        state
            .calibration_cache(CALIBRATION_IDENTITY)
            .into_snapshot()
    );
}
