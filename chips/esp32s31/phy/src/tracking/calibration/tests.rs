use super::*;
use std::vec::Vec;

const PARAMETERS: PhyCalibrationTrackingParameters = PhyCalibrationTrackingParameters {
    current_temperature: 50,
    common_reference_temperature: 20,
    transmit_reference_temperature: 20,

    threshold_override: None,
    current_channel: 11,
    channel_bandwidth: 1,
    crystal_selector: 0x31,
};

const RX_GAIN_OUTCOME: crate::rx::gain::PhyRxGainInitOutcome =
    crate::rx::gain::PhyRxGainInitOutcome {
        dc: Some(crate::rx::gain_calibration::PhyRxGainDcOutcome {
            quality: crate::rx::gain_calibration::PhyRxGainDcQuality::EMPTY,
            wifi_index_dc: [[0; 2]; 8],
            wifi_fine_dc: [[0; 2]; crate::rx::gain_calibration::FINE_CODES],
            shared_index_dc: [[0; 2]; 11],
        }),
        generated_tables: true,
        wifi_last_index: 69,
        shared_last_index: 75,
    };

const fn channel_outcome(
    channel: u16,
    cbw: u8,
    temperature: i16,
) -> crate::channel::PhyChipChannelOutcome {
    crate::channel::PhyChipChannelOutcome {
        channel,
        frequency_mhz: 2_407 + channel * 5,
        cbw,
        init_complete: cbw != 0,
        temperature: crate::analog::temperature::PhyTemperatureOutcome {
            temperature,
            sensor_index: 2,
            next_dac: 15,
        },
    }
}

const fn tx_dc_pwdet_outcome(
    class: PhyCalibrationTrackClass,
) -> crate::tx::dc_power_detector::PhyTxDcPwdetOutcome {
    let value = match class {
        PhyCalibrationTrackClass::Wifi => 0x111,
        PhyCalibrationTrackClass::BluetoothIeee802154 => 0x222,
    };
    crate::tx::dc_power_detector::PhyTxDcPwdetOutcome {
        dco: [[value; 4]; 3],
        total_measurements: 144,
    }
}

fn completion(action: PhyCalibrationTrackingAction) -> PhyCalibrationTrackingCompletion {
    match action {
        PhyCalibrationTrackingAction::ClearPbus => {
            PhyCalibrationTrackingCompletion::PbusClearCompleted(
                PhyCalibrationPbusClearCompletion {
                    outcome: crate::analog::pbus::PhyPbusClearOutcome::Cleared,
                },
            )
        }
        PhyCalibrationTrackingAction::CalibrateDcode => {
            PhyCalibrationTrackingCompletion::DcodeCompleted(PhyCalibrationDcodeCompletion {
                result: Ok(crate::analog::dcode::PhyDcodeOutcome { codes: [7; 8] }),
            })
        }
        PhyCalibrationTrackingAction::RecalibrateRxGain => {
            PhyCalibrationTrackingCompletion::RxGainRecalibrated(PhyCalibrationRxGainCompletion {
                result: Ok(RX_GAIN_OUTCOME),
            })
        }
        PhyCalibrationTrackingAction::RestoreChipChannel { channel, cbw } => {
            PhyCalibrationTrackingCompletion::ChipChannelRestored(PhyCalibrationChannelCompletion {
                result: Ok(channel_outcome(
                    channel,
                    cbw,
                    PARAMETERS.current_temperature,
                )),
            })
        }
        PhyCalibrationTrackingAction::SetForcedDigitalGain { enabled } => {
            PhyCalibrationTrackingCompletion::ForcedDigitalGainSet { enabled }
        }
        PhyCalibrationTrackingAction::AwaitSoftwareFrequencySettle => {
            PhyCalibrationTrackingCompletion::SoftwareFrequencySettled
        }
        PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled } => {
            PhyCalibrationTrackingCompletion::HardwareFrequencyControlSet { enabled }
        }
        PhyCalibrationTrackingAction::ForceTxRxOff { enabled } => {
            PhyCalibrationTrackingCompletion::ForceTxRxCompleted(
                PhyCalibrationForceTxRxCompletion { enabled },
            )
        }
        PhyCalibrationTrackingAction::DisableWifiBaseband => {
            PhyCalibrationTrackingCompletion::WifiBasebandDisabled
        }
        PhyCalibrationTrackingAction::ConfigureBasebandChannel { cbw } => {
            PhyCalibrationTrackingCompletion::BasebandChannelConfigured { cbw }
        }
        PhyCalibrationTrackingAction::CalibrateTxDcPwdet { class } => {
            PhyCalibrationTrackingCompletion::TxDcPwdetCalibrated(
                PhyCalibrationTxDcPwdetCompletion {
                    class,
                    result: Ok(tx_dc_pwdet_outcome(class)),
                },
            )
        }
        PhyCalibrationTrackingAction::PublishWifiTxGain { channel } => {
            PhyCalibrationTrackingCompletion::TxGainPublished(PhyCalibrationTxGainCompletion {
                class: PhyCalibrationTrackClass::Wifi,
                channel: Some(channel),
                published: true,
            })
        }
        PhyCalibrationTrackingAction::PublishBluetoothIeee802154TxGain => {
            PhyCalibrationTrackingCompletion::TxGainPublished(PhyCalibrationTxGainCompletion {
                class: PhyCalibrationTrackClass::BluetoothIeee802154,
                channel: None,
                published: true,
            })
        }
        PhyCalibrationTrackingAction::EnableMacBaseband => {
            PhyCalibrationTrackingCompletion::MacBasebandEnabled
        }
        PhyCalibrationTrackingAction::RestoreTxGainCompensation => {
            PhyCalibrationTrackingCompletion::TxGainCompensationRestored
        }
        PhyCalibrationTrackingAction::SetGrantProtect { enabled } => {
            PhyCalibrationTrackingCompletion::GrantProtectSet { enabled }
        }
        PhyCalibrationTrackingAction::Complete(_) | PhyCalibrationTrackingAction::Failed(_) => {
            panic!("terminal action")
        }
    }
}

/// Consume the grant-protect edge the transition requests next.
fn grant(transition: &mut PhyCalibrationTrackingTransition, enabled: bool) {
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::SetGrantProtect { enabled }
    );
    transition
        .advance(PhyCalibrationTrackingCompletion::GrantProtectSet { enabled })
        .unwrap();
}

/// A transition past its opening grant-protect acquisition, for tests that
/// drive the measurement branch directly.
fn admitted(
    request: PhyCalibrationTrackingRequest,
    parameters: PhyCalibrationTrackingParameters,
) -> PhyCalibrationTrackingTransition {
    let mut transition = PhyCalibrationTrackingTransition::new(request, parameters);
    if transition.action() == (PhyCalibrationTrackingAction::SetGrantProtect { enabled: true }) {
        transition
            .advance(PhyCalibrationTrackingCompletion::GrantProtectSet { enabled: true })
            .unwrap();
    }
    transition
}

