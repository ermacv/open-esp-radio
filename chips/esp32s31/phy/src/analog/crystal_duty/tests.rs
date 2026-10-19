use super::{
    XtalDutyCalibrationAction, XtalDutyCalibrationCompletion, XtalDutyCalibrationOutcome,
    XtalDutyCalibrationParameters, XtalDutyCalibrationTransition, XtalDutyHardwareFailure,
    XtalDutyPassAction, XtalDutyPassCompletion, XtalDutyPassOutcome, XtalDutyPassTransition,
    XtalDutyPassTransitionError, XtalDutyPrepareAction, XtalDutyPrepareCompletion,
    XtalDutyPrepareTransition, XtalDutyRestoreAction, XtalDutyRestoreCompletion,
    XtalDutyRestoreTransition, XtalDutySearchAction, XtalDutySearchCompletion,
    XtalDutySearchOutcome, XtalDutySearchTransition, XtalDutySearchTransitionError,
};
use crate::analog::i2c::analog_registers;
use crate::analog::pbus::PhyPbusForceTest;
use crate::analog::rfpll::{RfpllFrequencyAction, RfpllFrequencyCompletion};
use crate::calibration::estimator::{
    PhyDcIqAccumulatorSnapshot, PhyDcIqAction, PhyDcIqCompletion, PhyDcIqReadinessSnapshot,
};
use crate::rx::dc_offset::{PhyRxDcoAction, PhyRxDcoCompletion};
use crate::rx::signal_power::{
    PhySignalPowerAccumulatorSnapshot, PhySignalPowerAction, PhySignalPowerCompletion,
};

fn signal_components(value: i64) -> (i32, i32) {
    for first in 0..=512_i32 {
        for second in 0..=512_i32 {
            if i64::from(first * first + second * second) == value {
                return (first, second);
            }
        }
    }
    panic!("test signal power is not a bounded sum of two squares");
}

fn complete_signal_power_action(
    action: PhySignalPowerAction,
    value: i64,
) -> PhySignalPowerCompletion {
    match action {
        PhySignalPowerAction::ConfigureClock {
            request,
            clock,
            enabled,
        } => PhySignalPowerCompletion::ClockConfigured {
            request,
            clock,
            enabled,
        },
        PhySignalPowerAction::SetEstimatorEnable {
            request,
            phase,
            enabled,
        } => PhySignalPowerCompletion::EstimatorEnableSet {
            request,
            phase,
            enabled,
        },
        PhySignalPowerAction::DelayMicros {
            request,
            phase,
            micros,
        } => PhySignalPowerCompletion::DelayElapsed {
            request,
            phase,
            micros,
        },
        PhySignalPowerAction::ConfigureEstimator { request, control } => {
            PhySignalPowerCompletion::EstimatorConfigured { request, control }
        }
        PhySignalPowerAction::AwaitReadinessEdge { request, .. } => {
            PhySignalPowerCompletion::ReadinessObserved {
                request,
                snapshot: PhyDcIqReadinessSnapshot {
                    ready: true,
                    activity: false,
                },
            }
        }
        PhySignalPowerAction::ReadAccumulators(request) => {
            let (sum, difference) = signal_components(value);
            let shift = u32::from(request.shift.wrapping_sub(2)) & 0x1f;
            PhySignalPowerCompletion::AccumulatorsRead {
                request,
                snapshot: PhySignalPowerAccumulatorSnapshot {
                    sum_i: sum.wrapping_shl(shift),
                    difference_i: difference.wrapping_shl(shift),
                    difference_q: 0,
                    sum_q: 0,
                },
            }
        }
        action => panic!("unexpected terminal signal-power action: {action:?}"),
    }
}

