use super::{
    PHY_TX_CFR_ENTRY_COUNT, PhyBbBasebandMode, PhyBbMmioAction, PhyBbMmioBinding,
    PhyGainMemoryEntry, PhyRegisterInitParameters, PhyRfRxSaturationPhase, PhyRxGainBank,
    PhyRxGainMemoryParameters, PhyRxTableInitParameters, PhyTxCfrAction, PhyTxCfrBindingError,
    PhyTxCfrCompletion, PhyTxCfrEntry, PhyTxCfrMmioBinding, PhyTxCfrOutcome, PhyTxCfrTransition,
    PhyTxCfrTransitionError, generate_phy_rx_gain_table, phy_generated_rx_gain_memory_entry,
};

#[test]
fn rx_gain_generator_reproduces_both_cold_parent_tables() {
    let wifi = generate_phy_rx_gain_table(PhyRxGainBank::Wifi);
    assert_eq!(wifi.last_index, 71);
    assert_eq!(wifi.words[0], 0x0004_0003);
    assert_eq!(wifi.words[70], 0x0007_f3c4);
    assert_eq!(wifi.words[71], 0x0007_f3c5);
    assert_ne!(wifi.words[70], 0);
    assert_ne!(wifi.words[71], 0);
    assert_eq!(wifi.words[72], 0);

    let shared = generate_phy_rx_gain_table(PhyRxGainBank::Shared);
    assert_eq!(shared.last_index, 75);
    assert_eq!(shared.words[0], 0x0004_0000);
    assert_eq!(shared.words[74], 0x0007_f380);
    assert_eq!(shared.words[75], 0x0007_f381);
    assert_eq!(shared.words[76], 0);
}

#[test]
fn rx_gain_memory_entry_uses_only_copied_owner_state() {
    let mut wifi_index_dc = [[0_u16; 2]; 8];
    wifi_index_dc[0] = [3, 4];
    let parameters = PhyRxGainMemoryParameters {
        parameter_002: 0xbf,
        wifi_index_dc,
        wifi_fine_dc: [[11, 22]; crate::rx::gain_calibration::FINE_CODES],
        shared_index_dc: [[0; 2]; 11],
        wifi_auxiliary: 5,
    };
    let table = generate_phy_rx_gain_table(PhyRxGainBank::Wifi);
    assert_eq!(
        phy_generated_rx_gain_memory_entry(&parameters, PhyRxGainBank::Wifi, &table, 0),
        PhyGainMemoryEntry::generated_receive(0, [11, 22], [3, 4], 5, table.words[0], 7, 0xbf,)
    );
}

#[test]
fn transition_reproduces_all_32_reference_entries() {
    let mut transition = PhyTxCfrTransition::new();
    assert_eq!(transition.action(), PhyTxCfrAction::ReadStartIndex);
    transition
        .advance(PhyTxCfrCompletion::StartIndexRead { base_index: 0xfa })
        .unwrap();

    for index in 0..PHY_TX_CFR_ENTRY_COUNT {
        let expected = PhyTxCfrEntry {
            index,
            start_index: 0xfa,
            data: if index < 10 { 0xe13 } else { 0 },
        };
        assert_eq!(transition.action(), PhyTxCfrAction::ProgramEntry(expected));
        transition
            .advance(PhyTxCfrCompletion::EntryProgrammed(expected))
            .unwrap();
    }

    assert_eq!(
        transition.action(),
        PhyTxCfrAction::Complete(PhyTxCfrOutcome {
            entries_written: 32,
            start_index: 0xfa,
        })
    );
}

#[test]
fn memory_index_preserves_reference_byte_wrapping() {
    assert_eq!(
        PhyTxCfrEntry {
            index: 31,
            start_index: 0xfa,
            data: 0,
        }
        .memory_index(),
        0x19
    );
}

#[test]
fn transition_rejects_foreign_or_late_completions() {
    let mut transition = PhyTxCfrTransition::new();
    transition
        .advance(PhyTxCfrCompletion::StartIndexRead { base_index: 0x12 })
        .unwrap();
    assert_eq!(
        transition.advance(PhyTxCfrCompletion::EntryProgrammed(PhyTxCfrEntry {
            index: 1,
            start_index: 0x12,
            data: 0xe13,
        })),
        Err(PhyTxCfrTransitionError::WrongCompletion)
    );

    for index in 0..PHY_TX_CFR_ENTRY_COUNT {
        transition
            .advance(PhyTxCfrCompletion::EntryProgrammed(PhyTxCfrEntry {
                index,
                start_index: 0x12,
                data: if index < 10 { 0xe13 } else { 0 },
            }))
            .unwrap();
    }
    assert_eq!(
        transition.advance(PhyTxCfrCompletion::StartIndexRead { base_index: 0 }),
        Err(PhyTxCfrTransitionError::AlreadyComplete)
    );
}

