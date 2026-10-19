use super::{
    PhyColdExternalBinding, PhyColdI2cAction, PhyColdI2cBinding, PhyColdI2cConfigurationBinding,
    PhyColdI2cObservation, PhyColdI2cOutcome, PhyColdI2cRequest, PhyColdI2cTransaction,
    PhyColdLoweringError, PhyColdMmioBinding, PhyColdObservationBinding, PhyColdObservationRequest,
    PhyColdObservationResult, PhyColdPbusAction, PhyColdPbusBinding, PhyColdPbusHardwareResult,
    PhyColdPbusObservation, PhyColdTimerBinding, phy_sdm_deadline_expired,
};
use crate::analog::crystal_duty::{
    XtalDutyCalibrationAction, XtalDutyCalibrationCompletion, XtalDutyPassAction,
    XtalDutyPassCompletion, XtalDutyPrepareAction, XtalDutyPrepareCompletion,
    XtalDutyRestoreAction, XtalDutyRestoreCompletion, XtalDutySearchAction,
    XtalDutySearchCompletion,
};
use crate::analog::frequency::{PhyChannelFrequencyInitAction, PhyChannelFrequencyInitCompletion};
use crate::analog::i2c::{
    FilterDcapAction, FilterDcapParameters, I2cInit1Action, OpenI2cXpdAction, OpenI2cXpdCompletion,
    PhyI2cError, PhyRfInitParameterSnapshot, PhyRfInitPrefixAction, PhyRfInitPrefixCompletion,
    RcCalibrationAction, RcCalibrationCompletion, analog_registers,
};
use crate::analog::pbus::{PhyPbusClearAction, PhyPbusClearCompletion, PhyPbusForceTest};
use crate::analog::rfpll::{RfpllFrequencyAction, RfpllFrequencyCompletion};
use crate::calibration::estimator::{
    PhyDcIqAccumulatorSnapshot, PhyDcIqAction, PhyDcIqCompletion, PhyDcIqDelayPhase,
    PhyDcIqEnablePhase, PhyDcIqEstimateRequest, PhyDcIqReadinessSnapshot,
};
use crate::rx::dc_offset::{PhyRxDcoAction, PhyRxDcoCompletion};
use crate::rx::signal_power::{
    PhySignalPowerAccumulatorSnapshot, PhySignalPowerAction, PhySignalPowerClock,
    PhySignalPowerCompletion, PhySignalPowerRequest,
};
#[test]
fn busy_observation_preserves_await_state_without_self_progress() {
    let address = analog_registers::RFPLL_CAPACITOR_LOW;
    let mut transaction = PhyColdI2cTransaction::new(PhyColdI2cRequest::read_byte(address));
    assert_eq!(
        transaction.action(),
        PhyColdI2cAction::StartRead { address }
    );
    transaction.read_started().unwrap();
    let awaiting = PhyColdI2cAction::AwaitReadCompletionEdge { address };
    assert_eq!(transaction.action(), awaiting);

    assert_eq!(
        transaction.observe_read_result(Err(PhyI2cError::Busy)),
        Ok(PhyColdI2cObservation::StillPending)
    );
    assert_eq!(transaction.action(), awaiting);

    assert_eq!(
        transaction.observe_read_result(Ok(0xa5)),
        Ok(PhyColdI2cObservation::EdgeConsumed)
    );
    assert_eq!(
        transaction.action(),
        PhyColdI2cAction::Complete(PhyColdI2cOutcome::Read {
            address,
            value: 0xa5,
        })
    );
}

#[test]
fn masked_write_needs_two_distinct_external_edges() {
    let field = crate::analog::i2c::analog_registers::RC_CALIBRATION_ENABLE;
    let address = field.address();
    let request = PhyColdI2cRequest::write_field(field, 1);
    let mut transaction = PhyColdI2cTransaction::new(request);

    transaction.read_started().unwrap();
    transaction.observe_read_result(Ok(0xc3)).unwrap();
    assert_eq!(
        transaction.action(),
        PhyColdI2cAction::StartWrite {
            address,
            value: 0xc3,
        }
    );

    transaction.write_started().unwrap();
    assert_eq!(
        transaction.observe_write_result(Err(PhyI2cError::Busy)),
        Ok(PhyColdI2cObservation::StillPending)
    );
    assert_eq!(
        transaction.action(),
        PhyColdI2cAction::AwaitWriteCompletionEdge { address }
    );
    transaction.observe_write_result(Ok(())).unwrap();
    assert_eq!(
        transaction.action(),
        PhyColdI2cAction::Complete(PhyColdI2cOutcome::Written { address })
    );
}