fn signal_power_request(
    action: PhySignalPowerAction,
) -> crate::rx::signal_power::PhySignalPowerRequest {
    match action {
        PhySignalPowerAction::ConfigureClock { request, .. }
        | PhySignalPowerAction::SetEstimatorEnable { request, .. }
        | PhySignalPowerAction::DelayMicros { request, .. }
        | PhySignalPowerAction::ConfigureEstimator { request, .. }
        | PhySignalPowerAction::AwaitReadinessEdge { request, .. }
        | PhySignalPowerAction::ReadAccumulators(request) => request,
        action => panic!("unexpected terminal signal-power action: {action:?}"),
    }
}

fn complete_search_measurement(transition: &mut XtalDutySearchTransition, value: i64) {
    let XtalDutySearchAction::SignalPower(first_action) = transition.action() else {
        panic!("signal-power measurement was not armed");
    };
    let request = signal_power_request(first_action);
    loop {
        let XtalDutySearchAction::SignalPower(action) = transition.action() else {
            return;
        };
        if signal_power_request(action) != request {
            return;
        }
        transition
            .advance(XtalDutySearchCompletion::SignalPower(
                complete_signal_power_action(action, value),
            ))
            .unwrap();
    }
}

fn complete_dc_iq_action(action: PhyDcIqAction) -> PhyDcIqCompletion {
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
        action => panic!("unexpected terminal DC/IQ action: {action:?}"),
    }
}

fn complete_rx_dco_action(action: PhyRxDcoAction) -> PhyRxDcoCompletion {
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
        PhyRxDcoAction::DcIq(action) => PhyRxDcoCompletion::DcIq(complete_dc_iq_action(action)),
        PhyRxDcoAction::RestoreRxDcoControl => PhyRxDcoCompletion::RxDcoControlRestored,
        action => panic!("unexpected terminal RX-DCO action: {action:?}"),
    }
}

fn complete_rfpll_action(
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
            let value = if field == crate::analog::i2c::analog_registers::RFPLL_LOCK_STATUS {
                1
            } else if field == crate::analog::i2c::analog_registers::RFPLL_CAPACITOR_SEARCH_STATUS {
                let value = if (*cap_status_reads).is_multiple_of(3) {
                    0
                } else {
                    1
                };
                *cap_status_reads = (*cap_status_reads).wrapping_add(1);
                value
            } else {
                0
            };
            RfpllFrequencyCompletion::MaskedRead { field, value }
        }
        RfpllFrequencyAction::ReadByte { address } => {
            let value = if address
                == crate::analog::i2c::analog_registers::RFPLL_CALIBRATED_CAPACITOR_LOW
            {
                100
            } else {
                0
            };
            RfpllFrequencyCompletion::ByteRead { address, value }
        }
        RfpllFrequencyAction::DelayMicros(micros) => RfpllFrequencyCompletion::DelayElapsed(micros),
        action => panic!("unexpected terminal RFPLL action: {action:?}"),
    }
}

fn complete_prepare_action(
    action: XtalDutyPrepareAction,
    rfpll_cap_status_reads: &mut u8,
) -> XtalDutyPrepareCompletion {
    match action {
        XtalDutyPrepareAction::Rfpll(action) => {
            XtalDutyPrepareCompletion::Rfpll(complete_rfpll_action(action, rfpll_cap_status_reads))
        }
        XtalDutyPrepareAction::ConfigureCalibrationTone {
            enabled,
            selector,
            step,
        } => XtalDutyPrepareCompletion::CalibrationToneConfigured {
            enabled,
            selector,
            step,
        },
        XtalDutyPrepareAction::ConfigureRxClock { enabled } => {
            XtalDutyPrepareCompletion::RxClockConfigured { enabled }
        }
        XtalDutyPrepareAction::ConfigureTxClock { enabled } => {
            XtalDutyPrepareCompletion::TxClockConfigured { enabled }
        }
        XtalDutyPrepareAction::ConfigurePbusDebugMode => {
            XtalDutyPrepareCompletion::PbusDebugModeConfigured
        }
        XtalDutyPrepareAction::ForcePbus(transaction) => {
            XtalDutyPrepareCompletion::PbusForceCompleted(transaction)
        }
        XtalDutyPrepareAction::PrepareRxDcoControlRestore => {
            XtalDutyPrepareCompletion::RxDcoControlRestorePrepared
        }
        XtalDutyPrepareAction::RxDco(action) => {
            XtalDutyPrepareCompletion::RxDco(complete_rx_dco_action(action))
        }
        XtalDutyPrepareAction::RestoreRxDcoControl => {
            XtalDutyPrepareCompletion::RxDcoControlRestored
        }
        action => panic!("unexpected terminal preparation action: {action:?}"),
    }
}

