use super::RfpllFrequencyFailure;
use super::{
    CAP_SEARCH_SAMPLES_PER_DIRECTION, RfpllFrequencyAction, RfpllFrequencyBindingError,
    RfpllFrequencyCompletion, RfpllFrequencyExternalBinding, RfpllFrequencyI2cBinding,
    RfpllFrequencyOutcome, RfpllFrequencyRequest, RfpllFrequencyTransition, calculate_rfpll_sdm,
};
use crate::analog::i2c::analog_registers;

fn complete_write(action: RfpllFrequencyAction) -> RfpllFrequencyCompletion {
    match action {
        RfpllFrequencyAction::WriteMasked { field, .. } => {
            RfpllFrequencyCompletion::MaskedWrite { field }
        }
        RfpllFrequencyAction::WriteByte { address, .. } => {
            RfpllFrequencyCompletion::ByteWrite { address }
        }
        action => panic!("expected write action, got {action:?}"),
    }
}

fn advance_writes(transition: &mut RfpllFrequencyTransition, count: usize) {
    let mut index = 0;
    while index != count {
        let completion = complete_write(transition.action());
        transition.advance(completion).unwrap();
        index += 1;
    }
}

fn enter_cap_search(transition: &mut RfpllFrequencyTransition, low: u8, high: u8) {
    advance_writes(transition, 13);
    assert_eq!(transition.action(), RfpllFrequencyAction::DelayMicros(20));
    transition
        .advance(RfpllFrequencyCompletion::DelayElapsed(20))
        .unwrap();
    let RfpllFrequencyAction::ReadMasked { field } = transition.action() else {
        panic!("expected lock read");
    };
    transition
        .advance(RfpllFrequencyCompletion::MaskedRead { field, value: 1 })
        .unwrap();

    let RfpllFrequencyAction::ReadByte { address } = transition.action() else {
        panic!("expected cap low read");
    };
    transition
        .advance(RfpllFrequencyCompletion::ByteRead {
            address,
            value: low,
        })
        .unwrap();
    let RfpllFrequencyAction::ReadMasked { field } = transition.action() else {
        panic!("expected cap high read");
    };
    transition
        .advance(RfpllFrequencyCompletion::MaskedRead { field, value: high })
        .unwrap();
    let completion = complete_write(transition.action());
    transition.advance(completion).unwrap();
}

fn complete_cap_candidate(transition: &mut RfpllFrequencyTransition, status: u8) -> u8 {
    let RfpllFrequencyAction::WriteByte {
        value: candidate, ..
    } = transition.action()
    else {
        panic!("expected cap low write");
    };
    advance_writes(transition, 2);
    assert_eq!(transition.action(), RfpllFrequencyAction::DelayMicros(5));
    transition
        .advance(RfpllFrequencyCompletion::DelayElapsed(5))
        .unwrap();
    let RfpllFrequencyAction::ReadMasked { field } = transition.action() else {
        panic!("expected cap status read");
    };
    transition
        .advance(RfpllFrequencyCompletion::MaskedRead {
            field,
            value: status,
        })
        .unwrap();
    candidate
}

#[test]
fn sdm_image_matches_the_actual_xtal_duty_request() {
    assert_eq!(
        calculate_rfpll_sdm(0x983, 0x31, 0).bytes(),
        [0x05, 0xaa, 0x2a, 0x31]
    );
    assert_eq!(
        calculate_rfpll_sdm(0x0fa1, 1, 7).bytes(),
        [0x01, 0xe8, 0x30, 0x3b]
    );
}

#[test]
fn lock_deadline_is_one_hundred_external_delay_and_read_edges() {
    let mut transition = RfpllFrequencyTransition::new(RfpllFrequencyRequest {
        crystal_selector: 0x31,
        frequency_code: 0x983,
        offset: 0,
    });
    advance_writes(&mut transition, 13);

    let mut attempts = 0;
    while attempts != 100 {
        assert_eq!(transition.action(), RfpllFrequencyAction::DelayMicros(20));
        transition
            .advance(RfpllFrequencyCompletion::DelayElapsed(20))
            .unwrap();
        let RfpllFrequencyAction::ReadMasked { field } = transition.action() else {
            panic!("expected lock read");
        };
        transition
            .advance(RfpllFrequencyCompletion::MaskedRead { field, value: 0 })
            .unwrap();
        attempts += 1;
    }
    assert!(matches!(
        transition.action(),
        RfpllFrequencyAction::ReadByte { .. }
    ));
}

