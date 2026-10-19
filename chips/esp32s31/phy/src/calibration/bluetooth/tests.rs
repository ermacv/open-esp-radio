use super::*;

#[test]
fn gain_index_conversion_preserves_only_the_two_rom_encodings() {
    assert_eq!(bluetooth_gain_index_to_baseband(0), 0);
    assert_eq!(bluetooth_gain_index_to_baseband(1), 0x80);
    assert_eq!(bluetooth_gain_index_to_baseband(2), 0x100);
    assert_eq!(bluetooth_gain_index_to_baseband(3), 0);
    assert_eq!(bluetooth_gain_index_to_baseband(u32::MAX), 0);
}

#[test]
fn baseband_conversion_rejects_noncanonical_values() {
    assert_eq!(bluetooth_baseband_to_gain_index(0), 0);
    assert_eq!(bluetooth_baseband_to_gain_index(0x80), 1);
    assert_eq!(bluetooth_baseband_to_gain_index(0x100), 2);
    assert_eq!(bluetooth_baseband_to_gain_index(0x180), 0);
    assert_eq!(bluetooth_baseband_to_gain_index(u32::MAX), 0);
}

#[test]
fn bluetooth_gain_image_matches_the_linked_vendor_cold_state() {
    let state = crate::state::PhyState::default();
    let image = state.bluetooth_tx_gain_image();
    assert_eq!(
        image.output_72,
        [1, 1, 1, 2, 3, 5, 11, 13, 14, 22, 22, 31, 63, 63, 63, 63]
    );
    assert_eq!(
        image.output_64,
        [
            0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x100, 0x100, 0x100, 0x100, 0x100,
            0x100, 0x100, 0x100,
        ]
    );
    assert_eq!(
        image.output_32,
        [5, 5, 5, 2, 4, 2, 3, 1, 0, 0, 12, 10, 1, 13, 24, 24]
    );
}

#[test]
fn bluetooth_txdc_outcome_updates_only_three_bt_rows() {
    let mut state = crate::state::PhyState::default();
    let wifi_before = state.tx_dc_pwdet_parameters();
    let outcome = crate::tx::dc_offset::PhyTxDcOutcome {
        dco: [
            [0x101, 0x102, 0x103, 0x104],
            [0x201, 0x202, 0x203, 0x204],
            [0x301, 0x302, 0x303, 0x304],
            [0x401, 0x402, 0x403, 0x404],
            [0x501, 0x502, 0x503, 0x504],
        ],
    };

    state.apply_bluetooth_tx_dc_outcome(outcome);

    assert!(state.bluetooth_tx_dc_calibrated());
    assert_eq!(state.tx_dc_pwdet_parameters(), wifi_before);
    assert_eq!(
        state.bluetooth_tx_dco(),
        [outcome.dco[0], outcome.dco[1], outcome.dco[2]]
    );
}

fn power_parameters() -> PhyBluetoothTxPowerParameters {
    PhyBluetoothTxPowerParameters {
        calibration: crate::tx::power::PhyTxPowerParameters {
            already_calibrated: false,
            crystal_selector: 0,
            environment: crate::tx::calibration::PhyTxCalibrationParameters {
                pbus_tx_path_value: 0,
                pbus_rx_path_value: 0,
                dco: [0; 4],
            },
            capacitance: [1, 2, 3, 4, 5, 6],
            target_adjustment: 0,
            power_offset: 0,
            initial_attenuation: 8,
            clear_tone_after_ready: false,
            reference_codes: [80, 120],
        },
        pbus_power_path_value: 7,
        pbus_tx_path_value: 9,
        dco: [0x101, 0x102, 0x103, 0x104],
        tone_selector: 0x55,
    }
}

