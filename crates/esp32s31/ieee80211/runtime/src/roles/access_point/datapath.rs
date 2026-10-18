#![expect(
    clippy::manual_async_fn,
    reason = "the AP adapter keeps the role-neutral service traits' borrowed Future contracts explicit"
)]

//! Embassy DATAPATH binding for one active AP role.

use crate::datapath::{
    MaterializedTxFrame, SelectedBurstMaterializer, SoftwareTxFrame, TxBatchDemand,
    rx::turn::FusedRxTurn,
};
use oer_network_interface::NetworkInterfaceId;
use oer_time::Clock as _;

use super::*;

// Match the stable standalone-STA and same-channel paired-owner bound. An
// active aggregate must expose its TX completion after a small protocol turn,
// but every such turn must also revisit the physical DMA frontier even while
// the staged queue remains non-empty.
const AP_ACTIVE_TX_PROTOCOL_QUANTUM_FRAMES: usize = 4;

#[derive(Clone, Copy)]
#[cfg(feature = "diagnostics")]
pub(super) struct NetworkTxPending {
    started_micros: u64,
    attempts_before: u32,
}

#[derive(Default)]
#[cfg(any(feature = "diagnostics", test))]
pub(super) struct BlockAckObservationState {
    operational: bool,
}

#[cfg(any(feature = "diagnostics", test))]
impl BlockAckObservationState {
    fn update(&mut self, operational: bool, observer: Option<&dyn AggregateTxObserver>) {
        if operational == self.operational {
            return;
        }
        if let Some(observer) = observer {
            observer.observe(AggregateTxObservation::BlockAckOperational {
                tid: 0,
                operational,
            });
        }
        self.operational = operational;
    }
}

pub(super) struct AccessPointDatapathServices<
    'run,
    'storage,
    'beacon,
    'slot,
    'ampdu,
    RX,
    C,
    P,
    E,
    T,
    H,
    O,
    S,
    L,
    B: 'ampdu,
    N,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
> {
    pub(super) control: &'run mut AccessPointControl<
        'storage,
        'beacon,
        'slot,
        RX,
        C,
        P,
        E,
        T,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        TX_BUFFER_SIZE,
    >,
    pub(super) hardware: &'run mut H,
    pub(super) aggregate: &'run mut AccessPointAmpdu<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
    pub(super) network_tx: AccessPointNetworkTx<'run, B, N>,
    pub(super) status_observer: O,
    pub(super) security_material: S,
    pub(super) set_link_state: L,
    #[cfg(any(feature = "diagnostics", test))]
    pub(super) aggregate_tx_observer: Option<&'run dyn AggregateTxObserver>,
    #[cfg(feature = "diagnostics")]
    pub(super) delivery_observer: Option<&'run dyn RxNetworkDeliveryObserver>,
    pub(super) last_status_revision: u32,
    pub(super) network_link_up: bool,
    #[cfg(any(feature = "diagnostics", test))]
    pub(super) block_ack_observation: BlockAckObservationState,
    #[cfg(feature = "diagnostics")]
    pub(super) network_backpressure_since_micros: Option<u64>,
    #[cfg(feature = "diagnostics")]
    pub(super) tx_pending_since_micros: Option<u64>,
    #[cfg(feature = "diagnostics")]
    pub(super) network_tx_pending: Option<NetworkTxPending>,
    pub(super) next_control_deadline: Instant,
}

impl<
    RX,
    C,
    P,
    E,
    T,
    H,
    O,
    S,
    L,
    B,
    N,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
