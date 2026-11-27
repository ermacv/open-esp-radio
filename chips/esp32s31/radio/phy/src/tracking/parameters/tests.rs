use super::*;
use std::vec::Vec;

const POLICY: PhyParamTrackingPolicy = PhyParamTrackingPolicy {
    tracking_inhibited: false,
    rfpll_cap_tracking_enabled: true,
    rfpll_cap_tracking_threshold: None,
    calibration_tracking_threshold: None,
    diagnostics: PhyTrackingDiagnostics::Enabled,
    calibration_tracking_enabled: true,
    relaxed_power_tracking_threshold: false,
};

fn completion(action: PhyParamTrackingAction) -> PhyParamTrackingCompletion {
    match action {
        PhyParamTrackingAction::EnterCritical => PhyParamTrackingCompletion::EnteredCritical,
        PhyParamTrackingAction::RfpllCapTrack { .. } => {
            PhyParamTrackingCompletion::RfpllCapTracked(PhyParamTrackingRfpllCompletion {
                committed: (),
                corrected: false,
            })
        }
        PhyParamTrackingAction::CalibrationTrack { .. } => {
            PhyParamTrackingCompletion::CalibrationTracked(PhyParamTrackingCalibrationCompletion {
                common_updated: false,
                transmit_updated: false,
            })
        }
        PhyParamTrackingAction::WifiI2cTrack => PhyParamTrackingCompletion::WifiI2cTracked {
            band_changed: false,
        },
        PhyParamTrackingAction::WifiTxPowerTrack { enabled, .. } => {
            PhyParamTrackingCompletion::WifiTxPowerTracked {
                enabled,
                gain_updated: false,
            }
        }
        PhyParamTrackingAction::TemperatureRead => PhyParamTrackingCompletion::TemperatureRead,
        PhyParamTrackingAction::ExitCritical => PhyParamTrackingCompletion::ExitedCritical,
        PhyParamTrackingAction::Complete(_) => panic!("terminal action has no completion"),
    }
}

fn run(policy: PhyParamTrackingPolicy) -> Vec<PhyParamTrackingAction> {
    let mut transition = PhyParamTrackingTransition::new(policy);
    let mut actions = Vec::new();
    for _ in 0..10 {
        let action = transition.action();
        actions.push(action);
        if matches!(action, PhyParamTrackingAction::Complete(_)) {
            return actions;
        }
        transition.advance(completion(action)).unwrap();
    }
    panic!("finite tracking transition exceeded its maximum path length")
}

#[test]
fn wifi_runs_i2c_then_power_then_calibration_and_temperature_last() {
    assert_eq!(
        run(POLICY),
        [
            PhyParamTrackingAction::EnterCritical,
            PhyParamTrackingAction::RfpllCapTrack {
                diagnostics: PhyTrackingDiagnostics::Enabled,
            },
            PhyParamTrackingAction::WifiI2cTrack,
            PhyParamTrackingAction::WifiTxPowerTrack {
                enabled: true,
                diagnostics: PhyTrackingDiagnostics::Enabled,
            },
            PhyParamTrackingAction::CalibrationTrack {
                diagnostics: PhyTrackingDiagnostics::Enabled,
            },
            PhyParamTrackingAction::TemperatureRead,
            PhyParamTrackingAction::ExitCritical,
            PhyParamTrackingAction::Complete(PhyParamTrackingOutcome {
                tracking_inhibited: false,
                rfpll_corrected: false,
                wifi_i2c_changed: false,
                tx_power: TxPowerProgress::default(),
                calibration: CalibrationProgress::default(),
            }),
        ]
    );
}

#[test]
fn guard_exits_critical_section_without_running_children() {
    let mut policy = POLICY;
    policy.tracking_inhibited = true;
    assert_eq!(
        run(policy),
        [
            PhyParamTrackingAction::EnterCritical,
            PhyParamTrackingAction::ExitCritical,
            PhyParamTrackingAction::Complete(PhyParamTrackingOutcome {
                tracking_inhibited: true,
                rfpll_corrected: false,
                wifi_i2c_changed: false,
                tx_power: TxPowerProgress::default(),
                calibration: CalibrationProgress::default(),
            }),
        ]
    );
}

