//! Station connected HT/HE TX owner for the production Wi-Fi runner.
//!
//! The owner retains every physical frame owner referenced by DMA until
//! completion, queue detach, BlockAck processing and any retained aggregate
//! retry. It shares the ordinary descriptor, key token, sequence spaces,
//! contention state, power profile and clock through
//! [`SingleMpduTx`]; no parallel HIL TX state exists.
//!
//! This module is intentionally station-specific: HE/A-MSDU policy, station
//! rate control and individual-retry fallback do not belong to the AP owner.
//! Role-neutral completion, retry and retained-DMA mechanics live below this
//! adapter in the MAC and common ESP32-S31 Wi-Fi crates.

#[cfg(not(any(feature = "diagnostics", test)))]
use core::marker::PhantomData;
use oer_ieee80211_mac::sequence::SequenceNumber;

#[cfg(test)]
#[cfg(all(test, feature = "owned-network"))]
use oer_esp32s31_ieee80211_mac::irq::EVENT_TX_COMPLETE;

use core::{
    future::Future,
    mem,
    ops::{Deref, DerefMut},
};

use crate::datapath::{
    MaterializedTxFrame, SelectedBurstMaterializer, SoftwareTxFrame, TxBatchDemand,
};

use oer_esp32s31_hal::types::MacInterface;

use oer_esp32s31_ieee80211::{
    ampdu_tx::{
        AmpduTxRoleAdapter, HtAmpduPublicationInputs, HtAmpduTxRolePolicy,
        HtAmpduTxRolePolicyError, ht_ampdu_publication_config,
    },
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile},
};

use oer_esp32s31_ieee80211_mac::{
    rate::{
        control::{StaRateControlTx, StaTxRatePolicy},
        schedule::schedule_publication_limit,
    },
    tx::{
        AmpduTxConfig, HeAmpduTxConfig, HeEdcaTxopLimit, HeTriggerBasedTxConfig, LegacyTxQueue,
        TxCookie, TxPhyRate, TxSlotState,
        ampdu::{
            AmpduFrameLayout, AmpduFrameSize, AmpduRepublication, HeAmpduFrameRequest,
            HeAmpduPolicy, HtAmpduFrameRequest, HtAmpduHardware, HtAmpduTxError,
            RetainedAmpduRetryCompletionError, RetainedDmaAmpduTx,
        },
        protection::{ProtectedPpdu, TxPsdu, TxReceiver},
        runtime::{
            AmpduRetryDecision, AmpduRetryError, AmpduRetryPolicy, AmpduRetryState,
            VENDOR_AMPDU_MSDU_LIFETIME_MICROS, WifiTxTraffic, WifiTxTrafficError,
            WmmTxopUnsupported,
        },
    },
};
use oer_espressif_ieee80211_policy::rate_control::{
    AmpduRateObservationError, StaRateControlAssociation,
};

use oer_esp32s31_ieee80211_sta::single_mpdu_tx::{
    ActionTxConfig, ConnectedTxHandoff, SingleMpduTx, SingleMpduTxError, SingleMpduTxOutcome,
    SingleMpduTxParked, StaDataTxSelection, WifiTxResources,
};

use oer_ieee80211_mac::{
    data::DataHeControl,
    station::{
        STA_PROTECTED_QOS_ETHERNET_HEADROOM, STA_PROTECTED_QOS_ETHERNET_OVERHEAD,
        StaTxSequenceCounters, StationFrameError, sta_protected_amsdu_pair_frame_length,
    },
    station_power_save::StaPowerManagement,
};

use oer_ieee80211_datapath::PhysicalTxSource;

use oer_ieee80211_softmac::{
    MacAmpduTxResult, MacAmpduTxStatus, MacIndividualRetries, MacTxQueueState, MacTxResult,
};

#[cfg(any(feature = "diagnostics", test))]
use crate::diagnostics::aggregate_tx::{
    AggregateTxObservation, AggregateTxObserver, PreparedTxSchedulerPhase,
};

use crate::{
    datapath::{
        DatapathControlProgress, WifiTxProgress, WifiTxWake,
        services::DatapathNetworkTxService,
        tx::resources::{AggregateTxArenaPair, AggregateTxResources},
    },
    diagnostics::aggregate_tx::{AggregateBuildStop, NetworkSingleMpduReason},
    roles::station::control::{
        ConnectedControlTx, ConnectedHeControlRuntimeRejection, HeNdpaRuntimeRequest,
        HeTriggerRuntimeRequest,
    },
};