#[test]
fn masked_outer_write_is_two_edges_but_one_identity_bound_completion() {
    let field = crate::analog::i2c::analog_registers::RC_CALIBRATION_ENABLE;
    let address = field.address();
    let outer_action =
        PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::WriteMasked { field, value: 1 });
    let mut binding = PhyColdI2cBinding::new(outer_action).unwrap();

    binding.read_started().unwrap();
    binding.observe_read_result(Ok(0x83)).unwrap();
    assert_eq!(
        binding.action(),
        PhyColdI2cAction::StartWrite {
            address,
            value: 0x83,
        }
    );
    binding.write_started().unwrap();
    assert_eq!(
        binding.observe_write_result(Err(PhyI2cError::Busy)),
        Ok(PhyColdI2cObservation::StillPending)
    );
    assert_eq!(
        binding.action(),
        PhyColdI2cAction::AwaitWriteCompletionEdge { address }
    );

    binding.observe_write_result(Ok(())).unwrap();
    assert_eq!(
        binding.into_completion(),
        Ok(PhyRfInitPrefixCompletion::RcCalibration(
            RcCalibrationCompletion::Write
        ))
    );
}

#[test]
fn non_i2c_outer_action_is_rejected_instead_of_becoming_a_fallback() {
    assert_eq!(
        PhyColdI2cBinding::new(PhyRfInitPrefixAction::ConfigureFeBbClock),
        Err(PhyColdLoweringError::UnsupportedAction)
    );
}

#[test]
fn finite_mmio_binding_preserves_dynamic_frequency_identity() {
    let outer_action = PhyRfInitPrefixAction::ChannelFrequency(
        PhyChannelFrequencyInitAction::ConfigureFrequencyRegisters {
            parameter_override: true,
        },
    );
    let binding = PhyColdMmioBinding::new(outer_action).unwrap();
    assert_eq!(binding.outer_action(), outer_action);
    assert_eq!(
        binding.into_completion(),
        Ok(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::FrequencyRegistersConfigured {
                parameter_override: true,
            }
        ))
    );

    assert_eq!(
        PhyColdMmioBinding::new(PhyRfInitPrefixAction::DelayMicros(10)),
        Err(PhyColdLoweringError::UnsupportedAction)
    );
}

#[test]
fn nested_calibration_mmio_keeps_every_parent_identity_field() {
    let tone_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
        XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ConfigureCalibrationTone {
            enabled: true,
            selector: 0x80,
            step: 0,
        }),
    ));
    assert_eq!(
        PhyColdMmioBinding::new(tone_action)
            .unwrap()
            .into_completion(),
        Ok(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::CalibrationToneConfigured {
                    enabled: true,
                    selector: 0x80,
                    step: 0,
                }
            ))
        ))
    );

    let dc_iq_request = PhyDcIqEstimateRequest {
        iteration: 4,
        chain: 1,
        control: 0x1234,
        mode: 2,
    };
    let dc_iq_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
        XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
            PhyDcIqAction::SetEnable {
                request: dc_iq_request,
                phase: PhyDcIqEnablePhase::Measurement,
                enabled: true,
            },
        ))),
    ));
    assert_eq!(
        PhyColdMmioBinding::new(dc_iq_action)
            .unwrap()
            .into_completion(),
        Ok(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                    PhyDcIqCompletion::EnableSet {
                        request: dc_iq_request,
                        phase: PhyDcIqEnablePhase::Measurement,
                        enabled: true,
                    }
                ))
            ))
        ))
    );

    let signal_request = PhySignalPowerRequest {
        measurement: 0x3a7,
        shift: 12,
    };
    let signal_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
        XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
            PhySignalPowerAction::ConfigureClock {
                request: signal_request,
                clock: PhySignalPowerClock::Rx,
                enabled: false,
            },
        )),
    ));
    assert_eq!(
        PhyColdMmioBinding::new(signal_action)
            .unwrap()
            .into_completion(),
        Ok(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                XtalDutySearchCompletion::SignalPower(PhySignalPowerCompletion::ClockConfigured {
                    request: signal_request,
                    clock: PhySignalPowerClock::Rx,
                    enabled: false,
                })
            ))
        ))
    );

    let restore_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
        XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigurePbusWorkModePulse),
    ));
    assert_eq!(
        PhyColdMmioBinding::new(restore_action)
            .unwrap()
            .into_completion(),
        Ok(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                XtalDutyRestoreCompletion::PbusWorkModePulseConfigured
            ))
        ))
    );
}

