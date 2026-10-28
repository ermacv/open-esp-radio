//! A core-owned retry adapter for the port's bounded subframe masks.
use crate::{
    AmpduAttemptResult, AmpduRetryDecision as UpperAmpduRetryDecision,
    AmpduRetryError as UpperAmpduRetryError, AmpduRetryState as UpperAmpduRetryState,
};
use oer_ieee80211_port::{TxStatus, backend_aggregate::*};
use oer_ieee80211_wire::sequence::SequenceNumber;
/// Bounded core retry owner accepting detached port completion facts.
pub struct BoundedAmpduRetryState<const CAPACITY: usize> {
    state: UpperAmpduRetryState,
}

impl<const CAPACITY: usize> BoundedAmpduRetryState<CAPACITY> {
    /// Start at the first Sequence Control value already consumed by the
    /// encoded aggregate. Each MPDU ages from its MSDU's admission on the
    /// monotonic clock, which the caller gives with every completion.
    pub fn new(
        first_sequence: SequenceNumber,
        subframes: u8,
        retry_policy: crate::AmpduRetryPolicy,
    ) -> Result<Self, AmpduRetryError> {
        if retry_policy.lifetime.as_micros() == 0 {
            return Err(AmpduRetryError::ZeroLifetime);
        }
        if CAPACITY > AMPDU_SUBFRAME_MASK_BITS {
            return Err(AmpduRetryError::CapacityExceedsHardwareWindow { capacity: CAPACITY });
        }
        if subframes == 0 {
            return Err(AmpduRetryError::EmptyAggregate);
        }
        if usize::from(subframes) > CAPACITY {
            return Err(AmpduRetryError::AggregateExceedsCapacity {
                subframes,
                capacity: CAPACITY,
            });
        }
        UpperAmpduRetryState::new(first_sequence, subframes, retry_policy)
            .map(|state| Self { state })
            .map_err(Self::error)
    }

    /// Apply one completion after the hardware queue has been detached.
    ///
    /// A missing MPDU stays in the aggregate until it is aged at monotonic
    /// `now`, `admitted` giving its MSDU's admission; aged MPDUs are
    /// discarded, behind a BlockAckReq under the agreement. Once
    /// the TID's BlockAck agreement is no longer `block_ack_operational`,
    /// the live missing MPDUs leave the aggregate for individual retry.
    ///
    /// SOURCE: complete `libpp[lmac.o]::lmacProcessTxComplete` maps status
    /// five to `libpp[lmac.o]::lmacProcessAckTimeout`. Both its short- and long-frame
    /// leaves call `lmacProcessTBSuccess(queue, 0x7f)` instead of retrying
    /// when the queue is in Trigger flow and its applicable packet counts
    /// are zero (`BackendAmpduCompletion::trigger_flow_end`),
    /// which ends the exchange without an ordinary BlockAck. A CTS timeout is
    /// a failed protection exchange (`libpp[lmac.o]::lmacProcessCtsTimeout`). Without the
    /// hardware's BlockAck-received result every MPDU is retried; with it,
    /// the supplied valid BlockAck bitmap resorts the
    /// aggregate.
    pub fn observe(
        &mut self,
        completion: impl Into<BackendAmpduCompletion>,
        observed_subframes: u8,
        now: oer_time::Instant,
        block_ack_operational: bool,
        admitted: impl Fn(u8) -> Option<oer_time::Instant>,
    ) -> Result<AmpduRetryDecision, AmpduRetryError> {
        let completion = completion.into();
        let result = if completion.trigger_flow_end {
            AmpduAttemptResult::TriggerFlowEnd
        } else if completion.status == TxStatus::CtsTimeout {
            AmpduAttemptResult::ProtectionFailure
        } else if !completion.block_ack_received {
            AmpduAttemptResult::NoResponse
        } else {
            AmpduAttemptResult::Answered(completion.block_ack)
        };
        self.state
            .observe(
                result,
                observed_subframes,
                now,
                block_ack_operational,
                admitted,
            )
            .map(Self::decision)
            .map_err(Self::error)
    }

    /// Resort the kept aggregate by the answer to its BlockAckReq.
    ///
    /// `block_ack` is the BlockAck received for the request, or `None` when
    /// the request exhausted its retries unanswered: the resort then keeps
    /// every MPDU as missing, as the vendor resorts its unchanged queue
    /// record.
    pub fn observe_block_ack_request(
        &mut self,
        block_ack: Option<oer_ieee80211_port::BlockAckReport>,
        now: oer_time::Instant,
        block_ack_operational: bool,
        admitted: impl Fn(u8) -> Option<oer_time::Instant>,
    ) -> Result<AmpduRetryDecision, AmpduRetryError> {
        self.state
            .observe_block_ack_request(block_ack, now, block_ack_operational, admitted)
            .map(Self::decision)
            .map_err(Self::error)
    }

