use crate::analog::i2c::analog_registers;

use super::{
    PHY_FREQUENCY_TABLE_ENTRY_COUNT, PhyChannelFrequencyInitAction,
    PhyChannelFrequencyInitCompletion, PhyChannelFrequencyInitRequest,
    PhyChannelFrequencyInitTransition, PhyChannelFrequencyRfpllPoint, PhyFrequencyCapCorrection,
    PhyFrequencyCapMemoryAction, PhyFrequencyCapMemoryBindingError,
    PhyFrequencyCapMemoryCompletion, PhyFrequencyCapMemoryExternalBinding,
    PhyFrequencyCapMemoryRequest, PhyFrequencyCapMemoryTransition,
    PhyFrequencyCapMemoryTransitionError, PhyFrequencyI2cAction, PhyFrequencyI2cCompletion,
    PhyFrequencyI2cRequest, PhyFrequencyI2cTransition, PhyFrequencyI2cTransitionError,
    PhyFrequencyTableAction, PhyFrequencyTableCompletion, PhyFrequencyTableParameters,
    PhyFrequencyTableRequest, PhyFrequencyTableTransition, PhyFrequencyTableTransitionError,
    phy_frequency_cap_adjusted_word, phy_frequency_channel_index, phy_frequency_memory_record,
    phy_frequency_xtal_duty,
};
use crate::analog::rfpll::{RfpllFrequencyAction, RfpllFrequencyCompletion};

const REQUEST: PhyFrequencyTableRequest = PhyFrequencyTableRequest {
    parameters: PhyFrequencyTableParameters {
        crystal_selector: 0x31,
        middle_xtal_duty: 0x2a,
        outer_xtal_duty: 0x35,
        sdm_register_six_upper: 0xa8,
    },
    low_frequency_cap: 0x0c8,
    high_frequency_cap: 0x118,
};

#[test]
fn frequency_memory_address_matches_wrapping_rom_arithmetic() {
    assert_eq!(super::phy_get_freq_mem_addr(0x20, 7, 84, 6), 0x272);
    assert_eq!(
        super::phy_get_freq_mem_addr(0xffff_ffff, 0xffff_ffff, 2, 3),
        0
    );
}

#[test]
fn cap_adjustment_preserves_the_rom_mask_and_reencodes_bit_eight() {
    assert_eq!(
        phy_frequency_cap_adjusted_word(0x00aa_bffe, PhyFrequencyCapCorrection::from_delta(2),),
        0x0000_ff00
    );
    assert_eq!(
        phy_frequency_cap_adjusted_word(0x00aa_ff01, PhyFrequencyCapCorrection::from_delta(-2),),
        0x0000_bfff
    );
    assert_eq!(phy_frequency_channel_index(11), 62);
    assert_eq!(phy_frequency_channel_index(2_462), 62);
}

#[test]
fn cap_memory_underflow_retains_signed_mode_and_requires_its_completion() {
    let mut transition = PhyFrequencyCapMemoryTransition::new(PhyFrequencyCapMemoryRequest {
        correction: PhyFrequencyCapCorrection::from_delta(-5),
        current_channel: 13,
    });
    let PhyFrequencyCapMemoryAction::ReadMemory {
        entry_index,
        address,
        mode,
    } = transition.action()
    else {
        panic!("must read before adjusting");
    };
    transition
        .advance(PhyFrequencyCapMemoryCompletion::MemoryRead {
            entry_index,
            address,
            mode,
            value: 0x00aa_bf00,
        })
        .unwrap();
    let PhyFrequencyCapMemoryAction::WriteMemory { value, mode, .. } = transition.action() else {
        panic!("must publish the adjusted entry");
    };
    assert_eq!(value, 0x00ff_fffb);
    assert_eq!(mode, 0xff);
    assert_eq!(
        transition.advance(PhyFrequencyCapMemoryCompletion::MemoryWritten {
            entry_index,
            address,
            mode: 3,
        }),
        Err(PhyFrequencyCapMemoryTransitionError::WrongCompletion)
    );
    transition
        .advance(PhyFrequencyCapMemoryCompletion::MemoryWritten {
            entry_index,
            address,
            mode,
        })
        .unwrap();
    assert!(matches!(
        transition.action(),
        PhyFrequencyCapMemoryAction::ReadMemory { entry_index: 1, .. }
    ));
}

