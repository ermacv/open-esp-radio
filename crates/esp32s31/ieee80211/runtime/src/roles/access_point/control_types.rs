/// How full the network A-MPDUs started to one association were.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AccessPointAggregateFill {
    /// Association ID these counters belong to; zero while none was started.
    pub association_id: u16,
    pub aggregates: u32,
    /// Sum of the subframes of every started A-MPDU.
    pub subframes: u32,
    pub maximum_subframes: u8,
    /// A-MPDUs of 1, 2..=7, 8..=15, 16..=31 and 32 or more subframes.
    pub histogram: [u32; 5],
}

impl AccessPointAggregateFill {
    #[cfg(any(feature = "diagnostics", test))]
    fn record(&mut self, association_id: u16, subframes: u8) {
        self.association_id = association_id;
        self.aggregates = self.aggregates.saturating_add(1);
        self.subframes = self.subframes.saturating_add(u32::from(subframes));
        self.maximum_subframes = self.maximum_subframes.max(subframes);
        let bucket = match subframes {
            0..=1 => 0,
            2..=7 => 1,
            8..=15 => 2,
            16..=31 => 3,
            _ => 4,
        };
        self.histogram[bucket] = self.histogram[bucket].saturating_add(1);
    }
}

#[cfg(test)]
#[test]
fn aggregate_fill_buckets_every_subframe_count_and_keeps_the_maximum() {
    let mut fill = AccessPointAggregateFill::default();
    for subframes in [1, 2, 7, 8, 15, 16, 31, 32] {
        fill.record(3, subframes);
    }
    assert_eq!(fill.association_id, 3);
    assert_eq!(fill.aggregates, 8);
    assert_eq!(fill.subframes, 1 + 2 + 7 + 8 + 15 + 16 + 31 + 32);
    assert_eq!(fill.maximum_subframes, 32);
    assert_eq!(fill.histogram, [1, 2, 2, 2, 1]);
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AccessPointControlObservation {
    pub missed_beacon_intervals: u32,
    pub maximum_beacon_lateness_micros: u32,
    pub tx_interrupt_wakes: u32,
    pub tx_deadline_wakes: u32,
    pub maximum_tx_pending_micros: u32,
    /// Longest network-originated data transaction, excluding chained AP
    /// management, WPA2 and shutdown publications.
    pub maximum_network_tx_pending_micros: u32,
    /// Hardware publications made by the network frame which established
    /// `maximum_network_tx_pending_micros`.
    pub network_tx_attempts_at_maximum_pending: u8,
    pub maximum_rx_service_micros: u32,
    pub maximum_rx_dma_service_micros: u32,
    pub total_rx_dma_service_micros: u32,
    pub rx_dma_service_calls: u32,
    pub maximum_rx_protocol_service_micros: u32,
    pub maximum_rx_protected_data_service_micros: u32,
    pub total_rx_protected_data_service_micros: u32,
    pub maximum_rx_management_service_micros: u32,
    pub maximum_rx_eapol_service_micros: u32,
    pub maximum_network_backpressure_micros: u32,
    pub retained_rx_descriptors: u32,
    pub ignored_rx_frames: u32,
    pub rx_mic_failures: u32,
    pub rx_quarantined_frames: u32,
    pub rx_view_rejected: u32,
    pub control_frames_staged: u32,
    pub control_frames_dropped_while_busy: u32,
    pub ethernet_frames_staged: u32,
    pub ethernet_arp_requests_staged: u32,
    pub ethernet_tcp_frames_staged: u32,
    pub network_tx_frames_observed: u32,
    pub network_tx_arp_requests: u32,
    pub network_tx_arp_replies: u32,
    pub network_tx_rejected_no_peer: u32,
    pub network_tx_rejected_destination: u32,
    pub network_tx_frames_rejected: u32,
    /// Protected data MPDUs whose RX metadata identified an HT PPDU.
    pub rx_ht_data_frames: u32,
    /// Protected HT data MPDUs whose HT-SIG Aggregation bit was set.
    pub rx_ht_mpdus_with_aggregation_bit: u32,
    /// Protected data MPDUs with a hardware-observed RSSI sample.
    pub rx_rssi_samples: u32,
    /// Signed sum of hardware-observed RSSI samples in dBm.
    pub rx_rssi_sum_dbm: i32,
    pub rx_rssi_min_dbm: i8,
    pub rx_rssi_max_dbm: i8,
    /// Protected HT40 data MPDUs grouped by hardware-observed MCS0..MCS7.
    pub rx_ht40_mcs_frames: [u32; 8],
    /// Protected HT40 data MPDUs observed with the 800 ns guard interval.
    pub rx_ht40_long_gi_frames: u32,
    /// Protected HT40 data MPDUs observed with the 400 ns guard interval.
    pub rx_ht40_short_gi_frames: u32,
    /// Protected HT40 data MPDUs carrying the independent MCS32 selector.
    pub rx_ht40_mcs32_frames: u32,
    /// Protected HT data MPDUs carrying MCS32 without its required HT40 CBW.
    pub rx_ht_mcs32_width_mismatches: u32,
    /// Network A-MPDU transactions started with a typed HT rate.
    pub tx_ht_aggregates: u32,
    /// Network A-MPDU transactions started specifically at HT40 MCS7.
    pub tx_ht40_mcs7_aggregates: u32,
    /// Fill of the network A-MPDUs started to each association, indexed by
    /// association ID minus one.
    pub tx_aggregate_fill: [AccessPointAggregateFill; AP_MAX_CLIENTS],
    pub protected_data_frames: u32,
    pub protected_data_unauthorized: u32,
    pub protected_data_foreign: u32,
    pub protected_data_duplicates: u32,
    pub rx_reorder_buffered_mpdus: u32,
    pub rx_reorder_dispatched_mpdus: u32,
    pub rx_reorder_hardware_window_resets: u32,
    pub rx_reorder_gap_timeouts: u32,
    /// Client BlockAckReq frames addressed to this AP.
    pub rx_block_ack_requests: u32,
    /// Retained MPDUs released by client BlockAckReq window moves.
    pub rx_reorder_bar_released_mpdus: u32,
    /// Sequence numbers client BlockAckReq window moves passed without an
    /// MPDU: frames the client gave up on and this AP never received.
    pub rx_reorder_bar_missing_sequences: u32,
    pub protected_data_radio_rejected: u32,
    pub protected_data_protocol_rejected: u32,
    pub first_rx_protocol_rejection: Option<AccessPointRxRejection>,
    /// Data MPDUs whose Protected bit contradicted the requested AP mode.
    pub security_mode_mismatches: u32,
}

#[cfg(any(feature = "diagnostics", test))]
macro_rules! observe_access_point {
    ($owner:expr, $observation:ident, $body:block) => {{
        let $observation = &mut $owner.observer.observation;
        $body
    }};
}

#[cfg(not(any(feature = "diagnostics", test)))]
macro_rules! observe_access_point {
    ($owner:expr, $observation:ident, $body:block) => {{}};
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AccessPointRxProtocolClass {
    ProtectedData,
    Management,
    Eapol,
    Other,
    Rejected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointControlError {
    Receive(oer_esp32s31_ieee80211_mac::rx::pool::RxStageTransactionError),
    Mac(ApMacError),
    /// The caller-provided RX scratch cannot retain one fully decoded batch.
    ReceiveBatchCapacity,
    /// Protocol produced more value-only actions than one bounded turn owns.
    ProtocolActionCapacity,
    /// A non-data frame reached the protocol-only active-TX consumer.
    ProtocolFrameRequiresHardware,
    /// A second successful DTIM beacon edge arrived before the caller-owned
    /// group queue consumed the first exact advertised prefix.
    DtimGroupReleaseAlreadyPending,
    /// Portable TIM accounting and caller-owned pinned leases diverged. The
    /// adapter drops/rolls back both sides and refuses to guess a release.
    GroupBufferOwnershipMismatch,
    InvalidBeaconSchedule,
    RxBlockAckSession(RxBlockAckSessionsError),
    RxBlockAckHardware(S31RxBlockAckAgreementError),
    RxBlockAckReorder(AccessPointRxReorderError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointDatapathError {
    Control(AccessPointControlError),
    Network(FrameLengthError),
    Aggregate(ApAmpduError),
    Airtime(network_tx::AccessPointAirtimeError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointRunError<E> {
    Control(AccessPointControlError),
    InterruptActivate(MacInterruptEpochActivateError<E>),
    InterruptQuiesce(MacInterruptEpochQuiesceError<E>),
    Network(FrameLengthError),
    Aggregate(ApAmpduError),
    Airtime(network_tx::AccessPointAirtimeError),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AccessPointRunObservation {
    #[cfg(any(feature = "diagnostics", test))]
    pub interrupt_drain: MacInterruptEpochDrain,
    #[cfg(any(feature = "diagnostics", test))]
    pub rx_scheduler: Option<RxFrontierSchedulerSnapshot>,
}

/// Complete reusable frontier after IRQ, RX, TX, keys and AP TSF stop.
pub struct AccessPointStopped<
    'storage,
    'beacon,
    'slot,
    P,
    E,
    T,
    R,
    C,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
> {
    pub receive: R,
    pub protocol_rx: C,
    pub transmit: WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
    pub rx_frame: &'storage mut [u8],
    pub tx_frame: &'storage mut [u8],
    pub data_rx: &'storage mut ApRxDispatcher,
    pub rx_block_ack: &'storage StaApRxBlockAck,
    pub rx_reorder: &'storage mut AccessPointRxReorder<'storage, DMA_BUFFER_SIZE>,
    pub rx_reorder_storage:
        &'storage RxReorderFrameStorage<DMA_BUFFER_SIZE, RX_REORDER_BACKING_SLOT_COUNT>,
    #[cfg(feature = "diagnostics")]
    pub observation_storage: &'static mut AccessPointObservationStorage,
    pub engine: oer_esp32s31_ieee80211_ap::engine::ApEngineStop<'beacon>,
}

/// Quiescent AP protocol owners returned by a paired STA+AP DATAPATH.
///
/// Physical RX is owned by the common paired producer and is intentionally
/// absent.  Ordinary TX is returned here so the paired boundary can rejoin it
/// with the shared physical owner before restoring the station graph.
pub struct AccessPointProtocolStopped<
    'storage,
    'beacon,
    'slot,
    P,
    E,
    T,
    const DMA_BUFFER_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
> {
    pub transmit: WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
    pub rx_frame: &'storage mut [u8],
    pub tx_frame: &'storage mut [u8],
    pub data_rx: &'storage mut ApRxDispatcher,
    pub rx_block_ack: &'storage StaApRxBlockAck,
    pub rx_reorder: &'storage mut AccessPointRxReorder<'storage, DMA_BUFFER_SIZE>,
    pub rx_reorder_storage:
        &'storage RxReorderFrameStorage<DMA_BUFFER_SIZE, RX_REORDER_BACKING_SLOT_COUNT>,
    #[cfg(feature = "diagnostics")]
    pub observation_storage: &'static mut AccessPointObservationStorage,
    pub engine: oer_esp32s31_ieee80211_ap::engine::ApEngineStop<'beacon>,
}

/// AP role-local owners after ordinary TX has returned to the paired
/// physical owner.
pub struct AccessPointProtocolFinished<'storage, 'beacon, const DMA_BUFFER_SIZE: usize> {
    pub rx_frame: &'storage mut [u8],
    pub tx_frame: &'storage mut [u8],
    pub data_rx: &'storage mut ApRxDispatcher,
    pub rx_block_ack: &'storage StaApRxBlockAck,
    pub rx_reorder: &'storage mut AccessPointRxReorder<'storage, DMA_BUFFER_SIZE>,
    pub rx_reorder_storage:
        &'storage RxReorderFrameStorage<DMA_BUFFER_SIZE, RX_REORDER_BACKING_SLOT_COUNT>,
    #[cfg(feature = "diagnostics")]
    pub observation_storage: &'static mut AccessPointObservationStorage,
    pub engine: oer_esp32s31_ieee80211_ap::engine::ApEngineStop<'beacon>,
}

impl<'storage, 'beacon, 'slot, P, E, T, const DMA_BUFFER_SIZE: usize, const TX_BUFFER_SIZE: usize>
    AccessPointProtocolStopped<
        'storage,
        'beacon,
        'slot,
        P,
        E,
        T,
        DMA_BUFFER_SIZE,
        TX_BUFFER_SIZE,
    >
{
    pub fn into_parts(
        self,
    ) -> (
        WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
        AccessPointProtocolFinished<'storage, 'beacon, DMA_BUFFER_SIZE>,
    ) {
        (
            self.transmit,
            AccessPointProtocolFinished {
                rx_frame: self.rx_frame,
                tx_frame: self.tx_frame,
                data_rx: self.data_rx,
                rx_block_ack: self.rx_block_ack,
                rx_reorder: self.rx_reorder,
                rx_reorder_storage: self.rx_reorder_storage,
                #[cfg(feature = "diagnostics")]
                observation_storage: self.observation_storage,
                engine: self.engine,
            },
        )
    }
}

impl From<oer_esp32s31_ieee80211_mac::rx::pool::RxStageTransactionError>
    for AccessPointControlError
{
    fn from(error: oer_esp32s31_ieee80211_mac::rx::pool::RxStageTransactionError) -> Self {
        Self::Receive(error)
    }
}

impl From<ApMacError> for AccessPointControlError {
    fn from(error: ApMacError) -> Self {
        Self::Mac(error)
    }
}

impl From<RxBlockAckSessionsError> for AccessPointControlError {
    fn from(error: RxBlockAckSessionsError) -> Self {
        Self::RxBlockAckSession(error)
    }
}

impl From<S31RxBlockAckAgreementError> for AccessPointControlError {
    fn from(error: S31RxBlockAckAgreementError) -> Self {
        Self::RxBlockAckHardware(error)
    }
}

impl From<AccessPointRxReorderError> for AccessPointControlError {
    fn from(error: AccessPointRxReorderError) -> Self {
        Self::RxBlockAckReorder(error)
    }
}

impl From<oer_esp32s31_ieee80211_ap::engine::ApEngineError>
    for AccessPointControlError
{
    fn from(error: oer_esp32s31_ieee80211_ap::engine::ApEngineError) -> Self {
        Self::Mac(ApMacError::Engine(error))
    }
}
