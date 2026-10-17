use super::*;

fn target_profile(
    maximum: i8,
    targets: [i8; PHY_TX_TARGET_POWER_COUNT],
    regulatory_override: bool,
) -> PhyTxTargetPowerProfile {
    PhyTxTargetPowerProfile::new(maximum, targets, regulatory_override)
}

fn uncalibrated_parameters() -> PhyTxPowerParameters {
    PhyTxPowerParameters {
        already_calibrated: false,
        crystal_selector: 0,
        environment: PhyTxCalibrationParameters {
            pbus_tx_path_value: 0,
            pbus_rx_path_value: 0,
            dco: [0; 4],
        },
        capacitance: [0; 6],
        target_adjustment: 0,
        power_offset: 0,
        initial_attenuation: 0,
        clear_tone_after_ready: false,
        reference_codes: [0; 2],
    }
}

#[test]
fn bluetooth_calibration_does_not_publish_its_search_channel() {
    let wifi = PhyTxPowerTransition::new(uncalibrated_parameters());
    let bluetooth = PhyTxPowerTransition::new_bluetooth(uncalibrated_parameters(), 0x20);

    assert_eq!(wifi.outcome().current_channel, 11);
    assert_eq!(bluetooth.outcome().current_channel, 0);
}

#[test]
fn target_profile_matches_every_recovered_rate_mapping_class() {
    let profile = target_profile(
        100,
        core::array::from_fn(|index| 8 + (index as i8 * 4)),
        false,
    );
    assert_eq!(
        profile.pair(0),
        PhyTxTargetPowerPair {
            primary: 2,
            alternate: 2,
        }
    );
    assert_eq!(
        profile.pair(2),
        PhyTxTargetPowerPair {
            primary: 3,
            alternate: 3,
        }
    );
    assert_eq!(
        profile.pair(8),
        PhyTxTargetPowerPair {
            primary: 7,
            alternate: 7,
        }
    );
    assert_eq!(
        profile.pair(16),
        PhyTxTargetPowerPair {
            primary: 8,
            alternate: 16,
        }
    );
    assert_eq!(
        profile.pair(22),
        PhyTxTargetPowerPair {
            primary: 11,
            alternate: 19,
        }
    );
    assert_eq!(
        profile.pair(24),
        PhyTxTargetPowerPair {
            primary: 12,
            alternate: 12,
        }
    );
    assert_eq!(
        profile.pair(31),
        PhyTxTargetPowerPair {
            primary: 19,
            alternate: 19,
        }
    );
    assert_eq!(profile.pair(41), profile.pair(0));
    assert_eq!(profile.pair(42), profile.pair(0));
    assert_eq!(profile.pair(32), PhyTxTargetPowerPair::ZERO);
    assert_eq!(profile.pair(40), PhyTxTargetPowerPair::ZERO);
    assert_eq!(profile.pair(43), PhyTxTargetPowerPair::ZERO);
}

#[test]
fn target_profile_applies_default_fcc_and_calibrated_maximum_bounds() {
    let mut targets = [0_i8; PHY_TX_TARGET_POWER_COUNT];
    targets[0] = 120;
    assert_eq!(target_profile(100, targets, false).pair(0).primary, 21);
    assert_eq!(target_profile(80, targets, false).pair(0).primary, 20);
    assert_eq!(target_profile(100, targets, true).pair(0).primary, 25);
    targets[0] = -12;
    assert_eq!(target_profile(100, targets, false).pair(0).primary, -3);
}

#[test]
fn runtime_quarter_dbm_limit_matches_vendor_mac_power_code() {
    let targets = [80_i8; PHY_TX_TARGET_POWER_COUNT];
    let profile = target_profile(84, targets, false).with_maximum_quarter_dbm(20);
    assert_eq!(
        profile.pair(0),
        PhyTxTargetPowerPair {
            primary: 5,
            alternate: 5,
        }
    );
}

