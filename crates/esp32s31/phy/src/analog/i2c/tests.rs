use super::{
    AdcRateAction, AdcRateCompletion, FilterDcapAction, FilterDcapCompletion, FilterDcapParameters,
    FilterDcapTransition, FilterDcapTransitionError, I2cInit1Action, I2cInit1Completion,
    I2cInit1Transition, I2cInit1TransitionError, OpenI2cXpdAction, OpenI2cXpdCompletion,
    OpenI2cXpdOutcome, OpenI2cXpdTransition, OpenI2cXpdTransitionError, PhyRfInitParameterSnapshot,
    PhyRfInitPrefixAction, PhyRfInitPrefixCompletion, PhyRfInitPrefixOutcome, PhyRfInitPrefixStep,
    PhyRfInitPrefixTransition, PhyRfInitPrefixTransitionError, RcCalibrationAction,
    RcCalibrationCompletion, RcCalibrationTransition, RcCalibrationTransitionError,
    RfpllChargePumpAction, RfpllChargePumpCompletion, RfpllChargePumpOutcome,
    RfpllChargePumpTransition, analog_registers,
};
use crate::analog::frequency::{
    PhyChannelFrequencyInitAction, PhyChannelFrequencyInitCompletion,
    PhyChannelFrequencyInitControl, PhyFrequencyI2cAction, PhyFrequencyI2cCompletion,
};
use crate::calibration::cold::PhyColdExternalBinding;
use crate::calibration::estimator::{
    PhyDcIqAccumulatorSnapshot, PhyDcIqAction, PhyDcIqCompletion, PhyDcIqReadinessSnapshot,
};

use crate::analog::crystal_duty::{
    XtalDutyCalibrationAction, XtalDutyCalibrationCompletion, XtalDutyCalibrationOutcome,
    XtalDutyCalibrationParameters, XtalDutyPassAction, XtalDutyPassCompletion, XtalDutyPassOutcome,
    XtalDutyPrepareAction, XtalDutyPrepareCompletion, XtalDutyRestoreAction,
    XtalDutyRestoreCompletion, XtalDutySearchAction, XtalDutySearchCompletion,
};
use crate::analog::pbus::{PhyPbusClearAction, PhyPbusClearCompletion, PhyPbusForceTest};
use crate::analog::rfpll::{RfpllFrequencyAction, RfpllFrequencyCompletion};
use crate::rx::dc_offset::{PhyRxDcoAction, PhyRxDcoCompletion};
use crate::rx::signal_power::{
    PhySignalPowerAccumulatorSnapshot, PhySignalPowerAction, PhySignalPowerCompletion,
};
use oer_esp32s31_hal::phy::i2c::PhyAdcRate;

fn complete_dc_iq(action: PhyDcIqAction) -> PhyDcIqCompletion {
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

fn complete_signal_power(action: PhySignalPowerAction, component: i32) -> PhySignalPowerCompletion {
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
            let shift = u32::from(request.shift.wrapping_sub(2)) & 0x1f;
            PhySignalPowerCompletion::AccumulatorsRead {
                request,
                snapshot: PhySignalPowerAccumulatorSnapshot {
                    sum_i: component.wrapping_shl(shift),
                    difference_i: 0,
                    difference_q: 0,
                    sum_q: 0,
                },
            }
        }
        action => panic!("unexpected terminal signal-power action: {action:?}"),
    }
}

fn complete_rx_dco(action: PhyRxDcoAction) -> PhyRxDcoCompletion {
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
        PhyRxDcoAction::DcIq(action) => PhyRxDcoCompletion::DcIq(complete_dc_iq(action)),
        PhyRxDcoAction::RestoreRxDcoControl => PhyRxDcoCompletion::RxDcoControlRestored,
        action => panic!("unexpected terminal RX-DCO action: {action:?}"),
    }
}