fn run(
    request: PhyCalibrationTrackingRequest,
    parameters: PhyCalibrationTrackingParameters,
) -> Vec<PhyCalibrationTrackingAction> {
    let mut transition = PhyCalibrationTrackingTransition::new(request, parameters);
    let mut actions = Vec::new();
    for _ in 0..CALIBRATION_TRACKING_ACTION_LIMIT {
        let action = transition.action();
        actions.push(action);
        if matches!(
            action,
            PhyCalibrationTrackingAction::Complete(_) | PhyCalibrationTrackingAction::Failed(_)
        ) {
            return actions;
        }
        transition.advance(completion(action)).unwrap();
    }
    panic!("calibration tracking exceeded its finite path")
}

/// Each grant-protect pair belongs to one branch: the common branch's pair
/// first, then the transmit branch's, and a terminal transition has none.
/// The PHY's RF-hold timing names its intervals by this branch.
#[test]
fn every_grant_protect_pair_belongs_to_one_branch() {
    let mut transition = PhyCalibrationTrackingTransition::new(
        PhyCalibrationTrackingRequest {
            clients: (PhyCalibrationTrackClass::Wifi).clients(),
        },
        PARAMETERS,
    );
    let mut edges = Vec::new();
    for _ in 0..CALIBRATION_TRACKING_ACTION_LIMIT {
        let action = transition.action();
        if matches!(
            action,
            PhyCalibrationTrackingAction::Complete(_) | PhyCalibrationTrackingAction::Failed(_)
        ) {
            assert_eq!(transition.branch(), None);
            break;
        }
        if let PhyCalibrationTrackingAction::SetGrantProtect { enabled } = action {
            edges.push((transition.branch(), enabled));
        }
        transition.advance(completion(action)).unwrap();
    }
    assert_eq!(
        edges,
        [
            (Some(PhyCalibrationTrackingBranch::Common), true),
            (Some(PhyCalibrationTrackingBranch::Common), false),
            (Some(PhyCalibrationTrackingBranch::Transmit), true),
            (Some(PhyCalibrationTrackingBranch::Transmit), false),
        ]
    );
}

#[test]
fn wifi_inclusive_threshold_runs_common_then_wifi_and_restores_every_guard() {
    let actions = run(
        PhyCalibrationTrackingRequest {
            clients: (PhyCalibrationTrackClass::Wifi).clients(),
        },
        PARAMETERS,
    );
    assert_eq!(
        actions[0],
        PhyCalibrationTrackingAction::SetGrantProtect { enabled: true }
    );
    assert_eq!(
        actions[1],
        PhyCalibrationTrackingAction::ForceTxRxOff { enabled: true }
    );
    assert_eq!(actions[2], PhyCalibrationTrackingAction::ClearPbus);
    assert_eq!(actions[3], PhyCalibrationTrackingAction::CalibrateDcode);
    assert!(
        actions.contains(&PhyCalibrationTrackingAction::RestoreChipChannel {
            channel: 11,
            cbw: 1,
        })
    );
    assert!(
        actions.contains(&PhyCalibrationTrackingAction::CalibrateTxDcPwdet {
            class: PhyCalibrationTrackClass::Wifi,
        })
    );
    assert_eq!(
        actions[actions.len() - 3],
        PhyCalibrationTrackingAction::RestoreTxGainCompensation
    );
    assert_eq!(
        actions[actions.len() - 2],
        PhyCalibrationTrackingAction::SetGrantProtect { enabled: false }
    );
    let PhyCalibrationTrackingAction::Complete(outcome) = actions[actions.len() - 1] else {
        panic!("missing terminal outcome")
    };
    assert!(outcome.common_updated);
    assert!(outcome.transmit_updated);
    assert_eq!(
        outcome.dcode,
        Some(crate::analog::dcode::PhyDcodeOutcome { codes: [7; 8] })
    );
    assert_eq!(outcome.rx_gain, Some(RX_GAIN_OUTCOME));
    assert_eq!(outcome.channel, Some(channel_outcome(11, 1, 50)));
    assert_eq!(
        outcome.wifi_tx_dc_pwdet,
        Some(tx_dc_pwdet_outcome(PhyCalibrationTrackClass::Wifi))
    );
    assert_eq!(outcome.common_reference_temperature, 50);
    assert_eq!(outcome.transmit_reference_temperature, 50);
}

#[test]
fn bluetooth_class_uses_its_own_reference_and_skips_common_when_below_threshold() {
    let actions = run(
        PhyCalibrationTrackingRequest {
            clients: (PhyCalibrationTrackClass::BluetoothIeee802154).clients(),
        },
        PhyCalibrationTrackingParameters {
            common_reference_temperature: 50,
            transmit_reference_temperature: 0,

            ..PARAMETERS
        },
    );
    assert_eq!(
        actions[0],
        PhyCalibrationTrackingAction::SetGrantProtect { enabled: true }
    );
    assert_eq!(
        actions[1],
        PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled: false }
    );
    assert!(!actions.contains(&PhyCalibrationTrackingAction::CalibrateDcode));
    assert!(actions.contains(&PhyCalibrationTrackingAction::PublishBluetoothIeee802154TxGain));
}

#[test]
fn below_threshold_completes_without_hardware_operations() {
    let actions = run(
        PhyCalibrationTrackingRequest {
            clients: (PhyCalibrationTrackClass::Wifi).clients(),
        },
        PhyCalibrationTrackingParameters {
            threshold_override: Some(31),
            ..PARAMETERS
        },
    );
    assert_eq!(
        actions,
        [PhyCalibrationTrackingAction::Complete(
            PhyCalibrationTrackingOutcome {
                clients: (PhyCalibrationTrackClass::Wifi).clients(),
                threshold: 31,
                common_reference_temperature: 20,
                transmit_reference_temperature: 20,

                common_updated: false,
                transmit_updated: false,
                dcode: None,
                rx_gain: None,
                channel: None,
                wifi_tx_dc_pwdet: None,
                bluetooth_ieee802154_tx_dc_pwdet: None,
            }
        ),]
    );
}

#[test]
fn common_calibration_restores_gain_before_admitting_the_class_branch() {
    for class in [
        PhyCalibrationTrackClass::Wifi,
        PhyCalibrationTrackClass::BluetoothIeee802154,
    ] {
        for class_due in [false, true] {
            let reference = if class_due { 20 } else { 50 };
            let actions = run(
                PhyCalibrationTrackingRequest {
                    clients: class.clients(),
                },
                PhyCalibrationTrackingParameters {
                    transmit_reference_temperature: reference,

                    ..PARAMETERS
                },
            );
            let restored_channel = actions
                .iter()
                .position(|action| {
                    matches!(
                        action,
                        PhyCalibrationTrackingAction::RestoreChipChannel { .. }
                    )
                })
                .unwrap();
            assert_eq!(
                actions[restored_channel + 1],
                PhyCalibrationTrackingAction::EnableMacBaseband
            );
            assert_eq!(
                actions[restored_channel + 2],
                PhyCalibrationTrackingAction::ForceTxRxOff { enabled: false }
            );
            assert_eq!(
                actions[restored_channel + 3],
                PhyCalibrationTrackingAction::RestoreTxGainCompensation
            );
            // The common branch withdraws its grant protection before the
            // class branch requests its own.
            assert_eq!(
                actions[restored_channel + 4],
                PhyCalibrationTrackingAction::SetGrantProtect { enabled: false }
            );
            if class_due {
                assert_eq!(
                    actions[restored_channel + 5],
                    PhyCalibrationTrackingAction::SetGrantProtect { enabled: true }
                );
                assert_eq!(
                    actions[restored_channel + 6],
                    PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled: false }
                );
            } else {
                assert!(matches!(
                    actions[restored_channel + 5],
                    PhyCalibrationTrackingAction::Complete(_)
                ));
            }
            assert_eq!(
                actions
                    .iter()
                    .filter(|action| **action
                        == PhyCalibrationTrackingAction::RestoreTxGainCompensation)
                    .count(),
                if class_due { 2 } else { 1 }
            );
        }
    }
}