#[test]
fn cold_state_exports_an_owned_target_profile_snapshot() {
    let mut targets = [0; PHY_TX_TARGET_POWER_COUNT];
    targets[0] = 44;
    let state = crate::state::PhyState::new(
        crate::state::PhyConfig::esp32s31_default().with_target_power(80, targets, false),
    );
    let profile = state.tx_target_power_profile();
    let _ = state;
    assert_eq!(
        profile.pair(0),
        PhyTxTargetPowerPair {
            primary: 11,
            alternate: 11,
        }
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
fn tx_cap_selection_uses_three_recovered_channel_bands() {
    let cap = [1, 2, 3, 4, 5, 6];
    assert_eq!(tx_cap_value(cap, 1), 0xc1);
    assert_eq!(tx_cap_value(cap, 6), 0xc3);
    assert_eq!(tx_cap_value(cap, 11), 0xc5);
}

#[test]
fn point_power_average_matches_rom_zero_floor() {
    assert_eq!(average_measured_power(-20, -20), 0);
    assert_eq!(average_measured_power(20, 24), 6);
}

#[test]
fn point_search_keeps_the_rom_i16_serial_error_width() {
    let request = PhyPowerControlPointRequest {
        identity: 1,
        target: 0,
        tone_selector: 0x80,
        base_attenuation: 100,
        initial_serial_error: 130,
        power_offset: 0,
        reference_codes: [0, 100],
        clear_tone_after_ready: false,
    };
    let transition = PhyPowerControlPointTransition::new(request);
    assert!(matches!(
        transition.action(),
        PhyPowerControlPointAction::ConfigureTone {
            attenuation: 100,
            ..
        }
    ));
}

#[test]
fn point_search_is_bounded_to_ten_iterations_and_four_sar_samples_each() {
    let mut transition = PhyPowerControlPointTransition::new(PhyPowerControlPointRequest {
        identity: 1,
        target: -100,
        tone_selector: 0x80,
        base_attenuation: 52,
        initial_serial_error: 0,
        power_offset: 0,
        reference_codes: [0, 100],
        clear_tone_after_ready: false,
    });
    let mut reads = 0;
    loop {
        let completion = match transition.action() {
            PhyPowerControlPointAction::ConfigureTone {
                identity,
                iteration,
                selector,
                attenuation,
            } => PhyPowerControlPointCompletion::ToneConfigured {
                identity,
                iteration,
                selector,
                attenuation,
            },
            PhyPowerControlPointAction::ToneSar(action) => {
                if matches!(action, PhyToneSarAction::ReadSar { .. }) {
                    reads += 1;
                }
                PhyPowerControlPointCompletion::ToneSar(tone_sar_completion(action, 50))
            }
            PhyPowerControlPointAction::StopTone { identity } => {
                PhyPowerControlPointCompletion::ToneStopped { identity }
            }
            PhyPowerControlPointAction::Complete(outcome) => {
                assert!(outcome.iterations <= 10);
                break;
            }
            PhyPowerControlPointAction::Failed(failure) => {
                panic!("unexpected failure {failure:?}")
            }
        };
        transition.advance(completion).unwrap();
    }
    assert!(reads <= 40);
}

#[test]
fn bluetooth_point_preserves_rom_corrections_above_wifi_maximum() {
    let mut transition = PhyPowerControlPointTransition::new(PhyPowerControlPointRequest {
        identity: 0,
        target: -100,
        tone_selector: 0x20,
        base_attenuation: 8,
        initial_serial_error: 92,
        power_offset: 0,
        reference_codes: [0, 100],
        clear_tone_after_ready: true,
    });
    loop {
        let completion = match transition.action() {
            PhyPowerControlPointAction::ConfigureTone {
                identity,
                iteration,
                selector,
                attenuation,
            } => PhyPowerControlPointCompletion::ToneConfigured {
                identity,
                iteration,
                selector,
                attenuation,
            },
            PhyPowerControlPointAction::ToneSar(action) => {
                PhyPowerControlPointCompletion::ToneSar(tone_sar_completion(action, 50))
            }
            PhyPowerControlPointAction::StopTone { identity } => {
                PhyPowerControlPointCompletion::ToneStopped { identity }
            }
            PhyPowerControlPointAction::Complete(outcome) => {
                assert_eq!(outcome.attenuation, 100);
                assert_eq!(outcome.correction, 92);
                break;
            }
            PhyPowerControlPointAction::Failed(failure) => {
                panic!("unexpected failure {failure:?}")
            }
        };
        transition.advance(completion).unwrap();
    }
}

#[test]
fn already_calibrated_root_emits_no_hardware_action() {
    let transition = PhyTxPowerTransition::new(PhyTxPowerParameters {
        already_calibrated: true,
        crystal_selector: 0,
        environment: PhyTxCalibrationParameters {
            pbus_tx_path_value: 0,
            pbus_rx_path_value: 0,
            dco: [0; 4],
        },
        capacitance: [0; 6],
        target_adjustment: 0,
        power_offset: 0,
        initial_attenuation: 0,
        clear_tone_after_ready: false,
        reference_codes: [0; 2],
    });
    assert!(matches!(
        transition.action(),
        PhyTxPowerAction::Complete(PhyTxPowerOutcome {
            calibration_performed: false,
            ..
        })
    ));
}

#[test]
fn external_lowering_covers_every_tx_power_operation_class() {
    let i2c = analog_registers::RFPLL_CAPACITOR_LOW;
    assert!(matches!(
        PhyTxPowerExternalBinding::lower(PhyTxPowerAction::Environment(
            PhyTxCalibrationEnvironmentAction::ConfigurePowerDetector
        )),
        Ok(PhyTxPowerExternalBinding::Environment(_))
    ));
    assert!(matches!(
        PhyTxPowerExternalBinding::lower(PhyTxPowerAction::Rfpll(
            RfpllFrequencyAction::DelayMicros(5)
        )),
        Ok(PhyTxPowerExternalBinding::Rfpll(_))
    ));
    assert!(matches!(
        PhyTxPowerExternalBinding::lower(PhyTxPowerAction::WriteI2c {
            address: i2c,
            value: 7,
        }),
        Ok(PhyTxPowerExternalBinding::I2c(_))
    ));
    assert!(matches!(
        PhyTxPowerExternalBinding::lower(PhyTxPowerAction::WriteReferenceControl { value: 1 }),
        Ok(PhyTxPowerExternalBinding::Mmio(_))
    ));
    assert!(matches!(
        PhyTxPowerExternalBinding::lower(PhyTxPowerAction::ToneSar(
            PhyToneSarAction::DelayMicros {
                measurement: 0,
                sample: 0,
                phase: crate::tx::calibration::PhyToneSarDelayPhase::SarTriggered,
                micros: 2,
            }
        )),
        Ok(PhyTxPowerExternalBinding::ToneSar(_))
    ));
    assert!(matches!(
        PhyTxPowerExternalBinding::lower(PhyTxPowerAction::Point(
            PhyPowerControlPointAction::StopTone { identity: 1 }
        )),
        Ok(PhyTxPowerExternalBinding::Point(
            PhyPowerControlPointExternalBinding::Mmio(_)
        ))
    ));
    assert!(matches!(
        PhyTxPowerExternalBinding::lower(PhyTxPowerAction::Complete(PhyTxPowerOutcome {
            reference_codes: [0; 2],
            power_curve: [0; 3],
            point_corrections: [0; 3],
            power_adjustment: 0,
            final_attenuation: 0,
            current_channel: 0,
            calibration_performed: false,
        })),
        Err(PhyTxPowerExternalBindingError::UnsupportedAction)
    ));
}