#[test]
fn timer_binding_consumes_one_exact_delay_edge() {
    let outer_action = PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::DelayMicros(100));
    let binding = PhyColdTimerBinding::new(outer_action).unwrap();
    assert_eq!(binding.outer_action(), outer_action);
    assert_eq!(binding.micros(), 100);
    assert_eq!(
        binding.into_elapsed_completion(),
        Ok(PhyRfInitPrefixCompletion::RcCalibration(
            RcCalibrationCompletion::Delay
        ))
    );

    assert_eq!(
        PhyColdTimerBinding::new(PhyRfInitPrefixAction::ConfigureFeBbClock),
        Err(PhyColdLoweringError::UnsupportedAction)
    );
}

#[test]
fn nested_calibration_timers_preserve_every_parent_identity_field() {
    let rfpll_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
        XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
            RfpllFrequencyAction::DelayMicros(20),
        )),
    ));
    let rfpll = PhyColdTimerBinding::new(rfpll_action).unwrap();
    assert_eq!(rfpll.micros(), 20);
    assert_eq!(
        rfpll.into_elapsed_completion(),
        Ok(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::Rfpll(RfpllFrequencyCompletion::DelayElapsed(20))
            ))
        ))
    );

    let rx_dco_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
        XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DelayMicros {
            iteration: 7,
            micros: 10,
        })),
    ));
    assert_eq!(
        PhyColdTimerBinding::new(rx_dco_action)
            .unwrap()
            .into_elapsed_completion(),
        Ok(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DelayElapsed {
                    iteration: 7,
                    micros: 10,
                })
            ))
        ))
    );

    let dc_iq_request = PhyDcIqEstimateRequest {
        iteration: 7,
        chain: 1,
        control: 0x1234,
        mode: 2,
    };
    let dc_iq_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
        XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
            PhyDcIqAction::DelayMicros {
                request: dc_iq_request,
                phase: PhyDcIqDelayPhase::Stop,
                micros: 1,
            },
        ))),
    ));
    assert_eq!(
        PhyColdTimerBinding::new(dc_iq_action)
            .unwrap()
            .into_elapsed_completion(),
        Ok(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                    PhyDcIqCompletion::DelayElapsed {
                        request: dc_iq_request,
                        phase: PhyDcIqDelayPhase::Stop,
                        micros: 1,
                    }
                ))
            ))
        ))
    );

    let search_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
        XtalDutyPassAction::Search(XtalDutySearchAction::DelayMicros {
            candidate: 0x3a,
            micros: 20,
        }),
    ));
    assert_eq!(
        PhyColdTimerBinding::new(search_action)
            .unwrap()
            .into_elapsed_completion(),
        Ok(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                XtalDutySearchCompletion::DelayElapsed { candidate: 0x3a }
            ))
        ))
    );

    let signal_request = PhySignalPowerRequest {
        measurement: 0x3a7,
        shift: 12,
    };
    let signal_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
        XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
            PhySignalPowerAction::DelayMicros {
                request: signal_request,
                phase: PhyDcIqDelayPhase::Start,
                micros: 1,
            },
        )),
    ));
    assert_eq!(
        PhyColdTimerBinding::new(signal_action)
            .unwrap()
            .into_elapsed_completion(),
        Ok(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                XtalDutySearchCompletion::SignalPower(PhySignalPowerCompletion::DelayElapsed {
                    request: signal_request,
                    phase: PhyDcIqDelayPhase::Start,
                    micros: 1,
                })
            ))
        ))
    );

    let restore_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
        XtalDutyPassAction::Restore(XtalDutyRestoreAction::DelayMicros(2)),
    ));
    assert_eq!(
        PhyColdTimerBinding::new(restore_action)
            .unwrap()
            .into_elapsed_completion(),
        Ok(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                XtalDutyRestoreCompletion::DelayElapsed { micros: 2 }
            ))
        ))
    );
}