#[test]
fn disabled_optional_branches_are_absent() {
    let policy = PhyParamTrackingPolicy {
        rfpll_cap_tracking_enabled: false,
        calibration_tracking_enabled: false,
        ..POLICY
    };
    assert_eq!(
        run(policy),
        [
            PhyParamTrackingAction::EnterCritical,
            PhyParamTrackingAction::WifiI2cTrack,
            PhyParamTrackingAction::WifiTxPowerTrack {
                enabled: true,
                diagnostics: PhyTrackingDiagnostics::Enabled,
            },
            PhyParamTrackingAction::TemperatureRead,
            PhyParamTrackingAction::ExitCritical,
            PhyParamTrackingAction::Complete(PhyParamTrackingOutcome {
                tracking_inhibited: false,
                rfpll_corrected: false,
                wifi_i2c_changed: false,
                tx_power: TxPowerProgress::default(),
                calibration: CalibrationProgress::default(),
            }),
        ]
    );
}

#[test]
fn wrong_completion_does_not_advance_and_terminal_rejects_more_work() {
    let mut transition = PhyParamTrackingTransition::new(POLICY);
    assert_eq!(
        transition.advance(PhyParamTrackingCompletion::TemperatureRead),
        Err(PhyParamTrackingTransitionError::WrongCompletion)
    );
    assert_eq!(transition.action(), PhyParamTrackingAction::EnterCritical);

    while !matches!(transition.action(), PhyParamTrackingAction::Complete(_)) {
        transition.advance(completion(transition.action())).unwrap();
    }
    assert_eq!(
        transition.advance(PhyParamTrackingCompletion::ExitedCritical),
        Err(PhyParamTrackingTransitionError::AlreadyComplete)
    );
}