#[test]
fn capacitor_search_restarts_the_upward_offset_and_keeps_the_shared_sum() {
    let mut transition = RfpllFrequencyTransition::new(RfpllFrequencyRequest {
        crystal_selector: 0x31,
        frequency_code: 0x983,
        offset: 0,
    });
    enter_cap_search(&mut transition, 100, 0);

    // Each direction ends after its second, not necessarily consecutive,
    // boundary status: increase downward, decrease upward.
    let candidates =
        [0, 1, 2, 0, 1, 0, 2, 2].map(|status| complete_cap_candidate(&mut transition, status));
    assert_eq!(candidates, [100, 99, 98, 97, 96, 101, 102, 103]);

    advance_writes(&mut transition, 2);
    transition
        .advance(RfpllFrequencyCompletion::DelayElapsed(5))
        .unwrap();
    let RfpllFrequencyAction::Complete(outcome) = transition.action() else {
        panic!("expected completion");
    };
    assert_eq!(outcome.initial_cap, 100);
    assert_eq!(outcome.final_cap, (100 + 97 + 101) / 3);
    assert_eq!(outcome.accepted_cap_samples, 3);
    assert!(outcome.lock_observed);
}

#[test]
fn bounded_cap_path_preserves_initial_when_no_sample_is_accepted() {
    let mut transition = RfpllFrequencyTransition::new(RfpllFrequencyRequest {
        crystal_selector: 0x31,
        frequency_code: 0x983,
        offset: 0,
    });
    enter_cap_search(&mut transition, 100, 0);
    let mut index = 0;
    while index != CAP_SEARCH_SAMPLES_PER_DIRECTION * 2 {
        // Neither accepted nor a boundary: both directions run to their bound.
        complete_cap_candidate(&mut transition, 3);
        index += 1;
    }
    advance_writes(&mut transition, 2);
    transition
        .advance(RfpllFrequencyCompletion::DelayElapsed(5))
        .unwrap();
    let RfpllFrequencyAction::Complete(outcome) = transition.action() else {
        panic!("expected completion");
    };
    assert_eq!(outcome.initial_cap, 100);
    assert_eq!(outcome.final_cap, 100);
    assert_eq!(outcome.accepted_cap_samples, 0);
}

#[test]
fn wifi_channel_uses_the_rom_fast_switch_without_rfpll_i2c() {
    let mut transition = RfpllFrequencyTransition::channel(RfpllFrequencyRequest {
        crystal_selector: 0,
        frequency_code: 1,
        offset: 0,
    });
    assert_eq!(
        transition.action(),
        RfpllFrequencyAction::StartChannelSwitch {
            frequency_index: 12,
            crystal_selector: 0,
        }
    );
    transition
        .advance(RfpllFrequencyCompletion::ChannelSwitchStarted {
            frequency_index: 12,
            crystal_selector: 0,
        })
        .unwrap();
    assert_eq!(transition.action(), RfpllFrequencyAction::DelayMicros(1));
    transition
        .advance(RfpllFrequencyCompletion::DelayElapsed(1))
        .unwrap();
    assert_eq!(
        transition.action(),
        RfpllFrequencyAction::ClearChannelSwitch
    );
    transition
        .advance(RfpllFrequencyCompletion::ChannelSwitchCleared)
        .unwrap();
    assert_eq!(transition.action(), RfpllFrequencyAction::DelayMicros(10));
    transition
        .advance(RfpllFrequencyCompletion::DelayElapsed(10))
        .unwrap();
    assert_eq!(
        transition.action(),
        RfpllFrequencyAction::ReadChannelReady { samples: 0 }
    );
    transition
        .advance(RfpllFrequencyCompletion::ChannelReadyObserved { ready: true })
        .unwrap();
    assert_eq!(
        transition.action(),
        RfpllFrequencyAction::ConfigureNrx {
            frequency_mhz: 2_412,
        }
    );
    transition
        .advance(RfpllFrequencyCompletion::NrxConfigured {
            frequency_mhz: 2_412,
        })
        .unwrap();
    let RfpllFrequencyAction::Complete(outcome) = transition.action() else {
        panic!("expected fast-channel completion");
    };
    assert!(outcome.lock_observed);
    assert_eq!(outcome.accepted_cap_samples, 0);
}

