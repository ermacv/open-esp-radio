use super::*;

const POWER_REQUEST: PhyRxIqEstimatorRequest = PhyRxIqEstimatorRequest {
    identity: 7,
    control: 0x3ff,
    kind: PhyRxIqEstimatorKind::TotalPower,
};

fn estimator_completion(action: PhyRxIqEstimatorAction) -> PhyRxIqEstimatorCompletion {
    match action {
        PhyRxIqEstimatorAction::Configure(request) => {
            PhyRxIqEstimatorCompletion::Configured(request)
        }
        PhyRxIqEstimatorAction::SetEnable {
            request,
            phase,
            enabled,
        } => PhyRxIqEstimatorCompletion::EnableSet {
            request,
            phase,
            enabled,
        },
        PhyRxIqEstimatorAction::DelayMicros {
            request,
            phase,
            micros,
        } => PhyRxIqEstimatorCompletion::DelayElapsed {
            request,
            phase,
            micros,
        },
        PhyRxIqEstimatorAction::AwaitReadinessEdge { request, .. } => {
            PhyRxIqEstimatorCompletion::ReadinessObserved {
                request,
                snapshot: PhyDcIqReadinessSnapshot {
                    ready: true,
                    activity: false,
                },
            }
        }
        PhyRxIqEstimatorAction::ReadTotalPower(request) => {
            PhyRxIqEstimatorCompletion::TotalPowerRead {
                request,
                value: 0x12_3400,
            }
        }
        PhyRxIqEstimatorAction::ReadMismatch(request) => PhyRxIqEstimatorCompletion::MismatchRead {
            request,
            snapshot: PhyRxIqMismatchSnapshot {
                sum_i: 0,
                difference_i: 0,
                difference_q: 0,
                sum_q: 0,
            },
        },
        _ => panic!("unexpected test action: {action:?}"),
    }
}

fn cover_completion(action: PhyRxIqCoverAction) -> PhyRxIqCoverCompletion {
    match action {
        PhyRxIqCoverAction::ConfigureCoefficient {
            identity,
            iteration,
            kind,
            value,
            final_value,
        } => PhyRxIqCoverCompletion::CoefficientConfigured {
            identity,
            iteration,
            kind,
            value,
            final_value,
        },
        PhyRxIqCoverAction::Estimator(action) => {
            PhyRxIqCoverCompletion::Estimator(estimator_completion(action))
        }
        _ => panic!("unexpected cover action: {action:?}"),
    }
}

fn rf_completion(action: PhyRxIqRfCalibrationAction) -> PhyRxIqRfCalibrationCompletion {
    match action {
        PhyRxIqRfCalibrationAction::ConfigureCalibrationMode => {
            PhyRxIqRfCalibrationCompletion::CalibrationModeConfigured
        }
        PhyRxIqRfCalibrationAction::ConfigureTone {
            enabled,
            selector,
            attenuation,
        } => PhyRxIqRfCalibrationCompletion::ToneConfigured {
            enabled,
            selector,
            attenuation,
        },
        PhyRxIqRfCalibrationAction::Cover(action) => {
            PhyRxIqRfCalibrationCompletion::Cover(cover_completion(action))
        }
        _ => panic!("unexpected RF action: {action:?}"),
    }
}

const CAP_STATUSES: [u8; 6] = [0, 1, 1, 0, 2, 2];

