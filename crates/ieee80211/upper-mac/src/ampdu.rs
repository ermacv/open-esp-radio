//! Retry of an A-MPDU from its BlockAck.
//!
//! A backend without `HardwareServices::AMPDU_RETRY_SELECTION` reports the
//! recipient's BlockAck ([`BlockAckReport`]) and leaves the choice of what to
//! send next to the caller. [`AmpduRetryState`] makes that choice for one
//! aggregate: after each attempt it keeps the unacknowledged subframes, in
//! their original order, for another aggregate with their Retry bit set,
//! republishes the unchanged aggregate after a failed protection exchange,
//! hands the missing subframes over to individual transmission when the
//! Block Ack agreement ended or one subframe remains, or ends the exchange
//! when every subframe is acknowledged, a retry limit is reached or the MSDU
//! lifetime has run out.
//!
//! Subframes are named by their position in the first aggregate (bit `i`
//! of a mask is subframe `i`); a retry compacts the aggregate to the
//! selected subframes, so the helpers here ([`unacknowledged`],
//! [`compact`], [`set_retry_bit`]) work on those masks and on the encoded
//! MPDUs.

use oer_ieee80211_lower_mac::BlockAckReport;
use oer_ieee80211_lower_mac::{Ieee80211Duration, Ieee80211Instant};
use oer_ieee80211_mac::sequence::SequenceNumber;

/// Subframes one A-MPDU exchange tracks: the width of a 64-bit BlockAck
/// bitmap.
pub const MAX_AMPDU_SUBFRAMES: u8 = 64;

/// The Retry subfield of the Frame Control field's second octet.
const RETRY: u8 = 0x08;

/// Mark an encoded MPDU as a retransmission: set the Retry subfield of its
/// Frame Control field. `false` when the MPDU is too short to have one.
pub fn set_retry_bit(mpdu: &mut [u8]) -> bool {
    match mpdu.get_mut(1) {
        Some(flags) => {
            *flags |= RETRY;
            true
        }
        None => false,
    }
}

/// The mask of the first `count` subframes.
pub const fn all_subframes(count: u8) -> u64 {
    if count >= 64 {
        u64::MAX
    } else {
        (1_u64 << count).wrapping_sub(1)
    }
}

/// The subframes of `sequences` (subframe `i` carries `sequences[i]`) that
/// `report` does not acknowledge; every subframe without a report.
pub fn unacknowledged(sequences: &[SequenceNumber], report: Option<BlockAckReport>) -> u64 {
    let mut missing = 0_u64;
    for (index, sequence) in sequences.iter().take(64).enumerate() {
        if !report.is_some_and(|report| report.acknowledges(*sequence)) {
            missing |= 1_u64 << index;
        }
    }
    missing
}

/// Keep the entries of `items` whose bit is set in `mask`, moved to the
/// front in their order; returns how many were kept. Only the first 64
/// entries have mask bits. The suffix after the kept prefix is unspecified.
///
/// Compaction swaps each selected entry into the next free prefix position,
/// using at most one entry of temporary storage rather than copying the array.
pub fn compact<T: Copy, const N: usize>(items: &mut [T; N], mask: u64) -> usize {
    // This helper moves copyable metadata, never affine MPDU/body owners.
    let mut kept = 0_usize;
    // The mask represents at most MAX_AMPDU_SUBFRAMES entries.
    for index in 0..N.min(usize::from(MAX_AMPDU_SUBFRAMES)) {
        if mask & (1_u64 << index) != 0 {
            // kept counts selected entries before index, hence kept <= index
            // < N. Both swap indices are in bounds, and the displaced value
            // is behind the unread suffix. Selected order is preserved.
            items.swap(kept, index);
            // The count is bounded by the 64 mask positions.
            kept = kept.saturating_add(1);
        }
    }
    kept
}

/// The limits of one aggregate's retries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduRetryPolicy {
    /// Time from the aggregate's commit after which its MSDUs are aged and
    /// discarded instead of retried.
    pub lifetime: Ieee80211Duration,
    /// An MSDU with less than this much lifetime left is already aged.
    pub aged_margin: Ieee80211Duration,
    /// Attempts that end without any BlockAck, and failed protection
    /// exchanges in a row, that the aggregate survives.
    pub retry_limit: u8,
    /// Keep one missing MPDU in an aggregate instead of sending it alone.
    pub retain_single_mpdu: bool,
}

