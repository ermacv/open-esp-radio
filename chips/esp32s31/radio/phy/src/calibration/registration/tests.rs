use super::{
    PHY_REGISTER_I2C_RESET_SAMPLE_LIMIT, PhyCalibrationIdentity, PhyCalibrationPath,
    PhyRegisterAction, PhyRegisterBindingError, PhyRegisterCompletion, PhyRegisterDelayPhase,
    PhyRegisterFailure, PhyRegisterFinalI2cBinding, PhyRegisterLocalStep, PhyRegisterMmioAction,
    PhyRegisterMmioCompletion, PhyRegisterOutcome, PhyRegisterResetSampleBinding,
    PhyRegisterTimerBinding, PhyRegisterTransition,
};
use oer_chips_esp32s31_hal::radio::phy::i2c::Host;
use oer_chips_esp32s31_hal::types::PhyBbpllCalibration;
use oer_chips_esp32s31_hal::types::PhyForceTxRxPhase;
use oer_chips_esp32s31_hal::types::PhyFrequencyControl;

fn complete_mmio(
    transition: &mut PhyRegisterTransition,
    phy: &mut crate::state::PhyState,
    action: PhyRegisterMmioAction,
) {
    transition
        .advance_external(
            phy,
            PhyRegisterCompletion::Mmio(PhyRegisterMmioCompletion { action }),
        )
        .unwrap();
}

fn complete_delay(
    transition: &mut PhyRegisterTransition,
    phy: &mut crate::state::PhyState,
    phase: PhyRegisterDelayPhase,
    micros: u32,
) {
    transition
        .advance_external(phy, PhyRegisterCompletion::DelayElapsed { phase, micros })
        .unwrap();
}

fn complete_failure_cleanup(
    transition: &mut PhyRegisterTransition,
    phy: &mut crate::state::PhyState,
) {
    assert!(!transition.failure_cleanup_completed());
    complete_mmio(
        transition,
        phy,
        PhyRegisterMmioAction::SetCalibrationClock { enabled: false },
    );
    complete_mmio(
        transition,
        phy,
        PhyRegisterMmioAction::SetBbpllCalibration {
            calibration: PhyBbpllCalibration::Disabled,
        },
    );
    complete_mmio(
        transition,
        phy,
        PhyRegisterMmioAction::SetHardwareFrequencyControl {
            control: PhyFrequencyControl::Hardware,
        },
    );
    complete_mmio(
        transition,
        phy,
        PhyRegisterMmioAction::ConfigureForceTxRx {
            phase: PhyForceTxRxPhase::DisableInitialPhase,
        },
    );
    complete_delay(
        transition,
        phy,
        PhyRegisterDelayPhase::ForceTxRx {
            enabled: false,
            completed_phase: 0,
        },
        1,
    );
    assert!(!transition.failure_cleanup_completed());
    complete_mmio(
        transition,
        phy,
        PhyRegisterMmioAction::ConfigureForceTxRx {
            phase: PhyForceTxRxPhase::DisableFinalPhase,
        },
    );
    assert!(!transition.failure_cleanup_completed());
    complete_delay(
        transition,
        phy,
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
        )
    );
}

#[test]
fn the_chip_maximum_is_the_largest_production_target_power() {
    // 0x50 quarter dBm: 20 dBm.
    assert_eq!(
        crate::state::PhyConfig::production().highest_target_power_quarter_dbm(),
        0x50
    );
}

#[test]
fn apply_profile_invariant_error_restores_the_exact_parent_phase() {
    let mut phy = crate::state::PhyState::new(crate::state::PhyConfig::esp32s31_default());
    let mut transition = PhyRegisterTransition::with_production_config();
    transition.phase = Some(super::Phase::ApplyProfile);
    let config = transition.config.take().unwrap();

    assert_eq!(
        transition.step_local(&mut phy),
        Err(super::PhyRegisterTransitionError::MissingStateOwner)
    );
    assert!(matches!(transition.phase, Some(super::Phase::ApplyProfile)));

    transition.config = Some(config);
    assert_eq!(
        transition.step_local(&mut phy).unwrap(),
        PhyRegisterLocalStep::StateAdvanced
    );
}