#[test]
fn rfpll_child_routes_threshold_and_mints_parent_proof_only_after_commit() {
    let policy = PhyParamTrackingPolicy {
        rfpll_cap_tracking_threshold: Some(6),
        calibration_tracking_enabled: false,
        ..POLICY
    };
    let mut transition = PhyParamTrackingTransition::new(policy);
    transition
        .advance(PhyParamTrackingCompletion::EnteredCritical)
        .unwrap();

    let mut state = crate::state::PhyState::new(crate::state::PhyConfig::production());
    state.apply_register_temperature_outcome(
        crate::state::PhyRegisterTemperatureControl::FULL,
        crate::analog::temperature::PhyTemperatureOutcome {
            temperature: 20,
            sensor_index: 2,
            next_dac: 15,
        },
    );
    state.apply_temperature_outcome(crate::analog::temperature::PhyTemperatureOutcome {
        temperature: 25,
        sensor_index: 2,
        next_dac: 15,
    });

    let child = transition.begin_rfpll_cap_tracking(&mut state).unwrap();
    let crate::tracking::rfpll::thermal::Action::Complete(outcome) = child.action() else {
        panic!("six-unit override must skip a five-unit delta")
    };
    assert!(outcome.correction.is_none());
    let completion = child.commit().unwrap();
    assert_eq!(state.rfpll_tracking_request(None).reference_temperature, 20);
    transition.advance(completion).unwrap();
    assert!(matches!(
        transition.action(),
        PhyParamTrackingAction::WifiI2cTrack
    ));

    let mut incomplete = PhyParamTrackingTransition::new(PhyParamTrackingPolicy {
        rfpll_cap_tracking_threshold: None,
        ..policy
    });
    incomplete
        .advance(PhyParamTrackingCompletion::EnteredCritical)
        .unwrap();
    state.apply_temperature_outcome(crate::analog::temperature::PhyTemperatureOutcome {
        temperature: 35,
        sensor_index: 2,
        next_dac: 15,
    });
    let child = incomplete.begin_rfpll_cap_tracking(&mut state).unwrap();
    assert_eq!(
        child.action(),
        crate::tracking::rfpll::thermal::Action::SetGrantProtect { enabled: true }
    );
    let child = child.commit().unwrap_err();
    assert_eq!(
        child.action(),
        crate::tracking::rfpll::thermal::Action::SetGrantProtect { enabled: true }
    );
}
#[test]
fn measured_rfpll_reference_waits_for_restoration_and_parent_commit() {
    use crate::tracking::rfpll::{self, search, thermal};
    let mut parent = PhyParamTrackingTransition::new(POLICY);
    parent
        .advance(PhyParamTrackingCompletion::EnteredCritical)
        .unwrap();
    let mut state = crate::state::PhyState::new(crate::state::PhyConfig::production());
    state.apply_register_temperature_outcome(
        crate::state::PhyRegisterTemperatureControl::FULL,
        crate::analog::temperature::PhyTemperatureOutcome {
            temperature: 20,
            sensor_index: 2,
            next_dac: 15,
        },
    );
    state.apply_temperature_outcome(crate::analog::temperature::PhyTemperatureOutcome {
        temperature: 35,
        sensor_index: 2,
        next_dac: 15,
    });
    let mut child = parent.begin_rfpll_cap_tracking(&mut state).unwrap();
    for _ in 0..256 {
        assert_eq!(
            child
                .state()
                .rfpll_tracking_request(None)
                .reference_temperature,
            20
        );
        if child.action() == thermal::Action::RestoreHardwareControl {
            break;
        }
        child = child
            .commit()
            .expect_err("no parent proof before physical restoration");
        let completion = match child.action() {
            thermal::Action::SetGrantProtect { enabled } => {
                thermal::Completion::GrantProtectSet { enabled }
            }
            thermal::Action::SelectSoftwareControl => thermal::Completion::SoftwareControlSelected,
            thermal::Action::Settle => thermal::Completion::Settled,
            thermal::Action::ObserveBoundary => thermal::Completion::BoundaryObserved,
            thermal::Action::Correct(rfpll::Action::Search(action)) => {
                let completion = match action {
                    search::Action::ReadInitialCap => search::Completion::InitialCap(100),
                    search::Action::EnableSearch => search::Completion::SearchEnabled,
                    search::Action::WriteCap(value) => search::Completion::CapWritten(value),
                    search::Action::DelayMicros(value) => search::Completion::DelayElapsed(value),
                    search::Action::ReadStatus => search::Completion::Status(search::Status::Other),
                    _ => panic!("unexpected search terminal"),
                };
                child
                    .transition_mut()
                    .correction_mut()
                    .expect("admitted correction")
                    .advance(rfpll::Completion::Search(completion))
                    .unwrap();
                if matches!(
                    child.action(),
                    thermal::Action::Correct(rfpll::Action::Complete(_))
                ) {
                    child
                        .transition_mut()
                        .finish_correction()
                        .expect("correction complete");
                }
                continue;
            }
            action => panic!("unexpected action {action:?}"),
        };
        child.transition_mut().advance(completion).unwrap();
    }
    assert_eq!(child.action(), thermal::Action::RestoreHardwareControl);
    child = child.commit().unwrap_err();
    assert_eq!(
        child.transition_mut().advance(thermal::Completion::Settled),
        Err(rfpll::Error::WrongCompletion)
    );
    child
        .transition_mut()
        .advance(thermal::Completion::HardwareControlRestored)
        .unwrap();
    // The correction withdraws its grant protection before the parent commit.
    assert_eq!(
        child.action(),
        thermal::Action::SetGrantProtect { enabled: false }
    );
    child = child.commit().unwrap_err();
    child
        .transition_mut()
        .advance(thermal::Completion::GrantProtectSet { enabled: false })
        .unwrap();
    assert_eq!(
        child
            .state()
            .rfpll_tracking_request(None)
            .reference_temperature,
        20
    );
    let proof = child.commit().unwrap();
    assert_eq!(state.rfpll_tracking_request(None).reference_temperature, 35);
    parent.advance(proof).unwrap();
}

