use super::*;

const PARAMETERS: PhyChipChannelParameters = PhyChipChannelParameters {
    frequency_offset: 0,
    crystal_selector: 3,
    channel_14_mic_enabled: false,
    tx_gain_skip_publication: false,
    tx_gain_seed: [1, 2, 3, 4, 5, 6],
    tx_gain_config: 0x1234,
    tx_gain_curve: [7, 8, 9, 10, 11, 12],
    tx_gain_correction: -3,
    tx_gain_base: 20,
    tx_gain_adjustment: 2,
    tx_capacitance: [1, 2, 3, 4, 5, 6],
};

const REQUEST: PhyChipChannelRequest = PhyChipChannelRequest {
    channel_or_frequency: 11,
    cbw: 0,
    parameters: PARAMETERS,
};

#[test]
fn channel_gain_adds_independent_adjustment_with_signed_byte_wrapping() {
    for (base, adjustment, expected) in [(20, 2, 22), (127, 1, -128), (128, -1, 127), (0, -1, -1)] {
        let transition = PhyChipChannelTransition::new(PhyChipChannelRequest {
            parameters: PhyChipChannelParameters {
                tx_gain_base: base,
                tx_gain_adjustment: adjustment,
                ..PARAMETERS
            },
            ..REQUEST
        });
        assert_eq!(transition.tx_gain_request().base_and_delta, expected);
    }
}

fn temperature_completion(action: PhyTemperatureAction) -> PhyTemperatureCompletion {
    match action {
        PhyTemperatureAction::ReadMasked { field } => {
            PhyTemperatureCompletion::MaskedRead { field, value: 15 }
        }
        PhyTemperatureAction::SampleCode => PhyTemperatureCompletion::CodeSampled { value: 128 },
        PhyTemperatureAction::WriteMasked { field, value } => {
            PhyTemperatureCompletion::MaskedWrite { field, value }
        }
        action => panic!("unexpected terminal temperature action: {action:?}"),
    }
}

fn direct_completion(action: PhyChipChannelAction, ready: bool) -> PhyChipChannelCompletion {
    match action {
        PhyChipChannelAction::SetAgc { enabled } => PhyChipChannelCompletion::AgcSet { enabled },
        PhyChipChannelAction::ConfigureForceTxRx { enabled, phase } => {
            PhyChipChannelCompletion::ForceTxRxConfigured { enabled, phase }
        }
        PhyChipChannelAction::SetBbpllCalibration { enabled } => {
            PhyChipChannelCompletion::BbpllCalibrationSet { enabled }
        }
        PhyChipChannelAction::Temperature(action) => {
            PhyChipChannelCompletion::Temperature(temperature_completion(action))
        }
        PhyChipChannelAction::StartFrequencySwitch {
            frequency_index,
            crystal_selector,
        } => PhyChipChannelCompletion::FrequencySwitchStarted {
            frequency_index,
            crystal_selector,
        },
        PhyChipChannelAction::DelayMicros { phase, micros } => {
            PhyChipChannelCompletion::DelayElapsed { phase, micros }
        }
        PhyChipChannelAction::ClearFrequencySwitch => {
            PhyChipChannelCompletion::FrequencySwitchCleared
        }
        PhyChipChannelAction::AwaitFrequencyReadyEdge { .. } => {
            PhyChipChannelCompletion::FrequencyReadyObserved { ready }
        }
        PhyChipChannelAction::ConfigureNrx { frequency_mhz } => {
            PhyChipChannelCompletion::NrxConfigured { frequency_mhz }
        }
        PhyChipChannelAction::ConfigureBssCbw { cbw } => {
            PhyChipChannelCompletion::BssCbwConfigured { cbw }
        }
        PhyChipChannelAction::ConfigureRxCompensation => {
            PhyChipChannelCompletion::RxCompensationConfigured
        }
        PhyChipChannelAction::WriteI2c {
            phase,
            address,
            value,
        } => PhyChipChannelCompletion::I2cWriteCompleted {
            phase,
            address,
            value,
        },
        PhyChipChannelAction::CalculateTxGain(request) => {
            PhyChipChannelCompletion::TxGainCalculated {
                request,
                image: PhyWifiTxGainImage {
                    seed: [0; 6],
                    output_32: [0x20; 8],
                    output_64: [0x40; 16],
                    output_72: [0x48; 16],
                    config: 0,
                },
            }
        }
        PhyChipChannelAction::PublishTxGain(_) => PhyChipChannelCompletion::TxGainPublished,
        PhyChipChannelAction::ReadI2c { phase, address } => {
            PhyChipChannelCompletion::I2cReadCompleted {
                phase,
                address,
                value: 0xc5,
            }
        }
        PhyChipChannelAction::PublishTxCapCommandMemory { value } => {
            PhyChipChannelCompletion::TxCapCommandMemoryPublished { value }
        }
        PhyChipChannelAction::ConfigureChannelCbw { cbw } => {
            PhyChipChannelCompletion::ChannelCbwConfigured { cbw }
        }
        PhyChipChannelAction::ClearDcMemory => PhyChipChannelCompletion::DcMemoryCleared,
        action => panic!("unexpected terminal channel action: {action:?}"),
    }
}