/// How one A-MPDU attempt ended, as the retry policy reads it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmpduAttemptResult {
    /// A BlockAck answered the aggregate. `None` when the backend received
    /// a response but cannot vouch for its bitmap: every subframe counts as
    /// missing.
    Answered(Option<BlockAckReport>),
    /// No BlockAck arrived.
    NoResponse,
    /// The protection exchange failed before the aggregate: no subframe was
    /// sent.
    ProtectionFailure,
    /// The exchange ended through a trigger-based completion that carries no
    /// ordinary BlockAck.
    TriggerFlowEnd,
}

/// What to send after one A-MPDU attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmpduRetryDecision {
    /// Compact the aggregate to the selected subframes (positions in the
    /// aggregate just sent), set their Retry bit and send another A-MPDU.
    RetainAggregate { retry_mask: u64 },
    /// The protection exchange failed: send the same aggregate again,
    /// unchanged; no subframe gains the Retry bit.
    RepublishUnchanged { retry_mask: u64 },
    /// The protection exchange failed at every attempt: keep the aggregate
    /// and send a BlockAckReq starting at its first subframe; the BlockAck
    /// that answers it goes to [`AmpduRetryState::observe_block_ack_request`].
    RequestBlockAck {
        retry_mask: u64,
        starting_sequence: SequenceNumber,
    },
    /// End the aggregate and send the selected subframes individually, in
    /// order, each with its Retry bit set.
    Unaggregate { retry_mask: u64 },
    /// End the exchange; the selected subframes were not acknowledged and are
    /// discarded.
    Finish { retry_mask: u64 },
    /// End the exchange through the trigger-based completion; no BlockAck
    /// was received.
    FinishTriggerFlow,
}

impl AmpduRetryDecision {
    pub const fn retry_mask(self) -> u64 {
        match self {
            Self::RetainAggregate { retry_mask }
            | Self::RepublishUnchanged { retry_mask }
            | Self::RequestBlockAck { retry_mask, .. }
            | Self::Unaggregate { retry_mask }
            | Self::Finish { retry_mask } => retry_mask,
            Self::FinishTriggerFlow => 0,
        }
    }

    pub const fn missing(self) -> u8 {
        self.retry_mask().count_ones() as u8
    }
}

/// Why an aggregate's retry state was not created or not advanced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmpduRetryError {
    ZeroLifetime,
    /// The strict aging threshold lies outside this radio epoch.
    AgingThresholdOutsideEpoch,
    EmptyAggregate,
    TooManySubframes {
        subframes: u8,
    },
    /// The attempt carried another number of subframes than the state kept.
    FrameCountChanged {
        expected: u8,
        observed: u8,
    },
}

/// The retry state of one aggregate.
///
/// Sequence numbers are the first aggregate's, consecutive from its first
/// subframe; a retry compacts only the missing subframes, and the state
/// keeps which original positions remain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduRetryState {
    first_sequence: SequenceNumber,
    /// Original positions the current aggregate still carries.
    pending_original_indices: u64,
    /// Original positions absent from the last observed completion.
    missing_original_indices: u64,
    current_subframes: u8,
    retry_limit: u8,
    retain_single_mpdu: bool,
    /// From this instant on the aggregate's MSDUs are aged.
    aged_from: Ieee80211Instant,
    aggregate_attempts: u8,
    acknowledged: u8,
    block_ack_mpdu_attempts: u16,
    trigger_flow_completions: u8,
    protection_failures: u8,
    /// Attempts that ended without any BlockAck.
    ack_timeouts: u8,
}