#[test]
fn skipped_calibration_commits_without_hardware_or_reference_changes() {
    let policy = PhyParamTrackingPolicy {
        rfpll_cap_tracking_enabled: false,
        calibration_tracking_threshold: Some(31),
        ..POLICY
    };
    let mut transition = PhyParamTrackingTransition::new(policy);
    transition
        .advance(PhyParamTrackingCompletion::EnteredCritical)
        .unwrap();
    transition
        .advance(PhyParamTrackingCompletion::WifiI2cTracked {
            band_changed: false,
        })
        .unwrap();
    transition
        .advance(PhyParamTrackingCompletion::WifiTxPowerTracked {
            enabled: true,
            gain_updated: false,
        })
        .unwrap();

    let mut state = crate::state::PhyState::new(crate::state::PhyConfig::production());
    state.apply_register_temperature_outcome(
        crate::state::PhyRegisterTemperatureControl::FULL,
        crate::analog::temperature::PhyTemperatureOutcome {
            temperature: 20,
            sensor_index: 2,
            next_dac: 15,
        },
    );
    state.apply_rx_gain_init_outcome(crate::rx::gain::PhyRxGainInitOutcome {
        dc: None,
        generated_tables: true,
        wifi_last_index: 69,
        shared_last_index: 75,
    });
    state.apply_temperature_outcome(crate::analog::temperature::PhyTemperatureOutcome {
        temperature: 50,
        sensor_index: 2,
        next_dac: 15,
    });

    let child = transition.begin_calibration_tracking(&mut state).unwrap();
    assert_eq!(child.parent_action(), transition.action());
    assert!(matches!(
        child.action(),
        crate::tracking::calibration::PhyCalibrationTrackingAction::Complete(_)
    ));
    assert!(child.lower_external().is_err());
    let completion = child.commit().unwrap();

    let committed = state.calibration_tracking_parameters(None);
    assert_eq!(committed.common_reference_temperature, 20);
    assert_eq!(committed.transmit_reference_temperature, 20);
    transition.advance(completion).unwrap();
    assert_eq!(transition.action(), PhyParamTrackingAction::TemperatureRead);
    transition
        .advance(PhyParamTrackingCompletion::TemperatureRead)
        .unwrap();
    transition
        .advance(PhyParamTrackingCompletion::ExitedCritical)
        .unwrap();
    let PhyParamTrackingAction::Complete(outcome) = transition.action() else {
        panic!("complete")
    };
    assert_eq!(outcome.calibration, CalibrationProgress::default());
}

#[test]
fn calibration_progress_counts_only_accepted_completed_branches() {
    let mut transition = PhyParamTrackingTransition::new(POLICY);
    while !matches!(transition.action(), PhyParamTrackingAction::Complete(_)) {
        let action = transition.action();
        let completed = if let PhyParamTrackingAction::CalibrationTrack { .. } = action {
            let before = transition;
            assert_eq!(
                transition.advance(PhyParamTrackingCompletion::TemperatureRead),
                Err(PhyParamTrackingTransitionError::WrongCompletion)
            );
            assert_eq!(transition, before);
            PhyParamTrackingCompletion::CalibrationTracked(PhyParamTrackingCalibrationCompletion {
                common_updated: true,
                transmit_updated: true,
            })
        } else {
            completion(action)
        };
        transition.advance(completed).unwrap();
    }
    let PhyParamTrackingAction::Complete(outcome) = transition.action() else {
        panic!("complete")
    };
    assert_eq!(
        outcome.calibration,
        CalibrationProgress {
            common: true,
            transmit: true,
        }
    );
}

#[test]
fn failed_temperature_child_cannot_complete_parent_or_mutate_state() {
    let mut policy = POLICY;
    policy.rfpll_cap_tracking_enabled = false;
    policy.calibration_tracking_enabled = false;
    let mut transition = PhyParamTrackingTransition::new(policy);
    for done in [
        PhyParamTrackingCompletion::EnteredCritical,
        PhyParamTrackingCompletion::WifiI2cTracked {
            band_changed: false,
        },
        PhyParamTrackingCompletion::WifiTxPowerTracked {
            enabled: true,
            gain_updated: false,
        },
    ] {
        transition.advance(done).unwrap();
    }
    assert_eq!(transition.action(), PhyParamTrackingAction::TemperatureRead);

    let mut state = crate::state::PhyState::new(crate::state::PhyConfig::production());
    let mut temperature = transition.begin_temperature_read(&mut state).unwrap();
    let crate::analog::temperature::PhyTemperatureAction::ReadMasked { field } =
        temperature.action()
    else {
        panic!("temperature child did not begin with its DAC read")
    };
    temperature
        .transition_mut()
        .advance(
            crate::analog::temperature::PhyTemperatureCompletion::MaskedRead { field, value: 3 },
        )
        .unwrap();
    assert_eq!(
        temperature.action(),
        crate::analog::temperature::PhyTemperatureAction::Failed(
            crate::analog::temperature::PhyTemperatureFailure::InvalidDac(3),
        )
    );
    let temperature = temperature.commit_observed(None, None).unwrap_err();
    assert_eq!(
        temperature
            .state()
            .tx_power_tracking_parameters(false)
            .current_temperature,
        0
    );
}

