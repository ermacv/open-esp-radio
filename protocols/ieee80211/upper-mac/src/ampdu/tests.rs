use super::*;

fn at(micros: u64) -> Instant {
    Instant::from_micros(micros)
}

/// Every subframe admitted at zero.
const FRESH: fn(u8) -> Option<Instant> = |_| Some(Instant::from_micros(0));

const POLICY: AmpduRetryPolicy = AmpduRetryPolicy {
    lifetime: oer_time::Duration::from_micros(10_000),
    aged_margin: oer_time::Duration::from_micros(1_000),
    retry_limit: 3,
    retain_single_mpdu: false,
};

fn seq(value: u16) -> SequenceNumber {
    SequenceNumber::new(value).unwrap()
}

fn answered(start: u16, bitmap: u64) -> AmpduAttemptResult {
    AmpduAttemptResult::Answered(Some(BlockAckReport {
        start_sequence: seq(start),
        bitmap,
    }))
}

#[test]
fn a_partial_block_ack_keeps_only_the_missing_subframes_in_order() {
    let mut state = AmpduRetryState::new(seq(100), 4, POLICY).unwrap();
    // 100 and 102 acknowledged; 101 and 103 missing.
    assert_eq!(
        state.observe(answered(100, 0b0101), 4, at(10), true, FRESH),
        Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b1010 })
    );
    assert_eq!(state.current_subframes(), 2);
    assert_eq!(state.current_first_sequence(), seq(101));
    assert_eq!(state.pending_original_indices(), 0b1010);
    assert_eq!(state.original_index(1), 3);
    assert_eq!(state.acknowledged(), 2);
    assert_eq!(state.aggregate_attempts(), 2);

    // The compacted aggregate: position 0 is 101, position 1 is 103.
    assert_eq!(
        state.observe(answered(101, 0b100), 2, at(20), true, FRESH),
        Ok(AmpduRetryDecision::Unaggregate { retry_mask: 0b01 })
    );
    assert_eq!(state.missing_original_indices(), 0b0010);
}

#[test]
fn a_complete_block_ack_finishes_without_a_retry() {
    let mut state = AmpduRetryState::new(seq(4095), 3, POLICY).unwrap();
    assert_eq!(
        state.observe(answered(4095, 0b111), 3, at(10), true, FRESH),
        Ok(AmpduRetryDecision::Finish { retry_mask: 0 })
    );
    assert_eq!(state.acknowledged(), 3);
    assert_eq!(state.block_ack_mpdu_attempts(), 3);
}

#[test]
fn a_failed_protection_republishes_unchanged_then_requests_the_block_ack() {
    let mut state = AmpduRetryState::new(seq(10), 3, POLICY).unwrap();
    for _ in 0..2 {
        assert_eq!(
            state.observe(
                AmpduAttemptResult::ProtectionFailure,
                3,
                at(10),
                true,
                FRESH
            ),
            Ok(AmpduRetryDecision::RepublishUnchanged { retry_mask: 0b111 })
        );
    }
    assert_eq!(
        state.observe(
            AmpduAttemptResult::ProtectionFailure,
            3,
            at(10),
            true,
            FRESH
        ),
        Ok(AmpduRetryDecision::RequestBlockAck {
            retry_mask: 0b111,
            starting_sequence: seq(10),
        })
    );
    assert_eq!(state.block_ack_mpdu_attempts(), 0);
    // The request's answer resorts the kept aggregate.
    assert_eq!(
        state.observe_block_ack_request(
            Some(BlockAckReport {
                start_sequence: seq(10),
                bitmap: 0b001,
            }),
            at(20),
            true,
            FRESH
        ),
        Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b110 })
    );
    // An unanswered request keeps every subframe missing.
    assert_eq!(
        state.observe_block_ack_request(None, at(30), true, FRESH),
        Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b11 })
    );
}

#[test]
fn an_aggregate_without_any_block_ack_ends_on_the_retry_limit() {
    let mut state = AmpduRetryState::new(seq(0), 2, POLICY).unwrap();
    for _ in 0..2 {
        assert_eq!(
            state.observe(AmpduAttemptResult::NoResponse, 2, at(10), true, FRESH),
            Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b11 })
        );
    }
    assert_eq!(
        state.observe(AmpduAttemptResult::NoResponse, 2, at(10), true, FRESH),
        Ok(AmpduRetryDecision::Finish { retry_mask: 0b11 })
    );
    assert_eq!(state.block_ack_mpdu_attempts(), 6);
}