impl AmpduRetryState {
    /// The state of an aggregate of `subframes` consecutive sequence numbers
    /// from `first_sequence`, whose MSDUs were committed at
    /// `committed_at` on the backend's radio clock, before its first attempt.
    pub fn new(
        first_sequence: SequenceNumber,
        subframes: u8,
        policy: AmpduRetryPolicy,
        committed_at: Ieee80211Instant,
    ) -> Result<Self, AmpduRetryError> {
        if policy.lifetime.as_micros() == 0 {
            return Err(AmpduRetryError::ZeroLifetime);
        }
        if subframes == 0 {
            return Err(AmpduRetryError::EmptyAggregate);
        }
        if subframes > MAX_AMPDU_SUBFRAMES {
            return Err(AmpduRetryError::TooManySubframes { subframes });
        }
        // Aging is strictly after committed + lifetime - margin. Form the
        // relative offset first so a representable threshold is not rejected
        // because an unnecessary intermediate endpoint would overflow.
        let one = Ieee80211Duration::from_micros(1);
        let aged_from = if let Some(delay) = policy.lifetime.checked_sub(policy.aged_margin) {
            delay
                .checked_add(one)
                .and_then(|delay| committed_at.checked_add(delay))
        } else {
            policy
                .aged_margin
                .checked_sub(policy.lifetime)
                .and_then(|advance| advance.checked_sub(one))
                .and_then(|advance| committed_at.checked_sub(advance))
        }
        .ok_or(AmpduRetryError::AgingThresholdOutsideEpoch)?;
        Ok(Self {
            first_sequence,
            pending_original_indices: all_subframes(subframes),
            missing_original_indices: 0,
            current_subframes: subframes,
            retry_limit: policy.retry_limit,
            retain_single_mpdu: policy.retain_single_mpdu,
            aged_from,
            aggregate_attempts: 1,
            acknowledged: 0,
            block_ack_mpdu_attempts: 0,
            trigger_flow_completions: 0,
            protection_failures: 0,
            ack_timeouts: 0,
        })
    }

    /// Apply how the attempt carrying `observed_subframes` ended at
    /// `now`. While the Block Ack agreement is not
    /// `block_ack_operational`, missing subframes leave the aggregate for
    /// individual retries.
    pub fn observe(
        &mut self,
        result: AmpduAttemptResult,
        observed_subframes: u8,
        now: Ieee80211Instant,
        block_ack_operational: bool,
    ) -> Result<AmpduRetryDecision, AmpduRetryError> {
        if observed_subframes != self.current_subframes {
            return Err(AmpduRetryError::FrameCountChanged {
                expected: self.current_subframes,
                observed: observed_subframes,
            });
        }
        let every = all_subframes(observed_subframes);
        match result {
            AmpduAttemptResult::TriggerFlowEnd => {
                self.trigger_flow_completions = self.trigger_flow_completions.saturating_add(1);
                self.missing_original_indices = 0;
                Ok(AmpduRetryDecision::FinishTriggerFlow)
            }
            AmpduAttemptResult::ProtectionFailure => {
                self.protection_failures = self.protection_failures.saturating_add(1);
                // No subframe reached the recipient.
                self.missing_original_indices = self.pending_original_indices;
                if self.aged(now) {
                    return Ok(AmpduRetryDecision::Finish { retry_mask: every });
                }
                if self.protection_failures < self.retry_limit {
                    return Ok(AmpduRetryDecision::RepublishUnchanged { retry_mask: every });
                }
                // The exhausted exchange keeps the aggregate and asks for
                // its BlockAck.
                self.protection_failures = 0;
                Ok(AmpduRetryDecision::RequestBlockAck {
                    retry_mask: every,
                    starting_sequence: self.current_first_sequence(),
                })
            }
            AmpduAttemptResult::NoResponse => {
                self.count_mpdu_attempts(observed_subframes);
                self.ack_timeouts = self.ack_timeouts.saturating_add(1);
                self.missing_original_indices = self.pending_original_indices;
                if self.ack_timeouts >= self.retry_limit || self.aged(now) {
                    return Ok(AmpduRetryDecision::Finish { retry_mask: every });
                }
                Ok(self.retain_or_unaggregate(every, observed_subframes, block_ack_operational))
            }
            AmpduAttemptResult::Answered(report) => {
                self.count_mpdu_attempts(observed_subframes);
                Ok(self.resort(report, now, block_ack_operational))
            }
        }
    }

