use super::*;

/// Named protocol resources supplied by the platform composition.
pub struct ConnectedStaRxProtocolResources<
    'queue,
    'pool,
    'scratch,
    'irq,
    M: RawMutex,
    S,
    K,
    const DEPTH: usize,
    const CAPACITY: usize,
    const SLOTS: usize,
    const REORDER_SLOTS: usize = RX_REORDER_BACKING_SLOT_COUNT,
> {
    pub frames: StagedRxReceiver<'queue, 'pool, M, DEPTH, CAPACITY, SLOTS>,
    pub irq: &'irq EmbassyMacIrqRuntime<M>,
    pub sink: S,
    pub mpdu: &'scratch mut [u8],
    pub ethernet: &'scratch mut [u8],
    pub reorder_commands: RxReorderCommandReceiver<'queue, M>,
    pub reorder_storage: &'pool RxReorderFrameStorage<CAPACITY, REORDER_SLOTS>,
    pub runtime: &'pool mut ConnectedReceiveStorage<'pool, CAPACITY, SLOTS, REORDER_SLOTS>,
    pub reorder_scratch: Option<&'scratch mut [u8]>,
    /// Monotonic clock of the reorder gap deadlines.
    pub clock: K,
    /// Optional observation-only counters used by qualification fixtures.
    #[cfg(any(feature = "diagnostics", test))]
    pub pipeline_observer: Option<&'queue dyn RxPipelineObserver>,
    #[cfg(any(feature = "diagnostics", test))]
    pub reorder_observer: Option<&'queue dyn RxReorderAgreementObserver>,
}

/// Queue-independent connected-station RX resources.
///
/// Standalone STA attaches its own staging receiver after this processor is
/// built. Same-channel STA+AP instead feeds already classified leases from
/// the sole paired queue, so no compatibility receiver exists in that graph.
pub struct ConnectedStaRxProcessorResources<
    'queue,
    'pool,
    'scratch,
    'irq,
    M: RawMutex,
    S,
    K,
    const CAPACITY: usize,
    const SLOTS: usize,
    const REORDER_SLOTS: usize = RX_REORDER_BACKING_SLOT_COUNT,
> {
    pub irq: &'irq EmbassyMacIrqRuntime<M>,
    pub sink: S,
    pub mpdu: &'scratch mut [u8],
    pub ethernet: &'scratch mut [u8],
    pub reorder_commands: RxReorderCommandReceiver<'queue, M>,
    pub reorder_storage: &'pool RxReorderFrameStorage<CAPACITY, REORDER_SLOTS>,
    pub runtime: &'pool mut ConnectedReceiveStorage<'pool, CAPACITY, SLOTS, REORDER_SLOTS>,
    pub reorder_scratch: Option<&'scratch mut [u8]>,
    /// Monotonic clock of the reorder gap deadlines.
    pub clock: K,
    #[cfg(any(feature = "diagnostics", test))]
    pub pipeline_observer: Option<&'queue dyn RxPipelineObserver>,
    #[cfg(any(feature = "diagnostics", test))]
    pub reorder_observer: Option<&'queue dyn RxReorderAgreementObserver>,
}

/// Named resources consumed by the control-to-connected TX handoff.
pub struct ConnectedStaTxResources<
    'slot,
    'resources,
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const AGGREGATE_SLOTS: usize,
    const AGGREGATE_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> {
    pub control: ControlTransmitter<'slot, P, E, T, ORDINARY_BUFFER_SIZE>,
    pub aggregate: AggregateTxResources<
        'resources,
        PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        AGGREGATE_SLOTS,
        AGGREGATE_BUFFER_SIZE,
    >,
    pub security: oer_esp32s31_ieee80211_sta::single_mpdu_tx::ConnectedTxSecurity,
    pub sequences: StaTxSequenceCounters,
    /// Optional observation-only hook supplied by the composition root.
    #[cfg(any(feature = "diagnostics", test))]
    pub aggregate_tx_observer: Option<&'resources dyn AggregateTxObserver>,
    /// Application-visible negotiated BlockAck status, independent of
    /// diagnostic aggregate observation.
    pub tx_block_ack_status_sink: Option<StationTxBlockAckStatusSink>,
    pub network_domain:
        ConnectedStaNetworkTxDomain<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
}

