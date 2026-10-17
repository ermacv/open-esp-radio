use super::*;

const POLICY: AmpduRetryPolicy = AmpduRetryPolicy {
    lifetime: oer_time::RadioDuration::from_micros(10_000),
    aged_margin: oer_time::RadioDuration::from_micros(1_000),
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
    let mut state = AmpduRetryState::new(
        seq(100),
        4,
        POLICY,
        oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(0),
    )
    .unwrap();
    // 100 and 102 acknowledged; 101 and 103 missing.
    assert_eq!(
        state.observe(
            answered(100, 0b0101),
            4,
            oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(10),
            true
        ),
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
        state.observe(
            answered(101, 0b100),
            2,
            oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(20),
            true
        ),
        Ok(AmpduRetryDecision::Unaggregate { retry_mask: 0b01 })
    );
    assert_eq!(state.missing_original_indices(), 0b0010);
}

#[test]
fn a_complete_block_ack_finishes_without_a_retry() {
    let mut state = AmpduRetryState::new(
        seq(4095),
        3,
        POLICY,
        oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(0),
    )
    .unwrap();
    assert_eq!(
        state.observe(
            answered(4095, 0b111),
            3,
            oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(10),
            true
        ),
        Ok(AmpduRetryDecision::Finish { retry_mask: 0 })
    );
    assert_eq!(state.acknowledged(), 3);
    assert_eq!(state.block_ack_mpdu_attempts(), 3);
}

#[test]
fn a_failed_protection_republishes_unchanged_then_requests_the_block_ack() {
    let mut state = AmpduRetryState::new(
        seq(10),
        3,
        POLICY,
        oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(0),
    )
    .unwrap();
    for _ in 0..2 {
        assert_eq!(
            state.observe(
                AmpduAttemptResult::ProtectionFailure,
                3,
                oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(10),
                true
            ),
            Ok(AmpduRetryDecision::RepublishUnchanged { retry_mask: 0b111 })
        );
    }
    assert_eq!(
        state.observe(
            AmpduAttemptResult::ProtectionFailure,
            3,
            oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(10),
            true
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
            oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(20),
            true,
        ),
        AmpduRetryDecision::RetainAggregate { retry_mask: 0b110 }
    );
    // An unanswered request keeps every subframe missing.
    assert_eq!(
        state.observe_block_ack_request(
            None,
            oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(30),
            true
        ),
        AmpduRetryDecision::RetainAggregate { retry_mask: 0b11 }
    );
}

#[test]
fn an_aggregate_without_any_block_ack_ends_on_the_retry_limit() {
    let mut state = AmpduRetryState::new(
        seq(0),
        2,
        POLICY,
        oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(0),
    )
    .unwrap();
    for _ in 0..2 {
        assert_eq!(
            state.observe(
                AmpduAttemptResult::NoResponse,
                2,
                oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(10),
                true
            ),
            Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b11 })
        );
    }
    assert_eq!(
        state.observe(
            AmpduAttemptResult::NoResponse,
            2,
            oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(10),
            true
        ),
        Ok(AmpduRetryDecision::Finish { retry_mask: 0b11 })
    );
    assert_eq!(state.block_ack_mpdu_attempts(), 6);
}

#[test]
fn aged_subframes_are_discarded_instead_of_retried() {
    let mut state = AmpduRetryState::new(
        seq(0),
        2,
        POLICY,
        oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(1_000),
    )
    .unwrap();
    // Aged from 1_000 + 10_000 - 1_000 + 1.
    assert!(
        !state.aged(oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(
            10_000
        ))
    );
    assert!(
        state.aged(oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(
            10_001
        ))
    );
    assert_eq!(
        state.observe(
            answered(0, 0b01),
            2,
            oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(10_001),
            true
        ),
        Ok(AmpduRetryDecision::Finish { retry_mask: 0b10 })
    );
}

#[test]
fn an_ended_agreement_or_one_missing_subframe_leaves_the_aggregate() {
    let mut state = AmpduRetryState::new(
        seq(0),
        3,
        POLICY,
        oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(0),
    )
    .unwrap();
    assert_eq!(
        state.observe(
            answered(0, 0b001),
            3,
            oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(10),
            false
        ),
        Ok(AmpduRetryDecision::Unaggregate { retry_mask: 0b110 })
    );

    let retain = AmpduRetryPolicy {
        retain_single_mpdu: true,
        ..POLICY
    };
    let mut state = AmpduRetryState::new(
        seq(0),
        2,
        retain,
        oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(0),
    )
    .unwrap();
    assert_eq!(
        state.observe(
            answered(0, 0b01),
            2,
            oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(10),
            true
        ),
        Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b10 })
    );
}