use oer_esp32s31_ieee80211_sta::connected_control::{
    ConnectedDisconnectReason, NetworkTxPowerReport,
};

const AMPDU_ABORT_SETTLE: oer_time::Duration = oer_time::Duration::from_micros(16);
const HE_TRIGGER_DATA_TID: u8 = 0;

/// Control-plane notification for the current station TX BlockAck state.
///
/// Unlike [`crate::diagnostics::aggregate_tx::AggregateTxObserver`], this is application-visible link state,
/// not diagnostic telemetry. It runs only when a negotiated agreement changes
/// state and therefore never observes aggregate publication or completion.
pub type StationTxBlockAckStatusSink = fn(tid: u8, operational: bool);

/// Selected-request admission preserves the request until a physical frame
/// transfers to the STA owner. Radio errors follow that ownership transfer.
#[derive(Debug, Eq, PartialEq)]
pub enum RequestTxError<Request> {
    /// A live or prepared transaction prevents admission; the source was not
    /// asked to construct a frame.
    Busy(Request),
    /// The source could not construct the first frame and returned the request.
    Unmaterialized(Request),
    /// Admission transferred a physical frame to the ordinary STA TX path.
    Radio(AggregateTxError),
}

/// Peer-independent TX resources returned at the connected-to-disconnected
/// station boundary.
pub struct ConnectedTxTeardownParts<R, A> {
    pub resources: R,
    pub security: oer_esp32s31_ieee80211_sta::single_mpdu_tx::ConnectedTxSecurity,
    pub sequences: StaTxSequenceCounters,
    pub aggregate: A,
}

/// Station-local TX policy retained while the one physical ordinary/A-MPDU
/// owner is lent to the AP role.
///
/// This value contains no descriptor, DMA backing or hardware publication
/// capability. Resuming it requires both exact physical owners returned by
/// the other role.
pub struct ConnectedTxParked<'observer, const SLOTS: usize> {
    ordinary: SingleMpduTxParked,
    block_ack_windows: [u8; 8],
    block_ack_generations: [u32; 8],
    block_ack_generation_exhausted: u8,
    config: AggregateTxConfig,
    rate_control: StaRateControlAssociation,
    data_rate_policy: StaTxRatePolicy,
    aggregate_rate_policy: StaTxRatePolicy,
    he_trigger_based: Option<HeTriggerBasedTxConfig>,
    last_aggregate_status: Option<MacAmpduTxStatus<TxPhyRate>>,
    network_power: NetworkTxPowerReport,
    #[cfg(any(feature = "diagnostics", test))]
    observer: Option<&'observer dyn crate::diagnostics::aggregate_tx::AggregateTxObserver>,
    #[cfg(not(any(feature = "diagnostics", test)))]
    observer_lifetime: PhantomData<&'observer ()>,
    block_ack_status_sink: Option<StationTxBlockAckStatusSink>,
}

