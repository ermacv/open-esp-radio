use oer_time::Clock as _;
/// Physical ordinary-TX ownership visible to one standalone AP RX turn.
///
/// The AP MAC's local `pending` bit cannot represent a live retained A-MPDU:
/// that transaction is owned by `AccessPointNetworkTx`.  Carry the
/// outer ownership edge explicitly so RX protocol dispatch cannot infer an
/// idle transmitter from the narrower AP MAC state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AccessPointRxTxDomain {
    IdleBoundary,
    ActiveTransaction,
}

impl AccessPointRxTxDomain {
    const fn tx_pending(self, mac_pending: bool) -> bool {
        mac_pending || matches!(self, Self::ActiveTransaction)
    }

    const fn is_externally_owned(self) -> bool {
        matches!(self, Self::ActiveTransaction)
    }

    const fn protocol_mailbox_ready(self, remaining: usize) -> bool {
        !self.is_externally_owned() || remaining >= AP_PROTOCOL_ACTIONS_PER_RX_FRAME
    }
}

/// Standalone AP composition of a physical RX transport and the
/// queue-independent AP protocol processor.
pub struct AccessPointControl<
    'storage,
    'beacon,
    'slot,
    R,
    C,
    P,
    E,
    T,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
> {
    receive: R,
    protocol_rx: C,
    role: AccessPointRoleRuntime<
        AccessPointProtocolProcessor<
            'storage,
            'beacon,
            'slot,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        (),
        (),
        (),
    >,
}

impl<
    'storage,
    'beacon,
    'slot,
    R,
    C,
    P,
    E,
    T,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
> core::ops::Deref
    for AccessPointControl<
        'storage,
        'beacon,
        'slot,
        R,
        C,
        P,
        E,
        T,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        TX_BUFFER_SIZE,
    >
{
    type Target = AccessPointProtocolProcessor<
        'storage,
        'beacon,
        'slot,
        P,
        E,
        T,
        DMA_BUFFER_SIZE,
        TX_BUFFER_SIZE,
    >;

    fn deref(&self) -> &Self::Target {
        self.role.protocol()
    }
}

impl<
    'storage,
    'beacon,
    'slot,
    R,
    C,
    P,
    E,
    T,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
> core::ops::DerefMut
    for AccessPointControl<
        'storage,
        'beacon,
        'slot,
        R,
        C,
        P,
        E,
        T,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        TX_BUFFER_SIZE,
    >
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.role.protocol_mut()
    }
}

impl<
    'storage,
    'beacon,
    'slot,
    R,
    C,
    P,
    E,
    T,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
