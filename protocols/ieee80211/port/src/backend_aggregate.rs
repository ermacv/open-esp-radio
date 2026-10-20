//! Bounded aggregate retry commands supplied by a core.
use oer_ieee80211_wire::sequence::SequenceNumber;
/// Policy for retained A-MPDU retries.
///
/// As the vendor's `ppResortTxAMPDU` does, a partial BlockAck keeps the
/// missing MPDUs in the aggregate without counting publications; only their
/// MSDU lifetime bounds the retries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduRetryPolicy {
    /// Time from an MSDU's admission after which its MPDU is aged and
    /// discarded instead of retried.
    pub lifetime: oer_time::Duration,
    /// Keep one missing MPDU in the aggregate owner.
    ///
    /// The recovered HE path requires this because converting a one-member
    /// HE A-MPDU to the ordinary queue first needs the distinct
    /// `ppHEAMPDU2Normal` metadata transition. The qualified HT path instead
    /// sends one remaining MPDU through its ordinary retry owner.
    pub retain_single_mpdu: bool,
}

/// Invalid construction or a disagreement with the pinned DMA owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmpduRetryError {
    /// An MSDU cannot be retried with a zero lifetime.
    ZeroLifetime,
    /// The caller has no admission instant for this original MPDU.
    UnknownSubframe {
        /// Original subframe index without an admission instant.
        index: u8,
    },
    /// An aggregate must contain at least one MPDU.
    EmptyAggregate,
    /// The bounded owner exceeds the hardware report window.
    CapacityExceedsHardwareWindow {
        /// Configured maximum number of subframes.
        capacity: usize,
    },
    /// The requested aggregate exceeds the configured bound.
    AggregateExceedsCapacity {
        /// Number of MPDUs supplied by the caller.
        subframes: u8,
        /// Configured maximum number of subframes.
        capacity: usize,
    },
    /// Completion disagrees with the retained aggregate.
    FrameCountChanged {
        /// Number of retained MPDUs.
        expected: u8,
        /// Number of MPDUs reported by the backend.
        observed: u8,
    },
}

/// Driver-owned action after one BlockAck completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmpduRetryDecision {
    /// Retain and compact the selected MPDUs, then publish another A-MPDU.
    RetainAggregate {
        #[doc = "Original MPDU positions that remain unacknowledged."]
        retry_mask: u32,
    },
    /// The protection exchange failed before the data PPDU: publish the same
    /// aggregate again. No MPDU was transmitted, so none gains the Retry bit.
    ///
    /// SOURCE: complete `libpp[lmac.o]::lmacProcessCtsTimeout` calls
    /// `libpp[lmac.o]::lmacProcessShortRetryFail(queue, 0, 1)`, which advances the short
    /// retry count and contention window, skips the Retry-bit leaf and
    /// re-enters `libpp[lmac.o]::lmacEndFrameExchangeSequence` with the unchanged frame.
    RepublishUnchanged {
        #[doc = "Original MPDU positions that remain unacknowledged."]
        retry_mask: u32,
    },
    /// The protection exchange failed at every attempt: keep the aggregate
    /// and send a BlockAckReq starting at its head. The BlockAck it solicits
    /// drives the resort (a later BlockAck report).
    ///
    /// SOURCE: `libpp[lmac.o]::lmacEndRetryAMPDUFail` keeps an
    /// RTS-protected aggregate whose CTS never arrived and sends a
    /// BlockAckReq through `libpp[pp.o]::ppFillAMPDUBar`/`libpp[pp.o]::ppReSendBar` with the starting
    /// sequence of its head.
    RequestBlockAck {
        /// Original MPDU positions that remain unacknowledged.
        retry_mask: u32,
        /// Sequence number at the compacted aggregate head.
        starting_sequence: SequenceNumber,
    },
    /// End aggregate ownership and send the selected MPDUs individually,
    /// in order, each already carrying its Retry bit.
    ///
    /// SOURCE: `libpp[pp.o]::ppResortTxAMPDU` moves the remaining MPDUs
    /// to the head of the queue's ordinary list when
    /// `libpp[trc.o]::trc_isTxAmpduOperational` or `libpp[trc.o]::trc_tid_isTxAmpduOperational` is
    /// false after its resort, first converting a missing aggregate head
    /// through `libpp[pp_he.o]::ppHEAMPDU2Normal`. A single missing HT
    /// MPDU takes the same ordinary path.
    Unaggregate {
        #[doc = "Original MPDU positions that remain unacknowledged."]
        retry_mask: u32,
    },
    /// Missing MPDUs aged under an operational Block Ack agreement: compact
    /// the aggregate to the selected MPDUs, discard those of `aged_mask` and
    /// send a BlockAckReq at `starting_sequence`, whose BlockAck drives the
    /// resort (a later BlockAck report). Every MPDU
    /// ages from its MSDU's admission, not from the aggregate's commit as in
    /// `libpp[lmac.o]::lmacMSDUAged` (`docs/decisions.md`, MSDU age).
    DiscardAged {
        /// Original MPDU positions that remain unacknowledged.
        retry_mask: u32,
        /// Original MPDU positions whose MSDUs expired.
        aged_mask: u32,
        /// Sequence number at the compacted aggregate head.
        starting_sequence: SequenceNumber,
    },
    /// End aggregate ownership. The selected MPDUs were not acknowledged
    /// and are discarded.
    Finish {
        #[doc = "Original MPDU positions that remain unacknowledged."]
        retry_mask: u32,
    },
    /// End ownership through the vendor Trigger-based completion path.
    ///
    /// No ordinary BlockAck was received, so this must remain distinct from
    /// `Finish { retry_mask: 0 }` for statistics and rate-control purposes.
    FinishTriggerFlow,
}

impl AmpduRetryDecision {
    /// Original MPDU positions that remain unacknowledged.
    pub const fn retry_mask(self) -> u32 {
        match self {
            Self::RetainAggregate { retry_mask }
            | Self::RepublishUnchanged { retry_mask }
            | Self::RequestBlockAck { retry_mask, .. }
            | Self::DiscardAged { retry_mask, .. }
            | Self::Unaggregate { retry_mask }
            | Self::Finish { retry_mask } => retry_mask,
            Self::FinishTriggerFlow => 0,
        }
    }

    /// Number of unacknowledged original MPDUs.
    pub const fn missing(self) -> u8 {
        self.retry_mask().count_ones() as u8
    }
}

/// Facts of one completed hardware aggregate publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BackendAmpduCompletion {
    /// Detached hardware completion status.
    pub status: crate::TxStatus,
    /// Completion ended a Trigger exchange without ordinary BlockAck.
    pub trigger_flow_end: bool,
    /// The hardware observed a BlockAck response.
    pub block_ack_received: bool,
    /// Decoded valid BlockAck report, when available.
    pub block_ack: Option<crate::BlockAckReport>,
}
/// Core-owned aggregate retry observation, called after DMA detachment.
pub trait BackendAmpduRetry {
    /// Sequence number at the current retained aggregate head.
    fn current_first_sequence(&self) -> SequenceNumber;
    /// Apply detached completion facts and each MSDU's admission instant.
    fn observe(
        &mut self,
        completion: BackendAmpduCompletion,
        subframes: u8,
        now: oer_time::Instant,
        operational: bool,
        admitted: impl Fn(u8) -> Option<oer_time::Instant>,
    ) -> Result<AmpduRetryDecision, AmpduRetryError>;
}