>
    AccessPointDatapathServices<
        '_,
        '_,
        '_,
        '_,
        '_,
        RX,
        C,
        P,
        E,
        T,
        H,
        O,
        S,
        L,
        B,
        N,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        TX_BUFFER_SIZE,
        AMPDU_SLOTS,
        AMPDU_BUFFER_SIZE,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    O: FnMut(AccessPointServiceStatus),
    L: FnMut(LinkState),
    B: MaterializedTxFrame,
{
    fn tx_pending(&self) -> bool {
        self.network_tx.aggregate_pending() || self.control.tx_pending()
    }

    fn observe_role_state(&mut self) {
        let status_revision = self.control.role_status_revision();
        if status_revision == self.last_status_revision {
            return;
        }
        let status = self.control.role_status();
        let link_state = access_point_network_link_state(status.authorized);
        let authorized = matches!(link_state, LinkState::Up);
        #[cfg(any(feature = "diagnostics", test))]
        self.block_ack_observation.update(
            self.control.has_operational_tx_block_ack(),
            self.aggregate_tx_observer,
        );
        (self.status_observer)(status);
        self.last_status_revision = status_revision;
        if authorized != self.network_link_up {
            (self.set_link_state)(link_state);
            self.network_link_up = authorized;
        }
    }

    pub(super) fn clear_block_ack_observation(&mut self) {
        #[cfg(any(feature = "diagnostics", test))]
        self.block_ack_observation
            .update(false, self.aggregate_tx_observer);
    }

    fn observe_tx_started(&mut self) {
        #[cfg(feature = "diagnostics")]
        if self.tx_pending() && self.tx_pending_since_micros.is_none() {
            self.tx_pending_since_micros = Some(self.control.now().as_micros());
        }
    }

    fn observe_tx_terminal(&mut self) {
        #[cfg(feature = "diagnostics")]
        if !self.tx_pending()
            && let Some(started) = self.tx_pending_since_micros.take()
        {
            let elapsed = self.control.now().as_micros().saturating_sub(started);
            self.control.observer.observation.maximum_tx_pending_micros = self
                .control
                .observer
                .observation
                .maximum_tx_pending_micros
                .max(u32::try_from(elapsed).unwrap_or(u32::MAX));
        }
    }

    fn observe_network_tx_terminal(&mut self) {
        #[cfg(feature = "diagnostics")]
        {
            let Some(pending) = self.network_tx_pending.take() else {
                return;
            };
            let elapsed = self
                .control
                .now()
                .as_micros()
                .saturating_sub(pending.started_micros);
            let elapsed = u32::try_from(elapsed).unwrap_or(u32::MAX);
            if elapsed
                > self
                    .control
                    .observer
                    .observation
                    .maximum_network_tx_pending_micros
            {
                self.control
                    .observer
                    .observation
                    .maximum_network_tx_pending_micros = elapsed;
                self.control
                    .observer
                    .observation
                    .network_tx_attempts_at_maximum_pending = self
                    .control
                    .mac_observation()
                    .data_tx
                    .attempts
                    .saturating_sub(pending.attempts_before)
                    .try_into()
                    .unwrap_or(u8::MAX);
            }
        }
    }

    /// Publish the retained AP Ethernet batch without consuming its cursor on
    /// backpressure. The DATAPATH scheduler may service beacon/control/TX work and
    /// retry the exact same record after the network capacity edge.
    fn publish_pending_rx_batch(
        &mut self,
        network: &mut dyn DatapathNetworkRx,
    ) -> Result<DatapathRxProgress, AccessPointDatapathError> {
        while let Some(record) = self
            .control
            .rx_batch_record()
            .map_err(AccessPointDatapathError::Control)?
        {
            let frame = record.frame;
            let next_offset = record.next_offset;
            #[cfg(not(feature = "diagnostics"))]
            let result = network.try_send_parts(frame);
            #[cfg(feature = "diagnostics")]
            let result = {
                let delivery = RxNetworkDeliveryEvent::decoded(frame, None);
                let observer = self.delivery_observer;
                let mut before_publish = || {
                    if let Some(observer) = observer {
                        observer.admitted(delivery);
                    }
                };
                network.try_send_parts_observed(frame, &mut before_publish)
            };

            match result {
                Ok(()) => {
                    #[cfg(feature = "diagnostics")]
                    let protocol = ethernet_parts_protocol(frame);
                    self.control.commit_rx_batch_record(next_offset);
                    #[cfg(feature = "diagnostics")]
                    {
                        let observation = &mut self.control.observer.observation;
                        observation.ethernet_frames_staged =
                            observation.ethernet_frames_staged.saturating_add(1);
                        match protocol {
                            Some(EthernetProtocol::ArpRequest) => {
                                observation.ethernet_arp_requests_staged =
                                    observation.ethernet_arp_requests_staged.saturating_add(1);
                            }
                            Some(EthernetProtocol::Ipv4Tcp) => {
                                observation.ethernet_tcp_frames_staged =
                                    observation.ethernet_tcp_frames_staged.saturating_add(1);
                            }
                            _ => {}
                        }
                    }
                }
                Err(RxEnqueueError::PoolExhausted)
                    if network.pool_exhaustion()
                        == crate::datapath::network::RxPoolExhaustion::DropFrame =>
                {
                    #[cfg(feature = "diagnostics")]
                    if let Some(observer) = self.delivery_observer {
                        observer.dropped(
                            RxNetworkDeliveryEvent::decoded(frame, None),
                            RxEnqueueError::PoolExhausted,
                        );
                    }
                    self.control.commit_rx_batch_record(next_offset);
                }
                Err(
                    RxEnqueueError::QueueFull
                    | RxEnqueueError::PoolExhausted
                    | RxEnqueueError::LinkDown,
                ) => {
                    #[cfg(feature = "diagnostics")]
                    self.network_backpressure_since_micros
                        .get_or_insert_with(|| self.control.now().as_micros());
                    return Ok(DatapathRxProgress::NetworkBackpressured);
                }
                Err(RxEnqueueError::InvalidLength(error)) => {
                    #[cfg(feature = "diagnostics")]
                    if let Some(observer) = self.delivery_observer {
                        observer.dropped(
                            RxNetworkDeliveryEvent::decoded(frame, None),
                            RxEnqueueError::InvalidLength(error),
                        );
                    }
                    return Err(AccessPointDatapathError::Network(error));
                }
            }
        }

        #[cfg(feature = "diagnostics")]
        if let Some(started) = self.network_backpressure_since_micros.take() {
            let elapsed = self.control.now().as_micros().saturating_sub(started);
            self.control
                .observer
                .observation
                .maximum_network_backpressure_micros = self
                .control
                .observer
                .observation
                .maximum_network_backpressure_micros
                .max(u32::try_from(elapsed).unwrap_or(u32::MAX));
        }
        Ok(DatapathRxProgress::Drained)
    }
}