#[test]
fn binding_rejects_terminal_action_and_preserves_identity() {
    let entry = PhyTxCfrEntry {
        index: 7,
        start_index: 3,
        data: 0xe13,
    };
    let binding = PhyTxCfrMmioBinding::new(PhyTxCfrAction::ProgramEntry(entry)).unwrap();
    assert_eq!(binding.action(), PhyTxCfrAction::ProgramEntry(entry));
    assert_eq!(
        PhyTxCfrMmioBinding::new(PhyTxCfrAction::Complete(PhyTxCfrOutcome {
            entries_written: 32,
            start_index: 3,
        })),
        Err(PhyTxCfrBindingError::TerminalAction)
    );
}

#[test]
fn finite_baseband_mmio_binding_preserves_dynamic_identity() {
    for action in [
        PhyBbMmioAction::EnableBasebandInitialization,
        PhyBbMmioAction::SetBasebandMode {
            mode: PhyBbBasebandMode::Calibration,
        },
        PhyBbMmioAction::UpdateAgcRegisters,
        PhyBbMmioAction::UpdatePostInitRegisters,
        PhyBbMmioAction::EnableAgc,
        PhyBbMmioAction::SetWifiEnabled { enabled: false },
        PhyBbMmioAction::ConfigureTxPowerTracking { enabled: true },
        PhyBbMmioAction::ConfigureRfRxSaturation {
            phase: PhyRfRxSaturationPhase::PrepareCheck,
        },
        PhyBbMmioAction::ConfigureRfRxSaturation {
            phase: PhyRfRxSaturationPhase::Finalize,
        },
        PhyBbMmioAction::ConfigureI2cTxRate,
        PhyBbMmioAction::ConfigurePhyRegisters {
            parameters: PhyRegisterInitParameters {
                parameter_121: 0x4f,
                parameter_120: 0x4e,
            },
        },
        PhyBbMmioAction::ConfigureRxTable {
            parameters: PhyRxTableInitParameters {
                parameter_002: 0xa5,
                parameter_121: 0x4e,
            },
        },
    ] {
        assert_eq!(PhyBbMmioBinding::new(action).action(), action);
    }
}

fn complete_parent_mmio(transition: &mut super::PhyBbInitTransition, action: PhyBbMmioAction) {
    transition
        .advance_external(super::PhyBbInitCompletion::Mmio(
            super::PhyBbMmioCompletion { action },
        ))
        .unwrap();
}

#[test]
fn complete_parent_enters_or_skips_the_guarded_calibration_prefix() {
    let mut fresh = super::PhyBbInitTransition::new(crate::state::PhyState::default());
    assert_eq!(
        fresh.step_local().unwrap(),
        super::PhyBbInitLocalStep::External(super::PhyBbInitAction::Mmio(
            PhyBbMmioAction::EnableBasebandInitialization
        ))
    );
    complete_parent_mmio(&mut fresh, PhyBbMmioAction::EnableBasebandInitialization);
    complete_parent_mmio(
        &mut fresh,
        PhyBbMmioAction::SetBasebandMode {
            mode: PhyBbBasebandMode::Calibration,
        },
    );
    complete_parent_mmio(
        &mut fresh,
        PhyBbMmioAction::SetForcedDigitalGain { enabled: true },
    );
    assert_eq!(
        fresh.step_local().unwrap(),
        super::PhyBbInitLocalStep::External(super::PhyBbInitAction::TxDc(
            crate::tx::dc_offset::PhyTxDcAction::ConfigurePbusDebugMode
        ))
    );

    let mut retained_state = crate::state::PhyState::default();
    retained_state.mark_baseband_calibration_complete();
    let mut retained = super::PhyBbInitTransition::new(retained_state);
    complete_parent_mmio(&mut retained, PhyBbMmioAction::EnableBasebandInitialization);
    complete_parent_mmio(
        &mut retained,
        PhyBbMmioAction::SetBasebandMode {
            mode: PhyBbBasebandMode::Calibration,
        },
    );
    complete_parent_mmio(
        &mut retained,
        PhyBbMmioAction::SetForcedDigitalGain { enabled: true },
    );
    assert_eq!(
        retained.step_local().unwrap(),
        super::PhyBbInitLocalStep::External(super::PhyBbInitAction::TxCfr(
            PhyTxCfrAction::ReadStartIndex
        ))
    );
}