#[test]
fn an_ended_agreement_or_one_missing_subframe_leaves_the_aggregate() {
    let mut state = AmpduRetryState::new(seq(0), 3, POLICY).unwrap();
    assert_eq!(
        state.observe(answered(0, 0b001), 3, at(10), false, FRESH),
        Ok(AmpduRetryDecision::Unaggregate { retry_mask: 0b110 })
    );

    let retain = AmpduRetryPolicy {
        retain_single_mpdu: true,
        ..POLICY
    };
    let mut state = AmpduRetryState::new(seq(0), 2, retain).unwrap();
    assert_eq!(
        state.observe(answered(0, 0b01), 2, at(10), true, FRESH),
        Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b10 })
    );
}

#[test]
fn a_trigger_flow_end_and_a_changed_count_are_distinct() {
    let mut state = AmpduRetryState::new(seq(0), 2, POLICY).unwrap();
    assert_eq!(
        state.observe(AmpduAttemptResult::TriggerFlowEnd, 3, at(10), true, FRESH),
        Err(AmpduRetryError::FrameCountChanged {
            expected: 2,
            observed: 3
        })
    );
    assert_eq!(
        state.observe(AmpduAttemptResult::TriggerFlowEnd, 2, at(10), true, FRESH),
        Ok(AmpduRetryDecision::FinishTriggerFlow)
    );
    assert_eq!(state.trigger_flow_completions(), 1);
    assert_eq!(
        AmpduRetryState::new(seq(0), 0, POLICY),
        Err(AmpduRetryError::EmptyAggregate)
    );
    assert_eq!(
        AmpduRetryState::new(seq(0), 65, POLICY),
        Err(AmpduRetryError::TooManySubframes { subframes: 65 })
    );
    assert_eq!(
        AmpduRetryState::new(
            seq(0),
            1,
            AmpduRetryPolicy {
                lifetime: oer_time::Duration::from_micros(0),
                ..POLICY
            }
        ),
        Err(AmpduRetryError::ZeroLifetime)
    );
    // Sixty-four subframes fill the mask.
    let full = AmpduRetryState::new(seq(0), 64, POLICY).unwrap();
    assert_eq!(full.pending_original_indices(), u64::MAX);
}

#[test]
fn helpers_select_and_compact_retries() {
    let sequences = [seq(7), seq(8), seq(9), seq(10)];
    let report = BlockAckReport {
        start_sequence: seq(7),
        bitmap: 0b0101,
    };
    assert_eq!(unacknowledged(&sequences, Some(report)), 0b1010);
    assert_eq!(unacknowledged(&sequences, None), 0b1111);

    let mut kept = sequences;
    assert_eq!(compact(&mut kept, 0b1010), 2);
    assert_eq!(kept[..2], [seq(8), seq(10)]);

    assert_eq!(all_subframes(3), 0b111);
    assert_eq!(all_subframes(64), u64::MAX);
}

#[test]
fn compaction_keeps_exactly_the_selected_prefix_in_order() {
    let original = [11, 22, 33, 44, 55];
    for mask in 0..=u8::MAX {
        let mut items = original;
        let kept = compact(&mut items, u64::from(mask));
        let expected = original
            .into_iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, item)| item);
        assert!(items[..kept].iter().copied().eq(expected));
    }
}

#[test]
fn compaction_respects_empty_arrays_and_the_last_mask_bit() {
    assert_eq!(compact(&mut [0_u8; 0], u64::MAX), 0);
    let original: [usize; 65] = core::array::from_fn(|index| index);
    for mask in [
        0,
        1,
        1 << 63,
        u64::MAX,
        0xaaaa_aaaa_aaaa_aaaa,
        0x5555_5555_5555_5555,
    ] {
        let mut items = original;
        let kept = compact(&mut items, mask);
        let expected = (0..64).filter(|index| mask & (1 << index) != 0);
        assert!(items[..kept].iter().copied().eq(expected));
        assert_eq!(items[64], original[64]);
    }
}

/// Subframe 0 admitted long ago, the others at zero.
fn head_old(index: u8) -> Option<Instant> {
    Some(if index == 0 {
        Instant::from_micros(0)
    } else {
        Instant::from_micros(5_000)
    })
}

#[test]
fn an_msdu_ages_from_its_admission_within_the_margin() {
    let state = AmpduRetryState::new(seq(0), 2, POLICY).unwrap();
    // Aged once less than the 1 ms margin of its 10 ms lifetime is left.
    assert!(!state.aged(at(1_000), at(10_000)));
    assert!(state.aged(at(1_000), at(10_001)));
    // An admission after now keeps its whole lifetime.
    assert!(!state.aged(at(20_000), at(10_000)));
    // A margin beyond the lifetime ages at once.
    let early = AmpduRetryState::new(
        seq(0),
        2,
        AmpduRetryPolicy {
            aged_margin: oer_time::Duration::from_micros(11_000),
            ..POLICY
        },
    )
    .unwrap();
    assert!(early.aged(at(7), at(7)));
}