#[test]
fn wrong_completion_preserves_action_and_terminal_rejects_more_work() {
    let mut transition = admitted(
        PhyCalibrationTrackingRequest {
            clients: (PhyCalibrationTrackClass::Wifi).clients(),
        },
        PARAMETERS,
    );
    assert_eq!(
        transition.advance(PhyCalibrationTrackingCompletion::MacBasebandEnabled),
        Err(PhyCalibrationTrackingTransitionError::WrongCompletion)
    );
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::ForceTxRxOff { enabled: true }
    );
    while !matches!(
        transition.action(),
        PhyCalibrationTrackingAction::Complete(_)
    ) {
        transition.advance(completion(transition.action())).unwrap();
    }
    assert_eq!(
        transition.advance(PhyCalibrationTrackingCompletion::TxGainCompensationRestored),
        Err(PhyCalibrationTrackingTransitionError::AlreadyComplete)
    );
}

#[test]
fn external_lowering_owns_only_complete_direct_hardware_leaves() {
    let direct = [
        PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled: false },
        PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled: true },
        PhyCalibrationTrackingAction::ConfigureBasebandChannel { cbw: 0 },
        PhyCalibrationTrackingAction::ConfigureBasebandChannel { cbw: 0x13 },
        PhyCalibrationTrackingAction::DisableWifiBaseband,
        PhyCalibrationTrackingAction::EnableMacBaseband,
        PhyCalibrationTrackingAction::RestoreTxGainCompensation,
    ];
    for action in direct {
        let binding = PhyCalibrationTrackingExternalBinding::lower(action).unwrap();
        let lowered = match &binding {
            PhyCalibrationTrackingExternalBinding::Register(binding) => binding.action(),
            PhyCalibrationTrackingExternalBinding::MacBaseband(binding) => binding.action(),
        };
        assert_eq!(lowered, action);
    }

    let unresolved = [
        PhyCalibrationTrackingAction::ClearPbus,
        PhyCalibrationTrackingAction::CalibrateDcode,
        PhyCalibrationTrackingAction::RecalibrateRxGain,
        PhyCalibrationTrackingAction::RestoreChipChannel {
            channel: 11,
            cbw: 1,
        },
        PhyCalibrationTrackingAction::ForceTxRxOff { enabled: true },
        PhyCalibrationTrackingAction::CalibrateTxDcPwdet {
            class: PhyCalibrationTrackClass::Wifi,
        },
        PhyCalibrationTrackingAction::PublishWifiTxGain { channel: 11 },
        PhyCalibrationTrackingAction::PublishBluetoothIeee802154TxGain,
        PhyCalibrationTrackingAction::Complete(PhyCalibrationTrackingOutcome {
            clients: (PhyCalibrationTrackClass::Wifi).clients(),
            threshold: 30,
            common_reference_temperature: 20,
            transmit_reference_temperature: 20,

            common_updated: false,
            transmit_updated: false,
            dcode: None,
            rx_gain: None,
            channel: None,
            wifi_tx_dc_pwdet: None,
            bluetooth_ieee802154_tx_dc_pwdet: None,
        }),
        PhyCalibrationTrackingAction::Failed(PhyCalibrationTrackingFailure::PbusClearTimedOut(
            crate::analog::pbus::PhyPbusForceTest::new(4, 1, 0),
        )),
    ];
    for action in unresolved {
        assert_eq!(
            PhyCalibrationTrackingExternalBinding::lower(action),
            Err(PhyCalibrationTrackingBindingError::UnsupportedAction)
        );
    }
}

#[test]
fn force_txrx_parent_proof_requires_both_writes_and_timer_edges() {
    let mut transition = admitted(
        PhyCalibrationTrackingRequest {
            clients: (PhyCalibrationTrackClass::BluetoothIeee802154).clients(),
        },
        PhyCalibrationTrackingParameters {
            common_reference_temperature: 50,
            transmit_reference_temperature: 0,

            ..PARAMETERS
        },
    );
    transition
        .advance(PhyCalibrationTrackingCompletion::HardwareFrequencyControlSet { enabled: false })
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::AwaitSoftwareFrequencySettle
    );
    assert!(transition.begin_force_txrx().is_err());
    transition
        .advance(PhyCalibrationTrackingCompletion::SoftwareFrequencySettled)
        .unwrap();

    let child = transition.begin_force_txrx().unwrap();
    assert_eq!(child.parent_action(), transition.action());
    let mut child = child.commit().unwrap_err();
    loop {
        let completion = match child.lower_external() {
            Ok(crate::analog::pbus::PhyForceTxRxExternalBinding::Mmio(binding)) => {
                let crate::analog::pbus::PhyForceTxRxAction::Configure { enabled, phase } =
                    binding.action()
                else {
                    panic!("force MMIO binding lost its identity")
                };
                crate::analog::pbus::PhyForceTxRxCompletion::Configured { enabled, phase }
            }
            Ok(crate::analog::pbus::PhyForceTxRxExternalBinding::Timer(binding)) => {
                assert_eq!(binding.micros(), 1);
                binding.into_completion()
            }
            Err(_) => break,
        };
        child.advance(completion).unwrap();
    }
    let completion = child.commit().unwrap();
    transition.advance(completion).unwrap();
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::SetForcedDigitalGain { enabled: true }
    );
    transition
        .advance(PhyCalibrationTrackingCompletion::ForcedDigitalGainSet { enabled: true })
        .unwrap();
    assert_eq!(transition.action(), PhyCalibrationTrackingAction::ClearPbus);
}

#[test]
fn dcode_parent_proof_starts_the_existing_complete_hardware_graph() {
    let mut transition = admitted(
        PhyCalibrationTrackingRequest {
            clients: (PhyCalibrationTrackClass::Wifi).clients(),
        },
        PARAMETERS,
    );
    transition
        .advance(completion(PhyCalibrationTrackingAction::ForceTxRxOff {
            enabled: true,
        }))
        .unwrap();
    transition
        .advance(completion(PhyCalibrationTrackingAction::ClearPbus))
        .unwrap();

    let child = transition.begin_dcode().unwrap();
    assert!(matches!(
        child.action(),
        crate::analog::dcode::PhyDcodeAction::Rfpll(_)
    ));
    assert!(matches!(
        child.lower_external(),
        Ok(crate::analog::dcode::PhyDcodeExternalBinding::Rfpll(_))
    ));
    assert!(child.commit().is_err());
}