fn complete_restore_action(action: XtalDutyRestoreAction) -> XtalDutyRestoreCompletion {
    match action {
        XtalDutyRestoreAction::ConfigureCalibrationTone {
            enabled,
            selector,
            step,
        } => XtalDutyRestoreCompletion::CalibrationToneConfigured {
            enabled,
            selector,
            step,
        },
        XtalDutyRestoreAction::ConfigureRxClock { enabled } => {
            XtalDutyRestoreCompletion::RxClockConfigured { enabled }
        }
        XtalDutyRestoreAction::ConfigureTxClock { enabled } => {
            XtalDutyRestoreCompletion::TxClockConfigured { enabled }
        }
        XtalDutyRestoreAction::ForcePbus(transaction) => {
            XtalDutyRestoreCompletion::PbusForceCompleted(transaction)
        }
        XtalDutyRestoreAction::ConfigurePbusWorkMode => {
            XtalDutyRestoreCompletion::PbusWorkModeConfigured {
                settle_required: false,
            }
        }
        action => panic!("unexpected restoration action: {action:?}"),
    }
}

fn drive_pass(
    transition: &mut XtalDutyCalibrationTransition,
    _expected_frequency_code: u16,
    initial_duty: u8,
    restore_duty: u8,
) {
    let mut current_candidate = None;
    let mut duty_writes = 0;
    let mut rfpll_cap_status_reads = 0;
    loop {
        match transition.action() {
            XtalDutyCalibrationAction::Pass(XtalDutyPassAction::WriteMasked {
                field,
                value: 0,
            }) => {
                transition
                    .advance(XtalDutyCalibrationCompletion::Pass(
                        XtalDutyPassCompletion::MaskedWrite { field },
                    ))
                    .unwrap();
            }
            XtalDutyCalibrationAction::Pass(XtalDutyPassAction::WriteByte { address, value }) => {
                assert_eq!(
                    value,
                    if duty_writes == 0 {
                        initial_duty
                    } else {
                        restore_duty
                    }
                );
                duty_writes += 1;
                transition
                    .advance(XtalDutyCalibrationCompletion::Pass(
                        XtalDutyPassCompletion::ByteWrite { address },
                    ))
                    .unwrap();
            }
            XtalDutyCalibrationAction::Pass(XtalDutyPassAction::Prepare(action)) => {
                transition
                    .advance(XtalDutyCalibrationCompletion::Pass(
                        XtalDutyPassCompletion::Prepare(complete_prepare_action(
                            action,
                            &mut rfpll_cap_status_reads,
                        )),
                    ))
                    .unwrap();
            }
            XtalDutyCalibrationAction::Pass(XtalDutyPassAction::Search(
                XtalDutySearchAction::WriteCandidate { address, candidate },
            )) => {
                current_candidate = Some(candidate);
                transition
                    .advance(XtalDutyCalibrationCompletion::Pass(
                        XtalDutyPassCompletion::Search(
                            XtalDutySearchCompletion::CandidateWritten { address, candidate },
                        ),
                    ))
                    .unwrap();
            }
            XtalDutyCalibrationAction::Pass(XtalDutyPassAction::Search(
                XtalDutySearchAction::DelayMicros {
                    candidate,
                    micros: 20,
                },
            )) => {
                assert_eq!(current_candidate, Some(candidate));
                transition
                    .advance(XtalDutyCalibrationCompletion::Pass(
                        XtalDutyPassCompletion::Search(XtalDutySearchCompletion::DelayElapsed {
                            candidate,
                        }),
                    ))
                    .unwrap();
            }
            XtalDutyCalibrationAction::Pass(XtalDutyPassAction::Search(
                XtalDutySearchAction::SignalPower(action),
            )) => {
                let candidate = current_candidate.unwrap();
                let component = i64::from(0x80 - candidate);
                transition
                    .advance(XtalDutyCalibrationCompletion::Pass(
                        XtalDutyPassCompletion::Search(XtalDutySearchCompletion::SignalPower(
                            complete_signal_power_action(action, component.wrapping_mul(component)),
                        )),
                    ))
                    .unwrap();
            }
            XtalDutyCalibrationAction::Pass(XtalDutyPassAction::Restore(action)) => {
                let pass_complete = matches!(action, XtalDutyRestoreAction::ConfigurePbusWorkMode);
                transition
                    .advance(XtalDutyCalibrationCompletion::Pass(
                        XtalDutyPassCompletion::Restore(complete_restore_action(action)),
                    ))
                    .unwrap();
                if pass_complete {
                    assert_eq!(duty_writes, 2);
                    break;
                }
            }
            action => panic!("unexpected pass action: {action:?}"),
        }
    }
}

