use super::*;
use crate::analog::rfpll::RfpllFrequencyAction;
use std::vec::Vec;

#[test]
fn channel_visits_update_nrx_once_before_each_measured_pair() {
    let mut transition = PhyDcodeTransition::new(PhyDcodeParameters {
        crystal_selector: 0,
    });
    let mut frequencies = Vec::new();
    let mut values = 0;
    for _ in 0..100 {
        let completion = match transition.action() {
            PhyDcodeAction::Rfpll(action) => PhyDcodeCompletion::Rfpll(match action {
                RfpllFrequencyAction::StartChannelSwitch {
                    frequency_index,
                    crystal_selector,
                } => RfpllFrequencyCompletion::ChannelSwitchStarted {
                    frequency_index,
                    crystal_selector,
                },
                RfpllFrequencyAction::ClearChannelSwitch => {
                    RfpllFrequencyCompletion::ChannelSwitchCleared
                }
                RfpllFrequencyAction::DelayMicros(micros) => {
                    RfpllFrequencyCompletion::DelayElapsed(micros)
                }
                RfpllFrequencyAction::ReadChannelReady { .. } => {
                    RfpllFrequencyCompletion::ChannelReadyObserved { ready: true }
                }
                RfpllFrequencyAction::ConfigureNrx { frequency_mhz } => {
                    assert_eq!(
                        values,
                        frequencies.len() * 2,
                        "NRX must precede each pair exactly once"
                    );
                    frequencies.push(frequency_mhz);
                    RfpllFrequencyCompletion::NrxConfigured { frequency_mhz }
                }
                _ => panic!("D-code must use channel-table switching, not a full PLL search"),
            }),
            PhyDcodeAction::WriteMasked { field, value } => {
                PhyDcodeCompletion::MaskedWrite { field, value }
            }
            PhyDcodeAction::ReadMasked { field } => {
                assert_eq!(frequencies.len(), values / 2 + 1);
                let value = values as u8;
                values += 1;
                PhyDcodeCompletion::MaskedRead { field, value }
            }
            PhyDcodeAction::Complete(outcome) => {
                assert_eq!(frequencies, [2412, 2432, 2457, 2484]);
                assert_eq!(outcome.codes, [0, 1, 2, 3, 4, 5, 6, 7]);
                return;
            }
            PhyDcodeAction::Failed(failure) => panic!("unexpected failure: {failure:?}"),
        };
        transition.advance(completion).unwrap();
    }
    panic!("D-code did not finish within its finite operation bound");
}

#[test]
fn first_frequency_request_uses_the_channel_table() {
    let transition = PhyDcodeTransition::new(PhyDcodeParameters {
        crystal_selector: 0x31,
    });
    assert!(matches!(
        transition.action(),
        PhyDcodeAction::Rfpll(RfpllFrequencyAction::StartChannelSwitch {
            crystal_selector: 0x31,
            ..
        })
    ));
}

#[test]
fn foreign_completion_is_rejected_without_advancing() {
    let mut transition = PhyDcodeTransition::new(PhyDcodeParameters {
        crystal_selector: 0x31,
    });
    assert_eq!(
        transition.advance(PhyDcodeCompletion::Rfpll(
            RfpllFrequencyCompletion::ChannelSwitchCleared
        )),
        Err(PhyDcodeTransitionError::WrongCompletion)
    );
    assert!(matches!(transition.action(), PhyDcodeAction::Rfpll(_)));
}

#[test]
fn ckgen_and_two_reads_commit_the_final_owned_pair() {
    let mut transition = PhyDcodeTransition {
        parameters: PhyDcodeParameters {
            crystal_selector: 0x31,
        },
        codes: [1, 2, 3, 4, 5, 6, 0, 0],
        step: PhyDcodeStep::ResetCkgen {
            calibration_index: 3,
            write_index: 0,
        },
    };

    for _ in 0..4 {
        let PhyDcodeAction::WriteMasked { field, value } = transition.action() else {
            panic!("expected CKGEN write");
        };
        transition
            .advance(PhyDcodeCompletion::MaskedWrite { field, value })
            .unwrap();
    }

    for value in [7, 8] {
        let PhyDcodeAction::ReadMasked { field } = transition.action() else {
            panic!("expected D-code read");
        };
        transition
            .advance(PhyDcodeCompletion::MaskedRead { field, value })
            .unwrap();
    }

    assert_eq!(
        transition.action(),
        PhyDcodeAction::Complete(PhyDcodeOutcome {
            codes: [1, 2, 3, 4, 5, 6, 7, 8]
        })
    );
}

#[test]
fn six_bit_read_validation_fails_closed() {
    let mut transition = PhyDcodeTransition {
        parameters: PhyDcodeParameters {
            crystal_selector: 0x31,
        },
        codes: [0; 8],
        step: PhyDcodeStep::ReadLow {
            calibration_index: 0,
        },
    };
    let PhyDcodeAction::ReadMasked { field } = transition.action() else {
        panic!("expected D-code read");
    };
    assert_eq!(
        transition.advance(PhyDcodeCompletion::MaskedRead { field, value: 0x40 }),
        Err(PhyDcodeTransitionError::WrongCompletion)
    );
}

#[test]
fn external_lowering_covers_every_dcode_operation_class() {
    assert!(matches!(
        PhyDcodeExternalBinding::lower(PhyDcodeAction::Rfpll(RfpllFrequencyAction::DelayMicros(5))),
        Ok(PhyDcodeExternalBinding::Rfpll(_))
    ));
    assert!(matches!(
        PhyDcodeExternalBinding::lower(PhyDcodeAction::WriteMasked {
            field: analog_registers::RFPLL_DCODE_0_SOURCE_SELECT,
            value: 0,
        }),
        Ok(PhyDcodeExternalBinding::I2c(_))
    ));
    assert!(matches!(
        PhyDcodeExternalBinding::lower(PhyDcodeAction::ReadMasked {
            field: analog_registers::RFPLL_INTERNAL_DCODE_0,
        }),
        Ok(PhyDcodeExternalBinding::I2c(_))
    ));
    assert!(matches!(
        PhyDcodeExternalBinding::lower(PhyDcodeAction::Complete(PhyDcodeOutcome { codes: [0; 8] })),
        Err(PhyDcodeBindingError::UnsupportedAction)
    ));
}