fn rfpll_completion(
    action: RfpllFrequencyAction,
    cap_status_reads: &mut u8,
) -> RfpllFrequencyCompletion {
    match action {
        RfpllFrequencyAction::WriteMasked { field, .. } => {
            RfpllFrequencyCompletion::MaskedWrite { field }
        }
        RfpllFrequencyAction::WriteByte { address, .. } => {
            RfpllFrequencyCompletion::ByteWrite { address }
        }
        RfpllFrequencyAction::ReadMasked { field } => {
            let value = if field == analog_registers::RFPLL_LOCK_STATUS {
                1
            } else if field == analog_registers::RFPLL_CAPACITOR_SEARCH_STATUS {
                // Accept the first candidate of each direction, then end the
                // direction with its two boundary statuses.
                let status = CAP_STATUSES[usize::from(*cap_status_reads)];
                *cap_status_reads += 1;
                status
            } else {
                0
            };
            RfpllFrequencyCompletion::MaskedRead { field, value }
        }
        RfpllFrequencyAction::ReadByte { address } => {
            let value = if address == analog_registers::RFPLL_CALIBRATED_CAPACITOR_LOW {
                0xc8
            } else {
                4
            };
            RfpllFrequencyCompletion::ByteRead { address, value }
        }
        RfpllFrequencyAction::DelayMicros(micros) => RfpllFrequencyCompletion::DelayElapsed(micros),
        action => panic!("unexpected RFPLL action: {action:?}"),
    }
}

fn loopback_completion(action: PhyTxIqLoopbackAction) -> PhyTxIqLoopbackCompletion {
    use crate::analog::i2c;

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
        action => panic!("unexpected loopback terminal: {action:?}"),
    }
}

fn dc_iq_completion(
    action: crate::calibration::estimator::PhyDcIqAction,
) -> crate::calibration::estimator::PhyDcIqCompletion {
    use crate::calibration::estimator::{
        PhyDcIqAccumulatorSnapshot, PhyDcIqAction, PhyDcIqCompletion,
    };

    match action {
        PhyDcIqAction::Configure(request) => PhyDcIqCompletion::Configured(request),
        PhyDcIqAction::SetEnable {
            request,
            phase,
            enabled,
        } => PhyDcIqCompletion::EnableSet {
            request,
            phase,
            enabled,
        },
        PhyDcIqAction::DelayMicros {
            request,
            phase,
            micros,
        } => PhyDcIqCompletion::DelayElapsed {
            request,
            phase,
            micros,
        },
        PhyDcIqAction::AwaitReadinessEdge { request, .. } => PhyDcIqCompletion::ReadinessObserved {
            request,
            snapshot: PhyDcIqReadinessSnapshot {
                ready: true,
                activity: false,
            },
        },
        PhyDcIqAction::ReadAccumulators(request) => PhyDcIqCompletion::AccumulatorsRead {
            request,
            snapshot: PhyDcIqAccumulatorSnapshot {
                i: 0,
                q: 0,
                power: 0,
            },
        },
        action => panic!("unexpected DC/IQ terminal: {action:?}"),
    }
}

fn dco_completion(action: PhyRxDcoAction) -> PhyRxDcoCompletion {
    match action {
        PhyRxDcoAction::PrepareRxDcoControlRestore => {
            PhyRxDcoCompletion::RxDcoControlRestorePrepared
        }
        PhyRxDcoAction::ReadPbus { selector, path } => PhyRxDcoCompletion::PbusRead {
            selector,
            path,
            value: 0,
        },
        PhyRxDcoAction::ForcePbus(transaction) => {
            PhyRxDcoCompletion::PbusForceCompleted(transaction)
        }
        PhyRxDcoAction::DelayMicros { iteration, micros } => {
            PhyRxDcoCompletion::DelayElapsed { iteration, micros }
        }
        PhyRxDcoAction::DcIq(action) => PhyRxDcoCompletion::DcIq(dc_iq_completion(action)),
        PhyRxDcoAction::RestoreRxDcoControl => PhyRxDcoCompletion::RxDcoControlRestored,
        action => panic!("unexpected RX-DCO terminal: {action:?}"),
    }
}