fn complete_i2c_control(action: PhyBluetoothTxPowerAction) -> PhyBluetoothTxPowerCompletion {
    let PhyBluetoothTxPowerAction::I2cControl(operation) = action else {
        panic!("expected I2C action, got {action:?}");
    };
    use oer_esp32s31_hal::phy::i2c::{
        BluetoothTxPowerControlCompletion as Completion,
        BluetoothTxPowerControlOperation as Operation,
    };
    PhyBluetoothTxPowerCompletion::I2cControl(match operation {
        Operation::PrepareRestore => Completion::RestorePrepared,
        Operation::ConfigureCalibration => Completion::CalibrationConfigured,
        Operation::Restore => Completion::Restored,
    })
}

#[test]
fn bluetooth_power_root_preserves_vendor_prefix_and_bt_child_mode() {
    let mut transition = PhyBluetoothTxPowerTransition::new(power_parameters());
    for _ in 0..2 {
        let completion = complete_i2c_control(transition.action());
        transition.advance(completion).unwrap();
    }
    while let PhyBluetoothTxPowerAction::Prepare(action) = transition.action() {
        use crate::tx::calibration::{
            PhyTxCalibrationEnvironmentAction as Action,
            PhyTxCalibrationEnvironmentCompletion as Completion,
        };
        let completion = match action {
            Action::ConfigurePbusDebugMode => Completion::PbusDebugModeConfigured,
            Action::ForcePbus(transaction) => Completion::PbusCompleted(transaction),
            Action::ConfigureTxClock { enabled } => Completion::TxClockConfigured { enabled },
            Action::ConfigurePowerDetector => Completion::PowerDetectorConfigured,
            Action::ConfigureCalibrationMode => Completion::CalibrationModeConfigured,
            terminal => panic!("unexpected prepare action {terminal:?}"),
        };
        transition
            .advance(PhyBluetoothTxPowerCompletion::Prepare(completion))
            .unwrap();
    }

    for expected in [
        crate::analog::pbus::PhyPbusForceTest::new(5, 1, 0x1c7),
        crate::analog::pbus::PhyPbusForceTest::new(1, 2, 0),
    ] {
        assert_eq!(
            transition.action(),
            PhyBluetoothTxPowerAction::ForcePbus(expected)
        );
        transition
            .advance(PhyBluetoothTxPowerCompletion::PbusCompleted(expected))
            .unwrap();
    }
    assert_eq!(
        transition.action(),
        PhyBluetoothTxPowerAction::ReadPbus {
            selector: 1,
            path: 1
        }
    );
    transition
        .advance(PhyBluetoothTxPowerCompletion::PbusRead {
            selector: 1,
            path: 1,
            value: 0x41,
        })
        .unwrap();
    for expected in [
        crate::analog::pbus::PhyPbusForceTest::new(1, 1, 0x43),
        crate::analog::pbus::PhyPbusForceTest::new(4, 2, 0x48),
        crate::analog::pbus::PhyPbusForceTest::new(2, 1, 0x101),
        crate::analog::pbus::PhyPbusForceTest::new(3, 1, 0x102),
        crate::analog::pbus::PhyPbusForceTest::new(2, 2, 0x103),
        crate::analog::pbus::PhyPbusForceTest::new(3, 2, 0x104),
    ] {
        assert_eq!(
            transition.action(),
            PhyBluetoothTxPowerAction::ForcePbus(expected)
        );
        transition
            .advance(PhyBluetoothTxPowerCompletion::PbusCompleted(expected))
            .unwrap();
    }
    assert!(matches!(
        transition.action(),
        PhyBluetoothTxPowerAction::Calibration(crate::tx::power::PhyTxPowerAction::WriteI2c {
            value: 0xc3,
            ..
        })
    ));
}