#[test]
fn wrong_rf_completion_preserves_the_exact_child_owner() {
    let mut phy = crate::state::PhyState::new(crate::state::PhyConfig::esp32s31_default());
    let mut transition = PhyRegisterTransition::with_production_config();
    transition.phase = Some(super::Phase::Rf(
        crate::calibration::cold::PhyRfColdInit::new(),
    ));

    assert_eq!(
        transition.advance_external(
            &mut phy,
            PhyRegisterCompletion::Rf(
                crate::analog::i2c::PhyRfInitPrefixCompletion::BbpllCalibrationConfigured,
            )
        ),
        Err(super::PhyRegisterTransitionError::WrongCompletion)
    );
    assert_eq!(
        transition.step_local(&mut phy).unwrap(),
        PhyRegisterLocalStep::External(PhyRegisterAction::Rf(
            crate::analog::i2c::PhyRfInitPrefixAction::ConfigureFeBbClock,
        ))
    );
}

#[test]
fn wrong_baseband_completion_preserves_the_exact_child_owner() {
    let mut phy = crate::state::PhyState::new(crate::state::PhyConfig::esp32s31_default());
    let mut transition = PhyRegisterTransition::with_production_config();
    transition.phase = Some(super::Phase::Baseband(
        crate::calibration::baseband::PhyBbInitTransition::new(),
    ));

    assert_eq!(
        transition.advance_external(
            &mut phy,
            PhyRegisterCompletion::Baseband(
                crate::calibration::baseband::PhyBbInitCompletion::Mmio(
                    crate::calibration::baseband::PhyBbMmioCompletion {
                        action: crate::calibration::baseband::PhyBbMmioAction::SetBasebandMode {
                            mode: oer_chips_esp32s31_hal::types::BasebandMode::Calibration,
                        },
                    }
                ),
            )
        ),
        Err(super::PhyRegisterTransitionError::WrongCompletion)
    );
    assert_eq!(
        transition.step_local(&mut phy).unwrap(),
        PhyRegisterLocalStep::External(PhyRegisterAction::Baseband(
            crate::calibration::baseband::PhyBbInitAction::Mmio(
                crate::calibration::baseband::PhyBbMmioAction::EnableBasebandInitialization,
            ),
        ))
    );
}