#[test]
fn pure_channel_frequency_helpers_match_24ghz_reference_edges() {
    assert_eq!(channel_to_frequency(1), 2_412);
    assert_eq!(channel_to_frequency(11), 2_462);
    assert_eq!(channel_to_frequency(14), 2_484);
    assert_eq!(channel_to_frequency(2_462), 2_462);
    assert_eq!(frequency_to_channel(2_412), 1);
    assert_eq!(frequency_to_channel(2_462), 11);
    assert_eq!(frequency_to_channel(2_484), 14);
}

/// Force-TX/RX writes and the neighbouring AGC/BBPLL edges of one channel run.
fn force_edges(mut transition: PhyChipChannelTransition) -> std::vec::Vec<PhyChipChannelAction> {
    let mut edges = std::vec::Vec::new();
    loop {
        let action = transition.action();
        match action {
            PhyChipChannelAction::Complete(_) | PhyChipChannelAction::Failed(_) => return edges,
            PhyChipChannelAction::SetAgc { .. }
            | PhyChipChannelAction::SetBbpllCalibration { .. }
            | PhyChipChannelAction::ConfigureForceTxRx { .. }
            | PhyChipChannelAction::ClearDcMemory => edges.push(action),
            PhyChipChannelAction::DelayMicros {
                phase: PhyChipChannelDelay::ForceTxRx { .. },
                ..
            } => edges.push(action),
            _ => {}
        }
        transition.advance(direct_completion(action, true)).unwrap();
    }
}

#[test]
fn channel_force_pairs_follow_the_vendor_nesting_count() {
    let sequence = |enabled| {
        (0..2).flat_map(move |phase| {
            [
                PhyChipChannelAction::ConfigureForceTxRx { enabled, phase },
                PhyChipChannelAction::DelayMicros {
                    phase: PhyChipChannelDelay::ForceTxRx {
                        enabled,
                        completed_phase: phase,
                    },
                    micros: 1,
                },
            ]
        })
    };
    let agc = |enabled| PhyChipChannelAction::SetAgc { enabled };
    let bbpll = |enabled| PhyChipChannelAction::SetBbpllCalibration { enabled };
    let depth_one = PhyForceTxRxDepth::OUTERMOST.nested();
    // Outermost: force, re-force when the nested TX-gain pair leaves the
    // count at one, release when the root leaves it at zero.
    let outermost: std::vec::Vec<_> = [agc(false)]
        .into_iter()
        .chain(sequence(true))
        .chain([bbpll(true)])
        .chain(sequence(true))
        .chain([bbpll(false)])
        .chain(sequence(false))
        .chain([PhyChipChannelAction::ClearDcMemory, agc(true)])
        .collect();
    assert_eq!(
        force_edges(PhyChipChannelTransition::new(REQUEST)),
        outermost
    );
    // Depth one: only the root's release, which leaves the count at one.
    let nested: std::vec::Vec<_> = [agc(false), bbpll(true), bbpll(false)]
        .into_iter()
        .chain(sequence(true))
        .chain([PhyChipChannelAction::ClearDcMemory, agc(true)])
        .collect();
    assert_eq!(
        force_edges(PhyChipChannelTransition::at_depth(REQUEST, depth_one)),
        nested
    );
    // Depth two: every pair stays above one.
    assert_eq!(
        force_edges(PhyChipChannelTransition::at_depth(
            REQUEST,
            depth_one.nested()
        )),
        [
            agc(false),
            bbpll(true),
            bbpll(false),
            PhyChipChannelAction::ClearDcMemory,
            agc(true),
        ]
    );
}