#[test]
fn evaluates_all_24_candidates_only_after_timer_and_measurement_edges() {
    let mut transition = XtalDutySearchTransition::new();
    let mut writes = 0;
    let mut delays = 0;
    let mut measurements = 0;
    loop {
        match transition.action() {
            XtalDutySearchAction::WriteCandidate { address, candidate } => {
                writes += 1;
                transition
                    .advance(XtalDutySearchCompletion::CandidateWritten { address, candidate })
                    .unwrap();
            }
            XtalDutySearchAction::DelayMicros {
                candidate,
                micros: 20,
            } => {
                delays += 1;
                assert_eq!(candidate, 0x20 + delays - 1);
                transition
                    .advance(XtalDutySearchCompletion::DelayElapsed { candidate })
                    .unwrap();
            }
            XtalDutySearchAction::SignalPower(_) => {
                measurements += 1;
                let candidate = 0x20 + writes - 1;
                let component = i64::from(0x80 - candidate);
                complete_search_measurement(&mut transition, component.wrapping_mul(component));
            }
            XtalDutySearchAction::Complete(outcome) => {
                assert_eq!(
                    outcome,
                    XtalDutySearchOutcome {
                        best_candidate: 0x37,
                        best_filtered_power: 0x49 * 0x49,
                    }
                );
                break;
            }
            action => panic!("unexpected action: {action:?}"),
        }
    }
    assert_eq!(writes, 24);
    assert_eq!(delays, 24);
    assert_eq!(measurements, 24 * 4);
}

#[test]
fn each_outlier_uses_at_most_two_identity_bound_replacements() {
    let mut transition = XtalDutySearchTransition::new();
    let duty_address = analog_registers::XTAL_DUTY_CANDIDATE;
    transition
        .advance(XtalDutySearchCompletion::CandidateWritten {
            address: duty_address,
            candidate: 0x20,
        })
        .unwrap();
    transition
        .advance(XtalDutySearchCompletion::DelayElapsed { candidate: 0x20 })
        .unwrap();
    for value in [1, 100, 100, 100] {
        complete_search_measurement(&mut transition, value);
    }
    assert!(matches!(
        transition.action(),
        XtalDutySearchAction::SignalPower(_)
    ));
    assert_eq!(
        transition.advance(XtalDutySearchCompletion::CandidateWritten {
            address: duty_address,
            candidate: 0x21,
        }),
        Err(XtalDutySearchTransitionError::WrongCompletion)
    );
    complete_search_measurement(&mut transition, 200);
    assert!(matches!(
        transition.action(),
        XtalDutySearchAction::SignalPower(_)
    ));
    complete_search_measurement(&mut transition, 64);
    assert_eq!(
        transition.action(),
        XtalDutySearchAction::WriteCandidate {
            address: duty_address,
            candidate: 0x21,
        }
    );
}