fn complete_rfpll(
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

fn complete_xtal_prepare(
    action: XtalDutyPrepareAction,
    rfpll_cap_status_reads: &mut u8,
) -> XtalDutyPrepareCompletion {
    match action {
        XtalDutyPrepareAction::Rfpll(action) => {
            XtalDutyPrepareCompletion::Rfpll(complete_rfpll(action, rfpll_cap_status_reads))
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
            XtalDutyPrepareCompletion::RxDco(complete_rx_dco(action))
        }
        XtalDutyPrepareAction::RestoreRxDcoControl => {
            XtalDutyPrepareCompletion::RxDcoControlRestored
        }
        action => panic!("unexpected terminal preparation action: {action:?}"),
    }
}

fn complete_xtal_restore(action: XtalDutyRestoreAction) -> XtalDutyRestoreCompletion {
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

fn drive_rf_init_xtal_duty(
    transition: &mut PhyRfInitPrefixTransition,
    initial_duty: u8,
    mut inject: impl FnMut(PhyRfInitPrefixAction) -> Option<PhyRfInitPrefixCompletion>,
) -> Result<XtalDutyCalibrationOutcome, crate::analog::crystal_duty::XtalDutyFailure> {
    let mut current_candidate = None;
    let mut duty_writes = 0;
    let mut rfpll_cap_status_reads = 0;
    loop {
        let outer_action = transition.action();
        if !matches!(
            outer_action,
            PhyRfInitPrefixAction::Complete(_)
                | PhyRfInitPrefixAction::CaptureChannelFrequencyControl
        ) {
            assert!(
                PhyColdExternalBinding::lower(outer_action).is_ok(),
                "reachable crystal-duty action has no external lowering: {outer_action:?}"
            );
        }
        if let Some(completion) = inject(outer_action) {
            transition.advance(completion).unwrap();
            continue;
        }
        match outer_action {
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::ReadInitialDuty {
                field,
            }) => {
                transition
                    .advance(PhyRfInitPrefixCompletion::XtalDuty(
                        XtalDutyCalibrationCompletion::InitialDutyRead {
                            field,
                            value: initial_duty,
                        },
                    ))
                    .unwrap();
            }
            PhyRfInitPrefixAction::XtalDuty(
                XtalDutyCalibrationAction::DisableCalibrationPath { field, .. },
            ) => {
                transition
                    .advance(PhyRfInitPrefixCompletion::XtalDuty(
                        XtalDutyCalibrationCompletion::CalibrationPathDisabled { field },
                    ))
                    .unwrap();
            }
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::WriteMasked { field, .. },
            )) => {
                transition
                    .advance(PhyRfInitPrefixCompletion::XtalDuty(
                        XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::MaskedWrite {
                            field,
                        }),
                    ))
                    .unwrap();
            }
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::WriteByte { address, value },
            )) => {
                assert_eq!(value, [initial_duty, 0, initial_duty, 0x35][duty_writes]);
                duty_writes += 1;
                transition
                    .advance(PhyRfInitPrefixCompletion::XtalDuty(
                        XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::ByteWrite {
                            address,
                        }),
                    ))
                    .unwrap();
            }
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(action),
            )) => {
                transition
                    .advance(PhyRfInitPrefixCompletion::XtalDuty(
                        XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                            complete_xtal_prepare(action, &mut rfpll_cap_status_reads),
                        )),
                    ))
                    .unwrap();
            }
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::WriteCandidate {
                    address,
                    candidate,
                }),
            )) => {
                current_candidate = Some(candidate);
                transition
                    .advance(PhyRfInitPrefixCompletion::XtalDuty(
                        XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                            XtalDutySearchCompletion::CandidateWritten { address, candidate },
                        )),
                    ))
                    .unwrap();
            }
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::DelayMicros {
                    candidate,
                    micros: 20,
                }),
            )) => {
                assert_eq!(current_candidate, Some(candidate));
                transition
                    .advance(PhyRfInitPrefixCompletion::XtalDuty(
                        XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                            XtalDutySearchCompletion::DelayElapsed { candidate },
                        )),
                    ))
                    .unwrap();
            }
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(action)),
            )) => {
                let candidate = current_candidate.unwrap();
                transition
                    .advance(PhyRfInitPrefixCompletion::XtalDuty(
                        XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                            XtalDutySearchCompletion::SignalPower(complete_signal_power(
                                action,
                                i32::from(0x80 - candidate),
                            )),
                        )),
                    ))
                    .unwrap();
            }
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Restore(action),
            )) => {
                transition
                    .advance(PhyRfInitPrefixCompletion::XtalDuty(
                        XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                            complete_xtal_restore(action),
                        )),
                    ))
                    .unwrap();
            }
            PhyRfInitPrefixAction::CaptureChannelFrequencyControl => {
                if let PhyRfInitPrefixStep::ChannelFrequencyControl { xtal_duty, .. } =
                    transition.step
                {
                    return Ok(xtal_duty);
                }
                panic!("front-end update action without its owned step");
            }
            PhyRfInitPrefixAction::Complete(
                PhyRfInitPrefixOutcome::ChannelFrequencyInitialized { xtal_duty, .. },
            ) => return Ok(xtal_duty),
            PhyRfInitPrefixAction::Complete(PhyRfInitPrefixOutcome::XtalDutyCalibrationFailed(
                failure,
            )) => return Err(failure),
            action => panic!("unexpected RF-init crystal-duty action: {action:?}"),
        }
    }
}

fn drive_warm_channel_frequency(transition: &mut PhyRfInitPrefixTransition) {
    loop {
        let completion = match transition.action() {
            PhyRfInitPrefixAction::ChannelFrequency(
                PhyChannelFrequencyInitAction::ConfigureFrequencyRegisters { parameter_override },
            ) => PhyChannelFrequencyInitCompletion::FrequencyRegistersConfigured {
                parameter_override,
            },
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::I2c(action)) => {
                let completion = match action {
                    PhyFrequencyI2cAction::WriteMasked { field, .. } => {
                        PhyFrequencyI2cCompletion::MaskedWrite { field }
                    }
                    PhyFrequencyI2cAction::ReadByte { address } => {
                        let value = if address
                            == analog_registers::RFPLL_CAPACITOR_SEARCH_ENABLE.address()
                        {
                            0x5a
                        } else if address == analog_registers::RFPLL_SDM_UPDATE_ENABLE.address() {
                            0x8f
                        } else {
                            0x10
                        };
                        PhyFrequencyI2cCompletion::ByteRead { address, value }
                    }
                    PhyFrequencyI2cAction::WriteMemory {
                        descriptor_index,
                        copy_index,
                        address,
                        ..
                    } => PhyFrequencyI2cCompletion::MemoryWrite {
                        descriptor_index,
                        copy_index,
                        address,
                    },
                    PhyFrequencyI2cAction::ConfigureNumberAddresses(addresses) => {
                        PhyFrequencyI2cCompletion::NumberAddressesConfigured(addresses)
                    }
                    action => panic!("unexpected terminal frequency-I2C action: {action:?}"),
                };
                PhyChannelFrequencyInitCompletion::I2c(completion)
            }
            PhyRfInitPrefixAction::Complete(_) => return,
            action => panic!("unexpected warm channel-frequency action: {action:?}"),
        };
        transition
            .advance(PhyRfInitPrefixCompletion::ChannelFrequency(completion))
            .unwrap();
    }
}