#[test]
fn complete_parent_enters_the_recovered_bluetooth_gain_parent() {
    let mut transition = super::PhyBbInitTransition::new(crate::state::PhyState::default());
    transition.step = super::PhyBbInitStep::TxCfr(PhyTxCfrTransition::new());
    transition
        .advance_external(super::PhyBbInitCompletion::TxCfr(
            PhyTxCfrCompletion::StartIndexRead { base_index: 3 },
        ))
        .unwrap();
    let mut index = 0;
    while index != PHY_TX_CFR_ENTRY_COUNT {
        let action = match transition.step_local().unwrap() {
            super::PhyBbInitLocalStep::External(super::PhyBbInitAction::TxCfr(
                PhyTxCfrAction::ProgramEntry(entry),
            )) => entry,
            other => panic!("unexpected CFR action: {other:?}"),
        };
        transition
            .advance_external(super::PhyBbInitCompletion::TxCfr(
                PhyTxCfrCompletion::EntryProgrammed(action),
            ))
            .unwrap();
        index += 1;
    }
    assert_eq!(
        transition.step_local().unwrap(),
        super::PhyBbInitLocalStep::StateAdvanced
    );
    assert!(matches!(
        transition.step_local().unwrap(),
        super::PhyBbInitLocalStep::External(super::PhyBbInitAction::BluetoothTxGain(
            crate::calibration::bluetooth::PhyBluetoothTxGainInitAction::Rfpll(_)
        ))
    ));
}

#[test]
fn complete_parent_tail_preserves_conditional_disable_and_tracking_order() {
    let mut state = crate::state::PhyState::default();
    state.set_initialization_parameter(1);
    let mut transition = super::PhyBbInitTransition::new(state);
    transition.step = super::PhyBbInitStep::SetIdleMode;

    complete_parent_mmio(
        &mut transition,
        PhyBbMmioAction::SetBasebandMode {
            mode: PhyBbBasebandMode::Idle,
        },
    );
    assert_eq!(
        transition.step_local().unwrap(),
        super::PhyBbInitLocalStep::External(super::PhyBbInitAction::Mmio(
            PhyBbMmioAction::SetWifiEnabled { enabled: false }
        ))
    );
    complete_parent_mmio(
        &mut transition,
        PhyBbMmioAction::SetWifiEnabled { enabled: false },
    );
    complete_parent_mmio(&mut transition, PhyBbMmioAction::ConfigureI2cTxRate);
    complete_parent_mmio(
        &mut transition,
        PhyBbMmioAction::ConfigureTxPowerTracking { enabled: true },
    );
    assert_eq!(
        transition.step_local().unwrap(),
        super::PhyBbInitLocalStep::Complete(super::PhyBbInitOutcome {
            calibration_performed: false,
        })
    );
}

#[test]
fn complete_parent_failure_always_restores_idle_mode_and_agc() {
    let state = crate::state::PhyState::default();
    let parameters = state.channel_parameters();
    let mut transition = super::PhyBbInitTransition::new(state);
    transition.step =
        super::PhyBbInitStep::Channel(crate::channel::PhyChipChannelTransition::at_depth(
            crate::channel::PhyChipChannelRequest {
                channel_or_frequency: 0,
                cbw: 0,
                parameters,
            },
            crate::analog::pbus::PhyForceTxRxDepth::OUTERMOST.nested(),
        ));
    assert_eq!(
        transition.step_local().unwrap(),
        super::PhyBbInitLocalStep::StateAdvanced
    );
    assert_eq!(
        transition.step_local().unwrap(),
        super::PhyBbInitLocalStep::External(super::PhyBbInitAction::Mmio(
            PhyBbMmioAction::SetForcedDigitalGain { enabled: false }
        ))
    );
    complete_parent_mmio(
        &mut transition,
        PhyBbMmioAction::SetForcedDigitalGain { enabled: false },
    );
    assert_eq!(
        transition.step_local().unwrap(),
        super::PhyBbInitLocalStep::External(super::PhyBbInitAction::Mmio(
            PhyBbMmioAction::SetBasebandMode {
                mode: PhyBbBasebandMode::Idle,
            }
        ))
    );
    complete_parent_mmio(
        &mut transition,
        PhyBbMmioAction::SetBasebandMode {
            mode: PhyBbBasebandMode::Idle,
        },
    );
    assert_eq!(
        transition.step_local().unwrap(),
        super::PhyBbInitLocalStep::External(super::PhyBbInitAction::Mmio(
            PhyBbMmioAction::EnableAgc
        ))
    );
    complete_parent_mmio(&mut transition, PhyBbMmioAction::EnableAgc);
    assert_eq!(
        transition.step_local().unwrap(),
        super::PhyBbInitLocalStep::Failed(super::PhyBbInitFailure::Channel(
            crate::channel::PhyChipChannelFailure::UnsupportedChannel(0)
        ))
    );
}
