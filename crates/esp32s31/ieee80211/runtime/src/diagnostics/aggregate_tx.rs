//! Optional observation boundary for connected aggregate TX.
//!
//! The driver publishes typed, value-only events. It does not prescribe
//! counters, histograms, storage or report formatting. Attaching an observer
//! must not affect retry, queue, DMA ownership or scheduling decisions.

use oer_ieee80211_mac::sequence::SequenceNumber;

pub use oer_esp32s31_ieee80211::ordinary_tx::{
    OrdinaryTxOutcome, OrdinaryTxReport, OrdinaryTxRetryReport,
};
pub use oer_ieee80211_softmac::MacTxWork;

/// Why a network frame used the ordinary MPDU path instead of starting an
/// aggregate exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkSingleMpduReason {
    LegacyRate,
    BlockAckUnavailable,
    HtNeedsPair,
    FreshAggregateCapacity,
}

/// The first resource boundary reached while building one aggregate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AggregateBuildStop {
    FrameLimit,
    CapacityLimit,
    QueueEmpty,
}

/// Diagnostic boundary crossed while a completed network transaction returns
/// to the scheduler and its already-prepared successor becomes publishable.
///
/// The generic DATAPATH runner owns these boundaries. Role implementations
/// only retain their timestamps when an observer is attached; ordinary builds
/// do not read a clock or emit an event for them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreparedTxSchedulerPhase {
    /// The terminal TX service future returned to `drive_active_tx`.
    ActiveServiceReturned,
    /// The outer scheduler loop resumed after the active TX driver returned.
    SchedulerLoopResumed,
    /// The ordered non-blocking stop probe completed.
    StopPollCompleted,
    /// Stale TX wakes were drained and O(1) control readiness was sampled.
    ControlReadinessChecked { ready: bool },
    /// Prepared ownership and competing RX readiness were sampled.
    PreparedReadinessChecked,
    /// The retained frame count and negotiated batch target were sampled.
    PreparedBatchChecked,
    /// The scheduler committed to the already-prepared publication path.
    PreparedEntry,
}

/// Value-only observations emitted by the production aggregate TX owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AggregateTxObservation {
    /// Ordinary TX serviced through connected STA or AP network data, including
    /// aggregate fallback. Separate from aggregate receipts: never double count.
    OrdinaryWorkCompleted {
        work: MacTxWork,
    },
    /// One negotiated TX BlockAck agreement changed its operational state.
    /// This is a protocol-control edge, not evidence that an aggregate has
    /// already been published.
    BlockAckOperational {
        tid: u8,
        operational: bool,
    },
    /// A coalesced hardware TX interrupt has reached the connected TX owner.
    ///
    /// This timestamp is emitted only when an observer is attached. It lets a
    /// HIL observer correlate the hard-ISR publication with bottom-half
    /// service without placing timing policy in the production owner.
    InterruptServiceStarted {
        at_micros: u64,
    },
    NetworkSingleMpdu {
        reason: NetworkSingleMpduReason,
        ethernet_length: usize,
    },
    /// Actual PHY vector bound to a prepared aggregate. This observation is
    /// role-neutral: AP qualification must not depend on the associated-STA
    /// link snapshot to describe its own transmission.
    RateSelected {
        bandwidth_mhz: u16,
        nominal_kbps: u32,
    },
    Prepared {
        subframes: u8,
        stop: AggregateBuildStop,
    },
    PreparationCompleted {
        micros: u64,
    },
    StandbyPrepared,
    StandbyPublished,
    StandbyCancelled,
    /// One timestamped phase of an already-prepared DATAPATH publication.
    ///
    /// The observer owns trace assembly and storage. The driver never retains
    /// diagnostic state outside its ordinary role owner graph.
    PreparedSchedulerPhase {
        phase: PreparedTxSchedulerPhase,
        at_micros: u64,
    },
    Published {
        /// Clock image captured immediately before queue programming began.
        at_micros: u64,
        program_micros: u64,
    },
    /// The BlockAckReq of an aggregate whose protection exchange failed at
    /// every attempt completed; `answered` when a BlockAck arrived.
    BlockAckRequestCompleted {
        answered: bool,
    },
    /// One detached hardware A-MPDU completion after BlockAck classification.
    BlockAckProcessed {
        tx_status: u8,
        block_ack_received: bool,
        control: u8,
        first_sequence: SequenceNumber,
        starting_sequence: SequenceNumber,
        subframes: u8,
        /// Original aggregate positions absent from this completion; see
        /// `AmpduRetryState::missing_original_indices`.
        missing_original_indices: u32,
        /// Signed SNR of the response that completed this publication, when
        /// the completion status is successful.
        block_ack_snr_db: Option<i8>,
    },
    /// Time spent sampling terminal hardware state, classifying BlockAck and
    /// detaching the completed queue, before releasing retained network
    /// leases. This exists only on diagnostic builds.
    CompletionCoreCompleted {
        micros: u64,
    },
    /// Time spent clearing the published descriptor prefix and returning the
    /// retained network leases after the terminal completion was classified.
    BackingReleaseCompleted {
        micros: u64,
    },
    Completed {
        acknowledged: u8,
        individual_retry: bool,
    },
    HardwareTimeout,
    Collision,
    /// Terminal aggregate publication work, excluding an ordinary fallback.
    /// Emitted once per completed/aborted exchange, never once per retry.
    WorkCompleted {
        work: MacTxWork,
    },
    ExchangeCompleted {
        micros: u64,
        /// Number of hardware aggregate publications required to reach the
        /// terminal result, including the initial publication.
        publications: u8,
    },
}