#[test]
fn xtal_child_timeouts_reach_the_rf_parent_in_both_frequency_passes() {
    use crate::analog::crystal_duty::{XtalDutyFailure, XtalDutyHardwareFailure};
    use crate::calibration::estimator::PhyDcIqFailure;
    use crate::rx::{dc_offset::PhyRxDcoFailure, signal_power::PhySignalPowerFailure};

    for failing_pass in [1, 2] {
        for failing_phase in ["prepare", "search", "restore"] {
            let mut transition = PhyRfInitPrefixTransition::new();
            transition.step = PhyRfInitPrefixStep::XtalDutyParameters {
                parameter: PhyRfInitParameterSnapshot::new(
                    FilterDcapParameters::new(0x12, 0x34, 0x3a, 0x56, 0x87),
                    0xaa,
                ),
                rfpll_lock_observed: true,
                sar2_reinitialized: true,
            };
            transition
                .advance(PhyRfInitPrefixCompletion::XtalDutyParametersCaptured(
                    XtalDutyCalibrationParameters {
                        restore_duty: 0,
                        rf_frequency_offset_base: 0x31,
                        pbus_rx_path_value: 0x42,
                    },
                ))
                .unwrap();

            let mut pass = 0;
            let mut expected = None;
            let mut dco_restored_after_timeout = false;
            let result = drive_rf_init_xtal_duty(&mut transition, 0x2a, |action| {
                let PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(action)) =
                    action
                else {
                    return None;
                };
                if matches!(action, XtalDutyPassAction::WriteMasked { .. }) {
                    pass += 1;
                }
                if expected.is_some() {
                    if matches!(
                        action,
                        XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RestoreRxDcoControl)
                    ) {
                        dco_restored_after_timeout = true;
                    }
                    return None;
                }
                if pass != failing_pass {
                    return None;
                }
                let completion = match (failing_phase, action) {
                    (
                        "prepare",
                        XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                            PhyRxDcoAction::DcIq(PhyDcIqAction::AwaitReadinessEdge {
                                request, ..
                            }),
                        )),
                    ) => {
                        expected = Some(XtalDutyFailure::Prepare(XtalDutyHardwareFailure::RxDco(
                            PhyRxDcoFailure::DcIq(PhyDcIqFailure::ReadinessTimedOut {
                                request,
                                readiness_activity_edges: 0,
                            }),
                        )));
                        XtalDutyPassCompletion::Prepare(XtalDutyPrepareCompletion::RxDco(
                            PhyRxDcoCompletion::DcIq(PhyDcIqCompletion::ReadinessTimedOut(request)),
                        ))
                    }
                    (
                        "search",
                        XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                            PhySignalPowerAction::AwaitReadinessEdge { request, .. },
                        )),
                    ) => {
                        expected = Some(XtalDutyFailure::Search(
                            PhySignalPowerFailure::ReadinessTimedOut {
                                request,
                                readiness_activity_edges: 0,
                            },
                        ));
                        XtalDutyPassCompletion::Search(XtalDutySearchCompletion::SignalPower(
                            PhySignalPowerCompletion::ReadinessTimedOut(request),
                        ))
                    }
                    (
                        "restore",
                        XtalDutyPassAction::Restore(XtalDutyRestoreAction::ForcePbus(transaction)),
                    ) => {
                        expected = Some(XtalDutyFailure::Restore(
                            XtalDutyHardwareFailure::PbusForceTestTimedOut(transaction),
                        ));
                        XtalDutyPassCompletion::Restore(
                            XtalDutyRestoreCompletion::PbusForceTimedOut(transaction),
                        )
                    }
                    _ => return None,
                };
                Some(PhyRfInitPrefixCompletion::XtalDuty(
                    XtalDutyCalibrationCompletion::Pass(completion),
                ))
            });
            let failure = expected.expect("the selected child must reach its deadline boundary");
            assert_eq!(result, Err(failure), "{failing_phase}, pass {failing_pass}");
            assert_eq!(pass, failing_pass, "failure must not start the next pass");
            if failing_phase == "prepare" {
                assert!(dco_restored_after_timeout);
            }
            let terminal = PhyRfInitPrefixAction::Complete(
                PhyRfInitPrefixOutcome::XtalDutyCalibrationFailed(failure),
            );
            assert_eq!(transition.action(), terminal);
            assert_eq!(
                transition.advance(PhyRfInitPrefixCompletion::FrontEndRegisterUpdateConfigured),
                Err(PhyRfInitPrefixTransitionError::AlreadyComplete),
            );
            assert_eq!(transition.action(), terminal);
        }
    }
}