#[test]
fn preparation_exposes_all_ten_pbus_commands_and_owned_rx_dco_field() {
    let parameter = XtalDutyCalibrationParameters {
        restore_duty: 0,
        rf_frequency_offset_base: 0x31,
        pbus_rx_path_value: 0x42,
    };
    let mut transition = XtalDutyPrepareTransition::new(0x988, parameter);
    let mut rfpll_cap_status_reads = 0;

    while let XtalDutyPrepareAction::Rfpll(action) = transition.action() {
        transition
            .advance(XtalDutyPrepareCompletion::Rfpll(complete_rfpll_action(
                action,
                &mut rfpll_cap_status_reads,
            )))
            .unwrap();
    }

    for expected in [
        XtalDutyPrepareAction::ConfigureCalibrationTone {
            enabled: true,
            selector: 0x80,
            step: 0,
        },
        XtalDutyPrepareAction::ConfigureRxClock { enabled: true },
        XtalDutyPrepareAction::ConfigureTxClock { enabled: true },
        XtalDutyPrepareAction::ConfigurePbusDebugMode,
    ] {
        assert_eq!(transition.action(), expected);
        transition
            .advance(complete_prepare_action(
                expected,
                &mut rfpll_cap_status_reads,
            ))
            .unwrap();
    }

    let expected_pbus = [
        PhyPbusForceTest::new(4, 1, 0),
        PhyPbusForceTest::new(4, 2, 1),
        PhyPbusForceTest::new(5, 1, 0),
        PhyPbusForceTest::new(0, 1, 0x40),
        PhyPbusForceTest::new(0, 2, 0x42),
        PhyPbusForceTest::new(1, 1, 0x189),
        PhyPbusForceTest::new(1, 2, 0xf0),
        PhyPbusForceTest::new(0, 1, 0x43),
        PhyPbusForceTest::new(1, 1, 0x38),
        PhyPbusForceTest::new(1, 1, 0x189),
    ];
    for transaction in expected_pbus {
        assert_eq!(
            transition.action(),
            XtalDutyPrepareAction::ForcePbus(transaction)
        );
        transition
            .advance(XtalDutyPrepareCompletion::PbusForceCompleted(transaction))
            .unwrap();
    }

    assert_eq!(
        transition.action(),
        XtalDutyPrepareAction::PrepareRxDcoControlRestore
    );
    transition
        .advance(XtalDutyPrepareCompletion::RxDcoControlRestorePrepared)
        .unwrap();
    assert_eq!(
        transition.action(),
        XtalDutyPrepareAction::RxDco(PhyRxDcoAction::PrepareRxDcoControlRestore)
    );
    while let XtalDutyPrepareAction::RxDco(action) = transition.action() {
        transition
            .advance(XtalDutyPrepareCompletion::RxDco(complete_rx_dco_action(
                action,
            )))
            .unwrap();
    }
    let outcome = crate::rx::dc_offset::PhyRxDcoOutcome {
        configuration: [0x0100_0100; 2],
        iterations: 1,
        converged: true,
        last_estimate: crate::calibration::estimator::PhyDcIqEstimate {
            i: 0,
            q: 0,
            power: 0,
        },
    };
    assert_eq!(
        transition.action(),
        XtalDutyPrepareAction::RestoreRxDcoControl
    );
    transition
        .advance(XtalDutyPrepareCompletion::RxDcoControlRestored)
        .unwrap();
    assert_eq!(
        transition.action(),
        XtalDutyPrepareAction::Complete(outcome)
    );
}