#[test]
fn external_lowering_covers_rfpll_mmio_i2c_and_timer_actions() {
    assert!(matches!(
        RfpllFrequencyExternalBinding::lower(RfpllFrequencyAction::StartChannelSwitch {
            frequency_index: 12,
            crystal_selector: 0,
        }),
        Ok(RfpllFrequencyExternalBinding::Mmio(_))
    ));
    assert!(matches!(
        RfpllFrequencyExternalBinding::lower(RfpllFrequencyAction::ReadMasked {
            field: super::analog_registers::RFPLL_LOCK_STATUS,
        }),
        Ok(RfpllFrequencyExternalBinding::I2c(_))
    ));
    assert!(matches!(
        RfpllFrequencyExternalBinding::lower(RfpllFrequencyAction::WriteByte {
            address: analog_registers::RFPLL_LOCK_STATUS.address(),
            value: 3,
        }),
        Ok(RfpllFrequencyExternalBinding::I2c(_))
    ));
    let timer =
        RfpllFrequencyExternalBinding::lower(RfpllFrequencyAction::DelayMicros(20)).unwrap();
    let RfpllFrequencyExternalBinding::Timer(timer) = timer else {
        panic!("expected timer");
    };
    assert_eq!(timer.micros(), 20);
    assert_eq!(
        timer.into_completion(),
        RfpllFrequencyCompletion::DelayElapsed(20)
    );
    assert!(matches!(
        RfpllFrequencyExternalBinding::lower(RfpllFrequencyAction::Complete(
            RfpllFrequencyOutcome {
                lock_observed: true,
                initial_cap: 1,
                final_cap: 1,
                accepted_cap_samples: 1,
            }
        )),
        Err(RfpllFrequencyBindingError::UnsupportedAction)
    ));
}

#[test]
fn rfpll_i2c_binding_preserves_the_masked_read_identity() {
    let field = super::analog_registers::RFPLL_CAPACITOR_CORRECTION_DIRECTION;
    let mut binding =
        RfpllFrequencyI2cBinding::new(RfpllFrequencyAction::ReadMasked { field }).unwrap();
    binding.read_started().unwrap();
    assert_eq!(
        binding.observe_read_result(Ok(0b0100)).unwrap(),
        crate::calibration::cold::PhyColdI2cObservation::EdgeConsumed
    );
    assert_eq!(
        binding.into_completion().unwrap(),
        RfpllFrequencyCompletion::MaskedRead { field, value: 1 }
    );
}

#[test]
fn channel_and_mhz_inputs_select_the_same_table_but_direct_calibration_does_not() {
    for (channel, mhz) in [(1, 2412), (6, 2437), (13, 2472), (14, 2484)] {
        let request = |frequency_code| RfpllFrequencyRequest {
            crystal_selector: 0,
            frequency_code,
            offset: 0,
        };
        let channel = RfpllFrequencyTransition::channel(request(channel));
        let frequency = RfpllFrequencyTransition::channel(request(mhz));
        assert_eq!(channel.action(), frequency.action());
        assert!(matches!(
            frequency.action(),
            RfpllFrequencyAction::StartChannelSwitch { .. }
        ));
        assert!(matches!(
            RfpllFrequencyTransition::new(request(mhz)).action(),
            RfpllFrequencyAction::WriteMasked { .. }
        ));
    }
    let unsupported = RfpllFrequencyTransition::channel(RfpllFrequencyRequest {
        crystal_selector: 0,
        frequency_code: 5000,
        offset: 0,
    });
    assert_eq!(
        unsupported.action(),
        RfpllFrequencyAction::Failed(RfpllFrequencyFailure::UnsupportedChannelFrequency(5000))
    );
}

