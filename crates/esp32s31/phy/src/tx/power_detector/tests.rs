use super::*;

const PARAMETERS: PhyPwdetParameters = PhyPwdetParameters {
    already_calibrated: false,
    pbus_tx_path_value: 0x1f,
    pbus_rx_path_value: 0xbf,
    dco: [0x101, 0x102, 0x103, 0x104],
    clear_tone_after_ready: true,
    reference_codes: [0, 0],
};

fn complete_direct_prefix(transition: &mut PhyPwdetTransition) {
    transition
        .advance(PhyPwdetCompletion::PbusDebugModeConfigured)
        .unwrap();
    for index in 0..ENTER_PBUS_COUNT {
        let transaction = enter_pbus_transaction(index, PARAMETERS);
        assert_eq!(transition.action(), PhyPwdetAction::ForcePbus(transaction));
        transition
            .advance(PhyPwdetCompletion::PbusCompleted(transaction))
            .unwrap();
    }
    transition
        .advance(PhyPwdetCompletion::TxClockConfigured { enabled: true })
        .unwrap();
    transition
        .advance(PhyPwdetCompletion::PowerDetectorConfigured)
        .unwrap();
    transition
        .advance(PhyPwdetCompletion::CalibrationModeConfigured)
        .unwrap();
    transition
        .advance(PhyPwdetCompletion::ToneConfigured)
        .unwrap();
}

fn complete_sample(
    transition: &mut PhyPwdetTransition,
    measurement_index: u8,
    sample_index: u8,
    sample: u16,
) {
    transition
        .advance(PhyPwdetCompletion::ToneArmed {
            measurement_index,
            sample_index,
        })
        .unwrap();
    transition
        .advance(PhyPwdetCompletion::DelayElapsed {
            phase: PhyPwdetDelayPhase::ToneArmed {
                measurement_index,
                sample_index,
            },
            micros: 1,
        })
        .unwrap();
    transition
        .advance(PhyPwdetCompletion::SarTriggered {
            measurement_index,
            sample_index,
        })
        .unwrap();
    transition
        .advance(PhyPwdetCompletion::DelayElapsed {
            phase: PhyPwdetDelayPhase::SarTriggered {
                measurement_index,
                sample_index,
            },
            micros: 2,
        })
        .unwrap();

    let poll = transition.action();
    assert!(PhyPwdetReadyBinding::new(poll).is_ok());
    transition
        .advance(PhyPwdetCompletion::SarReadySampled {
            measurement_index,
            sample_index,
            ready: false,
        })
        .unwrap();
    assert_eq!(transition.action(), poll);
    transition
        .advance(PhyPwdetCompletion::SarReadySampled {
            measurement_index,
            sample_index,
            ready: true,
        })
        .unwrap();
    transition
        .advance(PhyPwdetCompletion::ToneArmCleared {
            measurement_index,
            sample_index,
        })
        .unwrap();
    transition
        .advance(PhyPwdetCompletion::SarSampled {
            measurement_index,
            sample_index,
            value: sample,
        })
        .unwrap();
}

fn complete_restore(transition: &mut PhyPwdetTransition, expected_terminal: PhyPwdetAction) {
    transition.advance(PhyPwdetCompletion::ToneStopped).unwrap();
    transition
        .advance(PhyPwdetCompletion::TxClockConfigured { enabled: false })
        .unwrap();
    for index in 0..EXIT_PBUS_COUNT {
        let transaction = exit_pbus_transaction(index, PARAMETERS);
        transition
            .advance(PhyPwdetCompletion::PbusCompleted(transaction))
            .unwrap();
    }
    transition
        .advance(PhyPwdetCompletion::PbusWorkModeConfigured {
            settle_required: false,
        })
        .unwrap();
    assert_eq!(transition.action(), expected_terminal);
}