#[test]
fn dcode_failure_restores_gain_without_publishing_common_progress() {
    let mut transition = admitted(
        PhyCalibrationTrackingRequest {
            clients: (PhyCalibrationTrackClass::Wifi).clients(),
        },
        PARAMETERS,
    );
    transition
        .advance(completion(PhyCalibrationTrackingAction::ForceTxRxOff {
            enabled: true,
        }))
        .unwrap();
    transition
        .advance(completion(PhyCalibrationTrackingAction::ClearPbus))
        .unwrap();
    let failure = crate::analog::dcode::PhyDcodeFailure::Rfpll {
        calibration_index: 0,
        failure: crate::analog::rfpll::RfpllFrequencyFailure::FrequencyReadyDeadlineExceeded {
            samples: 100,
        },
    };
    transition
        .advance(PhyCalibrationTrackingCompletion::DcodeCompleted(
            PhyCalibrationDcodeCompletion {
                result: Err(failure),
            },
        ))
        .unwrap();
    // The failed common branch still leaves its force-TX/RX level.
    let release = PhyCalibrationTrackingAction::ForceTxRxOff { enabled: false };
    assert_eq!(transition.action(), release);
    transition.advance(completion(release)).unwrap();
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::RestoreTxGainCompensation
    );
    // The shared failure tail closes the bracket the common branch opened:
    // the transmit branch never acquired the request.
    assert_eq!(
        transition.branch(),
        Some(PhyCalibrationTrackingBranch::Common)
    );
    transition
        .advance(PhyCalibrationTrackingCompletion::TxGainCompensationRestored)
        .unwrap();
    assert_eq!(
        transition.branch(),
        Some(PhyCalibrationTrackingBranch::Common)
    );
    grant(&mut transition, false);
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::Failed(PhyCalibrationTrackingFailure::Dcode(failure))
    );
}

#[test]
fn rx_gain_parent_proof_forces_dc_and_tables_through_complete_hardware_graph() {
    let mut transition = admitted(
        PhyCalibrationTrackingRequest {
            clients: (PhyCalibrationTrackClass::Wifi).clients(),
        },
        PARAMETERS,
    );
    for action in [
        PhyCalibrationTrackingAction::ForceTxRxOff { enabled: true },
        PhyCalibrationTrackingAction::ClearPbus,
        PhyCalibrationTrackingAction::CalibrateDcode,
        PhyCalibrationTrackingAction::DisableWifiBaseband,
    ] {
        assert_eq!(transition.action(), action);
        transition.advance(completion(action)).unwrap();
    }

    let child = transition
        .begin_rx_gain_recalibration(crate::rx::gain::PhyRxGainInitParameters {
            dc_calibrated: true,
            tables_initialized: true,
            dc: crate::rx::gain_calibration::PhyRxGainDcParameters {
                crystal_selector: 0x31,
                pbus_rx_path_value: 3,
                rx_saturation_detected: false,
            },
            memory: crate::calibration::baseband::PhyRxGainMemoryParameters {
                parameter_002: 3,
                wifi_index_dc: [[0; 2]; 8],
                wifi_fine_dc: [[0; 2]; crate::rx::gain_calibration::FINE_CODES],
                shared_index_dc: [[0; 2]; 11],
                wifi_auxiliary: 0,
            },
        })
        .unwrap();
    assert_eq!(
        child.action(),
        crate::rx::gain::PhyRxGainInitAction::PrepareDcControlRestore
    );
    assert!(matches!(
        child.lower_external(),
        Ok(crate::rx::gain::PhyRxGainInitExternalBinding::Mmio(_))
    ));
    assert!(child.commit().is_err());
}

#[test]
fn channel_parent_proof_starts_the_complete_async_hardware_graph() {
    let mut transition = admitted(
        PhyCalibrationTrackingRequest {
            clients: (PhyCalibrationTrackClass::Wifi).clients(),
        },
        PARAMETERS,
    );
    for action in [
        PhyCalibrationTrackingAction::ForceTxRxOff { enabled: true },
        PhyCalibrationTrackingAction::ClearPbus,
        PhyCalibrationTrackingAction::CalibrateDcode,
        PhyCalibrationTrackingAction::DisableWifiBaseband,
        PhyCalibrationTrackingAction::RecalibrateRxGain,
    ] {
        assert_eq!(transition.action(), action);
        transition.advance(completion(action)).unwrap();
    }

    let state = crate::state::PhyState::new(crate::state::PhyConfig::production());
    let child = transition
        .begin_channel_restore(state.channel_parameters())
        .unwrap();
    assert_eq!(
        child.action(),
        crate::channel::PhyChipChannelAction::SetAgc { enabled: false }
    );
    assert!(matches!(
        child.lower_external(),
        Ok(crate::channel::PhyChipChannelExternalBinding::Mmio(_))
    ));
    assert!(child.commit().is_err());
}

#[test]
fn restored_channel_temperature_drives_following_class_threshold_and_references() {
    let mut transition = admitted(
        PhyCalibrationTrackingRequest {
            clients: (PhyCalibrationTrackClass::Wifi).clients(),
        },
        PhyCalibrationTrackingParameters {
            transmit_reference_temperature: 30,
            ..PARAMETERS
        },
    );
    for action in [
        PhyCalibrationTrackingAction::ForceTxRxOff { enabled: true },
        PhyCalibrationTrackingAction::ClearPbus,
        PhyCalibrationTrackingAction::CalibrateDcode,
        PhyCalibrationTrackingAction::DisableWifiBaseband,
        PhyCalibrationTrackingAction::RecalibrateRxGain,
    ] {
        transition.advance(completion(action)).unwrap();
    }
    transition
        .advance(PhyCalibrationTrackingCompletion::ChipChannelRestored(
            PhyCalibrationChannelCompletion {
                result: Ok(channel_outcome(11, 1, 60)),
            },
        ))
        .unwrap();
    transition
        .advance(PhyCalibrationTrackingCompletion::MacBasebandEnabled)
        .unwrap();
    transition
        .advance(completion(PhyCalibrationTrackingAction::ForceTxRxOff {
            enabled: false,
        }))
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::RestoreTxGainCompensation
    );
    transition
        .advance(PhyCalibrationTrackingCompletion::TxGainCompensationRestored)
        .unwrap();
    grant(&mut transition, false);
    grant(&mut transition, true);
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled: false }
    );

    loop {
        let action = transition.action();
        if let PhyCalibrationTrackingAction::Complete(outcome) = action {
            assert_eq!(outcome.common_reference_temperature, 60);
            assert_eq!(outcome.transmit_reference_temperature, 60);
            assert_eq!(outcome.channel, Some(channel_outcome(11, 1, 60)));
            break;
        }
        transition.advance(completion(action)).unwrap();
    }
}

