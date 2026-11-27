use super::*;
use oer_chips_esp32s31_hal::types::PhyForceTxRxPhase;

const PARAMETERS: PhyTxPowerTrackingParameters = PhyTxPowerTrackingParameters {
    current_temperature: 25,
    reference_temperature: 0,
    previous_tracking_temperature: 0,
    previous_tracking_gain_base: -3,
    wifi_gain_base: 1,
    relaxed_threshold: false,
    wifi_gain_publication_skipped: false,
};

const REQUEST: PhyTxPowerTrackingRequest = PhyTxPowerTrackingRequest {
    enabled: true,
    wifi_channel: 11,
};

#[test]
fn decision_preserves_threshold_and_temperature_boundaries() {
    let decision = decide_tx_power_tracking(
        REQUEST,
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
        REQUEST,
        PhyTxPowerTrackingParameters {
            current_temperature: 85,
            reference_temperature: 78,
            previous_tracking_temperature: 76,
            relaxed_threshold: true,
            wifi_gain_publication_skipped: false,
            ..PARAMETERS
        },
    );
    assert_eq!(decision.bounded_temperature, 80);
    assert_eq!(decision.threshold, 10);
    assert!(!decision.recomputed);
    assert_eq!(decision.gain_base, PARAMETERS.previous_tracking_gain_base);

    let decision = decide_tx_power_tracking(
        REQUEST,
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
    let mut disabled = REQUEST;
    disabled.enabled = false;
    let transition = PhyTxPowerTrackingTransition::new(disabled, PARAMETERS);
    assert!(transition.decision().recomputed);
    assert_eq!(
        transition.action(),
        PhyTxPowerTrackingAction::Complete(PhyTxPowerTrackingOutcome {
            gain_updated: false,
            tracking_temperature: PARAMETERS.previous_tracking_temperature,
            tracking_gain_base: PARAMETERS.previous_tracking_gain_base,
            wifi_gain_base: PARAMETERS.wifi_gain_base,
        })
    );

    // 25 above the reference recomputes the gain base 25 / 8 = 3: the
    // published one already.
    let same = PhyTxPowerTrackingParameters {
        wifi_gain_base: 3,
        ..PARAMETERS
    };
    assert_eq!(
        PhyTxPowerTrackingTransition::new(REQUEST, same).action(),
        PhyTxPowerTrackingAction::Complete(PhyTxPowerTrackingOutcome {
            gain_updated: false,
            tracking_temperature: same.previous_tracking_temperature,
            tracking_gain_base: same.previous_tracking_gain_base,
            wifi_gain_base: same.wifi_gain_base,
        })
    );
}

/// Complete every pending force-TX/RX edge; returns the configured writes.
fn complete_force(
    transition: &mut PhyTxPowerTrackingTransition,
) -> std::vec::Vec<PhyForceTxRxPhase> {
    let mut writes = std::vec::Vec::new();
    while let PhyTxPowerTrackingAction::ForceTxRx(action) = transition.action() {
        let completion = match action {
            PhyForceTxRxEdge::Configure { phase } => {
                writes.push(phase);
                PhyForceTxRxCompletion::Configured { phase }
            }
            PhyForceTxRxEdge::DelayMicros {
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
        };
        transition
            .advance(PhyTxPowerTrackingCompletion::ForceTxRx(completion))
            .unwrap();
    }
    writes
}

#[test]
fn wifi_update_owns_bbpll_and_gain_order() {
    let mut transition = PhyTxPowerTrackingTransition::new(REQUEST, PARAMETERS);
    assert_eq!(transition.decision().gain_base, 3);
    assert_eq!(
        transition.action(),
        PhyTxPowerTrackingAction::SetBbpllCalibration {
            calibration: PhyBbpllCalibration::Enabled
        }
    );
    transition
        .advance(PhyTxPowerTrackingCompletion::BbpllCalibrationSet {
            calibration: PhyBbpllCalibration::Enabled,
        })
        .unwrap();
    // The gain child runs at nesting level zero and owns the force pair.
    assert_eq!(
        complete_force(&mut transition),
        [phase_image(true, 0), phase_image(true, 1)]
    );
    assert_eq!(
        transition.action(),
        PhyTxPowerTrackingAction::RegenerateWifiGain {
            channel: 11,
            gain_base: 3
        }
    );
    transition.gain_published().unwrap();
    assert_eq!(
        complete_force(&mut transition),
        [phase_image(false, 0), phase_image(false, 1)]
    );
    transition
        .advance(PhyTxPowerTrackingCompletion::BbpllCalibrationSet {
            calibration: PhyBbpllCalibration::Disabled,
        })
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyTxPowerTrackingAction::Complete(PhyTxPowerTrackingOutcome {
            gain_updated: true,
            tracking_temperature: PARAMETERS.current_temperature,
            tracking_gain_base: 3,
            wifi_gain_base: 3,
        })
    );
}

#[test]
fn skipped_wifi_publication_leaves_force_mode_unchanged() {
    let mut transition = PhyTxPowerTrackingTransition::new(
        REQUEST,
        PhyTxPowerTrackingParameters {
            wifi_gain_publication_skipped: true,
            ..PARAMETERS
        },
    );
    transition
        .advance(PhyTxPowerTrackingCompletion::BbpllCalibrationSet {
            calibration: PhyBbpllCalibration::Enabled,
        })
        .unwrap();
    assert!(complete_force(&mut transition).is_empty());
    transition.gain_published().unwrap();
    assert!(complete_force(&mut transition).is_empty());
    assert_eq!(
        transition.action(),
        PhyTxPowerTrackingAction::SetBbpllCalibration {
            calibration: PhyBbpllCalibration::Disabled
        }
    );
}

/// The force-mode image the vendor writes for a sequence of `enabled`
/// at its `phase` (0 the initial write, 1 the final), written apart from
/// the code under test.
fn phase_image(enabled: bool, phase: u8) -> PhyForceTxRxPhase {
    match (enabled, phase) {
        (true, 0) => PhyForceTxRxPhase::EnableInitialPhase,
        (true, _) => PhyForceTxRxPhase::EnableFinalPhase,
        (false, 0) => PhyForceTxRxPhase::DisableInitialPhase,
        (false, _) => PhyForceTxRxPhase::DisableFinalPhase,
    }
}

impl PhyTxPowerTrackingTransition {
    /// The decision the transition was made with, for the tests' read-back.
    pub(crate) const fn decision(self) -> PhyTxPowerTrackingDecision {
        self.decision
    }
}

impl ControlBinding {
    /// The action a control edge lowers, for the executor's test port.
    pub(crate) const fn action(&self) -> PhyTxPowerTrackingAction {
        match self.0 {
            ControlEdge::SetBbpllCalibration { calibration } => {
                PhyTxPowerTrackingAction::SetBbpllCalibration { calibration }
            }
            ControlEdge::ForceTxRx(edge) => PhyTxPowerTrackingAction::ForceTxRx(edge),
        }
    }
}