#[test]
fn rc_calibration_plan_has_only_explicit_async_edges() {
    let mut transition = RcCalibrationTransition::new();
    for _ in 0..4 {
        assert!(matches!(
            transition.action(),
            RcCalibrationAction::WriteMasked { .. }
        ));
        transition.advance(RcCalibrationCompletion::Write).unwrap();
    }
    assert_eq!(transition.action(), RcCalibrationAction::DelayMicros(100));
    assert_eq!(
        transition.advance(RcCalibrationCompletion::Write),
        Err(RcCalibrationTransitionError::WrongCompletion)
    );
    transition.advance(RcCalibrationCompletion::Delay).unwrap();
    assert!(matches!(
        transition.action(),
        RcCalibrationAction::ReadMasked {
            field: analog_registers::RC_CALIBRATION_RESULT,
        }
    ));
    transition
        .advance(RcCalibrationCompletion::Read(0x2d))
        .unwrap();
    transition.advance(RcCalibrationCompletion::Write).unwrap();
    transition.advance(RcCalibrationCompletion::Write).unwrap();
    assert_eq!(transition.action(), RcCalibrationAction::ApplyResult(0x2d));
    transition
        .advance(RcCalibrationCompletion::Applied)
        .unwrap();
    assert_eq!(transition.action(), RcCalibrationAction::Complete);
    assert_eq!(
        transition.advance(RcCalibrationCompletion::Applied),
        Err(RcCalibrationTransitionError::AlreadyComplete)
    );
}

#[test]
fn masked_i2c_write_owns_read_transform_and_write_edges() {
    let field = analog_registers::WIFI_TX_TEMPERATURE_TRACKING_0;
    let address = field.address();
    let mut transition = crate::analog::i2c::field_write(field, 3);
    assert_eq!(
        transition.action(),
        oer_radio_analog::Action::Read { address }
    );
    assert_eq!(
        transition.advance(oer_radio_analog::Completion::Written { address }),
        Err(oer_radio_analog::TransitionError::WrongCompletion)
    );
    transition
        .advance(oer_radio_analog::Completion::Read {
            address,
            value: 0x0f,
        })
        .unwrap();
    assert_eq!(
        transition.action(),
        oer_radio_analog::Action::Write {
            address,
            value: 0x03,
        }
    );
    transition
        .advance(oer_radio_analog::Completion::Written { address })
        .unwrap();
    assert_eq!(transition.action(), oer_radio_analog::Action::Complete(()));
}

#[test]
fn filter_dcap_exposes_one_semantic_operation() {
    let parameter = FilterDcapParameters::new(0x12, 0x34, 0x3a, 0x56, 0x87);
    let mut transition = FilterDcapTransition::new(parameter);
    assert_eq!(transition.action(), FilterDcapAction::Configure(parameter));
    transition
        .advance(FilterDcapCompletion::Configured)
        .unwrap();
    assert_eq!(transition.action(), FilterDcapAction::Complete);
    assert_eq!(
        transition.advance(FilterDcapCompletion::Configured),
        Err(FilterDcapTransitionError::AlreadyComplete)
    );
}

#[test]
fn i2c_init1_exposes_one_semantic_operation() {
    let filter = FilterDcapParameters::new(1, 2, 3, 0xfe, 5);
    let parameter = PhyRfInitParameterSnapshot::new(filter, 0x55);
    let mut transition = I2cInit1Transition::new(parameter);
    assert_eq!(transition.action(), I2cInit1Action::Configure(parameter));
    transition.advance(I2cInit1Completion::Configured).unwrap();
    assert_eq!(transition.action(), I2cInit1Action::Complete);
    assert_eq!(
        transition.advance(I2cInit1Completion::Configured),
        Err(I2cInit1TransitionError::AlreadyComplete)
    );
    assert_eq!(parameter.parameter_18e(), 0x55);
    assert_eq!(parameter.filter_dcap(), filter);
}

fn complete_rfpll_initial_writes(transition: &mut RfpllChargePumpTransition) {
    for (field, value) in [
        (analog_registers::RFPLL_CHARGE_PUMP_CALIBRATION_ENABLE, 0),
        (analog_registers::RFPLL_CHARGE_PUMP_CALIBRATION_PULSE, 0),
        (analog_registers::RFPLL_CHARGE_PUMP_CALIBRATION_PULSE, 1),
    ] {
        assert_eq!(
            transition.action(),
            RfpllChargePumpAction::WriteMasked { field, value }
        );
        transition
            .advance(RfpllChargePumpCompletion::Write)
            .unwrap();
    }
}