#[test]
fn cap_memory_transition_reads_and_writes_all_entries_before_restore() {
    let request = PhyFrequencyCapMemoryRequest {
        correction: PhyFrequencyCapCorrection::from_delta(2),
        current_channel: 11,
    };
    let mut transition = PhyFrequencyCapMemoryTransition::new(request);
    let mut reads = 0_u8;
    let mut writes = 0_u8;

    loop {
        match transition.action() {
            PhyFrequencyCapMemoryAction::ReadMemory {
                entry_index,
                address,
                mode,
            } => {
                assert_eq!(entry_index, reads);
                assert_eq!(address, 0x20 + u16::from(entry_index) * 7);
                assert_eq!(mode, 2);
                transition
                    .advance(PhyFrequencyCapMemoryCompletion::MemoryRead {
                        entry_index,
                        address,
                        mode,
                        value: 0x00aa_bf20,
                    })
                    .unwrap();
                reads += 1;
            }
            PhyFrequencyCapMemoryAction::WriteMemory {
                entry_index,
                address,
                value,
                mode,
            } => {
                assert_eq!(entry_index, writes);
                assert_eq!(address, 0x20 + u16::from(entry_index) * 7);
                assert_eq!(value, 0x0000_bf22);
                assert_eq!(mode, 3);
                transition
                    .advance(PhyFrequencyCapMemoryCompletion::MemoryWritten {
                        entry_index,
                        address,
                        mode,
                    })
                    .unwrap();
                writes += 1;
            }
            PhyFrequencyCapMemoryAction::RestoreChannelIndex { frequency_index } => {
                assert_eq!(reads, PHY_FREQUENCY_TABLE_ENTRY_COUNT);
                assert_eq!(writes, PHY_FREQUENCY_TABLE_ENTRY_COUNT);
                assert_eq!(frequency_index, 62);
                transition
                    .advance(PhyFrequencyCapMemoryCompletion::ChannelIndexRestored {
                        frequency_index,
                    })
                    .unwrap();
            }
            PhyFrequencyCapMemoryAction::Complete(outcome) => {
                assert_eq!(outcome.entries_updated, PHY_FREQUENCY_TABLE_ENTRY_COUNT);
                assert_eq!(outcome.correction, request.correction);
                assert_eq!(outcome.restored_frequency_index, 62);
                break;
            }
        }
    }

    assert_eq!(
        transition.advance(PhyFrequencyCapMemoryCompletion::ChannelIndexRestored {
            frequency_index: 62,
        }),
        Err(PhyFrequencyCapMemoryTransitionError::AlreadyComplete)
    );
}

#[test]
fn cap_memory_transition_and_binding_reject_foreign_or_terminal_edges() {
    let mut transition = PhyFrequencyCapMemoryTransition::new(PhyFrequencyCapMemoryRequest {
        correction: PhyFrequencyCapCorrection::from_delta(-2),
        current_channel: 1,
    });
    assert_eq!(
        transition.advance(PhyFrequencyCapMemoryCompletion::MemoryRead {
            entry_index: 1,
            address: 0x27,
            mode: 2,
            value: 0x0000_ff80,
        }),
        Err(PhyFrequencyCapMemoryTransitionError::WrongCompletion)
    );

    let action = transition.action();
    let binding = PhyFrequencyCapMemoryExternalBinding::lower(action).unwrap();
    assert_eq!(binding.action(), action);
    assert_eq!(
        PhyFrequencyCapMemoryExternalBinding::lower(PhyFrequencyCapMemoryAction::Complete(
            super::PhyFrequencyCapMemoryOutcome {
                entries_updated: 85,
                correction: PhyFrequencyCapCorrection::from_delta(-2),
                restored_frequency_index: 12,
            }
        )),
        Err(PhyFrequencyCapMemoryBindingError::UnsupportedAction)
    );
}