#[test]
fn channel_readiness_counts_every_not_ready_sample_until_the_deadline() {
    let mut transition = RfpllFrequencyTransition::channel(RfpllFrequencyRequest {
        crystal_selector: 0,
        frequency_code: 1,
        offset: 0,
    });
    for completion in [
        RfpllFrequencyCompletion::ChannelSwitchStarted {
            frequency_index: 12,
            crystal_selector: 0,
        },
        RfpllFrequencyCompletion::DelayElapsed(1),
        RfpllFrequencyCompletion::ChannelSwitchCleared,
        RfpllFrequencyCompletion::DelayElapsed(10),
    ] {
        transition.advance(completion).unwrap();
    }
    for samples in 0..3 {
        assert_eq!(
            transition.action(),
            RfpllFrequencyAction::ReadChannelReady { samples }
        );
        transition
            .advance(RfpllFrequencyCompletion::ChannelReadyObserved { ready: false })
            .unwrap();
    }
    assert_eq!(
        transition.action(),
        RfpllFrequencyAction::ReadChannelReady { samples: 3 }
    );
    transition
        .advance(RfpllFrequencyCompletion::ChannelReadyTimedOut)
        .unwrap();
    assert_eq!(
        transition.action(),
        RfpllFrequencyAction::Failed(RfpllFrequencyFailure::FrequencyReadyDeadlineExceeded {
            samples: 3
        })
    );
}

fn finish_cap_search(transition: &mut RfpllFrequencyTransition) -> RfpllFrequencyOutcome {
    advance_writes(transition, 2);
    transition
        .advance(RfpllFrequencyCompletion::DelayElapsed(5))
        .unwrap();
    let RfpllFrequencyAction::Complete(outcome) = transition.action() else {
        panic!("expected completion");
    };
    outcome
}

fn cap_transition() -> RfpllFrequencyTransition {
    RfpllFrequencyTransition::new(RfpllFrequencyRequest {
        crystal_selector: 0x31,
        frequency_code: 0x983,
        offset: 0,
    })
}

#[test]
fn upward_cap_search_ends_at_the_nine_bit_bound_without_touching_bit_seven() {
    let mut transition = cap_transition();
    // Initial capacitor 0x1f0: low byte 0xf0, high bit 1.
    enter_cap_search(&mut transition, 0xf0, 1);
    for _ in 0..CAP_SEARCH_SAMPLES_PER_DIRECTION {
        complete_cap_candidate(&mut transition, 3);
    }
    let mut accepted = 0;
    loop {
        let RfpllFrequencyAction::WriteByte { .. } = transition.action() else {
            panic!("expected cap low write");
        };
        advance_writes(&mut transition, 1);
        let RfpllFrequencyAction::WriteMasked { field, value } = transition.action() else {
            panic!("expected cap high write");
        };
        assert_eq!(field, analog_registers::RFPLL_CAPACITOR_HIGH);
        assert!(
            value <= 1,
            "the high write never reaches the neighboring bit"
        );
        let completion = complete_write(transition.action());
        transition.advance(completion).unwrap();
        transition
            .advance(RfpllFrequencyCompletion::DelayElapsed(5))
            .unwrap();
        let RfpllFrequencyAction::ReadMasked { field } = transition.action() else {
            break;
        };
        transition
            .advance(RfpllFrequencyCompletion::MaskedRead { field, value: 0 })
            .unwrap();
        accepted += 1;
    }
    let RfpllFrequencyAction::Complete(outcome) = transition.action() else {
        panic!("expected completion");
    };
    assert_eq!(u16::from(accepted), 0x1ff - 0x1f0);
    assert_eq!(outcome.accepted_cap_samples, accepted);
    assert_eq!(outcome.final_cap, (0x1f1 + 0x1ff) / 2);
}

#[test]
fn cap_search_averages_programmed_codes_below_zero() {
    let mut transition = cap_transition();
    enter_cap_search(&mut transition, 2, 0);
    // Down: 2, 1, 0, then wrapped candidates the helper programs as zero.
    let mut down = std::vec::Vec::new();
    for _ in 0..CAP_SEARCH_SAMPLES_PER_DIRECTION {
        down.push(complete_cap_candidate(&mut transition, 0));
    }
    assert_eq!(&down[..4], &[2, 1, 0, 0]);
    for _ in 0..CAP_SEARCH_SAMPLES_PER_DIRECTION {
        complete_cap_candidate(&mut transition, 0);
    }
    let outcome = finish_cap_search(&mut transition);
    let programmed_down: u16 = 2 + 1;
    let up: u16 = (3..3 + u16::from(CAP_SEARCH_SAMPLES_PER_DIRECTION)).sum();
    assert_eq!(
        outcome.accepted_cap_samples,
        2 * CAP_SEARCH_SAMPLES_PER_DIRECTION
    );
    assert_eq!(
        outcome.final_cap,
        (programmed_down + up) / (2 * u16::from(CAP_SEARCH_SAMPLES_PER_DIRECTION))
    );
    assert!(outcome.final_cap <= 0x1ff);
}