/// Type-only binding to the pinned `embassy-net` TX resource domain.
///
/// The runner, rather than this port, owns the actual consumer. Carrying its
/// lifetime and const geometry here prevents inference from selecting a TX
/// owner incompatible with that runner without introducing another runtime
/// pointer or capability.
pub struct ConnectedStaNetworkTxDomain<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    #[allow(clippy::type_complexity)]
    marker: core::marker::PhantomData<&'resources (
        M,
        [u8; FRAME_CAPACITY],
        [u8; HEADROOM],
        [u8; TRAILER],
        [u8; QUEUE_DEPTH],
    )>,
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> Default
    for ConnectedStaNetworkTxDomain<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> ConnectedStaNetworkTxDomain<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    pub const fn new() -> Self {
        Self {
            marker: core::marker::PhantomData,
        }
    }
}

/// Complete owner return when control TX was still active at handoff.
pub struct ConnectedStaTxHandoffFailure<
    'slot,
    'resources,
    B: 'resources,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    const AGGREGATE_SLOTS: usize,
    const AGGREGATE_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> {
    pub control: ControlTransmitter<'slot, P, E, T, ORDINARY_BUFFER_SIZE>,
    pub handoff: ConnectedTxHandoff,
    pub aggregate: AggregateTxResources<'resources, B, AGGREGATE_SLOTS, AGGREGATE_BUFFER_SIZE>,
    #[cfg(any(feature = "diagnostics", test))]
    pub aggregate_tx_observer: Option<&'resources dyn AggregateTxObserver>,
    pub tx_block_ack_status_sink: Option<StationTxBlockAckStatusSink>,
}

/// Named control-plane resources for one connected epoch.
pub struct ConnectedStaControlResources<'resources, M: RawMutex, const CAPACITY: usize> {
    /// The slot the association's control is placed in.
    pub slot: &'resources mut crate::roles::station::control_slot::ConnectedControlStorage<
        'resources,
        M,
        CAPACITY,
    >,
    pub receiver: ConnectedControlReceiver<'resources, M, CAPACITY>,
    pub reorder_commands: RxReorderCommandSender<'resources, M>,
    pub rx_block_ack: &'resources StaApRxBlockAck,
    /// The epoch of the association's station TSF owner, a number no other
    /// owner took (`MacClockHandle::tsf_epoch`).
    pub tsf_epoch: u32,
    /// The power agent link of a station running power management.
    pub power: Option<StationPowerBinding<'resources, M>>,
}

/// Final owner graph immediately before the connected services begin running.
pub struct ConnectedStaDriverParts<H, R, X, C, P> {
    pub hardware: H,
    pub rx: R,
    pub tx: X,
    pub control: C,
    pub protocol: P,
}

/// Complete owner return when the atomic connected graph composition cannot
/// acquire the ordinary TX owner.
///
/// RX protocol and control resources remain unbuilt, so scratch buffers,
/// queues and hardware owners remain together with the returned TX frontier.
/// They cannot be mistaken for a reusable disconnected station owner.
pub struct ConnectedStaCompositionFailure<H, R, P, C, X> {
    pub plan: ConnectedStaPlan,
    pub hardware: H,
    pub rx: R,
    pub protocol: P,
    pub control: C,
    pub tx: X,
}

/// Driver composition returned to the executor/application layer.
pub struct ConnectedStaDrivers<H, R, X, C, P> {
    pub services:
        SingleRoleServices<H, crate::roles::station::connected::ConnectedStaRxService<R, P>, X, C>,
    pub report: ConnectedStaReport,
}

/// Copy-only observations useful to qualification and application policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectedStaReport {
    pub link: StaConnectedLink,
    pub data_tx_rate: TxPhyRate,
    pub aggregate_tx_rate: TxPhyRate,
    /// Explicit MCS32 selection/rejection retained independently of rates.
    pub ht_duplicate_tx_selection: HtDuplicateTxSelection,
}