#[test]
fn rfpll_charge_pump_lock_path_uses_async_delay_and_owned_result() {
    let mut transition = RfpllChargePumpTransition::new();
    complete_rfpll_initial_writes(&mut transition);
    assert_eq!(transition.action(), RfpllChargePumpAction::DelayMicros(20));
    transition
        .advance(RfpllChargePumpCompletion::Delay)
        .unwrap();
    assert_eq!(
        transition.action(),
        RfpllChargePumpAction::ReadMasked {
            field: analog_registers::RFPLL_CHARGE_PUMP_LOCK_STATUS,
        }
    );
    transition
        .advance(RfpllChargePumpCompletion::ReadMasked(1))
        .unwrap();
    assert_eq!(
        transition.action(),
        RfpllChargePumpAction::ReadMasked {
            field: analog_registers::RFPLL_CHARGE_PUMP_RESULT,
        }
    );
    transition
        .advance(RfpllChargePumpCompletion::ReadMasked(12))
        .unwrap();
    assert_eq!(
        transition.action(),
        RfpllChargePumpAction::WriteMasked {
            field: analog_registers::RFPLL_CHARGE_PUMP_CALIBRATION_ENABLE,
            value: 1,
        }
    );
    transition
        .advance(RfpllChargePumpCompletion::Write)
        .unwrap();
    assert_eq!(
        transition.action(),
        RfpllChargePumpAction::WriteMasked {
            field: analog_registers::RFPLL_CHARGE_PUMP_VALUE,
            value: 23,
        }
    );
    transition
        .advance(RfpllChargePumpCompletion::Write)
        .unwrap();
    let final_address = analog_registers::RFPLL_CHARGE_PUMP_VALUE.address();
    assert_eq!(
        transition.action(),
        RfpllChargePumpAction::ReadByte {
            address: final_address,
        }
    );
    transition
        .advance(RfpllChargePumpCompletion::ReadByte {
            address: final_address,
            value: 0xaa,
        })
        .unwrap();
    assert_eq!(
        transition.action(),
        RfpllChargePumpAction::Complete(RfpllChargePumpOutcome {
            parameter_18e: 0xaa,
            lock_observed: true,
        })
    );
}

#[test]
fn rfpll_charge_pump_final_miss_is_data_not_a_blocking_print() {
    let mut transition = RfpllChargePumpTransition::new();
    complete_rfpll_initial_writes(&mut transition);
    for attempt in 0..100 {
        assert_eq!(transition.action(), RfpllChargePumpAction::DelayMicros(20));
        transition
            .advance(RfpllChargePumpCompletion::Delay)
            .unwrap();
        transition
            .advance(RfpllChargePumpCompletion::ReadMasked(0))
            .unwrap();
        if attempt != 99 {
            assert_eq!(transition.action(), RfpllChargePumpAction::DelayMicros(20));
        }
    }
    assert_eq!(
        transition.action(),
        RfpllChargePumpAction::ReadMasked {
            field: analog_registers::RFPLL_CHARGE_PUMP_RESULT,
        }
    );
    transition
        .advance(RfpllChargePumpCompletion::ReadMasked(31))
        .unwrap();
    transition
        .advance(RfpllChargePumpCompletion::Write)
        .unwrap();
    transition
        .advance(RfpllChargePumpCompletion::Write)
        .unwrap();
    let final_address = analog_registers::RFPLL_CHARGE_PUMP_VALUE.address();
    transition
        .advance(RfpllChargePumpCompletion::ReadByte {
            address: final_address,
            value: 0xbb,
        })
        .unwrap();
    assert_eq!(
        transition.action(),
        RfpllChargePumpAction::Complete(RfpllChargePumpOutcome {
            parameter_18e: 0xbb,
            lock_observed: false,
        })
    );
}