/// Finite aggregate publication policy installed after Association.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AggregateTxConfig {
    /// Independent A-MPDU schedule rate selected by rate control.
    pub rate: TxPhyRate,
    /// Maximum descriptors claimed for one aggregate.
    pub frame_limit: u8,
    /// Executor watchdog for each hardware publication.
    pub completion_timeout: oer_time::Duration,
    /// HE duration/APEP ceiling selected from negotiated EDCA policy.
    pub he_txop_limit: HeEdcaTxopLimit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AggregateTxResetReason {
    CompletionInterruptWithoutState,
    TimeoutInterruptWithoutState,
    CollisionInterruptWithoutState,
    ConflictingInterruptEvents(u32),
    ExecutorDeadline,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AggregateTxError {
    /// A new network publication was requested while the shared TX owner
    /// still retained an ordinary or aggregate transaction.
    ActiveTransaction,
    /// TX service was entered without an ordinary or aggregate transaction.
    InactiveTransaction,
    /// The aggregate state lost the cookie that proves ownership of the
    /// reserved DMA arena.
    MissingCookie,
    /// A prepared aggregate did not reach the publication state expected by
    /// the initial submit edge.
    InvalidPublicationState,
    PeerDoesNotSupportQos,
    UnsupportedRate,
    InvalidFrameLimit {
        limit: u8,
        capacity: usize,
    },
    MissingQosSequence(u8),
    BufferSizeOverflow,
    DeadlineOverflow,
    RadioClockUnavailable,
    DmaPrefixGeometry {
        encoded_offset: usize,
        metadata_size: usize,
    },
    /// Control replaced or stopped the TX BlockAck agreement after a
    /// software-owned aggregate consumed sequence numbers and PNs but before
    /// that aggregate reached hardware.
    BlockAckAgreementChanged {
        tid: u8,
    },
    Encode(StationFrameError),
    Aggregate(HtAmpduTxError),
    Retry(AmpduRetryError),
    Traffic(WifiTxTrafficError),
    Unsupported(WmmTxopUnsupported),
    RolePolicy(HtAmpduTxRolePolicyError),
    Ordinary(SingleMpduTxError),
    /// An aggregate detached one MPDU into the ordinary owner, but that owner
    /// completed without publishing its normalized terminal status.
    MissingOrdinaryRetryStatus,
    RadioResetRequired(AggregateTxResetReason),
}

impl From<HtAmpduTxError> for AggregateTxError {
    fn from(error: HtAmpduTxError) -> Self {
        Self::Aggregate(error)
    }
}

impl From<AmpduRetryError> for AggregateTxError {
    fn from(error: AmpduRetryError) -> Self {
        Self::Retry(error)
    }
}

impl From<RetainedAmpduRetryCompletionError> for AggregateTxError {
    fn from(error: RetainedAmpduRetryCompletionError) -> Self {
        match error {
            RetainedAmpduRetryCompletionError::Hardware(error) => Self::Aggregate(error),
            RetainedAmpduRetryCompletionError::Retry(error) => Self::Retry(error),
        }
    }
}

impl From<HtAmpduTxRolePolicyError> for AggregateTxError {
    fn from(error: HtAmpduTxRolePolicyError) -> Self {
        Self::RolePolicy(error)
    }
}

impl From<SingleMpduTxError> for AggregateTxError {
    fn from(error: SingleMpduTxError) -> Self {
        Self::Ordinary(error)
    }
}

impl From<WifiTxTrafficError> for AggregateTxError {
    fn from(error: WifiTxTrafficError) -> Self {
        Self::Traffic(error)
    }
}

impl From<WmmTxopUnsupported> for AggregateTxError {
    fn from(error: WmmTxopUnsupported) -> Self {
        Self::Unsupported(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AggregateTraffic {
    selected: WifiTxTraffic,
    he_txop_limit: HeEdcaTxopLimit,
}

impl AggregateTraffic {
    const fn tid(self) -> u8 {
        self.selected.tid()
    }

    const fn queue(self) -> LegacyTxQueue {
        self.selected.queue()
    }
}

struct AggregateActive<const SLOTS: usize> {
    traffic: AggregateTraffic,
    /// Agreement the aggregate was built for; its retries stay aggregated
    /// only while this exact agreement is operational.
    block_ack_generation: u32,
    config: AmpduTxConfig,
    retry: AmpduRetryState<SLOTS>,
    original_subframes: u8,
    /// Next deadline for the enclosing published/abort-settling phase.
    deadline: oer_time::Instant,
    #[cfg(any(feature = "diagnostics", test))]
    first_publication_micros: Option<u64>,
    #[cfg(feature = "tx-wait-probe")]
    wait_probe: wait_probe::Schedule,
    #[cfg(feature = "tx-wait-probe")]
    first_sequence: SequenceNumber,
}

#[cfg(any(feature = "tx-wait-probe", test))]
mod wait_probe;

struct AggregatePrepared<const SLOTS: usize> {
    traffic: AggregateTraffic,
    block_ack_generation: u32,
    aggregate_length: u16,
    retry: AmpduRetryState<SLOTS>,
    original_subframes: u8,
    first_sequence: SequenceNumber,
    build_stop: AggregateBuildStop,
    #[cfg(any(feature = "diagnostics", test))]
    preparation_micros: u64,
}

enum ConnectedTxActive<const SLOTS: usize> {
    Idle,
    Ordinary,
    Aggregate(AggregateActive<SLOTS>),
    AbortSettling(AggregateActive<SLOTS>),
    /// The retained aggregate's missing MPDUs leave it one ordinary
    /// transmission at a time.
    Unaggregating(Unaggregating<SLOTS>),
    /// The retained aggregate waits for the BlockAck its BlockAckReq
    /// solicits from the ordinary owner.
    RequestingBlockAck(AggregateActive<SLOTS>),
}

/// Ordinary retries of MPDUs taken out of a completed aggregate. The
/// aggregate storage stays retained until the last missing MPDU has been
/// copied into the ordinary buffer.
struct Unaggregating<const SLOTS: usize> {
    aggregate: AggregateActive<SLOTS>,
    /// Retained-aggregate subframe indices not yet copied out.
    remaining: u32,
    /// Terminal aggregate status, accumulating the individual retries.
    status: MacAmpduTxStatus<TxPhyRate>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AggregateFrameAdmission {
    /// The exact frame geometry was checked before the aggregate reservation.
    FreshExact,
    /// HT admission used `B::MAX_ETHERNET_LENGTH`, which is an upper bound for every
    /// frame obtainable from this typed network queue.
    HtQueueCapacity,
    /// HE delimiter policy depends on the actual encoded length, so the
    /// dequeued frame still needs an exact check before consuming PN/sequence.
    NeedsExactCheck,
}

struct TeardownResource<T>(Option<T>);

impl<T> TeardownResource<T> {
    fn new(resource: T) -> Self {
        Self(Some(resource))
    }

    fn take(&mut self) -> T {
        self.0
            .take()
            .expect("connected TX resource exists until successful teardown")
    }

    fn is_present(&self) -> bool {
        self.0.is_some()
    }
}

impl<T> Deref for TeardownResource<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0
            .as_ref()
            .expect("connected TX resource is present while the owner is usable")
    }
}

impl<T> DerefMut for TeardownResource<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0
            .as_mut()
            .expect("connected TX resource is present while the owner is usable")
    }
}

/// Unique connected TX owner for ordinary frames and referenced A-MPDU.
pub struct ConnectedTx<
    'slot,
    'ampdu,
    B: MaterializedTxFrame,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> where
    B: 'ampdu,
{
    ordinary: TeardownResource<SingleMpduTx<'slot, P, E, T, ORDINARY_BUFFER_SIZE>>,
    ampdu: TeardownResource<
        AggregateTxArenaPair<RetainedDmaAmpduTx<'ampdu, B, SLOTS, AMPDU_BUFFER_SIZE>>,
    >,
    cookie: Option<TxCookie>,
    standby_cookie: Option<TxCookie>,
    standby_prepared: Option<AggregatePrepared<SLOTS>>,
    standby_error: Option<AggregateTxError>,
    /// One FIFO successor retained when its WMM TID differs from the current
    /// aggregate. It remains a network lease and is published before another
    /// queue entry can overtake it.
    deferred_network: Option<B>,
    /// Peer-negotiated BlockAck window per QoS TID; zero means inactive.
    block_ack_windows: [u8; 8],
    /// Generation of each agreement retained by software-prepared A-MPDUs.
    /// A DELBA/re-negotiation cannot therefore leave an old arena eligible
    /// merely because a later agreement happens to have the same window.
    block_ack_generations: [u32; 8],
    /// Sticky per-TID exhaustion for the monotonic agreement generation.
    /// Once set, no later agreement in this connected epoch may admit an
    /// aggregate whose prepared identity could alias an earlier generation.
    block_ack_generation_exhausted: u8,
    config: AggregateTxConfig,
    rate_control: TeardownResource<StaRateControlAssociation>,
    data_rate_policy: StaTxRatePolicy,
    aggregate_rate_policy: StaTxRatePolicy,
    he_trigger_based: Option<HeTriggerBasedTxConfig>,
    active: ConnectedTxActive<SLOTS>,
    last_aggregate_status: Option<MacAmpduTxStatus<TxPhyRate>>,
    /// The ordinary transaction in flight carries network data.
    network_ordinary: bool,
    /// Network data TX not yet read by power management.
    network_power: NetworkTxPowerReport,
    #[cfg(any(feature = "diagnostics", test))]
    observer: Option<&'ampdu dyn crate::diagnostics::aggregate_tx::AggregateTxObserver>,
    block_ack_status_sink: Option<StationTxBlockAckStatusSink>,
}

mod adapters;
mod completion;
mod owner;
mod publication;
mod resources;

use crate::datapath::tx::aggregate::AggregateTxServiceEvent;

#[cfg(any(test, all(feature = "test-support", not(target_pointer_width = "32"))))]
pub mod test_support;
#[cfg(all(test, feature = "owned-network"))]
mod tests;