#[test]
fn restored_temperature_can_remove_tx_demand_without_committing_tx_reference() {
    let mut transition = admitted(
        PhyCalibrationTrackingRequest {
            clients: (PhyCalibrationTrackClass::Wifi).clients(),
        },
        PARAMETERS,
    );
    for action in [
        PhyCalibrationTrackingAction::ForceTxRxOff { enabled: true },
        PhyCalibrationTrackingAction::ClearPbus,
        PhyCalibrationTrackingAction::CalibrateDcode,
        PhyCalibrationTrackingAction::DisableWifiBaseband,
        PhyCalibrationTrackingAction::RecalibrateRxGain,
    ] {
        transition.advance(completion(action)).unwrap();
    }
    transition
        .advance(PhyCalibrationTrackingCompletion::ChipChannelRestored(
            PhyCalibrationChannelCompletion {
                result: Ok(channel_outcome(11, 1, 21)),
            },
        ))
        .unwrap();
    transition
        .advance(PhyCalibrationTrackingCompletion::MacBasebandEnabled)
        .unwrap();
    transition
        .advance(completion(PhyCalibrationTrackingAction::ForceTxRxOff {
            enabled: false,
        }))
        .unwrap();
    transition
        .advance(PhyCalibrationTrackingCompletion::TxGainCompensationRestored)
        .unwrap();
    grant(&mut transition, false);
    let PhyCalibrationTrackingAction::Complete(outcome) = transition.action() else {
        panic!("TX demand must be reevaluated after channel restoration");
    };
    assert!(outcome.common_updated);
    assert!(!outcome.transmit_updated);
    assert_eq!(outcome.common_reference_temperature, 21);
    assert_eq!(
        outcome.transmit_reference_temperature,
        PARAMETERS.transmit_reference_temperature
    );
    assert_eq!(
        outcome.transmit_reference_temperature,
        PARAMETERS.transmit_reference_temperature
    );
    assert!(outcome.wifi_tx_dc_pwdet.is_none());
}

#[test]
fn class_tx_dc_pwdet_parent_selects_both_complete_hardware_graphs() {
    for class in [
        PhyCalibrationTrackClass::Wifi,
        PhyCalibrationTrackClass::BluetoothIeee802154,
    ] {
        let mut transition = admitted(
            PhyCalibrationTrackingRequest {
                clients: class.clients(),
            },
            PhyCalibrationTrackingParameters {
                common_reference_temperature: 50,
                ..PARAMETERS
            },
        );
        while !matches!(
            transition.action(),
            PhyCalibrationTrackingAction::CalibrateTxDcPwdet { .. }
        ) {
            let action = transition.action();
            transition.advance(completion(action)).unwrap();
        }

        let state = crate::state::PhyState::new(crate::state::PhyConfig::production());
        let child = transition
            .begin_tx_dc_pwdet(
                state.tx_dc_pwdet_parameters(),
                state.bluetooth_tx_dc_pwdet_transition(),
            )
            .unwrap();
        assert_eq!(child.class(), class);
        assert_eq!(
            child.action(),
            crate::tx::dc_power_detector::PhyTxDcPwdetAction::PrepareRegisters
        );
        assert!(matches!(
            child.lower_external(),
            Ok(crate::tx::dc_power_detector::PhyTxDcPwdetExternalBinding::Mmio(_))
        ));
        assert!(child.commit().is_err());
    }
}

#[test]
fn class_tx_gain_publication_captures_pending_dco_for_both_radio_banks() {
    for class in [
        PhyCalibrationTrackClass::Wifi,
        PhyCalibrationTrackClass::BluetoothIeee802154,
    ] {
        let mut transition = admitted(
            PhyCalibrationTrackingRequest {
                clients: class.clients(),
            },
            PhyCalibrationTrackingParameters {
                common_reference_temperature: 50,
                ..PARAMETERS
            },
        );
        while !matches!(
            transition.action(),
            PhyCalibrationTrackingAction::CalibrateTxDcPwdet { .. }
        ) {
            let action = transition.action();
            transition.advance(completion(action)).unwrap();
        }
        transition
            .advance(PhyCalibrationTrackingCompletion::TxDcPwdetCalibrated(
                PhyCalibrationTxDcPwdetCompletion {
                    class,
                    result: Ok(tx_dc_pwdet_outcome(class)),
                },
            ))
            .unwrap();

        let state = crate::state::PhyState::new(crate::state::PhyConfig::production());
        let binding = transition.begin_tx_gain_publication(&state).unwrap();
        assert_eq!(binding.action(), transition.action());
        let dco = match class {
            PhyCalibrationTrackClass::Wifi => 0x111,
            PhyCalibrationTrackClass::BluetoothIeee802154 => 0x222,
        };
        let expected_seed = [dco | (dco << 16); 6];
        match binding.publication {
            PhyCalibrationTxGainPublication::Wifi { channel, image } => {
                assert_eq!(channel, PARAMETERS.current_channel);
                assert_eq!(image.unwrap().seed, expected_seed);
            }
            PhyCalibrationTxGainPublication::BluetoothIeee802154 { image } => {
                assert_eq!(image.seed, expected_seed);
            }
        }

        let wrong = PhyCalibrationTxGainCompletion {
            class: match class {
                PhyCalibrationTrackClass::Wifi => PhyCalibrationTrackClass::BluetoothIeee802154,
                PhyCalibrationTrackClass::BluetoothIeee802154 => PhyCalibrationTrackClass::Wifi,
            },
            channel: None,
            published: true,
        };
        assert_eq!(
            transition.advance(PhyCalibrationTrackingCompletion::TxGainPublished(wrong)),
            Err(PhyCalibrationTrackingTransitionError::WrongCompletion)
        );
    }
}

#[test]
fn tx_dc_pwdet_failure_runs_outer_force_frequency_and_gain_cleanup() {
    let mut transition = admitted(
        PhyCalibrationTrackingRequest {
            clients: (PhyCalibrationTrackClass::BluetoothIeee802154).clients(),
        },
        PhyCalibrationTrackingParameters {
            common_reference_temperature: 50,
            ..PARAMETERS
        },
    );
    while !matches!(
        transition.action(),
        PhyCalibrationTrackingAction::CalibrateTxDcPwdet { .. }
    ) {
        let action = transition.action();
        transition.advance(completion(action)).unwrap();
    }
    let failure = crate::tx::dc_power_detector::PhyTxDcPwdetFailure::PbusTimedOut(
        crate::analog::pbus::PhyPbusForceTest::new(4, 1, 0),
    );
    transition
        .advance(PhyCalibrationTrackingCompletion::TxDcPwdetCalibrated(
            PhyCalibrationTxDcPwdetCompletion {
                class: PhyCalibrationTrackClass::BluetoothIeee802154,
                result: Err(failure),
            },
        ))
        .unwrap();
    for expected in [
        PhyCalibrationTrackingAction::SetForcedDigitalGain { enabled: false },
        PhyCalibrationTrackingAction::ForceTxRxOff { enabled: false },
        PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled: true },
        PhyCalibrationTrackingAction::RestoreTxGainCompensation,
        PhyCalibrationTrackingAction::SetGrantProtect { enabled: false },
    ] {
        assert_eq!(transition.action(), expected);
        transition.advance(completion(expected)).unwrap();
    }
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::Failed(PhyCalibrationTrackingFailure::TxDcPwdet(failure))
    );
}