#[test]
fn rf_init_prefix_composes_mmio_i2c_and_timer_edges_in_vendor_order() {
    let mut transition = PhyRfInitPrefixTransition::new();

    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::ConfigureFeBbClock
    );
    assert_eq!(
        transition.advance(PhyRfInitPrefixCompletion::BbpllCalibrationConfigured),
        Err(PhyRfInitPrefixTransitionError::WrongCompletion)
    );
    transition
        .advance(PhyRfInitPrefixCompletion::FeBbClockConfigured)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::ConfigureBbpllCalibration { enabled: true }
    );
    transition
        .advance(PhyRfInitPrefixCompletion::BbpllCalibrationConfigured)
        .unwrap();

    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::ConfigureBiasRegisters
    );
    transition
        .advance(PhyRfInitPrefixCompletion::BiasRegistersConfigured)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::ConfigurePreDelay)
    );

    transition
        .advance(PhyRfInitPrefixCompletion::OpenI2cXpd(
            OpenI2cXpdCompletion::PreDelayConfigured,
        ))
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::DelayMicros(100))
    );
    transition
        .advance(PhyRfInitPrefixCompletion::OpenI2cXpd(
            OpenI2cXpdCompletion::DelayElapsed,
        ))
        .unwrap();
    transition
        .advance(PhyRfInitPrefixCompletion::OpenI2cXpd(
            OpenI2cXpdCompletion::PowerAndPulseConfigured {
                started_at_cycle: 0x1234_5678,
            },
        ))
        .unwrap();
    transition
        .advance(PhyRfInitPrefixCompletion::OpenI2cXpd(
            OpenI2cXpdCompletion::DeadlineObserved { expired: false },
        ))
        .unwrap();
    transition
        .advance(PhyRfInitPrefixCompletion::OpenI2cXpd(
            OpenI2cXpdCompletion::SdmSample(0x5b),
        ))
        .unwrap();

    assert_eq!(transition.action(), PhyRfInitPrefixAction::DelayMicros(10));
    transition
        .advance(PhyRfInitPrefixCompletion::DelayElapsed)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ConfigureDebugMode)
    );
    transition
        .advance(PhyRfInitPrefixCompletion::PbusClear(
            PhyPbusClearCompletion::DebugModeConfigured,
        ))
        .unwrap();
    for transaction in [
        PhyPbusForceTest::new(4, 1, 0),
        PhyPbusForceTest::new(4, 2, 0),
        PhyPbusForceTest::new(5, 1, 0),
        PhyPbusForceTest::new(5, 2, 0),
        PhyPbusForceTest::new(0, 1, 0),
        PhyPbusForceTest::new(0, 2, 0),
        PhyPbusForceTest::new(1, 1, 0),
        PhyPbusForceTest::new(1, 2, 0),
        PhyPbusForceTest::new(2, 1, 0x100),
        PhyPbusForceTest::new(3, 1, 0x100),
        PhyPbusForceTest::new(2, 2, 0x100),
        PhyPbusForceTest::new(3, 2, 0x100),
    ] {
        transition
            .advance(PhyRfInitPrefixCompletion::PbusClear(
                PhyPbusClearCompletion::ForceTestCompleted(transaction),
            ))
            .unwrap();
    }
    transition
        .advance(PhyRfInitPrefixCompletion::PbusClear(
            PhyPbusClearCompletion::WorkModeConfigured {
                settle_required: false,
            },
        ))
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::ConfigureI2cClockSelection
    );
    transition
        .advance(PhyRfInitPrefixCompletion::I2cClockSelectionConfigured)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::ConfigureI2cBbpll
    );
    transition
        .advance(PhyRfInitPrefixCompletion::I2cBbpllConfigured)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::AdcRate(AdcRateAction::ConfigureI2c {
            rate: PhyAdcRate::High,
        })
    );
    transition
        .advance(PhyRfInitPrefixCompletion::AdcRate(
            AdcRateCompletion::I2cConfigured,
        ))
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::AdcRate(AdcRateAction::ConfigureMmio {
            rate: PhyAdcRate::High,
        })
    );
    transition
        .advance(PhyRfInitPrefixCompletion::AdcRate(
            AdcRateCompletion::MmioConfigured,
        ))
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::ConfigureI2cMasterRegisters
    );
    transition
        .advance(PhyRfInitPrefixCompletion::I2cMasterRegistersConfigured)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::ConfigurePowerDetectorRegisters
    );
    transition
        .advance(PhyRfInitPrefixCompletion::PowerDetectorRegistersConfigured)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::ConfigureFrontEndRegisters
    );
    transition
        .advance(PhyRfInitPrefixCompletion::FrontEndRegistersConfigured)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::ConfigureTemperatureSensorRead
    );
    transition
        .advance(PhyRfInitPrefixCompletion::TemperatureSensorReadConfigured)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::ConfigureTxPowerControlBackground
    );
    transition
        .advance(PhyRfInitPrefixCompletion::TxPowerControlBackgroundConfigured)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::ConfigureRcCalibrationSettings
    );
    transition
        .advance(PhyRfInitPrefixCompletion::RcCalibrationSettingsConfigured)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::InspectRcCalibrationState
    );

    let mut already_calibrated = transition;
    already_calibrated
        .advance(PhyRfInitPrefixCompletion::RcCalibrationStateInspected {
            already_complete: true,
        })
        .unwrap();
    assert_eq!(
        already_calibrated.action(),
        PhyRfInitPrefixAction::CaptureFilterDcapParameters
    );

    transition
        .advance(PhyRfInitPrefixCompletion::RcCalibrationStateInspected {
            already_complete: false,
        })
        .unwrap();
    for expected in [
        RcCalibrationAction::WriteMasked {
            field: analog_registers::RC_CALIBRATION_DOUT_PATH_ENABLE,
            value: 1,
        },
        RcCalibrationAction::WriteMasked {
            field: analog_registers::RC_CALIBRATION_ENABLE,
            value: 1,
        },
        RcCalibrationAction::WriteMasked {
            field: analog_registers::RC_CALIBRATION_PULSE,
            value: 0,
        },
        RcCalibrationAction::WriteMasked {
            field: analog_registers::RC_CALIBRATION_PULSE,
            value: 1,
        },
    ] {
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::RcCalibration(expected)
        );
        transition
            .advance(PhyRfInitPrefixCompletion::RcCalibration(
                RcCalibrationCompletion::Write,
            ))
            .unwrap();
    }
    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::DelayMicros(100))
    );
    transition
        .advance(PhyRfInitPrefixCompletion::RcCalibration(
            RcCalibrationCompletion::Delay,
        ))
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::ReadMasked {
            field: analog_registers::RC_CALIBRATION_RESULT,
        })
    );
    transition
        .advance(PhyRfInitPrefixCompletion::RcCalibration(
            RcCalibrationCompletion::Read(0x2d),
        ))
        .unwrap();
    for expected in [
        RcCalibrationAction::WriteMasked {
            field: analog_registers::RC_CALIBRATION_DOUT_PATH_ENABLE,
            value: 0,
        },
        RcCalibrationAction::WriteMasked {
            field: analog_registers::RC_CALIBRATION_ENABLE,
            value: 0,
        },
    ] {
        assert_eq!(
            transition.action(),
            PhyRfInitPrefixAction::RcCalibration(expected)
        );
        transition
            .advance(PhyRfInitPrefixCompletion::RcCalibration(
                RcCalibrationCompletion::Write,
            ))
            .unwrap();
    }
    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::ApplyResult(0x2d))
    );
    transition
        .advance(PhyRfInitPrefixCompletion::RcCalibration(
            RcCalibrationCompletion::Applied,
        ))
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::CaptureFilterDcapParameters
    );

    transition
        .advance(PhyRfInitPrefixCompletion::FilterDcapParametersCaptured(
            FilterDcapParameters::new(0x12, 0x34, 0x3a, 0x56, 0x87),
        ))
        .unwrap();
    assert!(matches!(
        transition.action(),
        PhyRfInitPrefixAction::FilterDcap(FilterDcapAction::Configure(_))
    ));
    transition
        .advance(PhyRfInitPrefixCompletion::FilterDcap(
            FilterDcapCompletion::Configured,
        ))
        .unwrap();
    let parameter_18e_address = analog_registers::RFPLL_CHARGE_PUMP_VALUE.address();
    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::ReadParameter18e {
            address: parameter_18e_address,
        }
    );
    assert_eq!(
        transition.advance(PhyRfInitPrefixCompletion::Parameter18eRead {
            address: analog_registers::RFPLL_CHARGE_PUMP_RESULT.address(),
            value: 0x55,
        }),
        Err(PhyRfInitPrefixTransitionError::WrongCompletion)
    );
    transition
        .advance(PhyRfInitPrefixCompletion::Parameter18eRead {
            address: parameter_18e_address,
            value: 0x55,
        })
        .unwrap();
    assert!(matches!(
        transition.action(),
        PhyRfInitPrefixAction::I2cInit1(I2cInit1Action::Configure(_))
    ));
    transition
        .advance(PhyRfInitPrefixCompletion::I2cInit1(
            I2cInit1Completion::Configured,
        ))
        .unwrap();
    for _ in 0..3 {
        transition
            .advance(PhyRfInitPrefixCompletion::RfpllChargePump(
                RfpllChargePumpCompletion::Write,
            ))
            .unwrap();
    }
    transition
        .advance(PhyRfInitPrefixCompletion::RfpllChargePump(
            RfpllChargePumpCompletion::Delay,
        ))
        .unwrap();
    transition
        .advance(PhyRfInitPrefixCompletion::RfpllChargePump(
            RfpllChargePumpCompletion::ReadMasked(1),
        ))
        .unwrap();
    transition
        .advance(PhyRfInitPrefixCompletion::RfpllChargePump(
            RfpllChargePumpCompletion::ReadMasked(12),
        ))
        .unwrap();
    transition
        .advance(PhyRfInitPrefixCompletion::RfpllChargePump(
            RfpllChargePumpCompletion::Write,
        ))
        .unwrap();
    transition
        .advance(PhyRfInitPrefixCompletion::RfpllChargePump(
            RfpllChargePumpCompletion::Write,
        ))
        .unwrap();
    transition
        .advance(PhyRfInitPrefixCompletion::RfpllChargePump(
            RfpllChargePumpCompletion::ReadByte {
                address: parameter_18e_address,
                value: 0xaa,
            },
        ))
        .unwrap();
    let final_parameter = PhyRfInitParameterSnapshot::new(
        FilterDcapParameters::new(0x12, 0x34, 0x3a, 0x56, 0x87),
        0xaa,
    );
    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::ConfigureI2cMasterCommandMemory {
            parameter: final_parameter,
        }
    );
    transition
        .advance(PhyRfInitPrefixCompletion::I2cMasterCommandMemoryConfigured)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::ReadMasked69 {
            field: analog_registers::TEMPERATURE_SENSOR_SAR2_STATUS,
        }
    );
    let mut already_initialized = transition;
    already_initialized
        .advance(PhyRfInitPrefixCompletion::Masked69Read(1))
        .unwrap();
    assert_eq!(
        already_initialized.action(),
        PhyRfInitPrefixAction::ConfigureFrontEndRegisterUpdate
    );
    transition
        .advance(PhyRfInitPrefixCompletion::Masked69Read(0))
        .unwrap();
    assert_eq!(transition.action(), PhyRfInitPrefixAction::ConfigureSar2);
    transition
        .advance(PhyRfInitPrefixCompletion::Sar2Configured)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::ConfigureFrontEndRegisterUpdate
    );
    transition
        .advance(PhyRfInitPrefixCompletion::FrontEndRegisterUpdateConfigured)
        .unwrap();
    transition
        .advance(PhyRfInitPrefixCompletion::XtalDutyParametersCaptured(
            XtalDutyCalibrationParameters {
                restore_duty: 0,
                rf_frequency_offset_base: 0x31,
                pbus_rx_path_value: 0x42,
            },
        ))
        .unwrap();
    let xtal_duty = drive_rf_init_xtal_duty(&mut transition, 0x2a, |_| None).unwrap();
    assert_eq!(
        xtal_duty,
        XtalDutyCalibrationOutcome {
            initial_duty: 0x2a,
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
        }
    );

    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::CaptureChannelFrequencyControl
    );
    transition
        .advance(PhyRfInitPrefixCompletion::ChannelFrequencyControlCaptured(
            PhyChannelFrequencyInitControl {
                frequency_register_parameter_override: false,
                frequency_table_initialized: true,
                front_end_parameter_bit: false,
            },
        ))
        .unwrap();
    drive_warm_channel_frequency(&mut transition);
    let PhyRfInitPrefixAction::Complete(PhyRfInitPrefixOutcome::ChannelFrequencyInitialized {
        parameter,
        rfpll_lock_observed,
        sar2_reinitialized,
        xtal_duty: completed_xtal_duty,
        channel_frequency,
    }) = transition.action()
    else {
        panic!("RF init did not complete channel-frequency initialization");
    };
    assert_eq!(parameter, final_parameter);
    assert!(rfpll_lock_observed);
    assert!(sar2_reinitialized);
    assert_eq!(completed_xtal_duty, xtal_duty);
    assert!(channel_frequency.table_was_initialized);
    assert!(channel_frequency.table_is_initialized);
    assert_eq!(channel_frequency.calibration, None);
}

