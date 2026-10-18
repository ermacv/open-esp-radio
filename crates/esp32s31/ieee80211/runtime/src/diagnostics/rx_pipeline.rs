//! Optional observation boundary for the connected RX pipeline.
//!
//! The driver publishes typed, value-only events. It does not prescribe
//! counters, sampling, storage, report formatting or a HIL clock. Attaching an
//! observer must not affect ownership, backpressure or scheduling decisions.

use oer_ieee80211_mac::sequence::SequenceNumber;

pub use oer_esp32s31_ieee80211::rx::transaction::{
    Discard as RxStageDiscard, ServiceObservation as RxServiceObservation,
};

/// Result of one bounded publication into the network adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxNetworkPublicationOutcome {
    Enqueued,
    /// The shared network packet pool could not provide an RX owner.
    PoolExhausted,
    Dropped,
}

/// Low-frequency BlockAck agreement facts required by correctness images.
///
/// This boundary is deliberately separate from [`RxPipelineObserver`]: an
/// agreement observer is called only on lifecycle edges and the first frame,
/// never for every DMA, protocol, or network phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxReorderAgreementObservation {
    Started {
        tid: u8,
        starting_sequence: SequenceNumber,
        window: u16,
    },
    Stopped,
    First {
        tid: u8,
        start: SequenceNumber,
        sequence: SequenceNumber,
    },
}

/// Non-owning correctness hook for negotiated RX BlockAck state.
pub trait RxReorderAgreementObserver: Sync {
    fn observe(&self, observation: RxReorderAgreementObservation);
}

/// Value-only observations emitted by the production RX owner graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxPipelineObservation {
    ServiceStarted {
        at_micros: u64,
    },
    ServiceCompleted(RxServiceObservation),
    /// One complete vendor-ordered append/reload transaction.
    ///
    /// The measurement covers the synchronous doorbell-clear loop and its
    /// immediate NEXT/LAST/conditional-BASE suffix. No observer means no
    /// clock reads on the production path.
    ReloadCompleted {
        micros: u64,
    },
    StageDiscarded(RxStageDiscard),
    NetworkReadyWait {
        micros: u64,
    },
    /// Time from ownership of one staged frame through reorder and protocol
    /// handling. Queue selection before the frame is acquired is excluded.
    ProtocolFrameCompleted {
        micros: u64,
    },
    /// Synchronous reorder-key lookup and BlockAck state transition before
    /// the frame enters ordinary MAC dispatch.
    ReorderPrepared {
        micros: u64,
    },
    /// Synchronous classification performed by the protocol owner before it
    /// waits for a network publication credit.
    ProtocolPreflightCompleted {
        micros: u64,
    },
    ProtocolDispatched {
        data: bool,
        amsdu: bool,
        amsdu_subframes: u8,
        unit_bytes: usize,
        micros: u64,
    },
    ReorderStarted {
        tid: u8,
        starting_sequence: SequenceNumber,
        window: u16,
    },
    ReorderStopped,
    ReorderFirst {
        tid: u8,
        start: SequenceNumber,
        sequence: SequenceNumber,
    },
    /// One addressed protected QoS MPDU before agreement/reorder handling.
    ReorderIngress {
        active: bool,
        retry: bool,
    },
    ReorderReleased {
        buffered: bool,
        released: u8,
        missing: u16,
        stale: bool,
    },
    ReorderGapExpired,
    ReorderDiscarded,
    ReorderOccupied {
        occupied: u32,
    },
    NetworkPublication {
        bytes: usize,
        micros: u64,
        outcome: RxNetworkPublicationOutcome,
    },
}

/// Non-owning diagnostics hook for the RX pipeline.
///
/// Implementations must be non-blocking and thread-safe. The timestamp source
/// is observer-owned so a firmware which attaches no observer performs no
/// diagnostic clock reads.
pub trait RxPipelineObserver: Sync {
    fn now_micros(&self) -> u64;

    fn observe(&self, observation: RxPipelineObservation);

    fn begin_service(&self) -> u64 {
        let at_micros = self.now_micros();
        self.observe(RxPipelineObservation::ServiceStarted { at_micros });
        at_micros
    }

    fn elapsed_micros_since(&self, started: u64) -> u64 {
        self.now_micros().wrapping_sub(started)
    }
}
