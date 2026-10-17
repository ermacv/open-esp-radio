use super::*;

const PARAMETERS: PhyTxPowerTrackingParameters = PhyTxPowerTrackingParameters {
    current_temperature: 25,
    reference_temperature: 0,
    previous_tracking_temperature: 0,
    previous_tracking_gain_base: -3,
    wifi_gain_base: 1,
    bluetooth_ieee802154_gain_base: 2,
    relaxed_threshold: false,
    wifi_gain_publication_skipped: false,
};

const fn request(class: PhyCalibrationTrackClass) -> PhyTxPowerTrackingRequest {
    PhyTxPowerTrackingRequest {
        class,
        enabled: true,
        wifi_channel: 11,
    }
}

#[test]
fn decision_preserves_threshold_and_temperature_boundaries() {
    let decision = decide_tx_power_tracking(
        request(PhyCalibrationTrackClass::Wifi),
        PhyTxPowerTrackingParameters {
            current_temperature: 81,
            reference_temperature: 74,
            previous_tracking_temperature: 76,
            ..PARAMETERS
        },
    );
    assert_eq!(decision.bounded_temperature, 80);
    assert_eq!(decision.threshold, 2);
    assert!(decision.recomputed);

    let decision = decide_tx_power_tracking(
        request(PhyCalibrationTrackClass::BluetoothIeee802154),
        PhyTxPowerTrackingParameters {
            current_temperature: 106,
            reference_temperature: 98,
            previous_tracking_temperature: 101,
            relaxed_threshold: true,
            wifi_gain_publication_skipped: false,
            ..PARAMETERS
        },
    );
    assert_eq!(decision.bounded_temperature, 105);
    assert_eq!(decision.threshold, 10);
    assert!(!decision.recomputed);
    assert_eq!(decision.gain_base, PARAMETERS.previous_tracking_gain_base);

    let decision = decide_tx_power_tracking(
        request(PhyCalibrationTrackClass::Wifi),
        PhyTxPowerTrackingParameters {
            current_temperature: 8,
            reference_temperature: 0,
            previous_tracking_temperature: 4,
            ..PARAMETERS
        },
    );
    assert_eq!(decision.threshold, 4);
    assert!(decision.recomputed);
}

#[test]
fn disabled_or_equal_gain_base_commits_nothing() {
    let mut disabled = request(PhyCalibrationTrackClass::BluetoothIeee802154);
    disabled.enabled = false;
    let transition = PhyTxPowerTrackingTransition::new(disabled, PARAMETERS);
    assert!(transition.decision().recomputed);
    assert_eq!(
        transition.action(),
        PhyTxPowerTrackingAction::Complete(PhyTxPowerTrackingOutcome {
            class: PhyCalibrationTrackClass::BluetoothIeee802154,
            gain_updated: false,
            tracking_temperature: PARAMETERS.previous_tracking_temperature,
            tracking_gain_base: PARAMETERS.previous_tracking_gain_base,
            wifi_gain_base: PARAMETERS.wifi_gain_base,
            bluetooth_ieee802154_gain_base: PARAMETERS.bluetooth_ieee802154_gain_base,
        })
    );

    let same = PhyTxPowerTrackingParameters {
        bluetooth_ieee802154_gain_base: 5,
        ..PARAMETERS
    };
    assert_eq!(
        PhyTxPowerTrackingTransition::new(
            request(PhyCalibrationTrackClass::BluetoothIeee802154),
            same,
        )
        .action(),
        PhyTxPowerTrackingAction::Complete(PhyTxPowerTrackingOutcome {
            class: PhyCalibrationTrackClass::BluetoothIeee802154,
            gain_updated: false,
            tracking_temperature: same.previous_tracking_temperature,
            tracking_gain_base: same.previous_tracking_gain_base,
            wifi_gain_base: same.wifi_gain_base,
            bluetooth_ieee802154_gain_base: same.bluetooth_ieee802154_gain_base,
        })
    );
}

/// Complete every pending force-TX/RX edge; returns the configured writes.
fn complete_force(transition: &mut PhyTxPowerTrackingTransition) -> std::vec::Vec<(bool, u8)> {
    let mut writes = std::vec::Vec::new();
    while let PhyTxPowerTrackingAction::ForceTxRx(action) = transition.action() {
        let completion = match action {
            PhyForceTxRxAction::Configure { enabled, phase } => {
                writes.push((enabled, phase));
                PhyForceTxRxCompletion::Configured { enabled, phase }
            }
            PhyForceTxRxAction::DelayMicros {
                enabled,
                completed_phase,
                micros,
            } => {
                assert_eq!(micros, 1);
                PhyForceTxRxCompletion::DelayElapsed {
                    enabled,
                    completed_phase,
                    micros,
                }
            }
            PhyForceTxRxAction::Complete { .. } => unreachable!(),
        };
        transition
            .advance(PhyTxPowerTrackingCompletion::ForceTxRx(completion))
            .unwrap();
    }
    writes
}