>
    AccessPointControl<
        'storage,
        'beacon,
        'slot,
        R,
        C,
        P,
        E,
        T,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        TX_BUFFER_SIZE,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
{
    pub fn new(
        receive: R,
        protocol_rx: C,
        mac: ApMac<'beacon, 'slot, P, E, T, TX_BUFFER_SIZE>,
        rx_frame: &'storage mut [u8],
        tx_frame: &'storage mut [u8],
        data_rx: &'storage mut ApRxDispatcher,
        rx_block_ack: &'storage StaApRxBlockAck,
        rx_reorder: &'storage mut AccessPointRxReorder<'storage, DMA_BUFFER_SIZE>,
        rx_reorder_storage: &'storage RxReorderFrameStorage<
            DMA_BUFFER_SIZE,
            RX_REORDER_BACKING_SLOT_COUNT,
        >,
        #[cfg(feature = "diagnostics")]
        observation_storage: &'static mut AccessPointObservationStorage,
    ) -> Self {
        Self {
            receive,
            protocol_rx,
            role: AccessPointRoleRuntime::standalone(AccessPointProtocolProcessor::new(
                mac,
                rx_frame,
                tx_frame,
                data_rx,
                rx_block_ack,
                rx_reorder,
                rx_reorder_storage,
                #[cfg(feature = "diagnostics")]
                observation_storage,
            )),
        }
    }

    /// Attach the non-owning terminal observer to the standalone AP role.
    #[cfg(any(feature = "diagnostics", test))]
    pub fn with_terminal_observer(
        mut self,
        observer: &'static dyn AccessPointTerminalObserver,
    ) -> Self {
        self.role.protocol_mut().terminal_observer = Some(observer);
        self
    }

    pub async fn start<H: RxDma>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), AccessPointControlError>
    where
        R: AccessPointRxProducer<H, COUNT>,
    {
        self.receive.start(hardware).await?;
        Ok(())
    }

    pub fn stop<H: RxDma>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), AccessPointControlError>
    where
        R: AccessPointRxProducer<H, COUNT>,
    {
        self.receive.stop(hardware)?;
        Ok(())
    }

    /// Observe one RX descriptor without exposing its DMA ownership.
    #[cfg(any(feature = "diagnostics", test))]
    pub fn rx_descriptor_snapshot(&self, index: usize) -> Option<RxDescriptorSnapshot>
    where
        R: AccessPointRxProducerObservation<COUNT>,
    {
        self.receive.descriptor_snapshot(index)
    }

    /// Observe the live RX scheduler frontier without exposing ownership.
    #[cfg(any(feature = "diagnostics", test))]
    pub fn rx_scheduler_snapshot(&self) -> Option<RxFrontierSchedulerSnapshot>
    where
        R: AccessPointRxProducerObservation<COUNT>,
    {
        self.receive.scheduler_snapshot()
    }

    /// Process one bounded batch of already-staged AP frames without
    /// observing DMA.
    ///
    /// The enclosing DATAPATH turn owns the station-style transaction order:
    /// drain existing protocol owners, refill DMA once, then consume newly
    /// staged owners with the remaining budget. The complete batch remains a
    /// synchronous Core0 transaction. Management/TX publication, deferred
    /// Ethernet output, reorder expiry and active-TX mailbox pressure are
    /// terminal batch boundaries.
    pub(super) fn service_rx_protocol_bounded<H, S>(
        &mut self,
        hardware: &mut H,
        tx_domain: AccessPointRxTxDomain,
        maximum_frames: usize,
        security_material: &mut S,
        mut now: oer_time::Instant,
        #[cfg(feature = "diagnostics")] delivery_observer: Option<&dyn RxNetworkDeliveryObserver>,
    ) -> Result<DatapathRxProgress, AccessPointControlError>
    where
        H: TxHardware + ApRuntimeHardware + RxBlockAckHardware,
        C: AccessPointRxProtocolConsumer,
        S: FnMut() -> ([u8; 32], u64),
    {
        let maximum_frames = maximum_frames.max(1);
        let mut serviced_frames = 0_usize;
        if !tx_domain.is_externally_owned() {
            #[cfg(feature = "diagnostics")]
            self.sample_rx_block_ack_hardware(hardware);
            self.apply_protocol_actions(hardware)?;
        }
        if self.rx_batch_pending() {
            crate::diagnostics::core0_ap_rx_cycles::CORE0_AP_RX_CYCLES.record_turn(
                serviced_frames,
                crate::diagnostics::core0_ap_rx_cycles::Core0ApRxTurnExit::InitialBatch,
            );
            return Ok(DatapathRxProgress::NetworkBackpressured);
        }
        if self.service_rx_reorder_expiry(now)? {
            crate::diagnostics::core0_ap_rx_cycles::CORE0_AP_RX_CYCLES.record_turn(
                serviced_frames,
                crate::diagnostics::core0_ap_rx_cycles::Core0ApRxTurnExit::InitialReorder,
            );
            return Ok(DatapathRxProgress::ProbePending);
        }

        while serviced_frames < maximum_frames {
            let tx_pending = tx_domain.tx_pending(self.mac.tx_pending());

            // An active retained A-MPDU keeps the radio-side mailbox consumer
            // behind the physical TX boundary. Preserve the exact staged head
            // until one complete frame's worst-case action set fits; consuming
            // it first would turn bounded backpressure into a role fault.
            if !tx_domain.protocol_mailbox_ready(self.protocol_actions.remaining_capacity()) {
                crate::diagnostics::core0_ap_rx_cycles::CORE0_AP_RX_CYCLES.record_turn(
                    serviced_frames,
                    crate::diagnostics::core0_ap_rx_cycles::Core0ApRxTurnExit::MailboxBlocked,
                );
                return Ok(DatapathRxProgress::ProtocolBlockedByTx);
            }

            let staged_frame = if tx_pending {
                self.protocol_rx
                    .try_receive_during_tx(self.mac.engine().link_protection())
            } else {
                self.protocol_rx.try_receive()
            };
            let Some(staged_frame) = staged_frame else {
                // `try_receive_during_tx` preserves a management/EAPOL head
                // which cannot borrow the ordinary-TX capability until the
                // live aggregate completes.
                if tx_domain.is_externally_owned() && self.protocol_rx.queued_frames() != 0 {
                    crate::diagnostics::core0_ap_rx_cycles::CORE0_AP_RX_CYCLES.record_turn(
                        serviced_frames,
                        crate::diagnostics::core0_ap_rx_cycles::Core0ApRxTurnExit::TxBlocked,
                    );
                    return Ok(DatapathRxProgress::ProtocolBlockedByTx);
                }
                crate::diagnostics::core0_ap_rx_cycles::CORE0_AP_RX_CYCLES.record_turn(
                    serviced_frames,
                    crate::diagnostics::core0_ap_rx_cycles::Core0ApRxTurnExit::Drained,
                );
                return Ok(DatapathRxProgress::Drained);
            };
            self.serviced_rx_frames = self.serviced_rx_frames.saturating_add(1);
            serviced_frames = serviced_frames.saturating_add(1);
            #[cfg(feature = "diagnostics")]
            let protocol_started = self.now().as_micros();
            let protocol_class = self.service_staged_rx(
                if rx_protocol_consumer_has_hardware(tx_pending) {
                    Some(hardware)
                } else {
                    None
                },
                staged_frame,
                security_material,
                now,
                #[cfg(feature = "diagnostics")]
                delivery_observer,
            )?;
            // `service_staged_rx` receives no hardware capability while TX is
            // active. Keep its value-only mailbox requests queued until the
            // physical aggregate returns the ordinary-TX owner. Executing them
            // here would let standalone AP RX manufacture a second MAC
            // transaction inside `DatapathRunner::drive_active_tx`.
            if !tx_domain.is_externally_owned() && !self.protocol_actions.is_empty() {
                self.apply_protocol_actions(hardware)?;
            }
            #[cfg(not(feature = "diagnostics"))]
            let _ = protocol_class;
            #[cfg(feature = "diagnostics")]
            {
                let elapsed = self.now().as_micros().saturating_sub(protocol_started);
                self.observe_rx_protocol_service(protocol_class, elapsed);
            }

            if self.rx_batch_pending() {
                crate::diagnostics::core0_ap_rx_cycles::CORE0_AP_RX_CYCLES.record_turn(
                    serviced_frames,
                    crate::diagnostics::core0_ap_rx_cycles::Core0ApRxTurnExit::BatchPending,
                );
                return Ok(DatapathRxProgress::ProbePending);
            }
            if self.mac.tx_pending() {
                crate::diagnostics::core0_ap_rx_cycles::CORE0_AP_RX_CYCLES.record_turn(
                    serviced_frames,
                    crate::diagnostics::core0_ap_rx_cycles::Core0ApRxTurnExit::TxBlocked,
                );
                return Ok(DatapathRxProgress::ProbePending);
            }
            now = self.now();
            if self.service_rx_reorder_expiry(now)? {
                crate::diagnostics::core0_ap_rx_cycles::CORE0_AP_RX_CYCLES.record_turn(
                    serviced_frames,
                    crate::diagnostics::core0_ap_rx_cycles::Core0ApRxTurnExit::ReorderPending,
                );
                return Ok(DatapathRxProgress::ProbePending);
            }
        }

        crate::diagnostics::core0_ap_rx_cycles::CORE0_AP_RX_CYCLES.record_turn(
            serviced_frames,
            crate::diagnostics::core0_ap_rx_cycles::Core0ApRxTurnExit::BudgetExhausted,
        );
        Ok(DatapathRxProgress::BudgetExhausted)
    }

    /// Drain the hardware RX completion frontier into independently owned
    /// staging slots without parsing a frame or producing a control action.
    /// This is the only AP RX operation allowed to touch DMA hardware while
    /// TX owns the shared MAC transaction domain. The separate protocol
    /// consumer may parse protected data but can only publish typed actions.
    pub async fn service_rx_dma<H>(
        &mut self,
        hardware: &mut H,
    ) -> Result<DatapathRxProgress, AccessPointControlError>
    where
        H: RxDma,
        R: AccessPointRxProducer<H, COUNT>,
    {
        #[cfg(feature = "diagnostics")]
        let started = self.now().as_micros();
        let progress = self.receive.stage_completed(hardware).await?;
        #[cfg(feature = "diagnostics")]
        {
            let elapsed = self.now().as_micros().saturating_sub(started);
            self.observer.observation.maximum_rx_dma_service_micros = self
                .observer
                .observation
                .maximum_rx_dma_service_micros
                .max(u32::try_from(elapsed).unwrap_or(u32::MAX));
            self.observer.observation.total_rx_dma_service_micros = self
                .observer
                .observation
                .total_rx_dma_service_micros
                .saturating_add(u32::try_from(elapsed).unwrap_or(u32::MAX));
            self.observer.observation.rx_dma_service_calls = self
                .observer
                .observation
                .rx_dma_service_calls
                .saturating_add(1);
        }
        self.serviced_rx_descriptors = self.receive.serviced_descriptors();
        Ok(progress)
    }

    /// Report the exact monotonic DMA work counters used by the role-neutral
    /// recycled-append continuation policy.
    pub fn rx_work_counters(&self) -> crate::datapath::DatapathRxWorkCounters
    where
        R: AccessPointRxProducerObservation<COUNT>,
    {
        self.receive.work_counters()
    }
}