#[test]
fn cold_channel_eleven_traverses_the_complete_ordered_graph() {
    let mut transition = PhyChipChannelTransition::new(REQUEST);
    let mut actions = 0;
    let mut ready_samples = 0;
    let mut rx_compensation_count = 0;
    let mut saw_gain_image = false;

    loop {
        actions += 1;
        assert!(actions < 80);
        let action = transition.action();
        match action {
            PhyChipChannelAction::AwaitFrequencyReadyEdge { samples, .. } => {
                assert_eq!(samples, ready_samples);
                ready_samples += 1;
            }
            PhyChipChannelAction::ConfigureRxCompensation => {
                rx_compensation_count += 1;
            }
            PhyChipChannelAction::PublishTxGain(image) => {
                saw_gain_image = true;
                assert_eq!(image.seed, PARAMETERS.tx_gain_seed);
                assert_eq!(image.config, PARAMETERS.tx_gain_config);
            }
            PhyChipChannelAction::Complete(outcome) => {
                assert_eq!(outcome.channel, 11);
                assert_eq!(outcome.frequency_mhz, 2_462);
                assert_eq!(outcome.cbw, 0);
                assert!(!outcome.init_complete);
                break;
            }
            PhyChipChannelAction::Failed(failure) => {
                panic!("channel transition failed: {failure:?}")
            }
            _ => {}
        }
        let completion = direct_completion(action, ready_samples == 3);
        transition.advance(completion).unwrap();
    }

    assert_eq!(ready_samples, 3);
    assert_eq!(rx_compensation_count, 2);
    assert!(saw_gain_image);
}

#[test]
fn off_grid_frequency_uses_raw_then_channel_normalized_nrx_values() {
    let mut request = REQUEST;
    request.channel_or_frequency = 2_413;
    let mut transition = PhyChipChannelTransition::new(request);
    let mut nrx = [0_u16; 2];
    let mut nrx_count = 0;

    loop {
        let action = transition.action();
        match action {
            PhyChipChannelAction::ConfigureNrx { frequency_mhz } => {
                nrx[nrx_count] = frequency_mhz;
                nrx_count += 1;
            }
            PhyChipChannelAction::Complete(outcome) => {
                assert_eq!(outcome.channel, 1);
                assert_eq!(outcome.frequency_mhz, 2_413);
                break;
            }
            PhyChipChannelAction::Failed(failure) => {
                panic!("off-grid channel transition failed: {failure:?}")
            }
            _ => {}
        }
        transition.advance(direct_completion(action, true)).unwrap();
    }

    assert_eq!(nrx, [2_413, 2_412]);
}

#[test]
fn frequency_timeout_runs_full_radio_cleanup() {
    let mut transition = PhyChipChannelTransition::new(REQUEST);
    loop {
        match transition.action() {
            PhyChipChannelAction::AwaitFrequencyReadyEdge { samples: 2, .. } => {
                transition
                    .advance(PhyChipChannelCompletion::FrequencyReadyTimedOut)
                    .unwrap();
                break;
            }
            action => {
                let completion = direct_completion(action, false);
                transition.advance(completion).unwrap();
            }
        }
    }

    assert_eq!(
        transition.action(),
        PhyChipChannelAction::SetBbpllCalibration { enabled: false }
    );
    transition
        .advance(PhyChipChannelCompletion::BbpllCalibrationSet { enabled: false })
        .unwrap();
    // The failed channel still leaves its force-TX/RX level.
    for phase in 0..2 {
        assert_eq!(
            transition.action(),
            PhyChipChannelAction::ConfigureForceTxRx {
                enabled: false,
                phase
            }
        );
        let completion = direct_completion(transition.action(), false);
        transition.advance(completion).unwrap();
        let completion = direct_completion(transition.action(), false);
        transition.advance(completion).unwrap();
    }
    assert_eq!(transition.action(), PhyChipChannelAction::ClearDcMemory);
    transition
        .advance(PhyChipChannelCompletion::DcMemoryCleared)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyChipChannelAction::SetAgc { enabled: true }
    );
    transition
        .advance(PhyChipChannelCompletion::AgcSet { enabled: true })
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyChipChannelAction::Failed(PhyChipChannelFailure::FrequencyReadyTimedOut { samples: 2 })
    );
}