#[test]
fn bluetooth_prepare_failure_restores_pac_owned_i2c_control_before_terminal_failure() {
    use crate::tx::calibration::{
        PhyTxCalibrationEnvironmentAction as Action,
        PhyTxCalibrationEnvironmentCompletion as Completion,
    };

    let mut transition = PhyBluetoothTxPowerTransition::new(power_parameters());
    for _ in 0..2 {
        let completion = complete_i2c_control(transition.action());
        transition.advance(completion).unwrap();
    }
    transition
        .advance(PhyBluetoothTxPowerCompletion::Prepare(
            Completion::PbusDebugModeConfigured,
        ))
        .unwrap();
    let PhyBluetoothTxPowerAction::Prepare(Action::ForcePbus(transaction)) = transition.action()
    else {
        panic!("Bluetooth prepare did not enter its first PBus command");
    };
    transition
        .advance(PhyBluetoothTxPowerCompletion::Prepare(
            Completion::PbusTimedOut(transaction),
        ))
        .unwrap();

    assert!(matches!(
        transition.action(),
        PhyBluetoothTxPowerAction::I2cControl(
            oer_esp32s31_hal::phy::i2c::BluetoothTxPowerControlOperation::Restore
        )
    ));
}

#[test]
fn bluetooth_power_outcome_publishes_bt_fields_only() {
    let mut state = crate::state::PhyState::default();
    let wifi_before = state.tx_power_parameters();
    state.apply_bluetooth_tx_power_outcome(PhyBluetoothTxPowerOutcome {
        calibration: crate::tx::power::PhyTxPowerOutcome {
            reference_codes: [80, 120],
            power_curve: [-3, 4, 5],
            point_corrections: [6, -7, 8],
            power_adjustment: -9,
            final_attenuation: 13,
            current_channel: 11,
            calibration_performed: true,
        },
    });
    assert!(state.bluetooth_tx_power_calibrated());
    let wifi_after = state.tx_power_parameters();
    assert_eq!(wifi_after.reference_codes, wifi_before.reference_codes);
    assert_eq!(wifi_after.capacitance, wifi_before.capacitance);
    assert_eq!(wifi_after.initial_attenuation, 13);
    assert_eq!(
        state.bluetooth_tx_power_result(),
        ([-3, 4, 5], [6, -7, 8], -9)
    );
}

#[test]
fn bluetooth_gain_parent_starts_with_the_recovered_full_frequency_rfpll() {
    let mut transition = crate::state::PhyState::default().bluetooth_tx_gain_init_transition(
        crate::analog::pbus::PhyForceTxRxDepth::OUTERMOST.nested(),
    );

    assert!(matches!(
        transition.step_local().unwrap(),
        PhyBluetoothTxGainInitLocalStep::External(PhyBluetoothTxGainInitAction::Rfpll(
            crate::analog::rfpll::RfpllFrequencyAction::WriteMasked { .. }
        ))
    ));
    assert_eq!(
        transition.advance_external(PhyBluetoothTxGainInitCompletion::Published(
            PhyBluetoothTxGainPublication::new(
                crate::state::PhyState::default().bluetooth_tx_gain_image(),
            ),
        )),
        Err(PhyBluetoothTxGainInitTransitionError::WrongCompletion)
    );
}

#[test]
fn retained_bluetooth_calibration_skips_only_expensive_children() {
    let mut transition = crate::state::PhyState::default().bluetooth_tx_gain_init_transition(
        crate::analog::pbus::PhyForceTxRxDepth::OUTERMOST.nested(),
    );
    transition.parameters.tx_dc_calibrated = true;
    transition
        .parameters
        .tx_power
        .calibration
        .already_calibrated = true;
    transition.step = PhyBluetoothTxGainInitStep::TxCap;

    let PhyBluetoothTxGainInitLocalStep::External(PhyBluetoothTxGainInitAction::TxCap(
        crate::tx::power::PhyTxPowerAction::WriteI2c { address, value },
    )) = transition.step_local().unwrap()
    else {
        panic!("Bluetooth parent did not publish channel-six TX-cap first");
    };
    transition
        .advance_external(PhyBluetoothTxGainInitCompletion::TxCap(
            crate::tx::power::PhyTxPowerCompletion::I2cWritten { address, value },
        ))
        .unwrap();

    assert_eq!(
        transition.step_local().unwrap(),
        PhyBluetoothTxGainInitLocalStep::StateAdvanced
    );
    assert!(matches!(
        transition.step_local().unwrap(),
        PhyBluetoothTxGainInitLocalStep::External(PhyBluetoothTxGainInitAction::TxDcPwdet(_))
    ));
}