#[test]
fn crystal_duty_preserves_both_unsigned_vendor_boundaries() {
    assert_eq!(phy_frequency_xtal_duty(0x967, 0x2a, 0x35), 0x35);
    assert_eq!(phy_frequency_xtal_duty(0x968, 0x2a, 0x35), 0x35);
    assert_eq!(phy_frequency_xtal_duty(0x974, 0x2a, 0x35), 0x35);
    assert_eq!(phy_frequency_xtal_duty(0x975, 0x2a, 0x35), 0x2a);
    assert_eq!(phy_frequency_xtal_duty(0x99b, 0x2a, 0x35), 0x2a);
    assert_eq!(phy_frequency_xtal_duty(0x99c, 0x2a, 0x35), 0x35);
}

#[test]
fn record_packs_cap_sdm_and_duty_without_a_backing_table() {
    assert_eq!(
        phy_frequency_memory_record(REQUEST, 0).words(),
        [0x00a8_bfc8, 0x0030_0000, 0x35]
    );
    assert_eq!(
        phy_frequency_memory_record(REQUEST, 64).words(),
        [0x00a9_ff18, 0x0032_2222, 0x35]
    );
    assert_eq!(
        phy_frequency_memory_record(REQUEST, 84).words(),
        [0x00ae_ff31, 0x0032_cccc, 0x35]
    );
}

#[test]
fn transition_publishes_exactly_three_words_for_all_85_entries() {
    let mut transition = PhyFrequencyTableTransition::new(REQUEST);
    let mut writes = 0;
    loop {
        match transition.action() {
            PhyFrequencyTableAction::WriteMemory {
                entry_index,
                word_index,
                address,
                mode,
                ..
            } => {
                assert_eq!(mode, 7);
                assert_eq!(
                    address,
                    0x20 + u16::from(entry_index) * 7 + u16::from(word_index) * 3
                );
                transition
                    .advance(PhyFrequencyTableCompletion {
                        entry_index,
                        word_index,
                        address,
                    })
                    .unwrap();
                writes += 1;
            }
            PhyFrequencyTableAction::Complete(outcome) => {
                assert_eq!(outcome.entries_written, PHY_FREQUENCY_TABLE_ENTRY_COUNT);
                break;
            }
        }
    }
    assert_eq!(writes, usize::from(PHY_FREQUENCY_TABLE_ENTRY_COUNT) * 3);
    assert_eq!(
        transition.advance(PhyFrequencyTableCompletion {
            entry_index: 0,
            word_index: 0,
            address: 0x20,
        }),
        Err(PhyFrequencyTableTransitionError::AlreadyComplete)
    );
}

#[test]
fn transition_rejects_out_of_order_memory_completion() {
    let mut transition = PhyFrequencyTableTransition::new(REQUEST);
    assert_eq!(
        transition.advance(PhyFrequencyTableCompletion {
            entry_index: 0,
            word_index: 1,
            address: 0x23,
        }),
        Err(PhyFrequencyTableTransitionError::WrongCompletion)
    );
}

fn complete_i2c_snapshot(
    transition: &mut PhyFrequencyI2cTransition,
    rfpll_register_0b: u8,
    sdm_register_0: u8,
    front_end_register_3: u8,
) {
    assert_eq!(
        transition.action(),
        PhyFrequencyI2cAction::WriteMasked {
            field: analog_registers::RFPLL_CAPACITOR_SEARCH_ENABLE,
            value: 1,
        }
    );
    transition
        .advance(PhyFrequencyI2cCompletion::MaskedWrite {
            field: analog_registers::RFPLL_CAPACITOR_SEARCH_ENABLE,
        })
        .unwrap();
    for (address, value) in [
        (
            analog_registers::RFPLL_CAPACITOR_SEARCH_ENABLE.address(),
            rfpll_register_0b,
        ),
        (
            analog_registers::RFPLL_SDM_UPDATE_ENABLE.address(),
            sdm_register_0,
        ),
        (
            analog_registers::SHARED_RX_GAIN_CALIBRATION_ENABLE.address(),
            front_end_register_3,
        ),
    ] {
        assert_eq!(
            transition.action(),
            PhyFrequencyI2cAction::ReadByte { address }
        );
        transition
            .advance(PhyFrequencyI2cCompletion::ByteRead { address, value })
            .unwrap();
    }
}