fn gain_completion(
    action: PhyRxIqGainAction,
    configured_phase: &mut Option<i8>,
) -> PhyRxIqGainCompletion {
    match action {
        PhyRxIqGainAction::ForcePbus { pass, transaction } => {
            PhyRxIqGainCompletion::PbusCompleted { pass, transaction }
        }
        PhyRxIqGainAction::WriteI2c { address, value } => {
            PhyRxIqGainCompletion::I2cWritten { address, value }
        }
        PhyRxIqGainAction::AdjustTx(PhyRxIqAdjustedTxAction::ReadI2cMasked { field }) => {
            PhyRxIqGainCompletion::AdjustTx(PhyRxIqAdjustedTxCompletion::I2cMaskedRead {
                field,
                value: 0,
            })
        }
        PhyRxIqGainAction::ConfigureTxIq { kind, value } => {
            if kind == PhyTxIqCoefficientKind::Phase {
                *configured_phase = Some(value);
            }
            PhyRxIqGainCompletion::TxIqConfigured { kind, value }
        }
        PhyRxIqGainAction::Dco(action) => PhyRxIqGainCompletion::Dco(dco_completion(action)),
        PhyRxIqGainAction::ConfigureTone {
            enabled,
            selector,
            attenuation,
        } => PhyRxIqGainCompletion::ToneConfigured {
            enabled,
            selector,
            attenuation,
        },
        PhyRxIqGainAction::Estimator(action) => {
            PhyRxIqGainCompletion::Estimator(estimator_completion(action))
        }
        PhyRxIqGainAction::Data(PhyRxIqDataAction::Calibration(action)) => {
            PhyRxIqGainCompletion::Data(PhyRxIqDataCompletion::Calibration(rf_completion(action)))
        }
        action => panic!("unexpected RXIQ gain terminal: {action:?}"),
    }
}

#[test]
fn total_power_estimator_always_runs_complete_disable_tail() {
    let mut transition = PhyRxIqEstimatorTransition::new(POWER_REQUEST);
    let mut steps = 0;
    loop {
        match transition.action() {
            PhyRxIqEstimatorAction::Complete(outcome) => {
                assert_eq!(outcome.measurement, PhyRxIqMeasurement::TotalPower(0x2468));
                break;
            }
            action => transition.advance(estimator_completion(action)).unwrap(),
        }
        steps += 1;
        assert!(steps < 16);
    }
    assert_eq!(steps, 9);
}

#[test]
fn timeout_preserves_external_cleanup_instead_of_polling() {
    let mut transition = PhyRxIqEstimatorTransition::new(POWER_REQUEST);
    loop {
        match transition.action() {
            PhyRxIqEstimatorAction::AwaitReadinessEdge { request, .. } => {
                transition
                    .advance(PhyRxIqEstimatorCompletion::ReadinessTimedOut(request))
                    .unwrap();
                break;
            }
            action => transition.advance(estimator_completion(action)).unwrap(),
        }
    }
    let mut cleanup = 0;
    loop {
        match transition.action() {
            PhyRxIqEstimatorAction::Failed(_) => break,
            action => transition.advance(estimator_completion(action)).unwrap(),
        }
        cleanup += 1;
        assert!(cleanup < 8);
    }
    assert_eq!(cleanup, 3);
}

#[test]
fn mismatch_and_txiq_adjustment_are_bounded_pure_transforms() {
    assert_eq!(
        rxiq_mismatch(
            14,
            PhyRxIqMismatchSnapshot {
                sum_i: 0x12000,
                difference_i: 0x5000,
                difference_q: -0x3000,
                sum_q: 0xe000,
            }
        ),
        [26, 45]
    );
    let adjusted = adjusted_txiq_coefficient(
        PhyRxIqAdjustedTxParameters {
            coefficient: (5 << 7) | 7,
            current_channel: 6,
            current_temperature: 100,
            calibration_temperature: 90,
            calibration_dcode: [12, 18],
        },
        [14, 21],
    );
    assert_eq!(adjusted[0], 5);
    assert!((-60..=60).contains(&adjusted[1]));
}