#[test]
fn restoration_requires_external_pbus_and_timer_completions() {
    let mut transition = XtalDutyRestoreTransition::new();
    for expected in [
        XtalDutyRestoreAction::ConfigureCalibrationTone {
            enabled: false,
            selector: 0x80,
            step: 0x28,
        },
        XtalDutyRestoreAction::ConfigureRxClock { enabled: false },
        XtalDutyRestoreAction::ConfigureTxClock { enabled: false },
    ] {
        assert_eq!(transition.action(), expected);
        transition
            .advance(complete_restore_action(expected))
            .unwrap();
    }

    for transaction in [
        PhyPbusForceTest::new(0, 1, 0),
        PhyPbusForceTest::new(1, 1, 0),
        PhyPbusForceTest::new(1, 2, 0),
    ] {
        assert_eq!(
            transition.action(),
            XtalDutyRestoreAction::ForcePbus(transaction)
        );
        transition
            .advance(XtalDutyRestoreCompletion::PbusForceCompleted(transaction))
            .unwrap();
    }

    assert_eq!(
        transition.action(),
        XtalDutyRestoreAction::ConfigurePbusWorkMode
    );
    transition
        .advance(XtalDutyRestoreCompletion::PbusWorkModeConfigured {
            settle_required: true,
        })
        .unwrap();
    assert_eq!(transition.action(), XtalDutyRestoreAction::DelayMicros(1));
    assert!(
        transition
            .advance(XtalDutyRestoreCompletion::DelayElapsed { micros: 2 })
            .is_err()
    );
    transition
        .advance(XtalDutyRestoreCompletion::DelayElapsed { micros: 1 })
        .unwrap();
    assert_eq!(
        transition.action(),
        XtalDutyRestoreAction::ConfigurePbusWorkModePulse
    );
    transition
        .advance(XtalDutyRestoreCompletion::PbusWorkModePulseConfigured)
        .unwrap();
    assert_eq!(transition.action(), XtalDutyRestoreAction::DelayMicros(2));
    transition
        .advance(XtalDutyRestoreCompletion::DelayElapsed { micros: 2 })
        .unwrap();
    assert_eq!(
        transition.action(),
        XtalDutyRestoreAction::ClearPbusWorkModePulse
    );
    transition
        .advance(XtalDutyRestoreCompletion::PbusWorkModePulseCleared)
        .unwrap();
    assert_eq!(transition.action(), XtalDutyRestoreAction::Complete);

    let mut timed_out = XtalDutyRestoreTransition::new();
    for _ in 0..3 {
        let action = timed_out.action();
        timed_out.advance(complete_restore_action(action)).unwrap();
    }
    let XtalDutyRestoreAction::ForcePbus(transaction) = timed_out.action() else {
        panic!("expected first restore PBus command");
    };
    timed_out
        .advance(XtalDutyRestoreCompletion::PbusForceTimedOut(transaction))
        .unwrap();
    assert_eq!(
        timed_out.action(),
        XtalDutyRestoreAction::Failed(XtalDutyHardwareFailure::PbusForceTestTimedOut(transaction))
    );
}