#[test]
fn unsupported_profile_fails_before_touching_radio() {
    let mut request = REQUEST;
    request.parameters.channel_14_mic_enabled = true;
    assert_eq!(
        PhyChipChannelTransition::new(request).action(),
        PhyChipChannelAction::Failed(PhyChipChannelFailure::Channel14MicEnabled)
    );

    request.parameters.channel_14_mic_enabled = false;
    request.channel_or_frequency = 14;
    assert_eq!(
        PhyChipChannelTransition::new(request).action(),
        PhyChipChannelAction::Failed(PhyChipChannelFailure::UnsupportedChannel(14))
    );
}

#[test]
fn direct_binding_rejects_timer_i2c_and_terminal_actions() {
    assert_eq!(
        PhyChipChannelMmioBinding::new(PhyChipChannelAction::DelayMicros {
            phase: PhyChipChannelDelay::FrequencySettle,
            micros: 10,
        }),
        Err(PhyChipChannelBindingError::NotDirectMmio)
    );
    assert_eq!(
        PhyChipChannelMmioBinding::new(PhyChipChannelAction::WriteI2c {
            phase: PhyChipChannelI2cPhase::ProgramTxCap,
            address: TX_CAP_ADDRESS,
            value: 0xc1,
        }),
        Err(PhyChipChannelBindingError::NotDirectMmio)
    );
}

#[test]
fn external_lowering_covers_each_channel_operation_class() {
    assert!(matches!(
        PhyChipChannelExternalBinding::lower(PhyChipChannelAction::SetAgc { enabled: false }),
        Ok(PhyChipChannelExternalBinding::Mmio(_))
    ));
    assert!(matches!(
        PhyChipChannelExternalBinding::lower(PhyChipChannelAction::Temperature(
            PhyTemperatureTransition::new().action()
        )),
        Ok(PhyChipChannelExternalBinding::Temperature(_))
    ));
    assert!(matches!(
        PhyChipChannelExternalBinding::lower(PhyChipChannelAction::DelayMicros {
            phase: PhyChipChannelDelay::FrequencySettle,
            micros: 10,
        }),
        Ok(PhyChipChannelExternalBinding::Timer(_))
    ));
    assert!(matches!(
        PhyChipChannelExternalBinding::lower(PhyChipChannelAction::WriteI2c {
            phase: PhyChipChannelI2cPhase::ProgramTxCap,
            address: TX_CAP_ADDRESS,
            value: 0xc1,
        }),
        Ok(PhyChipChannelExternalBinding::I2c(_))
    ));
    assert!(matches!(
        PhyChipChannelExternalBinding::lower(PhyChipChannelAction::CalculateTxGain(
            PhyWifiTxGainRequest {
                channel: 1,
                calibration_curve: [0; 6],
                correction: 0,
                base_and_delta: 0,
            }
        )),
        Ok(PhyChipChannelExternalBinding::TxGain(_))
    ));
    assert!(matches!(
        PhyChipChannelExternalBinding::lower(PhyChipChannelAction::Failed(
            PhyChipChannelFailure::UnsupportedChannel(14)
        )),
        Err(PhyChipChannelExternalBindingError::UnsupportedAction)
    ));
}

#[test]
fn retained_wake_tx_cap_selects_both_channel_group_nibbles() {
    let capacitance = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
    assert_eq!(retained_tx_cap_value(1, capacitance), 0x21);
    assert_eq!(retained_tx_cap_value(3, capacitance), 0x21);
    assert_eq!(retained_tx_cap_value(4, capacitance), 0x43);
    assert_eq!(retained_tx_cap_value(8, capacitance), 0x43);
    assert_eq!(retained_tx_cap_value(9, capacitance), 0x65);
    assert_eq!(retained_tx_cap_value(13, capacitance), 0x65);
}