#[test]
fn cover_has_exactly_two_measurements_and_six_coefficient_writes() {
    let mut transition = PhyRxIqCoverTransition::new(PhyRxIqCoverRequest {
        identity: 3,
        exponent: 14,
    });
    let mut reads = 0;
    let mut writes = 0;
    let mut steps = 0;
    loop {
        let action = transition.action();
        match action {
            PhyRxIqCoverAction::Complete(outcome) => {
                assert_eq!(outcome.gain, 0);
                assert_eq!(outcome.phase, 0);
                break;
            }
            PhyRxIqCoverAction::ConfigureCoefficient { .. } => writes += 1,
            PhyRxIqCoverAction::Estimator(PhyRxIqEstimatorAction::ReadMismatch(_)) => {
                reads += 1;
            }
            _ => {}
        }
        transition.advance(cover_completion(action)).unwrap();
        steps += 1;
        assert!(steps < 40);
    }
    assert_eq!(reads, 2);
    assert_eq!(writes, 6);
}

#[test]
fn rf_data_converges_after_two_equal_bounded_samples() {
    let mut transition = PhyRxIqDataTransition::new(PhyRxIqDataRequest {
        selector: 0x80,
        attenuation: 0x30,
    });
    let mut calibrations = 0;
    let mut steps = 0;
    loop {
        let action = transition.action();
        match action {
            PhyRxIqDataAction::Complete(outcome) => {
                assert_eq!(outcome.coefficient, 0);
                assert_eq!(outcome.attempts, 2);
                assert!(outcome.converged);
                break;
            }
            PhyRxIqDataAction::Calibration(
                PhyRxIqRfCalibrationAction::ConfigureCalibrationMode,
            ) => calibrations += 1,
            _ => {}
        }
        let completion = match action {
            PhyRxIqDataAction::Calibration(action) => {
                PhyRxIqDataCompletion::Calibration(rf_completion(action))
            }
            _ => panic!("unexpected data action: {action:?}"),
        };
        transition.advance(completion).unwrap();
        steps += 1;
        assert!(steps < 100);
    }
    assert_eq!(calibrations, 2);
}

#[test]
fn root_pbus_failure_runs_tx_off_and_work_mode_cleanup() {
    let mut transition = PhyRxIqInitTransition::new(PhyRxIqInitParameters {
        crystal_selector: 0,
        pbus_rx_path_value: 0x20,
        capacitance: [1, 2, 3, 4, 5, 6],
        channel_6_dcode: [0; 2],
        adjusted_tx: PhyRxIqAdjustedTxParameters {
            coefficient: 0,
            current_channel: 6,
            current_temperature: 0,
            calibration_temperature: 0,
            calibration_dcode: [0; 2],
        },
        coefficients: [0; 4],
    });
    let mut tx_off = 0;
    let mut cap_status_reads = 0;
    let mut steps = 0;
    loop {
        let action = transition.action();
        let completion = match action {
            PhyRxIqInitAction::Rfpll(action) => {
                PhyRxIqInitCompletion::Rfpll(rfpll_completion(action, &mut cap_status_reads))
            }
            PhyRxIqInitAction::WriteTxCap { address, value } => {
                PhyRxIqInitCompletion::TxCapWritten { address, value }
            }
            PhyRxIqInitAction::ConfigureRootStatus => PhyRxIqInitCompletion::RootStatusConfigured,
            PhyRxIqInitAction::ConfigurePbusDebugMode => {
                PhyRxIqInitCompletion::PbusDebugModeConfigured
            }
            PhyRxIqInitAction::ForcePbus(transaction) if tx_off == 0 => {
                tx_off = 1;
                PhyRxIqInitCompletion::PbusTimedOut(transaction)
            }
            PhyRxIqInitAction::ForcePbus(transaction) => {
                tx_off += 1;
                PhyRxIqInitCompletion::PbusCompleted(transaction)
            }
            PhyRxIqInitAction::ConfigurePbusWorkMode => {
                PhyRxIqInitCompletion::PbusWorkModeConfigured {
                    settle_required: false,
                }
            }
            PhyRxIqInitAction::Failed(PhyRxIqInitFailure::Pbus(_)) => break,
            action => panic!("unexpected root cleanup action: {action:?}"),
        };
        transition.advance(completion).unwrap();
        steps += 1;
        assert!(steps < 80);
    }
    // One failed RX-on publication followed by all five TX-off commands.
    assert_eq!(tx_off, 6);
}