#[test]
fn wrong_temperature_completion_preserves_the_exact_child_owner() {
    let mut phy = crate::state::PhyState::new(crate::state::PhyConfig::esp32s31_default());
    let mut transition = PhyRegisterTransition::with_production_config();
    transition.phase = Some(super::Phase::Temperature(
        crate::analog::temperature::PhyTemperatureTransition::new(),
    ));

    assert_eq!(
        transition.advance_external(
            &mut phy,
            PhyRegisterCompletion::Temperature(
                crate::analog::temperature::PhyTemperatureCompletion::CodeSampled { value: 0 },
            )
        ),
        Err(super::PhyRegisterTransitionError::WrongCompletion)
    );
    assert!(matches!(
        transition.step_local(&mut phy).unwrap(),
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

    let mut phy = crate::state::PhyState::new(crate::state::PhyConfig::esp32s31_default());

    let mut transition = PhyRegisterTransition::with_production_config();
    transition.phase = Some(super::Phase::Temperature(temperature));
    assert_eq!(
        transition.step_local(&mut phy),
        Err(super::PhyRegisterTransitionError::MissingStateOwner)
    );
    transition.temperature_control = Some(phy.register_temperature_control());
    assert_eq!(
        transition.step_local(&mut phy).unwrap(),
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
    let mut phy = crate::state::PhyState::new(crate::state::PhyConfig::esp32s31_default());
    let mut transition = PhyRegisterTransition::with_production_config();
    assert_eq!(
        transition.step_local(&mut phy).unwrap(),
        PhyRegisterLocalStep::External(PhyRegisterAction::Mmio(
            PhyRegisterMmioAction::PrepareColdStart
        ))
    );
    complete_mmio(
        &mut transition,
        &mut phy,
        PhyRegisterMmioAction::PrepareColdStart,
    );
    complete_mmio(
        &mut transition,
        &mut phy,
        PhyRegisterMmioAction::ConfigureForceTxRx {
            phase: PhyForceTxRxPhase::EnableInitialPhase,
        },
    );
    assert_eq!(
        transition.step_local(&mut phy).unwrap(),
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
        &mut phy,
        PhyRegisterDelayPhase::ForceTxRx {
            enabled: true,
            completed_phase: 0,
        },
        1,
    );
    complete_mmio(
        &mut transition,
        &mut phy,
        PhyRegisterMmioAction::ConfigureForceTxRx {
            phase: PhyForceTxRxPhase::EnableFinalPhase,
        },
    );
    complete_delay(
        &mut transition,
        &mut phy,
        PhyRegisterDelayPhase::ForceTxRx {
            enabled: true,
            completed_phase: 1,
        },
        1,
    );
    complete_mmio(
        &mut transition,
        &mut phy,
        PhyRegisterMmioAction::ResetFrequencyModule,
    );
    complete_mmio(
        &mut transition,
        &mut phy,
        PhyRegisterMmioAction::SetHardwareFrequencyControl {
            control: PhyFrequencyControl::Software,
        },
    );
    complete_delay(
        &mut transition,
        &mut phy,
        PhyRegisterDelayPhase::HardwareFrequencyDisabled,
        2,
    );
    let action = transition.step_local(&mut phy).unwrap();
    assert_eq!(
        action,
        PhyRegisterLocalStep::External(PhyRegisterAction::SampleI2cMasterReset {
            host: Host::Host0,
            sample: 0,
        })
    );
    assert_eq!(
        match action {
            PhyRegisterLocalStep::External(action) =>
                super::PhyRegisterExternalBinding::lower(action),
            _ => unreachable!(),
        },
        Ok(super::PhyRegisterExternalBinding::ResetSample(
            PhyRegisterResetSampleBinding::new(Host::Host0, 0)
        ))
    );
}

#[test]
fn stuck_i2c_reset_fails_only_after_bounded_async_samples_and_cleans_up() {
    let mut phy = crate::state::PhyState::new(crate::state::PhyConfig::esp32s31_default());
    let mut transition = PhyRegisterTransition::with_production_config_and_calibration(
        CALIBRATION_IDENTITY,
        Some(retained_cache(CALIBRATION_IDENTITY)),
    );
    transition.phase = Some(super::Phase::Prelude(super::PreludeStep::I2cResetSample {
        host: Host::Host0,
        sample: PHY_REGISTER_I2C_RESET_SAMPLE_LIMIT - 1,
    }));
    transition
        .advance_external(
            &mut phy,
            PhyRegisterCompletion::I2cMasterResetSampled {
                host: Host::Host0,
                sample: PHY_REGISTER_I2C_RESET_SAMPLE_LIMIT - 1,
                busy: true,
            },
        )
        .unwrap();
    assert_eq!(
        transition.step_local(&mut phy).unwrap(),
        PhyRegisterLocalStep::External(PhyRegisterAction::Mmio(
            PhyRegisterMmioAction::SetCalibrationClock { enabled: false }
        ))
    );
    complete_mmio(
        &mut transition,
        &mut phy,
        PhyRegisterMmioAction::SetCalibrationClock { enabled: false },
    );
    complete_mmio(
        &mut transition,
        &mut phy,
        PhyRegisterMmioAction::SetBbpllCalibration {
            calibration: PhyBbpllCalibration::Disabled,
        },
    );
    complete_mmio(
        &mut transition,
        &mut phy,
        PhyRegisterMmioAction::SetHardwareFrequencyControl {
            control: PhyFrequencyControl::Hardware,
        },
    );
    complete_mmio(
        &mut transition,
        &mut phy,
        PhyRegisterMmioAction::ConfigureForceTxRx {
            phase: PhyForceTxRxPhase::DisableInitialPhase,
        },
    );
    complete_delay(
        &mut transition,
        &mut phy,
        PhyRegisterDelayPhase::ForceTxRx {
            enabled: false,
            completed_phase: 0,
        },
        1,
    );
    complete_mmio(
        &mut transition,
        &mut phy,
        PhyRegisterMmioAction::ConfigureForceTxRx {
            phase: PhyForceTxRxPhase::DisableFinalPhase,
        },
    );
    complete_delay(
        &mut transition,
        &mut phy,
        PhyRegisterDelayPhase::ForceTxRx {
            enabled: false,
            completed_phase: 1,
        },
        1,
    );
    assert_eq!(
        transition.step_local(&mut phy).unwrap(),
        PhyRegisterLocalStep::Failed(PhyRegisterFailure::I2cMasterResetTimedOut {
            host: Host::Host0,
            samples: PHY_REGISTER_I2C_RESET_SAMPLE_LIMIT,
        })
    );
    assert!(transition.calibration_cache().is_none());
    let transition = match transition.into_completed_cache() {
        Ok(_) => panic!("a failed registration gives no completed cache"),
        Err(transition) => transition,
    };
    assert!(transition.failure_cleanup_completed());
    let retry_cache = match transition.into_failed_parts() {
        Ok(cache) => cache,
        Err(_) => panic!("terminal failure gives the caller's cache back"),
    };
    assert!(retry_cache.unwrap().matches(CALIBRATION_IDENTITY));
}

#[test]
fn success_tail_releases_the_radio_and_gives_the_cache() {
    let mut phy = crate::state::PhyState::new(crate::state::PhyConfig::esp32s31_default());
    let mut transition =
        PhyRegisterTransition::with_production_config_and_calibration(CALIBRATION_IDENTITY, None);
    #[cfg(oer_trace)]
    let diagnostics = Some(super::RfCalibrationDiagnostics {
        charge_pump_locked: false,
        frequency: None,
    });
    #[cfg(oer_trace)]
    {
        transition.rf_calibration = diagnostics;
    }
    transition.phase = Some(super::Phase::Tail(super::TailStep::BackupCalibration));
    assert_eq!(
        transition.step_local(&mut phy).unwrap(),
        PhyRegisterLocalStep::StateAdvanced
    );
    assert!(transition.calibration_cache().is_none());
    complete_mmio(
        &mut transition,
        &mut phy,
        PhyRegisterMmioAction::SetBbpllCalibration {
            calibration: PhyBbpllCalibration::Disabled,
        },
    );
    transition
        .advance_external(
            &mut phy,
            PhyRegisterCompletion::FinalI2cRead {
                address: super::PHY_REGISTER_FINAL_I2C_ADDRESS,
                value: 0,
            },
        )
        .unwrap();
    complete_mmio(
        &mut transition,
        &mut phy,
        PhyRegisterMmioAction::SetHardwareFrequencyControl {
            control: PhyFrequencyControl::Hardware,
        },
    );
    complete_mmio(
        &mut transition,
        &mut phy,
        PhyRegisterMmioAction::ConfigureForceTxRx {
            phase: PhyForceTxRxPhase::DisableInitialPhase,
        },
    );
    complete_delay(
        &mut transition,
        &mut phy,
        PhyRegisterDelayPhase::ForceTxRx {
            enabled: false,
            completed_phase: 0,
        },
        1,
    );
    complete_mmio(
        &mut transition,
        &mut phy,
        PhyRegisterMmioAction::ConfigureForceTxRx {
            phase: PhyForceTxRxPhase::DisableFinalPhase,
        },
    );
    complete_delay(
        &mut transition,
        &mut phy,
        PhyRegisterDelayPhase::ForceTxRx {
            enabled: false,
            completed_phase: 1,
        },
        1,
    );
    assert_eq!(
        transition.step_local(&mut phy).unwrap(),
        PhyRegisterLocalStep::Complete(PhyRegisterOutcome {
            #[cfg(oer_trace)]
            rf_calibration: diagnostics,
            #[cfg(oer_trace)]
            ready_waits: super::ReadyWaitDiagnostics::default(),
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
    let calibration_cache = match transition.into_completed_cache() {
        Ok(cache) => cache,
        Err(_) => panic!("a completed registration gives its cache"),
    };
    assert!(calibration_cache.unwrap().matches(CALIBRATION_IDENTITY));
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
    let mut phy = crate::state::PhyState::new(crate::state::PhyConfig::esp32s31_default());
    let mut transition = PhyRegisterTransition::with_production_config_and_calibration(
        CALIBRATION_IDENTITY,
        Some(cache),
    );
    transition.phase = Some(super::Phase::ApplyProfile);

    assert_eq!(
        transition.step_local(&mut phy).unwrap(),
        PhyRegisterLocalStep::StateAdvanced
    );
    assert_eq!(
        transition.calibration_path,
        PhyCalibrationPath::PartialFromCache
    );
    assert!(transition.calibration_cache().is_none());
    assert!(phy.baseband_calibration_complete());
    assert!(phy.tx_power_parameters().already_calibrated);
    assert!(phy.rc_calibration_complete());
    assert!(phy.rx_gain_init_parameters().dc_calibrated);
    assert!(!phy.rx_gain_init_parameters().tables_initialized);
    assert!(!phy.channel_frequency_control().frequency_table_initialized);
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
    let mut phy = crate::state::PhyState::new(crate::state::PhyConfig::esp32s31_default());
    let mut transition = PhyRegisterTransition::with_production_config_and_calibration(
        CALIBRATION_IDENTITY,
        Some(cache),
    );
    transition.phase = Some(super::Phase::ApplyProfile);

    assert_eq!(
        transition.step_local(&mut phy).unwrap(),
        PhyRegisterLocalStep::StateAdvanced
    );
    assert_eq!(
        transition.calibration_path,
        PhyCalibrationPath::FullAfterRejectedCache
    );
    assert!(transition.calibration_cache().is_none());
    assert!(!phy.baseband_calibration_complete());
    assert!(!phy.tx_power_parameters().already_calibrated);
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
    let mut phy = crate::state::PhyState::new(crate::state::PhyConfig::esp32s31_default());
    let mut transition = PhyRegisterTransition::with_production_config_and_calibration(
        CALIBRATION_IDENTITY,
        Some(cache),
    );
    transition.phase = Some(super::Phase::ApplyProfile);

    assert_eq!(
        transition.step_local(&mut phy).unwrap(),
        PhyRegisterLocalStep::StateAdvanced
    );
    assert_eq!(
        transition.calibration_path,
        PhyCalibrationPath::FullAfterRejectedCache
    );
    let state = &phy;
    assert!(!state.baseband_calibration_complete());
    assert!(!state.rx_gain_init_parameters().dc_calibrated);
    assert!(!state.rx_gain_init_parameters().tables_initialized);
    assert!(!state.tx_power_parameters().already_calibrated);
}

#[test]
fn final_i2c_failure_returns_only_the_original_retry_cache() {
    let retry_cache = retained_cache(CALIBRATION_IDENTITY);
    let retry_snapshot = *retry_cache.snapshot();
    let mut phy = crate::state::PhyState::new(crate::state::PhyConfig::esp32s31_default());
    let mut transition = PhyRegisterTransition::with_production_config_and_calibration(
        CALIBRATION_IDENTITY,
        Some(retry_cache),
    );
    transition.phase = Some(super::Phase::Tail(super::TailStep::BackupCalibration));

    assert_eq!(
        transition.step_local(&mut phy).unwrap(),
        PhyRegisterLocalStep::StateAdvanced
    );
    assert!(!transition.calibration_cache_ready);
    assert!(transition.calibration_cache().is_none());
    complete_mmio(
        &mut transition,
        &mut phy,
        PhyRegisterMmioAction::SetBbpllCalibration {
            calibration: PhyBbpllCalibration::Disabled,
        },
    );
    transition
        .advance_external(
            &mut phy,
            PhyRegisterCompletion::FinalI2cDeadlineExceeded {
                address: super::PHY_REGISTER_FINAL_I2C_ADDRESS,
            },
        )
        .unwrap();
    assert!(!transition.calibration_cache_ready);
    assert!(transition.calibration_cache().is_none());
    assert!(!transition.failure_cleanup_completed());
    complete_failure_cleanup(&mut transition, &mut phy);
    assert!(transition.failure_cleanup_completed());
    assert_eq!(
        transition.step_local(&mut phy).unwrap(),
        PhyRegisterLocalStep::Failed(PhyRegisterFailure::FinalI2cDeadlineExceeded)
    );
    let transition = match transition.into_completed_cache() {
        Ok(_) => panic!("late failure gives no completed cache"),
        Err(transition) => transition,
    };
    let recovered_retry_cache = transition
        .into_failed_parts()
        .unwrap_or_else(|_| panic!("late failure gives its retry input back"));
    let recovered_retry_cache = recovered_retry_cache.expect("caller input must be preserved");
    assert_eq!(*recovered_retry_cache.snapshot(), retry_snapshot);
    assert_ne!(
        retry_snapshot,
        phy.calibration_cache(CALIBRATION_IDENTITY).into_snapshot()
    );
}

/// What the registration's own steps produce, written apart from the code:
/// the action `step_local` gives at each prelude and tail step.
fn produced(transition: &mut PhyRegisterTransition, phase: super::Phase) -> PhyRegisterAction {
    let mut phy = crate::state::PhyState::new(crate::state::PhyConfig::esp32s31_default());
    transition.phase = Some(phase);
    match transition.step_local(&mut phy).unwrap() {
        PhyRegisterLocalStep::External(action) => action,
        other => panic!("a step that produces an action gave {other:?}"),
    }
}

#[test]
fn every_prelude_step_produces_the_vendor_s_action() {
    use super::{Phase, PreludeStep as Step};
    let mmio = PhyRegisterAction::Mmio;
    let force_delay = |enabled, completed_phase| PhyRegisterAction::DelayMicros {
        phase: PhyRegisterDelayPhase::ForceTxRx {
            enabled,
            completed_phase,
        },
        micros: 1,
    };
    let table = [
        (Step::Prepare, mmio(PhyRegisterMmioAction::PrepareColdStart)),
        (
            Step::ForceFirst,
            mmio(PhyRegisterMmioAction::ConfigureForceTxRx {
                phase: PhyForceTxRxPhase::EnableInitialPhase,
            }),
        ),
        (Step::ForceFirstDelay, force_delay(true, 0)),
        (
            Step::ForceSecond,
            mmio(PhyRegisterMmioAction::ConfigureForceTxRx {
                phase: PhyForceTxRxPhase::EnableFinalPhase,
            }),
        ),
        (Step::ForceSecondDelay, force_delay(true, 1)),
        (
            Step::FrequencyReset,
            mmio(PhyRegisterMmioAction::ResetFrequencyModule),
        ),
        (
            Step::DisableHardwareFrequency,
            mmio(PhyRegisterMmioAction::SetHardwareFrequencyControl {
                control: PhyFrequencyControl::Software,
            }),
        ),
        (
            Step::DisableHardwareFrequencyDelay,
            PhyRegisterAction::DelayMicros {
                phase: PhyRegisterDelayPhase::HardwareFrequencyDisabled,
                micros: 2,
            },
        ),
        (
            Step::I2cInitialSample { host: Host::Host1 },
            PhyRegisterAction::SampleI2cMasterReset {
                host: Host::Host1,
                sample: 0,
            },
        ),
        (
            Step::I2cResetPulse { host: Host::Host1 },
            mmio(PhyRegisterMmioAction::PulseI2cMasterReset { host: Host::Host1 }),
        ),
        (
            Step::I2cResetDelay {
                host: Host::Host0,
                sample: 3,
            },
            PhyRegisterAction::DelayMicros {
                phase: PhyRegisterDelayPhase::I2cMasterReset {
                    host: Host::Host0,
                    sample: 3,
                },
                micros: 1,
            },
        ),
        (
            Step::I2cResetSample {
                host: Host::Host0,
                sample: 2,
            },
            PhyRegisterAction::SampleI2cMasterReset {
                host: Host::Host0,
                sample: 2,
            },
        ),
        (
            Step::ConfigureXtal,
            mmio(PhyRegisterMmioAction::ConfigureXtal40Mhz),
        ),
        (
            Step::CalibrationClockOn,
            mmio(PhyRegisterMmioAction::SetCalibrationClock { enabled: true }),
        ),
    ];
    let mut transition = PhyRegisterTransition::with_production_config();
    for (step, expected) in table {
        assert_eq!(
            produced(&mut transition, Phase::Prelude(step)),
            expected,
            "{step:?}"
        );
    }
}

/// The tail's steps that produce an action, with the action each gives:
/// the success tail and the failure cleanup share them.
fn tail_table() -> [(super::TailStep, PhyRegisterAction); 8] {
    use super::TailStep as Step;
    let mmio = PhyRegisterAction::Mmio;
    let release_delay = |completed_phase| PhyRegisterAction::DelayMicros {
        phase: PhyRegisterDelayPhase::ForceTxRx {
            enabled: false,
            completed_phase,
        },
        micros: 1,
    };
    [
        (
            Step::CalibrationClockOff,
            mmio(PhyRegisterMmioAction::SetCalibrationClock { enabled: false }),
        ),
        (
            Step::BbpllOff,
            mmio(PhyRegisterMmioAction::SetBbpllCalibration {
                calibration: PhyBbpllCalibration::Disabled,
            }),
        ),
        (
            Step::FinalI2cRead,
            PhyRegisterAction::ReadFinalI2c {
                address: super::PHY_REGISTER_FINAL_I2C_ADDRESS,
            },
        ),
        (
            Step::EnableHardwareFrequency,
            mmio(PhyRegisterMmioAction::SetHardwareFrequencyControl {
                control: PhyFrequencyControl::Hardware,
            }),
        ),
        (
            Step::ReleaseFirst,
            mmio(PhyRegisterMmioAction::ConfigureForceTxRx {
                phase: PhyForceTxRxPhase::DisableInitialPhase,
            }),
        ),
        (Step::ReleaseFirstDelay, release_delay(0)),
        (
            Step::ReleaseSecond,
            mmio(PhyRegisterMmioAction::ConfigureForceTxRx {
                phase: PhyForceTxRxPhase::DisableFinalPhase,
            }),
        ),
        (Step::ReleaseSecondDelay, release_delay(1)),
    ]
}

#[test]
fn every_tail_step_produces_the_vendor_s_action() {
    let mut transition = PhyRegisterTransition::with_production_config();
    for (step, expected) in tail_table() {
        assert_eq!(
            produced(&mut transition, super::Phase::Tail(step)),
            expected,
            "{step:?}"
        );
    }
}

#[test]
fn every_cleanup_step_produces_the_tail_s_action() {
    let mut transition = PhyRegisterTransition::with_production_config();
    for (step, expected) in tail_table() {
        // A cleanup's final I2C read is skipped: it goes straight to the
        // BBPLL's disable.
        if step == super::TailStep::FinalI2cRead {
            continue;
        }
        assert_eq!(
            produced(
                &mut transition,
                super::Phase::Cleanup {
                    step,
                    failure: PhyRegisterFailure::FinalI2cDeadlineExceeded,
                },
            ),
            expected,
            "{step:?}"
        );
    }
}

#[test]
fn an_active_transition_refuses_both_ends_and_comes_back_whole() {
    let mut phy = crate::state::PhyState::new(crate::state::PhyConfig::esp32s31_default());
    let retry = retained_cache(CALIBRATION_IDENTITY);
    let retry_snapshot = *retry.snapshot();
    let transition = PhyRegisterTransition::with_production_config_and_calibration(
        CALIBRATION_IDENTITY,
        Some(retry),
    );
    assert!(!transition.failure_cleanup_completed());
    let transition = match transition.into_completed_cache() {
        Ok(_) => panic!("an active registration gives no completed cache"),
        Err(transition) => transition,
    };
    let mut transition = match transition.into_failed_parts() {
        Ok(_) => panic!("an active registration is not a terminal failure"),
        Err(transition) => transition,
    };
    // Whole: where it was, with the caller's cache, and it still steps.
    assert!(matches!(
        transition.phase,
        Some(super::Phase::Prelude(super::PreludeStep::Prepare))
    ));
    assert_eq!(
        transition
            .calibration_cache
            .as_ref()
            .map(|cache| *cache.snapshot()),
        Some(retry_snapshot)
    );
    assert!(transition.config.is_some());
    assert!(transition.step_local(&mut phy).is_ok());
}
