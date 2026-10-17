use oer_ieee80211_mac::phy::LegacyRate;

use super::*;

const VENDOR_LIKE: RetryLimits = RetryLimits {
    short: 4,
    long: 3,
    ack_failure: AckFailureAccounting::Short,
};

/// 54, 48, 24, then 6 Mb/s from the third failure on.
struct StepDown;

impl RateLadder for StepDown {
    fn rate(&self, initial: PhyRate, failures: u8) -> Option<PhyRate> {
        Some(match failures {
            0 => initial,
            1 => PhyRate::Legacy(LegacyRate::Ofdm48M),
            2 => PhyRate::Legacy(LegacyRate::Ofdm24M),
            3..=5 => PhyRate::Legacy(LegacyRate::Ofdm6M),
            _ => return None,
        })
    }
}

const OFDM54: PhyRate = PhyRate::Legacy(LegacyRate::Ofdm54M);

fn retry(set_retry_bit: bool) -> RetryDecision {
    RetryDecision::Retry { set_retry_bit }
}

fn complete(outcome: RetryOutcome) -> RetryDecision {
    RetryDecision::Complete(outcome)
}

#[test]
fn an_acknowledged_first_attempt_ends_the_frame() {
    let mut state = MpduRetryState::new(VENDOR_LIKE, 5, FrameClass::Short).unwrap();
    assert_eq!(state.rate(&StepDown, OFDM54, OFDM54), OFDM54);
    assert_eq!(
        state.observe(TxStatus::Success),
        complete(RetryOutcome::Delivered)
    );
    assert_eq!(state.attempts(), 1);
    assert_eq!(
        MpduRetryState::new(VENDOR_LIKE, 0, FrameClass::Short),
        Err(RetryStateError::ZeroMpduRetryLimit)
    );
}

#[test]
fn ack_timeouts_set_the_retry_bit_walk_the_ladder_and_stop_at_the_mpdu_limit() {
    let mut state = MpduRetryState::new(VENDOR_LIKE, 3, FrameClass::Long).unwrap();
    assert_eq!(state.observe(TxStatus::AckTimeout), retry(true));
    assert_eq!(
        state.rate(&StepDown, OFDM54, OFDM54),
        PhyRate::Legacy(LegacyRate::Ofdm48M)
    );
    assert_eq!(state.observe(TxStatus::AckTimeout), retry(true));
    // Short accounting: a long frame's ACK timeouts count as short.
    assert_eq!(
        state.counters(),
        RetryCounters {
            mpdu: 2,
            short: 2,
            long: 0
        }
    );
    assert_eq!(
        state.observe(TxStatus::AckTimeout),
        complete(RetryOutcome::RetryLimit(TxStatus::AckTimeout))
    );
    assert_eq!(state.attempts(), 3);
}

#[test]
fn cts_timeouts_walk_the_ladder_without_the_retry_bit_past_the_mpdu_limit() {
    let mut state = MpduRetryState::new(VENDOR_LIKE, 1, FrameClass::Short).unwrap();
    for _ in 0..3 {
        assert_eq!(state.observe(TxStatus::CtsTimeout), retry(false));
    }
    assert_eq!(state.counters().mpdu, 0);
    assert_eq!(
        state.rate(&StepDown, OFDM54, OFDM54),
        PhyRate::Legacy(LegacyRate::Ofdm6M)
    );
    assert_eq!(
        state.observe(TxStatus::CtsTimeout),
        complete(RetryOutcome::RetryLimit(TxStatus::CtsTimeout))
    );
}

#[test]
fn a_long_collision_keeps_the_rate_and_counts_against_the_long_limit() {
    let mut state = MpduRetryState::new(VENDOR_LIKE, 5, FrameClass::Long).unwrap();
    assert_eq!(state.observe(TxStatus::Collision), retry(false));
    assert_eq!(state.rate(&StepDown, OFDM54, OFDM54), OFDM54);
    assert_eq!(state.observe(TxStatus::Collision), retry(false));
    assert_eq!(
        state.observe(TxStatus::Collision),
        complete(RetryOutcome::RetryLimit(TxStatus::Collision))
    );
    assert_eq!(state.counters().long, 3);
}

#[test]
fn by_frame_class_accounting_counts_a_long_frame_ack_timeout_as_long() {
    let mut state = MpduRetryState::new(RetryLimits::IEEE_DEFAULT, 10, FrameClass::Long).unwrap();
    for _ in 0..3 {
        assert_eq!(state.observe(TxStatus::AckTimeout), retry(true));
    }
    assert_eq!(
        state.observe(TxStatus::AckTimeout),
        complete(RetryOutcome::RetryLimit(TxStatus::AckTimeout))
    );
    assert_eq!(
        state.counters(),
        RetryCounters {
            mpdu: 4,
            short: 0,
            long: 4
        }
    );
    // The ladder follows the MPDU counter.
    assert_eq!(state.counters().ladder_step(), 4);
}

#[test]
fn an_unknown_hardware_timeout_ends_the_frame_without_a_retry() {
    let mut state = MpduRetryState::new(VENDOR_LIKE, 5, FrameClass::Short).unwrap();
    let before = state.counters();
    assert_eq!(
        state.observe(TxStatus::TimedOut),
        complete(RetryOutcome::Terminal(TxStatus::TimedOut))
    );
    assert_eq!(state.counters(), before);
}

#[test]
fn a_ladder_without_a_step_keeps_the_previous_rate() {
    let mut state = MpduRetryState::new(RetryLimits::IEEE_DEFAULT, 10, FrameClass::Short).unwrap();
    for _ in 0..6 {
        state.observe(TxStatus::AckTimeout);
    }
    let previous = PhyRate::Legacy(LegacyRate::Ofdm6M);
    assert_eq!(state.rate(&StepDown, OFDM54, previous), previous);
    assert_eq!(state.rate(&FixedRate, OFDM54, previous), OFDM54);
}