#[test]
fn an_aged_head_under_the_agreement_is_discarded_behind_a_block_ack_request() {
    let mut state = AmpduRetryState::new(seq(100), 4, POLICY).unwrap();
    // 100, 101 and 103 missing at 13 ms: 100 is aged, the rest is not.
    assert_eq!(
        state.observe(answered(100, 0b0100), 4, at(13_000), true, head_old),
        Ok(AmpduRetryDecision::DiscardAged {
            retry_mask: 0b1010,
            aged_mask: 0b0001,
            starting_sequence: seq(101),
        })
    );
    assert_eq!(state.discarded_original_indices(), 0b0001);
    assert_eq!(state.block_ack_request_start(), seq(101));
    assert_eq!(state.current_subframes(), 2);
    assert_eq!(state.pending_original_indices(), 0b1010);
    // The BlockAck answering the request resorts what remains.
    assert_eq!(
        state.observe_block_ack_request(
            Some(BlockAckReport {
                start_sequence: seq(101),
                bitmap: 0b001,
            }),
            at(13_100),
            true,
            head_old,
        ),
        Ok(AmpduRetryDecision::Unaggregate { retry_mask: 0b10 })
    );
}

#[test]
fn a_whole_aged_window_is_discarded_and_the_request_starts_after_it() {
    let mut state = AmpduRetryState::new(seq(4094), 3, POLICY).unwrap();
    // 4094 acknowledged; 4095 and 0 missing and aged.
    assert_eq!(
        state.observe(answered(4094, 0b001), 3, at(9_001), true, FRESH),
        Ok(AmpduRetryDecision::DiscardAged {
            retry_mask: 0,
            aged_mask: 0b110,
            starting_sequence: seq(1),
        })
    );
    assert_eq!(state.current_subframes(), 0);
    // Whatever answers the request, nothing is left to send.
    assert_eq!(
        state.observe_block_ack_request(None, at(9_100), true, FRESH),
        Ok(AmpduRetryDecision::Finish { retry_mask: 0 })
    );
}

#[test]
fn without_the_agreement_aged_subframes_go_without_a_block_ack_request() {
    let mut state = AmpduRetryState::new(seq(100), 4, POLICY).unwrap();
    assert_eq!(
        state.observe(answered(100, 0b0100), 4, at(13_000), false, head_old),
        Ok(AmpduRetryDecision::Unaggregate { retry_mask: 0b1010 })
    );
    assert_eq!(state.discarded_original_indices(), 0b0001);
    let mut state = AmpduRetryState::new(seq(100), 2, POLICY).unwrap();
    assert_eq!(
        state.observe(AmpduAttemptResult::NoResponse, 2, at(9_001), false, FRESH),
        Ok(AmpduRetryDecision::Finish { retry_mask: 0b11 })
    );
    assert_eq!(state.discarded_original_indices(), 0b11);
}

#[test]
fn aging_applies_after_a_failed_protection_and_an_unanswered_aggregate() {
    for result in [
        AmpduAttemptResult::ProtectionFailure,
        AmpduAttemptResult::NoResponse,
    ] {
        let mut state = AmpduRetryState::new(seq(100), 2, POLICY).unwrap();
        // At 8 ms nothing is aged and the aggregate goes again.
        assert!(matches!(
            state.observe(result, 2, at(8_000), true, FRESH),
            Ok(AmpduRetryDecision::RepublishUnchanged { .. }
                | AmpduRetryDecision::RetainAggregate { .. })
        ));
        assert_eq!(
            state.observe(result, 2, at(13_000), true, head_old),
            Ok(AmpduRetryDecision::DiscardAged {
                retry_mask: 0b10,
                aged_mask: 0b01,
                starting_sequence: seq(101),
            }),
            "{result:?}"
        );
    }
}

#[test]
fn a_subframe_without_an_admission_instant_is_a_handled_error() {
    let mut state = AmpduRetryState::new(seq(100), 3, POLICY).unwrap();
    let unknown = |index: u8| (index != 2).then(|| Instant::from_micros(0));
    assert_eq!(
        state.observe(answered(100, 0b001), 3, at(10), true, unknown),
        Err(AmpduRetryError::UnknownSubframe { index: 2 })
    );
    // An acknowledged subframe needs no instant.
    let mut state = AmpduRetryState::new(seq(100), 3, POLICY).unwrap();
    assert_eq!(
        state.observe(answered(100, 0b100), 3, at(10), true, unknown),
        Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b011 })
    );
}