fn collect_i2c_memory_writes(
    transition: &mut PhyFrequencyI2cTransition,
) -> std::vec::Vec<(u8, u8, u16, u32, u8)> {
    let mut writes = std::vec::Vec::new();
    while let PhyFrequencyI2cAction::WriteMemory {
        descriptor_index,
        copy_index,
        address,
        value,
        mode,
    } = transition.action()
    {
        writes.push((descriptor_index, copy_index, address, value, mode));
        transition
            .advance(PhyFrequencyI2cCompletion::MemoryWrite {
                descriptor_index,
                copy_index,
                address,
            })
            .unwrap();
    }
    writes
}

#[test]
fn i2c_transition_publishes_the_fixed_graph_and_three_copy_tail() {
    let mut transition = PhyFrequencyI2cTransition::new(PhyFrequencyI2cRequest::cold(false));
    complete_i2c_snapshot(&mut transition, 0x5a, 0x8f, 0x10);

    let writes = collect_i2c_memory_writes(&mut transition);
    assert_eq!(writes.len(), 13);
    assert_eq!(writes[2], (2, 0, 9, 0x0087_0063, 7));
    assert_eq!(writes[7], (7, 0, 12, 0x008f_0063, 7));
    assert_eq!(writes[8], (8, 0, 15, 0x005a_0b62, 7));
    assert_eq!(
        &writes[10..],
        &[
            (10, 0, 0, 0x0010_0367, 7),
            (10, 1, 3, 0x0010_0367, 7),
            (10, 2, 6, 0x0010_0367, 7),
        ]
    );

    let PhyFrequencyI2cAction::ConfigureNumberAddresses(addresses) = transition.action() else {
        panic!("transition did not publish the number-address operation");
    };
    transition
        .advance(PhyFrequencyI2cCompletion::NumberAddressesConfigured(
            addresses,
        ))
        .unwrap();
    let PhyFrequencyI2cAction::Complete(outcome) = transition.action() else {
        panic!("transition did not complete");
    };
    assert_eq!(outcome.rfpll_register_0b, 0x5a);
    assert_eq!(outcome.sdm_register_0, 0x8f);
    assert_eq!(outcome.front_end_register_3, 0x10);
    assert_eq!(outcome.number_addresses, addresses);
    assert_eq!(
        transition.advance(PhyFrequencyI2cCompletion::NumberAddressesConfigured(
            addresses
        )),
        Err(PhyFrequencyI2cTransitionError::AlreadyComplete)
    );
}

#[test]
fn retained_wake_i2c_transition_preserves_command_memory() {
    let mut transition =
        PhyFrequencyI2cTransition::new(PhyFrequencyI2cRequest::retained_wake(false));
    complete_i2c_snapshot(&mut transition, 0x5a, 0x8f, 0x10);

    let writes = collect_i2c_memory_writes(&mut transition);
    assert!(writes.is_empty());

    let PhyFrequencyI2cAction::ConfigureNumberAddresses(addresses) = transition.action() else {
        panic!("retained wake did not preserve frequency command memory");
    };
    transition
        .advance(PhyFrequencyI2cCompletion::NumberAddressesConfigured(
            addresses,
        ))
        .unwrap();
    let PhyFrequencyI2cAction::Complete(outcome) = transition.action() else {
        panic!("retained-wake frequency-I2C transition did not complete");
    };
    assert_eq!(outcome.rfpll_register_0b, 0x5a);
    assert_eq!(outcome.sdm_register_0, 0x8f);
    assert_eq!(outcome.front_end_register_3, 0x10);
    assert_eq!(outcome.number_addresses, addresses);
}

#[test]
fn i2c_transition_applies_the_parameter_bit_only_to_outer_tail_copies() {
    let mut transition = PhyFrequencyI2cTransition::new(PhyFrequencyI2cRequest::cold(true));
    complete_i2c_snapshot(&mut transition, 0x5a, 0x8f, 0x10);

    let writes = collect_i2c_memory_writes(&mut transition);
    assert_eq!(writes.len(), 13);
    assert_eq!(
        &writes[10..],
        &[
            (10, 0, 0, 0x0014_0367, 7),
            (10, 1, 3, 0x0010_0367, 7),
            (10, 2, 6, 0x0014_0367, 7),
        ]
    );
}