/// Drive a transition parked before publication to completion; returns the
/// force-TX/RX writes before and after the gain-memory publication.
fn force_writes_around_publication(
    depth: crate::analog::pbus::PhyForceTxRxDepth,
) -> [std::vec::Vec<(bool, u8)>; 2] {
    use crate::analog::pbus::{PhyForceTxRxAction, PhyForceTxRxCompletion};
    let state = crate::state::PhyState::default();
    let mut transition = state.bluetooth_tx_gain_init_transition(depth);
    let publication = PhyBluetoothTxGainPublication::new(state.bluetooth_tx_gain_image());
    transition.power = Some(PhyBluetoothTxPowerOutcome {
        calibration: crate::tx::power::PhyTxPowerOutcome {
            reference_codes: [80, 120],
            power_curve: [-3, 4, 5],
            point_corrections: [6, -7, 8],
            power_adjustment: -9,
            final_attenuation: 13,
            current_channel: 11,
            calibration_performed: true,
        },
    });
    transition.step =
        PhyBluetoothTxGainInitStep::Gain(PhyBluetoothTxGainChild::new(publication, depth));
    let mut writes = [std::vec::Vec::new(), std::vec::Vec::new()];
    let mut published = false;
    loop {
        let completion = match transition.step_local().unwrap() {
            PhyBluetoothTxGainInitLocalStep::StateAdvanced => continue,
            PhyBluetoothTxGainInitLocalStep::Complete(_) => return writes,
            PhyBluetoothTxGainInitLocalStep::Failed(failure) => panic!("{failure:?}"),
            PhyBluetoothTxGainInitLocalStep::External(action) => match action {
                PhyBluetoothTxGainInitAction::Publish(publication) => {
                    published = true;
                    PhyBluetoothTxGainInitCompletion::Published(publication)
                }
                PhyBluetoothTxGainInitAction::ForceTxRx(PhyForceTxRxAction::Configure {
                    enabled,
                    phase,
                }) => {
                    writes[usize::from(published)].push((enabled, phase));
                    PhyBluetoothTxGainInitCompletion::ForceTxRx(
                        PhyForceTxRxCompletion::Configured { enabled, phase },
                    )
                }
                PhyBluetoothTxGainInitAction::ForceTxRx(PhyForceTxRxAction::DelayMicros {
                    enabled,
                    completed_phase,
                    micros,
                }) => PhyBluetoothTxGainInitCompletion::ForceTxRx(
                    PhyForceTxRxCompletion::DelayElapsed {
                        enabled,
                        completed_phase,
                        micros,
                    },
                ),
                action => panic!("unexpected action {action:?}"),
            },
        };
        transition.advance_external(completion).unwrap();
    }
}

#[test]
fn gain_child_force_pair_follows_the_vendor_nesting_count() {
    let outermost = crate::analog::pbus::PhyForceTxRxDepth::OUTERMOST;
    assert_eq!(
        force_writes_around_publication(outermost),
        [
            std::vec![(true, 0), (true, 1)],
            std::vec![(false, 0), (false, 1)]
        ]
    );
    // Inside the registration pair the child's release forces again.
    assert_eq!(
        force_writes_around_publication(outermost.nested()),
        [std::vec![], std::vec![(true, 0), (true, 1)]]
    );
    assert_eq!(
        force_writes_around_publication(outermost.nested().nested()),
        [std::vec![], std::vec![]]
    );
}