#[test]
fn pbus_sequences_match_both_recovered_helpers() {
    let enter = [
        (0, 1, 0x080),
        (0, 2, 0),
        (4, 2, 0),
        (1, 1, 0x07c),
        (2, 1, 0x100),
        (3, 1, 0x100),
        (2, 2, 0x100),
        (3, 2, 0x100),
        (1, 2, 0),
        (4, 1, 0x00b),
        (5, 1, 0x1df),
        (2, 1, 0x101),
        (3, 1, 0x102),
        (2, 2, 0x103),
        (3, 2, 0x104),
    ];
    for (index, (selector, path, value)) in enter.into_iter().enumerate() {
        assert_eq!(
            enter_pbus_transaction(index as u8, PARAMETERS),
            PhyPbusForceTest::new(selector, path, value)
        );
    }
    let exit = [
        (4, 1, 0),
        (4, 2, 1),
        (5, 1, 0),
        (0, 1, 0x40),
        (0, 2, 0xbf),
        (1, 1, 0x189),
        (1, 2, 0),
    ];
    for (index, (selector, path, value)) in exit.into_iter().enumerate() {
        assert_eq!(
            exit_pbus_transaction(index as u8, PARAMETERS),
            PhyPbusForceTest::new(selector, path, value)
        );
    }
}

#[test]
fn two_reference_measurements_poll_without_spinning_and_restore() {
    let mut transition = PhyPwdetTransition::new(PARAMETERS);
    complete_direct_prefix(&mut transition);

    let mut expected_references = PARAMETERS.reference_codes;
    for measurement_index in 0..2 {
        let control = reference_control(measurement_index);
        assert_eq!(
            transition.action(),
            PhyPwdetAction::WriteReferenceControl { value: control }
        );
        transition
            .advance(PhyPwdetCompletion::ReferenceControlWritten { value: control })
            .unwrap();
        for sample_index in 0..PHY_PWDET_SAMPLES_PER_REFERENCE {
            complete_sample(
                &mut transition,
                measurement_index,
                sample_index,
                100 + u16::from(measurement_index) + u16::from(sample_index) * 2,
            );
        }
        let average = 103 + measurement_index as u16;
        expected_references[measurement_index as usize] = average as i16;
    }
    assert_eq!(
        transition.action(),
        PhyPwdetAction::WriteReferenceControl { value: 0xaaaa }
    );
    transition
        .advance(PhyPwdetCompletion::ReferenceControlWritten { value: 0xaaaa })
        .unwrap();
    let outcome = PhyPwdetOutcome {
        reference_codes: expected_references,
        calibrated: true,
        measurement_performed: true,
    };
    complete_restore(&mut transition, PhyPwdetAction::Complete(outcome));
}

#[test]
fn ready_deadline_failure_still_runs_full_hardware_restore() {
    let mut transition = PhyPwdetTransition::new(PARAMETERS);
    complete_direct_prefix(&mut transition);
    transition
        .advance(PhyPwdetCompletion::ReferenceControlWritten { value: 0 })
        .unwrap();
    transition
        .advance(PhyPwdetCompletion::ToneArmed {
            measurement_index: 0,
            sample_index: 0,
        })
        .unwrap();
    transition
        .advance(PhyPwdetCompletion::DelayElapsed {
            phase: PhyPwdetDelayPhase::ToneArmed {
                measurement_index: 0,
                sample_index: 0,
            },
            micros: 1,
        })
        .unwrap();
    transition
        .advance(PhyPwdetCompletion::SarTriggered {
            measurement_index: 0,
            sample_index: 0,
        })
        .unwrap();
    transition
        .advance(PhyPwdetCompletion::DelayElapsed {
            phase: PhyPwdetDelayPhase::SarTriggered {
                measurement_index: 0,
                sample_index: 0,
            },
            micros: 2,
        })
        .unwrap();
    transition
        .advance(PhyPwdetCompletion::SarReadyDeadlineElapsed {
            measurement_index: 0,
            sample_index: 0,
        })
        .unwrap();
    assert_eq!(transition.action(), PhyPwdetAction::StopTone);
    complete_restore(
        &mut transition,
        PhyPwdetAction::Failed(PhyPwdetFailure::SarReadyDeadlineElapsed {
            measurement_index: 0,
            sample_index: 0,
        }),
    );
}