#[test]
fn root_success_traverses_every_child_and_commits_channel_six() {
    let initial_coefficients = [0x0181, 0x0282, 0x0383, 0x0484];
    let mut transition = PhyRxIqInitTransition::new(PhyRxIqInitParameters {
        crystal_selector: 0,
        pbus_rx_path_value: 0x20,
        capacitance: [1, 2, 3, 4, 5, 6],
        channel_6_dcode: [0; 2],
        adjusted_tx: PhyRxIqAdjustedTxParameters {
            coefficient: 0,
            // This deliberately differs from the calibration channel.
            // The root must override it before entering the child.
            current_channel: 11,
            current_temperature: 0,
            calibration_temperature: 0,
            calibration_dcode: [0; 2],
        },
        coefficients: initial_coefficients,
    });
    let mut cap_status_reads = 0;
    let mut configured_phase = None;
    let mut steps = 0;
    loop {
        let action = transition.action();
        let completion = match action {
            PhyRxIqInitAction::Rfpll(action) => {
                PhyRxIqInitCompletion::Rfpll(rfpll_completion(action, &mut cap_status_reads))
            }
            PhyRxIqInitAction::WriteTxCap { address, value } => {
                PhyRxIqInitCompletion::TxCapWritten { address, value }
            }
            PhyRxIqInitAction::ConfigureRootStatus => PhyRxIqInitCompletion::RootStatusConfigured,
            PhyRxIqInitAction::ConfigurePbusDebugMode => {
                PhyRxIqInitCompletion::PbusDebugModeConfigured
            }
            PhyRxIqInitAction::ForcePbus(transaction) => {
                PhyRxIqInitCompletion::PbusCompleted(transaction)
            }
            PhyRxIqInitAction::Loopback(action) => {
                PhyRxIqInitCompletion::Loopback(loopback_completion(action))
            }
            PhyRxIqInitAction::ConfigureCorrection { begin } => {
                PhyRxIqInitCompletion::CorrectionConfigured { begin }
            }
            PhyRxIqInitAction::Gain(action) => {
                PhyRxIqInitCompletion::Gain(gain_completion(action, &mut configured_phase))
            }
            PhyRxIqInitAction::ConfigurePbusWorkMode => {
                PhyRxIqInitCompletion::PbusWorkModeConfigured {
                    settle_required: false,
                }
            }
            PhyRxIqInitAction::Complete(outcome) => {
                assert_eq!(outcome.current_channel, 6);
                assert_eq!(outcome.coefficients[0], 0);
                assert_eq!(
                    outcome.coefficients[1],
                    convert_rxiq_coefficient(initial_coefficients[1])
                );
                assert_eq!(outcome.gain.rf_attempts, 2);
                break;
            }
            action => panic!("unexpected RXIQ root action: {action:?}"),
        };
        transition.advance(completion).unwrap();
        steps += 1;
        assert!(steps < 240);
    }
    assert_eq!(configured_phase, Some(0));
    assert_eq!(usize::from(cap_status_reads), CAP_STATUSES.len());
}