#[test]
fn pbus_busy_result_preserves_one_owned_awaiting_edge() {
    let transaction = PhyPbusForceTest::new(4, 1, 0);
    let outer_action = PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ForceTest(transaction));
    let mut binding = PhyColdPbusBinding::new(outer_action).unwrap();
    assert_eq!(binding.action(), PhyColdPbusAction::Start(transaction));

    binding.started().unwrap();
    let awaiting = PhyColdPbusAction::AwaitCompletionEdge(transaction);
    assert_eq!(binding.action(), awaiting);
    assert_eq!(
        binding.observe_result(PhyColdPbusHardwareResult::Busy),
        Ok(PhyColdPbusObservation::StillPending)
    );
    assert_eq!(binding.action(), awaiting);

    assert_eq!(
        binding.observe_result(PhyColdPbusHardwareResult::Completed),
        Ok(PhyColdPbusObservation::EdgeConsumed)
    );
    assert_eq!(binding.action(), PhyColdPbusAction::Complete(transaction));
    assert_eq!(
        binding.into_completion(),
        Ok(PhyRfInitPrefixCompletion::PbusClear(
            PhyPbusClearCompletion::ForceTestCompleted(transaction)
        ))
    );
}

#[test]
fn pbus_timeout_consumes_the_exact_awaiting_transaction() {
    let transaction = PhyPbusForceTest::new(3, 2, 0x100);
    let outer_action = PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ForceTest(transaction));
    let mut binding = PhyColdPbusBinding::new(outer_action).unwrap();
    binding.started().unwrap();
    assert_eq!(
        binding.into_timeout_completion(),
        Ok(PhyRfInitPrefixCompletion::PbusClear(
            PhyPbusClearCompletion::ForceTestTimedOut(transaction)
        ))
    );
}

#[test]
fn nested_xtal_pbus_edges_return_to_the_exact_parent_transition() {
    let prepare_transaction = PhyPbusForceTest::new(0, 2, 0x42);
    let prepare_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
        XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ForcePbus(prepare_transaction)),
    ));
    let mut prepare = PhyColdPbusBinding::new(prepare_action).unwrap();
    prepare.started().unwrap();
    prepare
        .observe_result(PhyColdPbusHardwareResult::Completed)
        .unwrap();
    assert_eq!(
        prepare.into_completion(),
        Ok(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::PbusForceCompleted(prepare_transaction)
            ))
        ))
    );

    let rx_dco_transaction = PhyPbusForceTest::new(3, 1, 0x1ff);
    let rx_dco_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
        XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::ForcePbus(
            rx_dco_transaction,
        ))),
    ));
    let mut rx_dco = PhyColdPbusBinding::new(rx_dco_action).unwrap();
    rx_dco.started().unwrap();
    assert_eq!(
        rx_dco.into_timeout_completion(),
        Ok(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::PbusForceTimedOut(
                    rx_dco_transaction
                ))
            ))
        ))
    );

    let restore_transaction = PhyPbusForceTest::new(1, 2, 0);
    let restore_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
        XtalDutyPassAction::Restore(XtalDutyRestoreAction::ForcePbus(restore_transaction)),
    ));
    let mut restore = PhyColdPbusBinding::new(restore_action).unwrap();
    restore.started().unwrap();
    restore
        .observe_result(PhyColdPbusHardwareResult::Completed)
        .unwrap();
    assert_eq!(
        restore.into_completion(),
        Ok(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                XtalDutyRestoreCompletion::PbusForceCompleted(restore_transaction)
            ))
        ))
    );
}