#[test]
fn registered_policy_projects_cold_facts_and_live_tracking_state() {
    let mut state = crate::state::PhyState::new(crate::state::PhyConfig::production());

    let cold = PhyParamTrackingPolicy::for_registered_state(&state);
    assert!(!cold.tracking_inhibited);
    assert!(cold.rfpll_cap_tracking_enabled);
    assert_eq!(cold.rfpll_cap_tracking_threshold, None);
    assert_eq!(cold.calibration_tracking_threshold, None);
    assert_eq!(cold.diagnostics, PhyTrackingDiagnostics::Disabled);
    assert!(cold.calibration_tracking_enabled);
    assert!(cold.relaxed_power_tracking_threshold);

    state.set_tx_power_tracking_slow(0);
    state.set_temperature_tracking_debug(3, 17);
    let configured = PhyParamTrackingPolicy::for_registered_state(&state);
    assert_eq!(configured.rfpll_cap_tracking_threshold, Some(17));
    assert_eq!(configured.calibration_tracking_threshold, Some(17));
    assert!(!configured.relaxed_power_tracking_threshold);
}

#[test]
fn temperature_to_power_matches_all_signed_16_bit_deltas() {
    for raw_delta in 0..=u16::MAX {
        let delta = raw_delta as i16;
        assert_eq!(
            temperature_to_tracking_power(0, 0_i16.wrapping_sub(delta)),
            if delta > 0 {
                (delta / 8) as i8
            } else {
                ((i32::from(delta) / 3) as u8) as i8
            },
        );
    }
}

#[test]
fn outcome_reports_the_children_that_committed_an_update() {
    let mut transition = PhyParamTrackingTransition::new(POLICY);
    let outcome = loop {
        let completion = match transition.action() {
            PhyParamTrackingAction::Complete(outcome) => break outcome,
            PhyParamTrackingAction::RfpllCapTrack { .. } => {
                PhyParamTrackingCompletion::RfpllCapTracked(PhyParamTrackingRfpllCompletion {
                    committed: (),
                    corrected: true,
                })
            }
            PhyParamTrackingAction::WifiI2cTrack => {
                PhyParamTrackingCompletion::WifiI2cTracked { band_changed: true }
            }
            PhyParamTrackingAction::WifiTxPowerTrack { enabled, .. } => {
                PhyParamTrackingCompletion::WifiTxPowerTracked {
                    enabled,
                    gain_updated: true,
                }
            }
            action => completion(action),
        };
        transition.advance(completion).unwrap();
    };
    assert!(outcome.rfpll_corrected);
    assert!(outcome.wifi_i2c_changed);
    assert_eq!(outcome.tx_power, TxPowerProgress { wifi: true });
}

#[test]
fn calibration_completion_reads_back_its_committed_branches() {
    let completion = PhyParamTrackingCalibrationCompletion {
        common_updated: true,
        transmit_updated: false,
    };
    assert!(completion.common_updated());
    assert!(!completion.transmit_updated());
}

// The tests' read-backs: no target path reads them.
impl PhyTrackingDiagnostics {
    pub(crate) const fn enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

impl PhyParamTrackingCalibrationCompletion {
    pub(crate) const fn common_updated(&self) -> bool {
        self.common_updated
    }

    pub(crate) const fn transmit_updated(&self) -> bool {
        self.transmit_updated
    }
}

impl PhyParamTrackingRfpllTransition<'_> {
    pub(crate) const fn parent_action(&self) -> PhyParamTrackingAction {
        self.parent_action
    }
}

impl PhyParamTrackingCalibrationTransition<'_> {
    pub(crate) const fn parent_action(&self) -> PhyParamTrackingAction {
        self.parent_action
    }

    pub(crate) const fn state(&self) -> &crate::state::PhyState {
        self.state
    }
}

impl PhyParamTrackingTxPowerTransition<'_> {
    pub(crate) const fn parent_action(&self) -> PhyParamTrackingAction {
        PhyParamTrackingAction::WifiTxPowerTrack {
            enabled: self.child.request().enabled,
            diagnostics: self.diagnostics,
        }
    }

    pub(crate) const fn state(&self) -> &crate::state::PhyState {
        self.state
    }
}

impl PhyParamTrackingWifiI2cTransition<'_> {
    pub(crate) const fn state(&self) -> &crate::state::PhyState {
        self.state
    }
}

impl PhyParamTrackingTemperatureTransition<'_> {
    pub(crate) const fn action(&self) -> crate::analog::temperature::PhyTemperatureAction {
        self.child.action()
    }

    pub(crate) const fn state(&self) -> &crate::state::PhyState {
        self.state
    }
}