#[test]
fn pass_rejects_wrong_field_and_stale_parameter_completion() {
    let parameter = XtalDutyCalibrationParameters {
        restore_duty: 0,
        rf_frequency_offset_base: 0x31,
        pbus_rx_path_value: 0x42,
    };
    let mut transition = XtalDutyPassTransition::new(0x988, 0x2a, parameter);
    let XtalDutyPassAction::WriteMasked { field, .. } = transition.action() else {
        panic!("expected path-disable write");
    };
    assert_eq!(
        transition.advance(XtalDutyPassCompletion::MaskedWrite {
            field: crate::analog::i2c::analog_registers::RFPLL_LOCK_STATUS,
        }),
        Err(XtalDutyPassTransitionError::WrongCompletion)
    );
    transition
        .advance(XtalDutyPassCompletion::MaskedWrite { field })
        .unwrap();

    let XtalDutyPassAction::WriteByte { address, .. } = transition.action() else {
        panic!("expected initial-duty write");
    };
    transition
        .advance(XtalDutyPassCompletion::ByteWrite { address })
        .unwrap();
    assert_eq!(
        transition.advance(XtalDutyPassCompletion::Prepare(
            XtalDutyPrepareCompletion::Rfpll(RfpllFrequencyCompletion::DelayElapsed(20))
        )),
        Err(XtalDutyPassTransitionError::WrongCompletion)
    );
    assert!(matches!(
        transition.action(),
        XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
            RfpllFrequencyAction::WriteMasked { .. }
        ))
    ));
}

#[test]
fn wrapper_orders_both_frequency_passes_without_hidden_progress() {
    let initial_duty = 0x2a;
    let mut transition = XtalDutyCalibrationTransition::new(XtalDutyCalibrationParameters {
        restore_duty: 0,
        rf_frequency_offset_base: 0x31,
        pbus_rx_path_value: 0x42,
    });

    let XtalDutyCalibrationAction::ReadInitialDuty { field } = transition.action() else {
        panic!("expected the initial duty read");
    };
    transition
        .advance(XtalDutyCalibrationCompletion::InitialDutyRead {
            field,
            value: initial_duty,
        })
        .unwrap();

    let XtalDutyCalibrationAction::DisableCalibrationPath { field, value: 0 } = transition.action()
    else {
        panic!("expected the calibration-path write");
    };
    transition
        .advance(XtalDutyCalibrationCompletion::CalibrationPathDisabled { field })
        .unwrap();

    drive_pass(&mut transition, 0x988, initial_duty, 0);
    drive_pass(&mut transition, 0x9b0, initial_duty, 0x35);

    assert_eq!(
        transition.action(),
        XtalDutyCalibrationAction::Complete(XtalDutyCalibrationOutcome {
            initial_duty,
            low_frequency: XtalDutyPassOutcome {
                frequency_code: 0x988,
                best_candidate: 0x35,
                best_filtered_power: 0x4b * 0x4b,
            },
            high_frequency: XtalDutyPassOutcome {
                frequency_code: 0x9b0,
                best_candidate: 0x35,
                best_filtered_power: 0x4b * 0x4b,
            },
        })
    );
}

#[test]
fn crystal_search_window_tracks_seed_boundaries_and_stays_in_hardware_domain() {
    for (seed, first, last) in [
        (0, 0, 23),
        (11, 11, 34),
        (12, 0, 23),
        (47, 35, 58),
        (48, 24, 47),
        (63, 39, 62),
    ] {
        let mut transition = XtalDutySearchTransition::from_initial_duty(seed);
        let mut candidates = std::vec::Vec::new();
        loop {
            match transition.action() {
                XtalDutySearchAction::WriteCandidate { address, candidate } => {
                    candidates.push(candidate);
                    transition
                        .advance(XtalDutySearchCompletion::CandidateWritten { address, candidate })
                        .unwrap();
                }
                XtalDutySearchAction::DelayMicros { candidate, .. } => {
                    transition
                        .advance(XtalDutySearchCompletion::DelayElapsed { candidate })
                        .unwrap();
                }
                XtalDutySearchAction::SignalPower(_) => {
                    complete_search_measurement(&mut transition, 64)
                }
                XtalDutySearchAction::Complete(_) => break,
                action => panic!("unexpected search action: {action:?}"),
            }
        }
        assert_eq!(candidates.first(), Some(&first));
        assert_eq!(candidates.last(), Some(&last));
        assert_eq!(candidates.len(), 24);
        assert!(candidates.windows(2).all(|pair| pair[1] == pair[0] + 1));
        assert!(candidates.iter().all(|candidate| *candidate < 64));
    }
}