#[test]
fn a_trigger_flow_end_and_a_changed_count_are_distinct() {
    let mut state = AmpduRetryState::new(
        seq(0),
        2,
        POLICY,
        oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(0),
    )
    .unwrap();
    assert_eq!(
        state.observe(
            AmpduAttemptResult::TriggerFlowEnd,
            3,
            oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(10),
            true
        ),
        Err(AmpduRetryError::FrameCountChanged {
            expected: 2,
            observed: 3
        })
    );
    assert_eq!(
        state.observe(
            AmpduAttemptResult::TriggerFlowEnd,
            2,
            oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(10),
            true
        ),
        Ok(AmpduRetryDecision::FinishTriggerFlow)
    );
    assert_eq!(state.trigger_flow_completions(), 1);
    assert_eq!(
        AmpduRetryState::new(
            seq(0),
            0,
            POLICY,
            oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(0)
        ),
        Err(AmpduRetryError::EmptyAggregate)
    );
    assert_eq!(
        AmpduRetryState::new(
            seq(0),
            65,
            POLICY,
            oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(0)
        ),
        Err(AmpduRetryError::TooManySubframes { subframes: 65 })
    );
    assert_eq!(
        AmpduRetryState::new(
            seq(0),
            1,
            AmpduRetryPolicy {
                lifetime: oer_time::RadioDuration::from_micros(0),
                ..POLICY
            },
            oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(0)
        ),
        Err(AmpduRetryError::ZeroLifetime)
    );
    // Sixty-four subframes fill the mask.
    let full = AmpduRetryState::new(
        seq(0),
        64,
        POLICY,
        oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(0),
    )
    .unwrap();
    assert_eq!(full.pending_original_indices(), u64::MAX);
}

#[test]
fn helpers_select_compact_and_mark_retries() {
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

    let mut mpdu = [0x88_u8, 0x01, 0, 0];
    assert!(set_retry_bit(&mut mpdu));
    assert_eq!(mpdu[1], 0x09);
    assert!(!set_retry_bit(&mut [0x88]));
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

#[test]
fn aging_threshold_is_exact_at_radio_epoch_boundaries() {
    use oer_time::RadioDuration;
    let policy = AmpduRetryPolicy {
        lifetime: RadioDuration::from_micros(10),
        aged_margin: RadioDuration::from_micros(3),
        ..POLICY
    };
    let first = Ieee80211Instant::from_micros(0);
    let state = AmpduRetryState::new(SequenceNumber::new(0).unwrap(), 2, policy, first).unwrap();
    assert!(!state.aged(Ieee80211Instant::from_micros(7)));
    assert!(state.aged(Ieee80211Instant::from_micros(8)));
    let near_end = Ieee80211Instant::from_micros(u64::MAX - 8);
    let state = AmpduRetryState::new(SequenceNumber::new(0).unwrap(), 2, policy, near_end).unwrap();
    assert!(!state.aged(Ieee80211Instant::from_micros(u64::MAX - 1)));
    assert!(state.aged(Ieee80211Instant::from_micros(u64::MAX)));
    assert!(matches!(
        AmpduRetryState::new(
            SequenceNumber::new(0).unwrap(),
            2,
            policy,
            Ieee80211Instant::from_micros(u64::MAX - 7)
        ),
        Err(AmpduRetryError::AgingThresholdOutsideEpoch)
    ));
    let early_policy = AmpduRetryPolicy {
        lifetime: RadioDuration::from_micros(10),
        aged_margin: RadioDuration::from_micros(11),
        ..POLICY
    };
    assert!(
        AmpduRetryState::new(SequenceNumber::new(0).unwrap(), 2, early_policy, first)
            .unwrap()
            .aged(first)
    );
    let before_epoch = AmpduRetryPolicy {
        aged_margin: RadioDuration::from_micros(12),
        ..early_policy
    };
    assert!(matches!(
        AmpduRetryState::new(SequenceNumber::new(0).unwrap(), 2, before_epoch, first),
        Err(AmpduRetryError::AgingThresholdOutsideEpoch)
    ));
}

#[test]
fn every_retry_outcome_finishes_at_the_first_aged_instant() {
    for prior_protection_failures in 0..POLICY.retry_limit {
        for result in [
            AmpduAttemptResult::ProtectionFailure,
            AmpduAttemptResult::NoResponse,
            AmpduAttemptResult::Answered(None),
            answered(100, 0b0101),
            answered(100, 0b1111),
            AmpduAttemptResult::TriggerFlowEnd,
        ] {
            for now in [9_000, 9_001] {
                let mut state =
                    AmpduRetryState::new(seq(100), 4, POLICY, Ieee80211Instant::from_micros(0))
                        .unwrap();
                for _ in 0..prior_protection_failures {
                    assert_eq!(
                        state.observe(
                            AmpduAttemptResult::ProtectionFailure,
                            4,
                            Ieee80211Instant::from_micros(1),
                            true,
                        ),
                        Ok(AmpduRetryDecision::RepublishUnchanged { retry_mask: 0b1111 })
                    );
                }
                let decision = state
                    .observe(result, 4, Ieee80211Instant::from_micros(now), true)
                    .unwrap();
                if result == AmpduAttemptResult::TriggerFlowEnd {
                    assert_eq!(decision, AmpduRetryDecision::FinishTriggerFlow);
                } else if now == 9_001 {
                    let retry_mask = match result {
                        AmpduAttemptResult::Answered(Some(report)) => !report.bitmap & 0b1111,
                        _ => 0b1111,
                    };
                    assert_eq!(
                        decision,
                        AmpduRetryDecision::Finish { retry_mask },
                        "{result:?}"
                    );
                    assert_eq!(state.missing_original_indices(), retry_mask);
                } else if result == answered(100, 0b1111) {
                    assert_eq!(decision, AmpduRetryDecision::Finish { retry_mask: 0 });
                } else {
                    assert!(
                        !matches!(decision, AmpduRetryDecision::Finish { .. }),
                        "{result:?}: {decision:?}"
                    );
                }
            }
        }
    }
}