#[test]
fn external_lowering_covers_every_rxiq_operation_layer() {
    let transaction = PhyPbusForceTest::new(1, 2, 0);
    assert!(matches!(
        PhyRxIqEstimatorExternalBinding::lower(PhyRxIqEstimatorAction::Configure(POWER_REQUEST)),
        Ok(PhyRxIqEstimatorExternalBinding::Mmio(_))
    ));
    assert!(matches!(
        PhyRxIqEstimatorExternalBinding::lower(PhyRxIqEstimatorAction::DelayMicros {
            request: POWER_REQUEST,
            phase: PhyDcIqDelayPhase::Start,
            micros: 1,
        }),
        Ok(PhyRxIqEstimatorExternalBinding::Timer(_))
    ));
    assert!(matches!(
        PhyRxIqEstimatorExternalBinding::lower(
            PhyRxIqEstimatorAction::AwaitReadinessEdge {
                request: POWER_REQUEST,
                readiness_activity_edges: 0,
                readiness_samples: 11,
            }
        ),
        Ok(PhyRxIqEstimatorExternalBinding::Readiness(binding)) if binding.samples() == 11
    ));
    assert!(matches!(
        PhyRxIqCoverExternalBinding::lower(PhyRxIqCoverAction::ConfigureCoefficient {
            identity: 0,
            iteration: 0,
            kind: PhyRxIqCoefficientKind::Gain,
            value: 0,
            final_value: false,
        }),
        Ok(PhyRxIqCoverExternalBinding::Mmio(_))
    ));
    assert!(matches!(
        PhyRxIqDataExternalBinding::lower(PhyRxIqDataAction::Calibration(
            PhyRxIqRfCalibrationAction::ConfigureCalibrationMode
        )),
        Ok(PhyRxIqDataExternalBinding::Calibration(_))
    ));
    assert!(matches!(
        PhyRxIqGainExternalBinding::lower(PhyRxIqGainAction::ForcePbus {
            pass: 0,
            transaction,
        }),
        Ok(PhyRxIqGainExternalBinding::Pbus(_))
    ));
    assert!(matches!(
        PhyRxIqGainExternalBinding::lower(PhyRxIqGainAction::WriteI2c {
            address: analog_registers::RFPLL_INTERNAL_DCODE_0.address(),
            value: 1,
        }),
        Ok(PhyRxIqGainExternalBinding::I2c(_))
    ));
    assert!(matches!(
        PhyRxIqGainExternalBinding::lower(PhyRxIqGainAction::AdjustTx(
            PhyRxIqAdjustedTxAction::ReadI2cMasked {
                field: analog_registers::RFPLL_INTERNAL_DCODE_0,
            }
        )),
        Ok(PhyRxIqGainExternalBinding::AdjustTx(_))
    ));
    assert!(matches!(
        PhyRxIqGainExternalBinding::lower(PhyRxIqGainAction::Dco(
            PhyRxDcoAction::PrepareRxDcoControlRestore
        )),
        Ok(PhyRxIqGainExternalBinding::Dco(_))
    ));
    assert!(matches!(
        PhyRxIqInitExternalBinding::lower(PhyRxIqInitAction::Rfpll(
            RfpllFrequencyAction::DelayMicros(5)
        )),
        Ok(PhyRxIqInitExternalBinding::Rfpll(_))
    ));
    assert!(matches!(
        PhyRxIqInitExternalBinding::lower(PhyRxIqInitAction::WriteTxCap {
            address: analog_registers::RFPLL_INTERNAL_DCODE_0.address(),
            value: 1,
        }),
        Ok(PhyRxIqInitExternalBinding::I2c(_))
    ));
    assert!(matches!(
        PhyRxIqInitExternalBinding::lower(PhyRxIqInitAction::ConfigureRootStatus),
        Ok(PhyRxIqInitExternalBinding::Mmio(_))
    ));
    assert!(matches!(
        PhyRxIqInitExternalBinding::lower(PhyRxIqInitAction::ForcePbus(transaction)),
        Ok(PhyRxIqInitExternalBinding::Pbus(_))
    ));
    assert!(matches!(
        PhyRxIqInitExternalBinding::lower(PhyRxIqInitAction::DelayMicros {
            phase: PhyRxIqWorkModeDelayPhase::Settle,
            micros: 1,
        }),
        Ok(PhyRxIqInitExternalBinding::Timer(_))
    ));
}