#[test]
fn i2c_transition_rejects_a_completion_for_another_snapshot_register() {
    let mut transition = PhyFrequencyI2cTransition::new(PhyFrequencyI2cRequest::cold(false));
    assert_eq!(
        transition.advance(PhyFrequencyI2cCompletion::MaskedWrite {
            field: analog_registers::RFPLL_CAPACITOR_HIGH,
        }),
        Err(PhyFrequencyI2cTransitionError::WrongCompletion)
    );
}

const CHANNEL_REQUEST: PhyChannelFrequencyInitRequest = PhyChannelFrequencyInitRequest {
    frequency_register_parameter_override: false,
    frequency_table_initialized: false,
    crystal_selector: 0x31,
    middle_xtal_duty: 0x2a,
    outer_xtal_duty: 0x35,
    front_end_parameter_bit: false,
};

fn point_index(point: PhyChannelFrequencyRfpllPoint) -> usize {
    match point {
        PhyChannelFrequencyRfpllPoint::Nominal => 0,
        PhyChannelFrequencyRfpllPoint::Low => 1,
        PhyChannelFrequencyRfpllPoint::High => 2,
    }
}

const CAP_STATUSES: [u8; 6] = [0, 1, 1, 0, 2, 2];

fn rfpll_completion(
    point: PhyChannelFrequencyRfpllPoint,
    action: RfpllFrequencyAction,
    status_reads: &mut [u8; 3],
) -> RfpllFrequencyCompletion {
    match action {
        RfpllFrequencyAction::WriteMasked { field, .. } => {
            RfpllFrequencyCompletion::MaskedWrite { field }
        }
        RfpllFrequencyAction::WriteByte { address, .. } => {
            RfpllFrequencyCompletion::ByteWrite { address }
        }
        RfpllFrequencyAction::ReadMasked { field } => {
            let value = if field == crate::analog::i2c::analog_registers::RFPLL_LOCK_STATUS {
                1
            } else if field == crate::analog::i2c::analog_registers::RFPLL_CAPACITOR_SEARCH_STATUS {
                let index = point_index(point);
                // Accept the first candidate of each direction, then end the
                // direction with its two boundary statuses.
                let status = CAP_STATUSES[usize::from(status_reads[index])];
                status_reads[index] += 1;
                status
            } else {
                0
            };
            RfpllFrequencyCompletion::MaskedRead { field, value }
        }
        RfpllFrequencyAction::ReadByte { address }
            if address == analog_registers::RFPLL_CALIBRATED_CAPACITOR_LOW =>
        {
            RfpllFrequencyCompletion::ByteRead {
                address,
                value: match point {
                    PhyChannelFrequencyRfpllPoint::Nominal => 0xc8,
                    PhyChannelFrequencyRfpllPoint::Low => 0x80,
                    PhyChannelFrequencyRfpllPoint::High => 0xc0,
                },
            }
        }
        RfpllFrequencyAction::ReadByte { address } => {
            RfpllFrequencyCompletion::ByteRead { address, value: 0 }
        }
        RfpllFrequencyAction::DelayMicros(micros) => RfpllFrequencyCompletion::DelayElapsed(micros),
        action => panic!("unexpected terminal RFPLL action: {action:?}"),
    }
}