#[test]
fn bluetooth_ieee802154_update_owns_bbpll_and_gain_order() {
    let mut transition = PhyTxPowerTrackingTransition::new(
        request(PhyCalibrationTrackClass::BluetoothIeee802154),
        PARAMETERS,
    );
    assert_eq!(transition.decision().gain_base, 5);
    assert_eq!(
        transition.action(),
        PhyTxPowerTrackingAction::SetBbpllCalibration { enabled: true }
    );
    transition
        .advance(PhyTxPowerTrackingCompletion::BbpllCalibrationSet { enabled: true })
        .unwrap();
    // The gain child runs at nesting level zero and owns the force pair.
    assert_eq!(complete_force(&mut transition), [(true, 0), (true, 1)]);
    assert_eq!(
        transition.action(),
        PhyTxPowerTrackingAction::RegenerateBluetoothIeee802154Gain { gain_base: 5 }
    );
    transition
        .advance(PhyTxPowerTrackingCompletion::BluetoothIeee802154GainRegenerated { gain_base: 5 })
        .unwrap();
    assert_eq!(complete_force(&mut transition), [(false, 0), (false, 1)]);
    assert_eq!(
        transition.action(),
        PhyTxPowerTrackingAction::SetBbpllCalibration { enabled: false }
    );
    transition
        .advance(PhyTxPowerTrackingCompletion::BbpllCalibrationSet { enabled: false })
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyTxPowerTrackingAction::Complete(PhyTxPowerTrackingOutcome {
            class: PhyCalibrationTrackClass::BluetoothIeee802154,
            gain_updated: true,
            tracking_temperature: PARAMETERS.current_temperature,
            tracking_gain_base: 5,
            wifi_gain_base: PARAMETERS.wifi_gain_base,
            bluetooth_ieee802154_gain_base: 5,
        })
    );
}

#[test]
fn wifi_update_binds_channel_and_rejects_foreign_completion() {
    let mut transition =
        PhyTxPowerTrackingTransition::new(request(PhyCalibrationTrackClass::Wifi), PARAMETERS);
    transition
        .advance(PhyTxPowerTrackingCompletion::BbpllCalibrationSet { enabled: true })
        .unwrap();
    complete_force(&mut transition);
    assert_eq!(
        transition.advance(PhyTxPowerTrackingCompletion::WifiGainRegenerated {
            channel: 6,
            gain_base: 3,
        }),
        Err(PhyTxPowerTrackingTransitionError::WrongCompletion)
    );
    assert_eq!(
        transition.action(),
        PhyTxPowerTrackingAction::RegenerateWifiGain {
            channel: 11,
            gain_base: 3,
        }
    );
}

#[test]
fn skipped_wifi_publication_leaves_force_mode_unchanged() {
    let mut transition = PhyTxPowerTrackingTransition::new(
        request(PhyCalibrationTrackClass::Wifi),
        PhyTxPowerTrackingParameters {
            wifi_gain_publication_skipped: true,
            ..PARAMETERS
        },
    );
    transition
        .advance(PhyTxPowerTrackingCompletion::BbpllCalibrationSet { enabled: true })
        .unwrap();
    assert!(complete_force(&mut transition).is_empty());
    transition
        .advance(PhyTxPowerTrackingCompletion::WifiGainRegenerated {
            channel: 11,
            gain_base: 3,
        })
        .unwrap();
    assert!(complete_force(&mut transition).is_empty());
    assert_eq!(
        transition.action(),
        PhyTxPowerTrackingAction::SetBbpllCalibration { enabled: false }
    );
}

#[test]
fn external_binding_captures_live_typed_gain_images_and_rejects_terminal() {
    let state = crate::state::PhyState::new(crate::state::PhyConfig::production());
    let wifi_action = PhyTxPowerTrackingAction::RegenerateWifiGain {
        channel: 11,
        gain_base: 5,
    };
    let wifi = PhyTxPowerTrackingExternalBinding::lower(wifi_action, &state).unwrap();
    assert_eq!(wifi.action(), wifi_action);
    assert_eq!(
        wifi.operation,
        PhyTxPowerTrackingExternalOperation::RegenerateWifiGain {
            channel: 11,
            gain_base: 5,
            image: state.wifi_tracking_gain_image(11, 5),
        }
    );

    let bluetooth_action =
        PhyTxPowerTrackingAction::RegenerateBluetoothIeee802154Gain { gain_base: -7 };
    let bluetooth = PhyTxPowerTrackingExternalBinding::lower(bluetooth_action, &state).unwrap();
    assert_eq!(bluetooth.action(), bluetooth_action);
    assert_eq!(
        bluetooth.operation,
        PhyTxPowerTrackingExternalOperation::RegenerateBluetoothIeee802154Gain {
            gain_base: -7,
            image: state.bluetooth_ieee802154_tracking_gain_image(-7),
        }
    );

    assert_eq!(
        PhyTxPowerTrackingExternalBinding::lower(
            PhyTxPowerTrackingAction::Complete(PhyTxPowerTrackingOutcome {
                class: PhyCalibrationTrackClass::Wifi,
                gain_updated: false,
                tracking_temperature: 0,
                tracking_gain_base: 0,
                wifi_gain_base: 0,
                bluetooth_ieee802154_gain_base: 0,
            }),
            &state,
        ),
        Err(PhyTxPowerTrackingBindingError::UnsupportedAction)
    );
}