#[test]
fn rx_gain_failure_restores_gain_without_publishing_common_progress() {
    let mut transition = admitted(
        PhyCalibrationTrackingRequest {
            clients: (PhyCalibrationTrackClass::Wifi).clients(),
        },
        PARAMETERS,
    );
    for action in [
        PhyCalibrationTrackingAction::ForceTxRxOff { enabled: true },
        PhyCalibrationTrackingAction::ClearPbus,
        PhyCalibrationTrackingAction::CalibrateDcode,
        PhyCalibrationTrackingAction::DisableWifiBaseband,
    ] {
        transition.advance(completion(action)).unwrap();
    }
    let failure = crate::rx::gain::PhyRxGainInitFailure::Publish(
        crate::rx::gain::PhyRxGainPublishFailure::PbusTimedOut {
            bank: crate::calibration::baseband::PhyRxGainBank::Wifi,
            transaction: crate::analog::pbus::PhyPbusForceTest::new(4, 1, 0),
        },
    );
    transition
        .advance(PhyCalibrationTrackingCompletion::RxGainRecalibrated(
            PhyCalibrationRxGainCompletion {
                result: Err(failure),
            },
        ))
        .unwrap();
    // The failed common branch still leaves its force-TX/RX level.
    let release = PhyCalibrationTrackingAction::ForceTxRxOff { enabled: false };
    assert_eq!(transition.action(), release);
    transition.advance(completion(release)).unwrap();
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::RestoreTxGainCompensation
    );
    transition
        .advance(PhyCalibrationTrackingCompletion::TxGainCompensationRestored)
        .unwrap();
    grant(&mut transition, false);
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::Failed(PhyCalibrationTrackingFailure::RxGain(failure))
    );
}

#[test]
fn class_pbus_timeout_restores_force_frequency_and_gain_before_failure() {
    let mut transition = admitted(
        PhyCalibrationTrackingRequest {
            clients: (PhyCalibrationTrackClass::BluetoothIeee802154).clients(),
        },
        PhyCalibrationTrackingParameters {
            common_reference_temperature: 50,
            transmit_reference_temperature: 0,

            ..PARAMETERS
        },
    );
    transition
        .advance(PhyCalibrationTrackingCompletion::HardwareFrequencyControlSet { enabled: false })
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::AwaitSoftwareFrequencySettle
    );
    assert!(transition.begin_force_txrx().is_err());
    transition
        .advance(PhyCalibrationTrackingCompletion::SoftwareFrequencySettled)
        .unwrap();
    transition
        .advance(PhyCalibrationTrackingCompletion::ForceTxRxCompleted(
            PhyCalibrationForceTxRxCompletion { enabled: true },
        ))
        .unwrap();

    transition
        .advance(PhyCalibrationTrackingCompletion::ForcedDigitalGainSet { enabled: true })
        .unwrap();
    let child = transition.begin_pbus_clear().unwrap();
    let mut child = child.commit().unwrap_err();
    let crate::calibration::cold::PhyColdExternalBinding::Mmio(binding) =
        child.lower_external().unwrap()
    else {
        panic!("PBus clear must begin with debug-mode MMIO")
    };
    child
        .advance_external(binding.into_completion().unwrap())
        .unwrap();

    let crate::analog::pbus::PhyPbusClearAction::ForceTest(transaction) = child.action() else {
        panic!("PBus clear did not publish its first transaction")
    };
    let crate::calibration::cold::PhyColdExternalBinding::Pbus(mut binding) =
        child.lower_external().unwrap()
    else {
        panic!("force-test action did not lower to a PBus owner")
    };
    binding.started().unwrap();
    child
        .advance_external(binding.into_timeout_completion().unwrap())
        .unwrap();
    let completion = child.commit().unwrap();
    transition.advance(completion).unwrap();

    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::SetForcedDigitalGain { enabled: false }
    );
    transition
        .advance(PhyCalibrationTrackingCompletion::ForcedDigitalGainSet { enabled: false })
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::ForceTxRxOff { enabled: false }
    );
    transition
        .advance(PhyCalibrationTrackingCompletion::ForceTxRxCompleted(
            PhyCalibrationForceTxRxCompletion { enabled: false },
        ))
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled: true }
    );
    transition
        .advance(PhyCalibrationTrackingCompletion::HardwareFrequencyControlSet { enabled: true })
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::RestoreTxGainCompensation
    );
    transition
        .advance(PhyCalibrationTrackingCompletion::TxGainCompensationRestored)
        .unwrap();
    grant(&mut transition, false);
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::Failed(PhyCalibrationTrackingFailure::PbusClearTimedOut(
            transaction
        ))
    );
    assert_eq!(
        transition.advance(PhyCalibrationTrackingCompletion::TxGainCompensationRestored),
        Err(PhyCalibrationTrackingTransitionError::AlreadyComplete)
    );
}