/// Non-owning diagnostics hook for aggregate TX.
///
/// Implementations must be non-blocking and thread-safe. The production
/// owner emits no events and performs no observation-only clock reads when no
/// observer is attached.
pub trait AggregateTxObserver: Sync {
    /// Terminal ordinary-MPDU outcome from the connected STA owner, including
    /// aggregate fallback. Missing evidence remains explicit. This is not an
    /// observation of AP, management, or autonomous hardware response traffic.
    fn observe_station_ordinary(&self, _outcome: Option<OrdinaryTxOutcome>) {}

    /// Observer-owned clock. The production path performs no diagnostic clock
    /// read when no observer is attached.
    fn now_micros(&self) -> u64;

    fn observe(&self, observation: AggregateTxObservation);

    /// Final station aggregate result, including any detached ordinary retry.
    /// Emitted once after safe terminal release, never for an intermediate BA.
    /// Reset-required/quarantined owners have no terminal receipt.
    fn observe_station_terminal(
        &self,
        _status: oer_ieee80211_softmac::MacAmpduTxStatus<oer_esp32s31_ieee80211_mac::tx::TxPhyRate>,
    ) {
    }

    /// Read-only queue evidence from the explicitly intrusive `tx-wait-probe`
    /// feature. Unlike attaching an observer, enabling that feature adds
    /// observation deadlines. The normal hardware timeout remains unchanged.
    fn observe_wait_probe(&self, _sample: TxWaitSample) {}

    /// Observe one AP Ethernet lease exactly when the radio claims it from
    /// the network frontier, before encoding mutates its backing.
    ///
    /// The borrowed bytes are diagnostic input only and must not be retained.
    /// The default keeps ordinary production observers value-only.
    fn observe_access_point_network_claim(&self, _ethernet: &[u8]) {}

    /// A previously admitted software owner was dropped by bounded AP
    /// retention. This is a loss, not a producer capacity refusal or TX ACK.
    fn observe_access_point_retention_drop(&self, _reason: NetworkTxRetentionDropReason) {}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkTxRetentionDropReason {
    ActiveQueueFull,
    UnicastPowerSaveFull,
    GroupPowerSaveFull,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxWaitSample {
    /// True when the owner was entered through its timer rather than an IRQ.
    pub deadline_wake: bool,
    pub at_micros: u64,
    pub elapsed_micros: u64,
    pub timer_lateness_micros: u64,
    /// Original aggregate sequence; retained retries may contain a subset.
    pub first_sequence: SequenceNumber,
    pub queue: u8,
    pub snapshot: Option<oer_esp32s31_hal::types::MacOrdinaryTxQueueSnapshot>,
}