impl<
    RX,
    C,
    P,
    E,
    T,
    H,
    O,
    S,
    L,
    B,
    N,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
> DatapathServices<N, B>
    for AccessPointDatapathServices<
        '_,
        '_,
        '_,
        '_,
        '_,
        RX,
        C,
        P,
        E,
        T,
        H,
        O,
        S,
        L,
        B,
        N,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        TX_BUFFER_SIZE,
        AMPDU_SLOTS,
        AMPDU_BUFFER_SIZE,
    >
where
    B: MaterializedTxFrame,
    N: SoftwareTxFrame,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + crate::mac_clock::MacClockReader,
    H: RxDma
        + TxHardware
        + ApRuntimeHardware
        + RxBlockAckHardware
        + oer_esp32s31_ieee80211_mac::tx::ampdu::HtAmpduHardware,
    RX: AccessPointRxProducer<H, COUNT>,
    C: AccessPointRxProtocolConsumer,
    O: FnMut(AccessPointServiceStatus),
    S: FnMut() -> ([u8; 32], u64),
    L: FnMut(LinkState),
{
    type Error = AccessPointDatapathError;
    type Exit = Infallible;

    fn service_rx_during_tx<'a>(
        &'a mut self,
        network_rx: &'a mut dyn crate::datapath::network::DatapathNetworkRxSet,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        async move {
            let network_rx = network_rx.primary_mut();
            let mut turn = FusedRxTurn::new(AP_ACTIVE_TX_PROTOCOL_QUANTUM_FRAMES);
            loop {
                if self.control.rx_batch_pending()
                    && self.publish_pending_rx_batch(network_rx)?
                        == DatapathRxProgress::NetworkBackpressured
                {
                    if turn.dma_service_required() {
                        let progress = self
                            .control
                            .service_rx_dma(self.hardware)
                            .await
                            .map_err(AccessPointDatapathError::Control)?;
                        turn.observe_dma(progress);
                    }
                    return Ok(DatapathRxProgress::NetworkBackpressured);
                }
                if !turn.has_protocol_budget() {
                    if turn.dma_service_required() {
                        let progress = self
                            .control
                            .service_rx_dma(self.hardware)
                            .await
                            .map_err(AccessPointDatapathError::Control)?;
                        turn.observe_dma(progress);
                    }
                    return Ok(turn.finish(self.control.queued_rx_frames() != 0));
                }

                // Match the station fused turn: do not enter the AP protocol
                // owner merely to prove that its queues are empty. AP has
                // additional action/reorder readiness, so use the complete
                // role predicate rather than inspecting only the staged SPSC.
                if !self.control.rx_work_due(self.control.now()) {
                    if turn.dma_service_required() {
                        let progress = self
                            .control
                            .service_rx_dma(self.hardware)
                            .await
                            .map_err(AccessPointDatapathError::Control)?;
                        turn.observe_dma(progress);
                        if self.control.rx_work_due(self.control.now()) {
                            continue;
                        }
                    }
                    return Ok(turn.finish(false));
                }

                let serviced_before = self.control.serviced_rx_frames();
                let progress = self
                    .control
                    .service_rx_protocol_bounded(
                        self.hardware,
                        AccessPointRxTxDomain::ActiveTransaction,
                        turn.remaining_protocol_frames(),
                        &mut self.security_material,
                        self.control.now(),
                        #[cfg(feature = "diagnostics")]
                        self.delivery_observer,
                    )
                    .map_err(AccessPointDatapathError::Control)?;
                self.network_tx
                    .refresh_awake_demand(self.control.mac.engine());
                let serviced = usize::try_from(
                    self.control
                        .serviced_rx_frames()
                        .saturating_sub(serviced_before),
                )
                .unwrap_or(usize::MAX);
                turn.observe_protocol(serviced, false);

                if self.control.rx_batch_pending()
                    && self.publish_pending_rx_batch(network_rx)?
                        == DatapathRxProgress::NetworkBackpressured
                {
                    if turn.dma_service_required() {
                        let dma_progress = self
                            .control
                            .service_rx_dma(self.hardware)
                            .await
                            .map_err(AccessPointDatapathError::Control)?;
                        turn.observe_dma(dma_progress);
                    }
                    return Ok(DatapathRxProgress::NetworkBackpressured);
                }
                if serviced == 0 {
                    if turn.dma_service_required() {
                        let dma_progress = self
                            .control
                            .service_rx_dma(self.hardware)
                            .await
                            .map_err(AccessPointDatapathError::Control)?;
                        turn.observe_dma(dma_progress);
                        if progress == DatapathRxProgress::Drained {
                            // The pre-DMA protocol queue was empty. Consume
                            // the newly staged protected data with the
                            // remaining active-TX budget before yielding.
                            continue;
                        }
                    }
                    let refill = turn
                        .dma_progress()
                        .expect("active AP RX refilled exactly once");
                    return Ok(if progress == DatapathRxProgress::ProtocolBlockedByTx {
                        ap_rx_progress_while_protocol_tx_blocked(refill)
                    } else {
                        turn.finish(self.control.queued_rx_frames() != 0)
                    });
                }
            }
        }
    }

    fn service_rx<'a>(
        &'a mut self,
        network_rx: &'a mut dyn crate::datapath::network::DatapathNetworkRxSet,
        context: DatapathRxServiceContext,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        async move {
            let network_rx = network_rx.primary_mut();
            // The idle protocol owner can consume the complete bounded
            // retained-frame domain before yielding. BA width constrains the
            // peer reorder window, not this software ownership capacity.
            // DATAPATH still shortens the turn when network TX is waiting.
            let mut turn = FusedRxTurn::from_context(context, C::MAXIMUM_RETAINED_FRAMES);
            loop {
                let network_backpressured = self.control.rx_batch_pending()
                    && self.publish_pending_rx_batch(network_rx)?
                        == DatapathRxProgress::NetworkBackpressured;
                if !turn.has_protocol_budget() {
                    if turn.dma_service_required() {
                        let dma_progress = self
                            .control
                            .service_rx_dma(self.hardware)
                            .await
                            .map_err(AccessPointDatapathError::Control)?;
                        turn.observe_dma(dma_progress);
                    }
                    return Ok(if network_backpressured {
                        DatapathRxProgress::NetworkBackpressured
                    } else {
                        turn.finish(self.control.queued_rx_frames() != 0)
                    });
                }
                if self.control.beacon_publication_due(self.control.now()) {
                    return Ok(if network_backpressured {
                        DatapathRxProgress::NetworkBackpressured
                    } else {
                        DatapathRxProgress::ProbePending
                    });
                }

                // `service_rx_protocol_bounded` owns AP-specific management,
                // action, reorder and staged-frame state. Its readiness
                // predicate covers all of those domains, allowing the common
                // station-style fused turn to skip empty protocol entries
                // without weakening AP correctness.
                if !self.control.rx_work_due(self.control.now()) {
                    if turn.dma_service_required() {
                        let dma_progress = self
                            .control
                            .service_rx_dma(self.hardware)
                            .await
                            .map_err(AccessPointDatapathError::Control)?;
                        turn.observe_dma(dma_progress);
                        if self.control.rx_work_due(self.control.now()) {
                            continue;
                        }
                    }
                    return Ok(if network_backpressured {
                        DatapathRxProgress::NetworkBackpressured
                    } else {
                        turn.finish(false)
                    });
                }
                let serviced_before = self.control.serviced_rx_frames();
                #[cfg(feature = "diagnostics")]
                let service_started = self.control.now().as_micros();
                let rx_progress = self
                    .control
                    .service_rx_protocol_bounded(
                        self.hardware,
                        AccessPointRxTxDomain::IdleBoundary,
                        turn.remaining_protocol_frames(),
                        &mut self.security_material,
                        self.control.now(),
                        #[cfg(feature = "diagnostics")]
                        self.delivery_observer,
                    )
                    .map_err(AccessPointDatapathError::Control)?;
                self.network_tx
                    .refresh_awake_demand(self.control.mac.engine());
                let serviced = usize::try_from(
                    self.control
                        .serviced_rx_frames()
                        .saturating_sub(serviced_before),
                )
                .unwrap_or(usize::MAX);
                turn.observe_protocol(serviced, false);
                #[cfg(feature = "diagnostics")]
                {
                    let service_elapsed = self
                        .control
                        .now()
                        .as_micros()
                        .saturating_sub(service_started);
                    self.control.observer.observation.maximum_rx_service_micros = self
                        .control
                        .observer
                        .observation
                        .maximum_rx_service_micros
                        .max(u32::try_from(service_elapsed).unwrap_or(u32::MAX));
                }
                self.observe_role_state();
                self.observe_tx_started();
                if !self.control.tx_pending() {
                    let _ = self.network_tx.refresh_power_save_demand(self.control)?;
                }

                if network_backpressured {
                    if turn.dma_service_required() {
                        let dma_progress = self
                            .control
                            .service_rx_dma(self.hardware)
                            .await
                            .map_err(AccessPointDatapathError::Control)?;
                        turn.observe_dma(dma_progress);
                    }
                    return Ok(DatapathRxProgress::NetworkBackpressured);
                }
                if self.control.rx_batch_pending()
                    && self.publish_pending_rx_batch(network_rx)?
                        == DatapathRxProgress::NetworkBackpressured
                {
                    if turn.dma_service_required() {
                        let dma_progress = self
                            .control
                            .service_rx_dma(self.hardware)
                            .await
                            .map_err(AccessPointDatapathError::Control)?;
                        turn.observe_dma(dma_progress);
                    }
                    return Ok(DatapathRxProgress::NetworkBackpressured);
                }
                if self.tx_pending() {
                    // This RX turn started at an idle boundary but protocol
                    // handling published a control response. Leave further
                    // queued work visible until that response reaches its
                    // terminal TX edge.
                    return Ok(rx_progress);
                }
                if serviced == 0 {
                    if turn.dma_service_required() {
                        let dma_progress = self
                            .control
                            .service_rx_dma(self.hardware)
                            .await
                            .map_err(AccessPointDatapathError::Control)?;
                        turn.observe_dma(dma_progress);
                        if rx_progress == DatapathRxProgress::Drained {
                            // Exactly the STA fused order: the old protocol
                            // queue was empty, DMA was serviced once, and the
                            // newly staged owners are consumed below without
                            // another scheduler/MMIO round trip.
                            continue;
                        }
                    }
                    return Ok(turn.finish(self.control.queued_rx_frames() != 0));
                }
            }
        }
    }

    fn has_rx_work(&self) -> bool {
        self.control.rx_work_due(self.control.now())
    }

    fn serviced_rx_frames(&self) -> u64 {
        self.control.serviced_rx_frames()
    }

    fn rx_work_counters(&self) -> crate::datapath::DatapathRxWorkCounters {
        self.control.rx_work_counters()
    }

    fn service_control<'a>(
        &'a mut self,
        _context: DatapathControlContext,
    ) -> impl Future<Output = Result<DatapathControlProgress<Self::Exit>, Self::Error>> + 'a {
        async move {
            let now = self.control.now();
            let progress = self
                .control
                .service_control(self.hardware, now)
                .map_err(AccessPointDatapathError::Control)?;
            if progress == DatapathControlProgress::Idle {
                self.next_control_deadline = self
                    .control
                    .next_control_deadline()
                    .map_err(AccessPointDatapathError::Control)?;
            }
            self.observe_role_state();
            self.observe_tx_started();
            Ok(progress)
        }
    }

    fn control_ready(&self, now: oer_time::Instant) -> bool {
        now >= self.next_control_deadline
    }

    fn has_active_tx(&self) -> bool {
        self.tx_pending()
    }

    fn service_stop(&mut self) -> Result<DatapathStopProgress, Self::Error> {
        self.network_tx.discard_group_buffer(self.control)?;
        let progress = self
            .control
            .service_stop(self.hardware)
            .map_err(AccessPointDatapathError::Control)?;
        #[cfg(feature = "diagnostics")]
        {
            let status = self.control.role_status();
            log::info!(
                "open-radio: AP stop progress={:?} assoc={} authorized={} tx_pending={}",
                progress,
                status.associated,
                status.authorized,
                self.control.tx_pending(),
            );
            log_access_point_queue_zero("stop-progress", self.hardware);
        }
        self.observe_role_state();
        self.observe_tx_started();
        Ok(progress)
    }

    fn wait_control_ready<'a>(&'a mut self) -> impl Future<Output = ()> + 'a {
        Timer::wait_until(&**self.control, self.next_control_deadline)
    }

    fn start_tx<'a, I>(
        &'a mut self,
        frame: N,
        network: &'a I,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a
    where
        I: SelectedBurstMaterializer<SoftwareFrame = N, PhysicalFrame = B> + 'a,
    {
        async move {
            let progress = self
                .network_tx
                .start(self.aggregate, self.control, self.hardware, frame, network)
                .await?;
            if progress == WifiTxProgress::Pending {
                #[cfg(feature = "diagnostics")]
                {
                    debug_assert!(self.network_tx_pending.is_none());
                    self.network_tx_pending = Some(NetworkTxPending {
                        started_micros: self.control.now().as_micros(),
                        attempts_before: self.control.mac_observation().data_tx.attempts,
                    });
                }
            }
            self.observe_tx_started();
            Ok(progress)
        }
    }

    fn last_started_tx_frame_count(&self) -> usize {
        self.network_tx.last_started_frame_count()
    }

    fn wait_tx_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        async move {
            self.network_tx.wait_deadline(self.control).await;
        }
    }

    fn has_prepared_tx(&self) -> bool {
        self.network_tx.has_prepared()
    }

    fn prepared_tx_frame_count(&self) -> usize {
        self.network_tx.prepared_frame_count()
    }

    fn prepared_tx_start_ready(&self) -> bool {
        self.network_tx.prepared_start_ready()
    }

    fn pulls_tx_queues(&self) -> bool {
        true
    }

    fn prepared_tx_destination(&self) -> Option<[u8; 6]> {
        self.network_tx.prepared_destination()
    }

    fn advance_prepared_tx<I>(&mut self, network: &I) -> Result<(), Self::Error>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = N, PhysicalFrame = B>,
    {
        self.network_tx
            .advance_prepared(self.aggregate, self.control, network)
    }

    #[cfg(any(feature = "diagnostics", test))]
    fn mark_prepared_tx_scheduler_phase(
        &mut self,
        phase: PreparedTxSchedulerPhase,
        at_micros: u64,
    ) {
        self.network_tx
            .mark_prepared_scheduler_phase(phase, at_micros);
    }

    fn can_prepare_tx(&self) -> bool {
        self.network_tx
            .can_prepare(self.aggregate, self.control.tx_pending())
    }

    fn prepare_tx<'a, I>(
        &'a mut self,
        frame: N,
        network: &'a I,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a
    where
        I: SelectedBurstMaterializer<SoftwareFrame = N, PhysicalFrame = B> + 'a,
    {
        async move {
            self.network_tx
                .prepare(self.aggregate, self.control, frame, network)
        }
    }

    fn start_prepared_tx<I>(&mut self, network: &I) -> Result<WifiTxProgress, Self::Error>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = N, PhysicalFrame = B>,
    {
        let progress =
            self.network_tx
                .start_prepared(self.aggregate, self.control, self.hardware, network)?;
        if progress == WifiTxProgress::Pending {
            #[cfg(feature = "diagnostics")]
            {
                debug_assert!(self.network_tx_pending.is_none());
                self.network_tx_pending = Some(NetworkTxPending {
                    started_micros: self.control.now().as_micros(),
                    attempts_before: self.control.mac_observation().data_tx.attempts,
                });
            }
        }
        self.observe_tx_started();
        Ok(progress)
    }

    fn cancel_prepared_tx<I>(&mut self, network: &I) -> Result<(), Self::Error>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = N, PhysicalFrame = B>,
    {
        self.network_tx
            .cancel_prepared(self.aggregate, self.control, network)
    }

    fn service_tx<'a>(
        &'a mut self,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            #[cfg(any(feature = "diagnostics", test))]
            if matches!(wake, WifiTxWake::Interrupt { .. })
                && let Some(observer) = self.aggregate_tx_observer
            {
                observer.observe(AggregateTxObservation::InterruptServiceStarted {
                    at_micros: observer.now_micros(),
                });
            }
            #[cfg(feature = "diagnostics")]
            {
                let observation = &mut self.control.observer.observation;
                match wake {
                    WifiTxWake::Interrupt { .. } => {
                        observation.tx_interrupt_wakes =
                            observation.tx_interrupt_wakes.saturating_add(1);
                    }
                    WifiTxWake::Deadline => {
                        observation.tx_deadline_wakes =
                            observation.tx_deadline_wakes.saturating_add(1);
                    }
                }
            }
            let progress =
                self.network_tx
                    .service(self.aggregate, self.control, self.hardware, wake)?;
            self.observe_role_state();
            self.observe_tx_started();
            self.observe_tx_terminal();
            if progress == WifiTxProgress::Complete {
                self.observe_network_tx_terminal();
            }
            #[cfg(any(feature = "diagnostics", test))]
            self.network_tx.observe_service_boundary();
            Ok(progress)
        }
    }

    fn tx_batch_demand<I>(&self, _interface: NetworkInterfaceId, network: &I) -> TxBatchDemand
    where
        I: SelectedBurstMaterializer<SoftwareFrame = N, PhysicalFrame = B>,
    {
        let engine = self.control.mac.engine();
        self.network_tx.batch_demand(
            |destination| {
                access_point_tx_batch_target(
                    engine.operational_tx_block_ack_window(destination),
                    AMPDU_SLOTS,
                )
            },
            network,
        )
    }
}

#[cfg(test)]
mod tests;