#[test]
fn failed_calibration_cannot_publish_partial_results_even_after_cleanup() {
    use crate::tracking::calibration::{
        PhyCalibrationDcodeCompletion, PhyCalibrationPbusClearCompletion,
        PhyCalibrationRxGainCompletion, PhyCalibrationTrackingAction as Action,
        PhyCalibrationTrackingCompletion as Completion, PhyCalibrationTrackingFailure,
    };
    use crate::tracking::parameters::{
        PhyParamTrackRequest, PhyParamTrackingCompletion, PhyParamTrackingPolicy,
        PhyParamTrackingTransition, PhyTrackingDiagnostics,
    };

    let policy = PhyParamTrackingPolicy {
        rfpll_cap_tracking_enabled: false,
        tracking_inhibited: false,
        rfpll_cap_tracking_threshold: None,
        calibration_tracking_threshold: None,
        diagnostics: PhyTrackingDiagnostics::Enabled,
        bluetooth_ieee802154_power_tracking_enabled: true,
        calibration_tracking_enabled: true,
        relaxed_power_tracking_threshold: false,
    };
    let mut parent =
        PhyParamTrackingTransition::new(PhyParamTrackRequest::new(false, true), policy);
    parent
        .advance(PhyParamTrackingCompletion::EnteredCritical)
        .unwrap();
    parent
        .advance(
            PhyParamTrackingCompletion::BluetoothIeee802154TxPowerTracked {
                enabled: true,
                gain_updated: false,
            },
        )
        .unwrap();
    let parent_action = parent.action();
    let mut state = crate::PhyState::new(crate::PhyConfig::production());
    state.apply_temperature_outcome(crate::analog::temperature::PhyTemperatureOutcome {
        temperature: 50,
        sensor_index: 2,
        next_dac: 15,
    });
    let before = state.calibration_tracking_parameters(None);
    let mut child = parent.begin_calibration_tracking(&mut state).unwrap();
    assert_eq!(child.action(), Action::SetGrantProtect { enabled: true });
    child
        .advance(completion(Action::SetGrantProtect { enabled: true }))
        .unwrap();
    assert_eq!(child.action(), Action::ForceTxRxOff { enabled: true });
    child
        .advance(completion(Action::ForceTxRxOff { enabled: true }))
        .unwrap();
    assert_eq!(child.action(), Action::ClearPbus);
    child
        .advance(Completion::PbusClearCompleted(
            PhyCalibrationPbusClearCompletion {
                outcome: crate::analog::pbus::PhyPbusClearOutcome::Cleared,
            },
        ))
        .unwrap();
    child
        .advance(Completion::DcodeCompleted(PhyCalibrationDcodeCompletion {
            result: Ok(crate::analog::dcode::PhyDcodeOutcome { codes: [7; 8] }),
        }))
        .unwrap();
    let failure = crate::rx::gain::PhyRxGainInitFailure::Publish(
        crate::rx::gain::PhyRxGainPublishFailure::PbusTimedOut {
            bank: crate::calibration::baseband::PhyRxGainBank::Wifi,
            transaction: crate::analog::pbus::PhyPbusForceTest::new(4, 1, 0),
        },
    );
    child.advance(Completion::WifiBasebandDisabled).unwrap();
    child
        .advance(Completion::RxGainRecalibrated(
            PhyCalibrationRxGainCompletion {
                result: Err(failure),
            },
        ))
        .unwrap();

    // Cleanup is still pending; the partially completed common branch cannot commit.
    let mut child = child.commit().unwrap_err();
    assert_eq!(child.action(), Action::ForceTxRxOff { enabled: false });
    child
        .advance(completion(Action::ForceTxRxOff { enabled: false }))
        .unwrap();
    let mut child = child.commit().unwrap_err();
    assert_eq!(child.action(), Action::RestoreTxGainCompensation);
    assert_eq!(child.state().calibration_tracking_parameters(None), before);
    child
        .advance(Completion::TxGainCompensationRestored)
        .unwrap();
    let mut child = child.commit().unwrap_err();
    assert_eq!(child.action(), Action::SetGrantProtect { enabled: false });
    child
        .advance(Completion::GrantProtectSet { enabled: false })
        .unwrap();
    // Completing cleanup is not a successful calibration proof either.
    let child = child.commit().unwrap_err();
    assert_eq!(
        child.action(),
        Action::Failed(PhyCalibrationTrackingFailure::RxGain(failure))
    );
    assert_eq!(child.state().calibration_tracking_parameters(None), before);
    assert_eq!(parent.action(), parent_action);
}

#[test]
fn zero_threshold_runs_both_branches_without_changing_temperature() {
    let actions = run(
        PhyCalibrationTrackingRequest {
            clients: (PhyCalibrationTrackClass::Wifi).clients(),
        },
        PhyCalibrationTrackingParameters {
            current_temperature: 50,
            common_reference_temperature: 50,
            transmit_reference_temperature: 50,

            threshold_override: Some(0),
            ..PARAMETERS
        },
    );
    let PhyCalibrationTrackingAction::Complete(outcome) = actions[actions.len() - 1] else {
        panic!("missing terminal outcome")
    };
    assert!(outcome.common_updated);
    assert!(outcome.transmit_updated);
    assert_eq!(outcome.common_reference_temperature, 50);
    assert_eq!(outcome.transmit_reference_temperature, 50);
    assert!(outcome.dcode.is_some());
    assert!(outcome.rx_gain.is_some());
    assert!(outcome.channel.is_some());
    assert!(outcome.wifi_tx_dc_pwdet.is_some());
}

#[test]
fn the_longest_path_fills_the_driver_action_bound() {
    let actions = run(
        PhyCalibrationTrackingRequest {
            clients: crate::tracking::parameters::PhyParamTrackRequest::new(true, true),
        },
        PhyCalibrationTrackingParameters {
            threshold_override: Some(0),
            ..PARAMETERS
        },
    );
    assert!(matches!(
        actions.last(),
        Some(PhyCalibrationTrackingAction::Complete(_))
    ));
    assert_eq!(
        actions.len(),
        usize::from(CALIBRATION_TRACKING_ACTION_LIMIT)
    );
}

#[test]
fn both_clients_share_one_tx_envelope_and_keep_distinct_results() {
    let clients = crate::tracking::parameters::PhyParamTrackRequest::new(true, true);
    let actions = run(PhyCalibrationTrackingRequest { clients }, PARAMETERS);
    let classes: Vec<_> = actions
        .iter()
        .filter_map(|action| match action {
            PhyCalibrationTrackingAction::CalibrateTxDcPwdet { class } => Some(*class),
            _ => None,
        })
        .collect();
    assert_eq!(
        classes,
        [
            PhyCalibrationTrackClass::Wifi,
            PhyCalibrationTrackClass::BluetoothIeee802154
        ]
    );
    for enabled in [false, true] {
        assert_eq!(
            actions
                .iter()
                .filter(|action| **action
                    == PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled })
                .count(),
            1
        );
    }
    let PhyCalibrationTrackingAction::Complete(outcome) = actions.last().unwrap() else {
        panic!("terminal result")
    };
    assert_eq!(outcome.clients, clients);
    assert!(outcome.common_updated && outcome.transmit_updated);
    assert_eq!(
        outcome.wifi_tx_dc_pwdet,
        Some(tx_dc_pwdet_outcome(PhyCalibrationTrackClass::Wifi))
    );
    assert_eq!(
        outcome.bluetooth_ieee802154_tx_dc_pwdet,
        Some(tx_dc_pwdet_outcome(
            PhyCalibrationTrackClass::BluetoothIeee802154
        ))
    );
    assert_eq!(
        outcome.transmit_reference_temperature,
        PARAMETERS.current_temperature
    );
}

#[test]
fn tx_envelope_without_selected_clients_has_no_class_calibration() {
    // The vendor child tests RX/TX demand independently of its two class
    // flags. A direct no-client invocation can restore TX and advance its
    // shared reference without either class child.
    let actions = run(
        PhyCalibrationTrackingRequest {
            clients: crate::tracking::parameters::PhyParamTrackRequest::new(false, false),
        },
        PARAMETERS,
    );
    assert!(!actions.iter().any(|action| matches!(
        action,
        PhyCalibrationTrackingAction::CalibrateTxDcPwdet { .. }
    )));
    let PhyCalibrationTrackingAction::Complete(outcome) = actions.last().unwrap() else {
        panic!("terminal result")
    };
    assert!(outcome.transmit_updated);
    assert!(
        outcome.wifi_tx_dc_pwdet.is_none() && outcome.bluetooth_ieee802154_tx_dc_pwdet.is_none()
    );
}