#[test]
fn already_calibrated_path_has_no_hardware_action() {
    let transition = PhyPwdetTransition::new(PhyPwdetParameters {
        already_calibrated: true,
        reference_codes: [-12, 34],
        ..PARAMETERS
    });
    assert_eq!(
        transition.action(),
        PhyPwdetAction::Complete(PhyPwdetOutcome {
            reference_codes: [-12, 34],
            calibrated: true,
            measurement_performed: false,
        })
    );
}

#[test]
fn pure_sar_translation_matches_rom_unsigned_rules() {
    assert_eq!(sar_signal_reference(100, [25, 75]), [100, 50]);
    assert_eq!(sar_signal_reference(0xffff, [0, -1]), [24, -1]);
}

#[test]
fn terminal_and_non_poll_actions_cannot_be_lowered_as_ready_samples() {
    assert_eq!(
        PhyPwdetReadyBinding::new(PhyPwdetAction::StopTone),
        Err(PhyPwdetBindingError::NotReadyPoll)
    );
    assert_eq!(
        PhyPwdetMmioBinding::new(PhyPwdetAction::Complete(PhyPwdetOutcome {
            reference_codes: [0, 0],
            calibrated: true,
            measurement_performed: true,
        })),
        Err(PhyPwdetBindingError::NotDirectMmio)
    );
}

#[test]
fn pbus_and_timer_bindings_preserve_identity_without_internal_retry() {
    let transaction = PhyPbusForceTest::new(5, 1, 0x1df);
    let mut pbus = PhyPwdetPbusBinding::new(PhyPwdetAction::ForcePbus(transaction)).unwrap();
    assert_eq!(pbus.action(), PhyPwdetPbusBindingAction::Start(transaction));
    pbus.started().unwrap();
    assert_eq!(
        pbus.observe_completed(false),
        Ok(PhyPwdetPbusObservation::StillPending)
    );
    assert_eq!(
        pbus.action(),
        PhyPwdetPbusBindingAction::SampleCompletion(transaction)
    );
    assert_eq!(
        pbus.observe_completed(true),
        Ok(PhyPwdetPbusObservation::Completed)
    );
    assert_eq!(
        pbus.into_completion(),
        Ok(PhyPwdetCompletion::PbusCompleted(transaction))
    );

    let phase = PhyPwdetDelayPhase::ToneArmed {
        measurement_index: 1,
        sample_index: 0,
    };
    let timer =
        PhyPwdetTimerBinding::new(PhyPwdetAction::DelayMicros { phase, micros: 1 }).unwrap();
    assert_eq!(timer.micros(), 1);
    assert_eq!(
        timer.into_completion(),
        PhyPwdetCompletion::DelayElapsed { phase, micros: 1 }
    );
}

#[test]
fn external_lowering_covers_each_pwdet_operation_class_and_rejects_terminals() {
    assert!(matches!(
        PhyPwdetExternalBinding::lower(PhyPwdetAction::ConfigurePowerDetector),
        Ok(PhyPwdetExternalBinding::Mmio(_))
    ));
    assert!(matches!(
        PhyPwdetExternalBinding::lower(PhyPwdetAction::ForcePbus(PhyPbusForceTest::new(4, 1, 0))),
        Ok(PhyPwdetExternalBinding::Pbus(_))
    ));
    assert!(matches!(
        PhyPwdetExternalBinding::lower(PhyPwdetAction::DelayMicros {
            phase: PhyPwdetDelayPhase::PbusWorkMode,
            micros: 1,
        }),
        Ok(PhyPwdetExternalBinding::Timer(_))
    ));
    assert!(matches!(
        PhyPwdetExternalBinding::lower(PhyPwdetAction::PollSarReady {
            measurement_index: 0,
            sample_index: 0,
        }),
        Ok(PhyPwdetExternalBinding::Ready(_))
    ));
    assert!(
        PhyPwdetExternalBinding::lower(PhyPwdetAction::Complete(PhyPwdetOutcome {
            reference_codes: [0; 2],
            calibrated: true,
            measurement_performed: true,
        }))
        .is_err()
    );
}