#[test]
fn rf_init_prefix_propagates_sdm_timeout_without_running_post_delay() {
    let mut transition = PhyRfInitPrefixTransition::new();
    transition
        .advance(PhyRfInitPrefixCompletion::FeBbClockConfigured)
        .unwrap();
    transition
        .advance(PhyRfInitPrefixCompletion::BbpllCalibrationConfigured)
        .unwrap();
    transition
        .advance(PhyRfInitPrefixCompletion::BiasRegistersConfigured)
        .unwrap();
    transition
        .advance(PhyRfInitPrefixCompletion::OpenI2cXpd(
            OpenI2cXpdCompletion::PreDelayConfigured,
        ))
        .unwrap();
    transition
        .advance(PhyRfInitPrefixCompletion::OpenI2cXpd(
            OpenI2cXpdCompletion::DelayElapsed,
        ))
        .unwrap();
    transition
        .advance(PhyRfInitPrefixCompletion::OpenI2cXpd(
            OpenI2cXpdCompletion::PowerAndPulseConfigured {
                started_at_cycle: 0x1234_5678,
            },
        ))
        .unwrap();
    transition
        .advance(PhyRfInitPrefixCompletion::OpenI2cXpd(
            OpenI2cXpdCompletion::DeadlineObserved { expired: true },
        ))
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRfInitPrefixAction::Complete(PhyRfInitPrefixOutcome::SdmTimedOut)
    );
    assert_eq!(
        transition.advance(PhyRfInitPrefixCompletion::DelayElapsed),
        Err(PhyRfInitPrefixTransitionError::AlreadyComplete)
    );
}