#[test]
fn second_class_failure_cannot_publish_first_class_or_shared_reference() {
    let mut transition = admitted(
        PhyCalibrationTrackingRequest {
            clients: crate::tracking::parameters::PhyParamTrackRequest::new(true, true),
        },
        PARAMETERS,
    );
    loop {
        let action = transition.action();
        if action
            == (PhyCalibrationTrackingAction::CalibrateTxDcPwdet {
                class: PhyCalibrationTrackClass::BluetoothIeee802154,
            })
        {
            break;
        }
        assert!(!matches!(
            action,
            PhyCalibrationTrackingAction::Complete(_) | PhyCalibrationTrackingAction::Failed(_)
        ));
        transition.advance(completion(action)).unwrap();
    }
    assert!(transition.tx_dc_pwdet[0].is_some());
    let before = transition;
    assert_eq!(
        transition.advance(PhyCalibrationTrackingCompletion::TxDcPwdetCalibrated(
            PhyCalibrationTxDcPwdetCompletion {
                class: PhyCalibrationTrackClass::Wifi,
                result: Ok(tx_dc_pwdet_outcome(PhyCalibrationTrackClass::Wifi)),
            }
        )),
        Err(PhyCalibrationTrackingTransitionError::WrongCompletion)
    );
    assert_eq!(transition, before);
    // A failure must traverse cleanup and terminate as Failed, never Complete.
    transition
        .advance(PhyCalibrationTrackingCompletion::TxDcPwdetCalibrated(
            PhyCalibrationTxDcPwdetCompletion {
                class: PhyCalibrationTrackClass::BluetoothIeee802154,
                result: Err(
                    crate::tx::dc_power_detector::PhyTxDcPwdetFailure::PbusTimedOut(
                        crate::analog::pbus::PhyPbusForceTest::new(4, 1, 0),
                    ),
                ),
            },
        ))
        .unwrap();
    for _ in 0..10 {
        let action = transition.action();
        assert!(!matches!(action, PhyCalibrationTrackingAction::Complete(_)));
        if matches!(action, PhyCalibrationTrackingAction::Failed(_)) {
            return;
        }
        transition.advance(completion(action)).unwrap();
    }
    panic!("failure cleanup did not terminate")
}

#[test]
fn each_measurement_branch_disables_wifi_baseband_before_its_children() {
    let actions = run(
        PhyCalibrationTrackingRequest {
            clients: super::super::parameters::PhyParamTrackRequest::new(true, true),
        },
        PARAMETERS,
    );
    let boundaries: Vec<_> = actions
        .windows(3)
        .filter(|window| window[1] == PhyCalibrationTrackingAction::DisableWifiBaseband)
        .collect();
    assert_eq!(boundaries.len(), 2);
    assert_eq!(
        boundaries[0],
        &[
            PhyCalibrationTrackingAction::CalibrateDcode,
            PhyCalibrationTrackingAction::DisableWifiBaseband,
            PhyCalibrationTrackingAction::RecalibrateRxGain,
        ]
    );
    assert_eq!(
        boundaries[1],
        &[
            PhyCalibrationTrackingAction::ClearPbus,
            PhyCalibrationTrackingAction::DisableWifiBaseband,
            PhyCalibrationTrackingAction::ConfigureBasebandChannel { cbw: 0 },
        ]
    );
}

#[test]
fn nested_gain_children_force_again_when_leaving_the_transmit_pair_level() {
    let actions = run(
        PhyCalibrationTrackingRequest {
            clients: crate::tracking::parameters::PhyParamTrackRequest::new(true, true),
        },
        PARAMETERS,
    );
    let force = PhyCalibrationTrackingAction::ForceTxRxOff { enabled: true };
    for publication in [
        PhyCalibrationTrackingAction::PublishWifiTxGain { channel: 11 },
        PhyCalibrationTrackingAction::PublishBluetoothIeee802154TxGain,
    ] {
        let index = actions.iter().position(|a| *a == publication).unwrap();
        assert_eq!(actions[index + 1], force, "{publication:?}");
    }
    // Common pair, transmit pair, and one re-force per gain child.
    assert_eq!(actions.iter().filter(|a| **a == force).count(), 4);
    assert_eq!(
        actions
            .iter()
            .filter(|a| **a == PhyCalibrationTrackingAction::ForceTxRxOff { enabled: false })
            .count(),
        2
    );
}

/// `phy_cal_param_track` brackets each measurement branch separately: the
/// common branch from its first TX/RX force through its gain-compensation
/// restore, and the class branch from hardware frequency disable through
/// the final restore.
#[test]
fn grant_protection_brackets_each_measurement_branch_like_the_vendor() {
    let actions = run(
        PhyCalibrationTrackingRequest {
            clients: (PhyCalibrationTrackClass::Wifi).clients(),
        },
        PARAMETERS,
    );
    let grants: Vec<(usize, bool)> = actions
        .iter()
        .enumerate()
        .filter_map(|(index, action)| match action {
            PhyCalibrationTrackingAction::SetGrantProtect { enabled } => Some((index, *enabled)),
            _ => None,
        })
        .collect();
    assert_eq!(
        grants
            .iter()
            .map(|(_, enabled)| *enabled)
            .collect::<Vec<_>>(),
        [true, false, true, false]
    );
    let [
        (common_start, _),
        (common_end, _),
        (class_start, _),
        (class_end, _),
    ] = grants[..]
    else {
        unreachable!()
    };
    assert_eq!(
        actions[common_start + 1],
        PhyCalibrationTrackingAction::ForceTxRxOff { enabled: true }
    );
    assert_eq!(
        actions[common_end - 1],
        PhyCalibrationTrackingAction::RestoreTxGainCompensation
    );
    assert_eq!(class_start, common_end + 1);
    assert_eq!(
        actions[class_start + 1],
        PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled: false }
    );
    assert_eq!(
        actions[class_end - 1],
        PhyCalibrationTrackingAction::RestoreTxGainCompensation
    );
    assert!(matches!(
        actions[class_end + 1],
        PhyCalibrationTrackingAction::Complete(_)
    ));
}

/// Every action of a complete pass with both classes, mapped to its RF-hold
/// edge, gives balanced intervals: one common and one transmit grant-protect
/// bracket, every Wi-Fi baseband-off window closed, and no edge at the
/// terminal action.
#[test]
fn a_complete_pass_maps_to_balanced_rf_hold_intervals() {
    use crate::tracking::observation::{Operation, Recorder, calibration_hold_edge};

    let mut transition = PhyCalibrationTrackingTransition::new(
        PhyCalibrationTrackingRequest {
            clients: crate::tracking::parameters::PhyParamTrackRequest::new(true, true),
        },
        PARAMETERS,
    );
    let mut recorder = Recorder::default();
    let mut now = 0;
    for _ in 0..CALIBRATION_TRACKING_ACTION_LIMIT {
        let action = transition.action();
        now += 10;
        if matches!(
            action,
            PhyCalibrationTrackingAction::Complete(_) | PhyCalibrationTrackingAction::Failed(_)
        ) {
            assert_eq!(calibration_hold_edge(action, transition.branch()), None);
            break;
        }
        if let Some((operation, event)) = calibration_hold_edge(action, transition.branch()) {
            recorder.observe(operation, event, now);
        }
        transition.advance(completion(action)).unwrap();
    }
    let report = recorder.report();
    assert!(!report.invalid);
    for operation in [
        Operation::GrantProtectCommon,
        Operation::GrantProtectTransmit,
    ] {
        let timing = report.timing(operation);
        assert_eq!((timing.started, timing.completed), (1, 1), "{operation:?}");
    }
    let baseband = report.timing(Operation::WifiBasebandOff);
    assert!(baseband.completed >= 2);
    assert_eq!(baseband.started, baseband.completed);
}