#[test]
fn sampled_pbus_work_mode_is_bound_to_its_exact_parent() {
    let clear_action = PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ConfigureWorkMode);
    let clear = PhyColdObservationBinding::new(clear_action).unwrap();
    assert_eq!(clear.outer_action(), clear_action);
    assert_eq!(
        clear.request(),
        PhyColdObservationRequest::ConfigurePbusWorkMode
    );
    assert_eq!(
        clear.into_completion(PhyColdObservationResult::PbusWorkMode {
            settle_required: true,
        }),
        Ok(PhyRfInitPrefixCompletion::PbusClear(
            PhyPbusClearCompletion::WorkModeConfigured {
                settle_required: true
            }
        ))
    );

    let restore_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
        XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigurePbusWorkMode),
    ));
    let restore = PhyColdObservationBinding::new(restore_action).unwrap();
    assert_eq!(
        restore.into_completion(PhyColdObservationResult::PbusWorkMode {
            settle_required: false,
        }),
        Ok(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                XtalDutyRestoreCompletion::PbusWorkModeConfigured {
                    settle_required: false
                }
            ))
        ))
    );
}

#[test]
fn open_i2c_deadline_keeps_one_epoch_and_the_inclusive_rom_bound() {
    assert!(!phy_sdm_deadline_expired(100, 10_099, 9_999));
    assert!(phy_sdm_deadline_expired(100, 10_100, 9_999));
    assert!(!phy_sdm_deadline_expired(0xffff_ff00, 0x0000_260f, 9_999));
    assert!(phy_sdm_deadline_expired(0xffff_ff00, 0x0000_2610, 9_999));

    let configure_action =
        PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::ConfigurePowerAndPulse);
    let configure = PhyColdObservationBinding::new(configure_action).unwrap();
    assert_eq!(
        configure.request(),
        PhyColdObservationRequest::ConfigureOpenI2cPowerAndPulse
    );
    assert_eq!(
        configure.into_completion(PhyColdObservationResult::OpenI2cPowerAndPulse {
            started_at_cycle: 0xffff_ff00,
        }),
        Ok(PhyRfInitPrefixCompletion::OpenI2cXpd(
            OpenI2cXpdCompletion::PowerAndPulseConfigured {
                started_at_cycle: 0xffff_ff00
            }
        ))
    );

    let deadline_action = PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::CheckSdmDeadline {
        started_at_cycle: 0xffff_ff00,
        maximum_cycles: 9_999,
    });
    let deadline = PhyColdObservationBinding::new(deadline_action).unwrap();
    assert_eq!(
        deadline.request(),
        PhyColdObservationRequest::CheckOpenI2cSdmDeadline {
            started_at_cycle: 0xffff_ff00,
            maximum_cycles: 9_999,
        }
    );
    assert_eq!(
        deadline.into_completion(PhyColdObservationResult::OpenI2cSdmDeadline { expired: false }),
        Ok(PhyRfInitPrefixCompletion::OpenI2cXpd(
            OpenI2cXpdCompletion::DeadlineObserved { expired: false }
        ))
    );
}