    const fn decision(decision: UpperAmpduRetryDecision) -> AmpduRetryDecision {
        // Construction bounds the aggregate by `AMPDU_SUBFRAME_MASK_BITS`,
        // so every result fits the port's subframe mask.
        match decision {
            UpperAmpduRetryDecision::RetainAggregate { retry_mask } => {
                AmpduRetryDecision::RetainAggregate {
                    retry_mask: retry_mask as u32,
                }
            }
            UpperAmpduRetryDecision::RepublishUnchanged { retry_mask } => {
                AmpduRetryDecision::RepublishUnchanged {
                    retry_mask: retry_mask as u32,
                }
            }
            UpperAmpduRetryDecision::RequestBlockAck {
                retry_mask,
                starting_sequence,
            } => AmpduRetryDecision::RequestBlockAck {
                retry_mask: retry_mask as u32,
                starting_sequence,
            },
            UpperAmpduRetryDecision::DiscardAged {
                retry_mask,
                aged_mask,
                starting_sequence,
            } => AmpduRetryDecision::DiscardAged {
                retry_mask: retry_mask as u32,
                aged_mask: aged_mask as u32,
                starting_sequence,
            },
            UpperAmpduRetryDecision::Unaggregate { retry_mask } => {
                AmpduRetryDecision::Unaggregate {
                    retry_mask: retry_mask as u32,
                }
            }
            UpperAmpduRetryDecision::Finish { retry_mask } => AmpduRetryDecision::Finish {
                retry_mask: retry_mask as u32,
            },
            UpperAmpduRetryDecision::FinishTriggerFlow => AmpduRetryDecision::FinishTriggerFlow,
        }
    }

    const fn error(error: UpperAmpduRetryError) -> AmpduRetryError {
        match error {
            UpperAmpduRetryError::UnknownSubframe { index } => {
                AmpduRetryError::UnknownSubframe { index }
            }
            UpperAmpduRetryError::ZeroLifetime => AmpduRetryError::ZeroLifetime,
            UpperAmpduRetryError::EmptyAggregate => AmpduRetryError::EmptyAggregate,
            UpperAmpduRetryError::TooManySubframes { subframes } => {
                AmpduRetryError::AggregateExceedsCapacity {
                    subframes,
                    capacity: CAPACITY,
                }
            }
            UpperAmpduRetryError::FrameCountChanged { expected, observed } => {
                AmpduRetryError::FrameCountChanged { expected, observed }
            }
        }
    }

    pub const fn current_subframes(&self) -> u8 {
        self.state.current_subframes()
    }

    /// Whether an MSDU admitted at `admitted` is aged at `now`: less than
    /// one lifetime unit remains.
    pub fn aged(&self, admitted: oer_time::Instant, now: oer_time::Instant) -> bool {
        self.state.aged(admitted, now)
    }

    /// Where the BlockAckReq of the last `RequestBlockAck` or
    /// `DiscardAged` decision starts.
    pub const fn block_ack_request_start(&self) -> SequenceNumber {
        self.state.block_ack_request_start()
    }

    /// Original aggregate positions discarded as aged.
    pub const fn discarded_original_indices(&self) -> u32 {
        self.state.discarded_original_indices() as u32
    }

    pub const fn current_first_sequence(&self) -> SequenceNumber {
        self.state.current_first_sequence()
    }

    /// Original aggregate positions (bit `i` is the `i`th MPDU of the first
    /// publication) absent from the last observed completion. Retries
    /// compact the descriptor chain; these positions do not move.
    pub const fn missing_original_indices(&self) -> u32 {
        self.state.missing_original_indices() as u32
    }

    pub const fn acknowledged(&self) -> u8 {
        self.state.acknowledged()
    }

    /// Protection exchanges (RTS/CTS) that failed before the data PPDU.
    pub const fn protection_failures(&self) -> u8 {
        self.state.protection_failures()
    }
}

impl<const CAPACITY: usize> BackendAmpduRetry for BoundedAmpduRetryState<CAPACITY> {
    fn current_first_sequence(&self) -> SequenceNumber {
        self.current_first_sequence()
    }
    fn observe(
        &mut self,
        completion: BackendAmpduCompletion,
        subframes: u8,
        now: oer_time::Instant,
        operational: bool,
        admitted: impl Fn(u8) -> Option<oer_time::Instant>,
    ) -> Result<AmpduRetryDecision, AmpduRetryError> {
        self.observe(completion, subframes, now, operational, admitted)
    }
}