    /// Resort the kept aggregate by the BlockAck answering its BlockAckReq;
    /// `None` when the request went unanswered and every subframe stays
    /// missing.
    pub fn observe_block_ack_request(
        &mut self,
        block_ack: Option<BlockAckReport>,
        now: Ieee80211Instant,
        block_ack_operational: bool,
    ) -> AmpduRetryDecision {
        self.resort(block_ack, now, block_ack_operational)
    }

    fn count_mpdu_attempts(&mut self, subframes: u8) {
        self.block_ack_mpdu_attempts = self
            .block_ack_mpdu_attempts
            .saturating_add(u16::from(subframes));
    }

    /// Keep the subframes `report` does not acknowledge.
    fn resort(
        &mut self,
        report: Option<BlockAckReport>,
        now: Ieee80211Instant,
        block_ack_operational: bool,
    ) -> AmpduRetryDecision {
        let mut retry_mask = 0_u64;
        let mut retry_original_indices = 0_u64;
        let mut index = 0_u8;
        while index < self.current_subframes {
            let original_index = self.original_index(index);
            let sequence = self.first_sequence.wrapping_add(u16::from(original_index));
            if report.is_some_and(|report| report.acknowledges(sequence)) {
                self.acknowledged = self.acknowledged.saturating_add(1);
            } else {
                retry_mask |= 1_u64 << index;
                retry_original_indices |= 1_u64 << original_index;
            }
            index = index.saturating_add(1);
        }
        self.missing_original_indices = retry_original_indices;
        let missing = retry_mask.count_ones() as u8;
        if missing == 0 || self.aged(now) {
            return AmpduRetryDecision::Finish { retry_mask };
        }
        let decision = self.retain_or_unaggregate(retry_mask, missing, block_ack_operational);
        if matches!(decision, AmpduRetryDecision::RetainAggregate { .. }) {
            self.pending_original_indices = retry_original_indices;
            self.current_subframes = missing;
        }
        decision
    }

    fn retain_or_unaggregate(
        &mut self,
        retry_mask: u64,
        missing: u8,
        block_ack_operational: bool,
    ) -> AmpduRetryDecision {
        if !block_ack_operational || (missing == 1 && !self.retain_single_mpdu) {
            return AmpduRetryDecision::Unaggregate { retry_mask };
        }
        self.aggregate_attempts = self.aggregate_attempts.saturating_add(1);
        AmpduRetryDecision::RetainAggregate { retry_mask }
    }

    /// Subframes of the current aggregate.
    pub const fn current_subframes(&self) -> u8 {
        self.current_subframes
    }

    /// Whether the aggregate's MSDUs are aged at `now`.
    pub fn aged(&self, now: Ieee80211Instant) -> bool {
        now >= self.aged_from
    }

    /// The sequence number of the current aggregate's first subframe.
    pub const fn current_first_sequence(&self) -> SequenceNumber {
        self.first_sequence
            .wrapping_add(self.pending_original_indices.trailing_zeros() as u16)
    }

    /// Original positions the current aggregate carries.
    pub const fn pending_original_indices(&self) -> u64 {
        self.pending_original_indices
    }

    /// Original positions absent from the last observed completion.
    pub const fn missing_original_indices(&self) -> u64 {
        self.missing_original_indices
    }

    /// A-MPDU attempts, the first included.
    pub const fn aggregate_attempts(&self) -> u8 {
        self.aggregate_attempts
    }

    /// Subframes a BlockAck acknowledged.
    pub const fn acknowledged(&self) -> u8 {
        self.acknowledged
    }

    /// Subframe transmissions a BlockAck or its absence accounted.
    pub const fn block_ack_mpdu_attempts(&self) -> u16 {
        self.block_ack_mpdu_attempts
    }

    /// Protection exchanges that failed before the aggregate.
    pub const fn protection_failures(&self) -> u8 {
        self.protection_failures
    }

    pub const fn trigger_flow_completions(&self) -> u8 {
        self.trigger_flow_completions
    }

    /// The original position of the `compacted_index`th current subframe.
    pub fn original_index(&self, compacted_index: u8) -> u8 {
        let mut remaining = self.pending_original_indices;
        let mut position = compacted_index;
        loop {
            let original = remaining.trailing_zeros() as u8;
            if position == 0 || remaining == 0 {
                return original;
            }
            remaining &= remaining.wrapping_sub(1);
            position = position.saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests;