#[test]
fn nested_external_edges_are_one_shot_semantic_bindings() {
    let prepare_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
        XtalDutyPassAction::Prepare(XtalDutyPrepareAction::PrepareRxDcoControlRestore),
    ));
    assert_eq!(
        PhyColdMmioBinding::new(prepare_action)
            .unwrap()
            .into_completion(),
        Ok(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::RxDcoControlRestorePrepared
            ))
        ))
    );

    let pbus_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
        XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::ReadPbus {
            selector: 1,
            path: 2,
        })),
    ));
    assert_eq!(
        PhyColdObservationBinding::new(pbus_action)
            .unwrap()
            .into_completion(PhyColdObservationResult::RxDcoPbusRead {
                selector: 1,
                path: 2,
                value: 0x1a5,
            }),
        Ok(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::PbusRead {
                    selector: 1,
                    path: 2,
                    value: 0x1a5,
                })
            ))
        ))
    );

    let dc_iq_request = PhyDcIqEstimateRequest {
        iteration: 6,
        chain: 1,
        control: 0x0fa0,
        mode: 0,
    };
    let dc_iq_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
        XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
            PhyDcIqAction::AwaitReadinessEdge {
                request: dc_iq_request,
                readiness_activity_edges: 3,
                readiness_samples: 5,
            },
        ))),
    ));
    assert_eq!(
        PhyColdObservationBinding::new(dc_iq_action)
            .unwrap()
            .into_completion(PhyColdObservationResult::DcIqReadiness {
                request: dc_iq_request,
                snapshot: PhyDcIqReadinessSnapshot {
                    ready: false,
                    activity: true,
                },
            }),
        Ok(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                    PhyDcIqCompletion::ReadinessObserved {
                        request: dc_iq_request,
                        snapshot: PhyDcIqReadinessSnapshot {
                            ready: false,
                            activity: true,
                        },
                    }
                ))
            ))
        ))
    );
    assert_eq!(
        PhyColdObservationBinding::new(dc_iq_action)
            .unwrap()
            .into_timeout_completion(),
        Ok(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                    PhyDcIqCompletion::ReadinessTimedOut(dc_iq_request)
                ))
            ))
        ))
    );

    let dc_iq_accumulators = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
        XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
            PhyDcIqAction::ReadAccumulators(dc_iq_request),
        ))),
    ));
    let dc_iq_snapshot = PhyDcIqAccumulatorSnapshot {
        i: -3,
        q: 7,
        power: 0x1234,
    };
    assert_eq!(
        PhyColdObservationBinding::new(dc_iq_accumulators)
            .unwrap()
            .into_completion(PhyColdObservationResult::DcIqAccumulators {
                request: dc_iq_request,
                snapshot: dc_iq_snapshot,
            }),
        Ok(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                    PhyDcIqCompletion::AccumulatorsRead {
                        request: dc_iq_request,
                        snapshot: dc_iq_snapshot,
                    }
                ))
            ))
        ))
    );

    let signal_request = PhySignalPowerRequest {
        measurement: 0x25,
        shift: 12,
    };
    let signal_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
        XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
            PhySignalPowerAction::ReadAccumulators(signal_request),
        )),
    ));
    let signal_snapshot = PhySignalPowerAccumulatorSnapshot {
        sum_i: 10,
        difference_i: -20,
        difference_q: 30,
        sum_q: -40,
    };
    assert_eq!(
        PhyColdObservationBinding::new(signal_action)
            .unwrap()
            .into_completion(PhyColdObservationResult::SignalPowerAccumulators {
                request: signal_request,
                snapshot: signal_snapshot,
            }),
        Ok(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                XtalDutySearchCompletion::SignalPower(PhySignalPowerCompletion::AccumulatorsRead {
                    request: signal_request,
                    snapshot: signal_snapshot,
                })
            ))
        ))
    );
}

#[test]
fn external_lowering_has_no_vendor_or_synchronous_fallback_variant() {
    assert!(matches!(
        PhyColdExternalBinding::lower(PhyRfInitPrefixAction::DelayMicros(10)),
        Ok(PhyColdExternalBinding::Timer(_))
    ));
    assert!(matches!(
        PhyColdExternalBinding::lower(PhyRfInitPrefixAction::ConfigureFrontEndRegisters),
        Ok(PhyColdExternalBinding::Mmio(_))
    ));
    assert!(matches!(
        PhyColdExternalBinding::lower(PhyRfInitPrefixAction::ConfigureBiasRegisters),
        Ok(PhyColdExternalBinding::I2cConfiguration(_))
    ));
    assert!(matches!(
        PhyColdExternalBinding::lower(PhyRfInitPrefixAction::ConfigureRcCalibrationSettings),
        Ok(PhyColdExternalBinding::I2cConfiguration(_))
    ));
    assert!(matches!(
        PhyColdExternalBinding::lower(PhyRfInitPrefixAction::ConfigureSar2),
        Ok(PhyColdExternalBinding::I2cConfiguration(_))
    ));

    let address = analog_registers::RFPLL_CAPACITOR_LOW;
    assert!(matches!(
        PhyColdExternalBinding::lower(PhyRfInitPrefixAction::ReadParameter18e { address }),
        Ok(PhyColdExternalBinding::I2c(_))
    ));
    let filter_parameters = FilterDcapParameters::new(1, 2, 3, 4, 5);
    assert!(matches!(
        PhyColdExternalBinding::lower(PhyRfInitPrefixAction::FilterDcap(
            FilterDcapAction::Configure(filter_parameters)
        )),
        Ok(PhyColdExternalBinding::I2cConfiguration(_))
    ));
    assert!(matches!(
        PhyColdExternalBinding::lower(PhyRfInitPrefixAction::I2cInit1(I2cInit1Action::Configure(
            PhyRfInitParameterSnapshot::new(filter_parameters, 6)
        ))),
        Ok(PhyColdExternalBinding::I2cConfiguration(_))
    ));
    let transaction = PhyPbusForceTest::new(4, 1, 0);
    assert!(matches!(
        PhyColdExternalBinding::lower(PhyRfInitPrefixAction::PbusClear(
            PhyPbusClearAction::ForceTest(transaction)
        )),
        Ok(PhyColdExternalBinding::Pbus(_))
    ));
    assert!(matches!(
        PhyColdExternalBinding::lower(PhyRfInitPrefixAction::PbusClear(
            PhyPbusClearAction::ConfigureWorkMode
        )),
        Ok(PhyColdExternalBinding::Observation(_))
    ));
    assert_eq!(
        PhyColdExternalBinding::lower(PhyRfInitPrefixAction::CaptureFilterDcapParameters),
        Err(PhyColdLoweringError::UnsupportedAction)
    );
}