fn i2c_completion(action: PhyFrequencyI2cAction) -> PhyFrequencyI2cCompletion {
    match action {
        PhyFrequencyI2cAction::WriteMasked { field, .. } => {
            PhyFrequencyI2cCompletion::MaskedWrite { field }
        }
        PhyFrequencyI2cAction::ReadByte { address } => {
            let value = if address == analog_registers::RFPLL_CAPACITOR_SEARCH_ENABLE.address() {
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
        action => panic!("unexpected terminal I2C action: {action:?}"),
    }
}

#[test]
fn channel_frequency_init_composes_the_complete_cold_graph() {
    let mut transition = PhyChannelFrequencyInitTransition::new(CHANNEL_REQUEST);
    let mut status_reads = [0_u8; 3];
    let mut rfpll_actions = [0_usize; 3];
    let mut table_writes = 0_usize;
    let mut steps = 0_usize;

    loop {
        steps += 1;
        assert!(steps < 1_000);
        let completion = match transition.action() {
            PhyChannelFrequencyInitAction::ConfigureFrequencyRegisters { parameter_override } => {
                PhyChannelFrequencyInitCompletion::FrequencyRegistersConfigured {
                    parameter_override,
                }
            }
            PhyChannelFrequencyInitAction::WriteMasked { field, .. } => {
                PhyChannelFrequencyInitCompletion::MaskedWrite { field }
            }
            PhyChannelFrequencyInitAction::WriteByte { address, .. } => {
                PhyChannelFrequencyInitCompletion::ByteWrite { address }
            }
            PhyChannelFrequencyInitAction::ReadByte { address } => {
                assert_eq!(address, analog_registers::RFPLL_SDM_LOW.address());
                PhyChannelFrequencyInitCompletion::ByteRead {
                    address,
                    value: 0xab,
                }
            }
            PhyChannelFrequencyInitAction::Rfpll { point, action } => {
                rfpll_actions[point_index(point)] += 1;
                PhyChannelFrequencyInitCompletion::Rfpll(rfpll_completion(
                    point,
                    action,
                    &mut status_reads,
                ))
            }
            PhyChannelFrequencyInitAction::Table(PhyFrequencyTableAction::WriteMemory {
                entry_index,
                word_index,
                address,
                ..
            }) => {
                table_writes += 1;
                PhyChannelFrequencyInitCompletion::Table(PhyFrequencyTableCompletion {
                    entry_index,
                    word_index,
                    address,
                })
            }
            PhyChannelFrequencyInitAction::I2c(action) => {
                PhyChannelFrequencyInitCompletion::I2c(i2c_completion(action))
            }
            PhyChannelFrequencyInitAction::Complete(outcome) => {
                assert!(!outcome.table_was_initialized);
                assert!(outcome.table_is_initialized);
                let calibration = outcome.calibration.unwrap();
                assert_eq!(calibration.nominal.final_cap, 0xc8);
                assert_eq!(calibration.low.final_cap, 0x80);
                assert_eq!(calibration.high.final_cap, 0xc0);
                assert_eq!(calibration.table.entries_written, 85);
                assert_eq!(calibration.table.low_frequency_cap, 0x80);
                assert_eq!(calibration.table.high_frequency_cap, 0xc0);
                assert_eq!(outcome.i2c.sdm_register_0, 0x8f);
                break;
            }
            action => panic!("unexpected terminal channel action: {action:?}"),
        };
        transition.advance(completion).unwrap();
    }

    assert!(rfpll_actions.iter().all(|count| *count != 0));
    assert_eq!(status_reads, [CAP_STATUSES.len() as u8; 3]);
    assert_eq!(table_writes, 85 * 3);
}

#[test]
fn warm_channel_frequency_init_skips_calibration_but_refreshes_i2c_graph() {
    let mut transition = PhyChannelFrequencyInitTransition::new(PhyChannelFrequencyInitRequest {
        frequency_table_initialized: true,
        ..CHANNEL_REQUEST
    });
    assert_eq!(
        transition.action(),
        PhyChannelFrequencyInitAction::ConfigureFrequencyRegisters {
            parameter_override: false,
        }
    );
    transition
        .advance(
            PhyChannelFrequencyInitCompletion::FrequencyRegistersConfigured {
                parameter_override: false,
            },
        )
        .unwrap();

    loop {
        match transition.action() {
            PhyChannelFrequencyInitAction::I2c(action) => transition
                .advance(PhyChannelFrequencyInitCompletion::I2c(i2c_completion(
                    action,
                )))
                .unwrap(),
            PhyChannelFrequencyInitAction::Complete(outcome) => {
                assert!(outcome.table_was_initialized);
                assert!(outcome.table_is_initialized);
                assert_eq!(outcome.calibration, None);
                break;
            }
            action => panic!("warm path must not calibrate: {action:?}"),
        }
    }
}