#[test]
fn open_i2c_xpd_delayed_path_requires_explicit_async_completions() {
    let mut transition = OpenI2cXpdTransition::new(true);
    assert_eq!(transition.action(), OpenI2cXpdAction::ConfigurePreDelay);
    assert_eq!(
        transition.advance(OpenI2cXpdCompletion::DelayElapsed),
        Err(OpenI2cXpdTransitionError::WrongCompletion)
    );
    transition
        .advance(OpenI2cXpdCompletion::PreDelayConfigured)
        .unwrap();
    assert_eq!(transition.action(), OpenI2cXpdAction::DelayMicros(100));
    transition
        .advance(OpenI2cXpdCompletion::DelayElapsed)
        .unwrap();
    assert_eq!(
        transition.action(),
        OpenI2cXpdAction::ConfigurePowerAndPulse
    );
    transition
        .advance(OpenI2cXpdCompletion::PowerAndPulseConfigured {
            started_at_cycle: 0x1234_5678,
        })
        .unwrap();
    assert_eq!(
        transition.action(),
        OpenI2cXpdAction::CheckSdmDeadline {
            started_at_cycle: 0x1234_5678,
            maximum_cycles: 9_999
        }
    );
}

#[test]
fn open_i2c_xpd_samples_only_after_deadline_and_i2c_edges() {
    let mut transition = OpenI2cXpdTransition::new(false);
    transition
        .advance(OpenI2cXpdCompletion::PowerAndPulseConfigured {
            started_at_cycle: 0xffff_ff00,
        })
        .unwrap();
    transition
        .advance(OpenI2cXpdCompletion::DeadlineObserved { expired: false })
        .unwrap();
    assert_eq!(
        transition.action(),
        OpenI2cXpdAction::ReadSdmSample {
            address: analog_registers::RFPLL_SDM_UPDATE_ENABLE.address()
        }
    );

    transition
        .advance(OpenI2cXpdCompletion::SdmSample(0x42))
        .unwrap();
    assert_eq!(transition.samples(), 1);
    assert!(matches!(
        transition.action(),
        OpenI2cXpdAction::CheckSdmDeadline {
            started_at_cycle: 0xffff_ff00,
            ..
        }
    ));

    transition
        .advance(OpenI2cXpdCompletion::DeadlineObserved { expired: false })
        .unwrap();
    transition
        .advance(OpenI2cXpdCompletion::SdmSample(0x5b))
        .unwrap();
    assert_eq!(
        transition.action(),
        OpenI2cXpdAction::Complete(OpenI2cXpdOutcome::Stable)
    );
    assert_eq!(transition.samples(), 2);
    assert_eq!(
        transition.advance(OpenI2cXpdCompletion::SdmSample(0x5b)),
        Err(OpenI2cXpdTransitionError::AlreadyComplete)
    );
}

#[test]
fn open_i2c_xpd_deadline_is_a_terminal_outcome() {
    let mut transition = OpenI2cXpdTransition::new(false);
    transition
        .advance(OpenI2cXpdCompletion::PowerAndPulseConfigured {
            started_at_cycle: 7,
        })
        .unwrap();
    transition
        .advance(OpenI2cXpdCompletion::DeadlineObserved { expired: true })
        .unwrap();
    assert_eq!(
        transition.action(),
        OpenI2cXpdAction::Complete(OpenI2cXpdOutcome::TimedOut)
    );
    assert_eq!(transition.samples(), 0);
}