#[test]
fn i2c_configuration_binding_requires_the_complete_pac_transaction() {
    let parameters = FilterDcapParameters::new(1, 2, 3, 4, 5);
    let binding = PhyColdI2cConfigurationBinding::new(PhyRfInitPrefixAction::FilterDcap(
        FilterDcapAction::Configure(parameters),
    ))
    .unwrap();
    assert_eq!(
        binding.action(),
        oer_esp32s31_hal::phy::i2c::PhyI2cConfigurationAction::StartCommand
    );
    assert_eq!(
        binding.into_completion(),
        Err(PhyColdLoweringError::IncompleteTransaction)
    );
}

#[test]
fn channel_frequency_i2c_completion_keeps_its_field_identity() {
    let field = crate::analog::i2c::analog_registers::RFPLL_SDM_LOW;
    let outer_action =
        PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::WriteMasked {
            field,
            value: 0x12,
        });
    let mut binding = PhyColdI2cBinding::new(outer_action).unwrap();
    binding.read_started().unwrap();
    binding.observe_read_result(Ok(0x05)).unwrap();
    binding.write_started().unwrap();
    binding.observe_write_result(Ok(())).unwrap();
    assert_eq!(
        binding.into_completion(),
        Ok(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::MaskedWrite { field }
        ))
    );
}

#[test]
fn xtal_and_rfpll_i2c_edges_keep_nested_identity() {
    let initial_field = crate::analog::i2c::analog_registers::XTAL_DUTY_INITIAL;
    let initial_action =
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::ReadInitialDuty {
            field: initial_field,
        });
    let mut initial = PhyColdI2cBinding::new(initial_action).unwrap();
    initial.read_started().unwrap();
    initial.observe_read_result(Ok(0xeb)).unwrap();
    assert_eq!(
        initial.into_completion(),
        Ok(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::InitialDutyRead {
                field: initial_field,
                value: 0x2b,
            }
        ))
    );

    let rfpll_field = crate::analog::i2c::analog_registers::RFPLL_SDM_LOW;
    let rfpll_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
        XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
            RfpllFrequencyAction::WriteMasked {
                field: rfpll_field,
                value: 0x12,
            },
        )),
    ));
    let mut rfpll = PhyColdI2cBinding::new(rfpll_action).unwrap();
    rfpll.read_started().unwrap();
    rfpll.observe_read_result(Ok(0x05)).unwrap();
    rfpll.write_started().unwrap();
    rfpll.observe_write_result(Ok(())).unwrap();
    assert_eq!(
        rfpll.into_completion(),
        Ok(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::Rfpll(RfpllFrequencyCompletion::MaskedWrite {
                    field: rfpll_field,
                })
            ))
        ))
    );

    let candidate_address = analog_registers::XTAL_DUTY_CANDIDATE;
    let candidate_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
        XtalDutyPassAction::Search(XtalDutySearchAction::WriteCandidate {
            address: candidate_address,
            candidate: 0x3a,
        }),
    ));
    let mut candidate = PhyColdI2cBinding::new(candidate_action).unwrap();
    candidate.write_started().unwrap();
    candidate.observe_write_result(Ok(())).unwrap();
    assert_eq!(
        candidate.into_completion(),
        Ok(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                XtalDutySearchCompletion::CandidateWritten {
                    address: candidate_address,
                    candidate: 0x3a,
                }
            ))
        ))
    );
}
