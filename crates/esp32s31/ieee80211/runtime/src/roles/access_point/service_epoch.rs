#[cfg(feature = "diagnostics")]
fn log_access_point_queue_zero<H: TxHardware>(edge: &str, hardware: &mut H) {
    for queue_index in [0, 2] {
        let Some(queue) = hardware.ordinary_tx_queue_snapshot(queue_index) else {
            log::info!("open-radio: AP q{queue_index} edge={edge} unavailable");
            continue;
        };
        log::info!(
            "open-radio: AP q{} edge={} head={:05x} fmt={} sig={} rate={} key={} bss={} vf={} if={} aifsn={} backoff={} pri={} cca={}/{} valid={} enable={} done={} timeout={} collision={}",
            queue_index,
            edge,
            queue.descriptor_address_low,
            queue.plcp_format,
            queue.legacy_signal,
            queue.rate,
            queue.key_entry_index,
            queue.bssid_select,
            queue.vector_format,
            queue.interface,
            queue.aifsn,
            queue.contention_window,
            queue.scheduler_priority,
            queue.cca_force,
            queue.cca_aux_force,
            queue.valid,
            queue.enabled,
            queue.completion_pending,
            queue.timeout_pending,
            queue.collision_pending,
        );
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
    T: oer_time::Timer + crate::mac_clock::MacClockReader,
{
    /// Run the AP control plane until the caller publishes stop.
    ///
    /// A pending TX descriptor is always driven to a terminal edge before IRQ
    /// routing is masked. RX is then stopped cooperatively; `Busy` means the
    /// walker has not yet acknowledged the request and is retried without
    /// weakening ownership.
    pub async fn run_until_stopped<
        'resources,
        IR,
        NM,
        H,
        F,
        N,
        NR,
        K,
        const FRAME_CAPACITY: usize,
        const HEADROOM: usize,
        const TRAILER: usize,
        const TX_QUEUE_DEPTH: usize,
        const AMPDU_SLOTS: usize,
        const AMPDU_BUFFER_SIZE: usize,
    >(
        &mut self,
        hardware: &mut H,
        interrupts: &mut InterruptEpoch<'_, IR, NM>,
        platform: &IR::Platform,
        network: &mut NR,
        aggregate: &mut AccessPointAmpdu<
            'resources,
            PinnedTxFrame<'resources, NM, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
            AMPDU_SLOTS,
            AMPDU_BUFFER_SIZE,
        >,
        tx_storage: &mut network_tx::AccessPointTxStorage<NR::TxFrame>,
        airtime: Option<(&mut network_tx::AccessPointAirtimeStorage, network_tx::AccessPointAirtimeConfiguration)>,
        #[cfg(any(feature = "diagnostics", test))] aggregate_tx_observer: Option<
            &dyn AggregateTxObserver,
        >,
        #[cfg(feature = "diagnostics")] delivery_observer: Option<&dyn RxNetworkDeliveryObserver>,
        #[cfg(feature = "diagnostics")] mut live_hardware_observer: impl FnMut(&mut H),
        // Construct the supervisor control future only after RX activation and
        // first beacon publication. It may acknowledge start before awaiting stop.
        control: impl FnOnce() -> F,
        mut status_observer: impl FnMut(AccessPointServiceStatus),
        security_material: N,
        // Monotonic clock of the datapath runner's scheduling deadlines.
        clock: K,
    ) -> Result<AccessPointRunObservation, AccessPointRunError<IR::Error>>
    where
        K: oer_time::Timer,
        IR: MacInterruptRoute,
        NM: RawMutex,
        NR: crate::datapath::network::DatapathNetwork<
                PhysicalTxFrame = PinnedTxFrame<
                    'resources,
                    NM,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    TX_QUEUE_DEPTH,
                >,
            >,
        H: RxDma
            + TxHardware
            + ApRuntimeHardware
            + oer_esp32s31_ieee80211_mac::init::MacRuntimeStopHardware
            + RxBlockAckHardware
            + oer_esp32s31_ieee80211_mac::tx::ampdu::HtAmpduHardware,
        R: AccessPointRxProducer<H, COUNT>,
        C: AccessPointRxProtocolConsumer,
        F: Future<Output = ()>,
        N: FnMut() -> ([u8; 32], u64),
    {
        let network_link = network.link_controller();
        network_link.set_link_state(
            crate::datapath::network::AP_NETWORK_INTERFACE_ID,
            LinkState::Down,
        );
        self.start(hardware)
            .await
            .map_err(AccessPointRunError::Control)?;
        // The descriptor walker is fully armed. Resume the vendor MAC
        // frontend before exposing its interrupt route to the CPU.
        oer_esp32s31_ieee80211_mac::init::MacRuntimeStopHardware::resume_mac_runtime(
            hardware,
        );
        if let Err(error) = interrupts
            .activate_or_resume_rx_moderated(platform, MAC_COLD_RX_INTERRUPT_MASK)
        {
            oer_esp32s31_ieee80211_mac::ap_policy::disable_ap_receive_policy(hardware);
            oer_esp32s31_ieee80211_mac::init::MacRuntimeStopHardware::request_mac_runtime_stop(
                hardware,
            );
            crate::time::wait_for(&**self, crate::time::MAC_STOP_SETTLE).await;
            while oer_esp32s31_ieee80211_mac::init::MacRuntimeStopHardware::mac_runtime_active_state(hardware) != 0 {
                crate::time::wait_for(&**self, crate::time::MAC_STOP_POLL).await;
            }
            loop {
                match self.stop(hardware) {
                    Ok(()) => break,
                    Err(AccessPointControlError::Receive(
                        oer_esp32s31_ieee80211_mac::rx::pool::RxStageTransactionError::Ring(
                            oer_esp32s31_ieee80211_mac::rx::RxRingError::Busy,
                        ),
                    )) => yield_now().await,
                    Err(stop) => return Err(AccessPointRunError::Control(stop)),
                }
            }
            return Err(AccessPointRunError::InterruptActivate(error));
        }
        interrupts.mac_runtime().notify_rx_handoff();
        let now = self.now();
        self.publish_beacon(hardware, now)
            .map_err(AccessPointRunError::Control)?;
        #[cfg(feature = "diagnostics")]
        log_access_point_queue_zero("after-first-beacon", hardware);
        let last_status_revision = self.role_status_revision();
        let status = self.role_status();
        status_observer(status);
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = aggregate_tx_observer {
            observer.observe(AggregateTxObservation::BlockAckOperational {
                tid: 0,
                operational: false,
            });
        }
        let network_tx = match airtime {
            Some((storage, config)) => match config.selection {
                network_tx::AccessPointAirtimeSelection::RoundRobin => AccessPointNetworkTx::new_with_airtime_admission(
                    tx_storage,
                    #[cfg(any(feature = "diagnostics", test))] aggregate_tx_observer,
                    storage, config.minimum_exchange_micros, config.cost, config.block_ack_timing,
                ),
                network_tx::AccessPointAirtimeSelection::Deficit => AccessPointNetworkTx::new_with_airtime_scheduling(
                    tx_storage,
                    #[cfg(any(feature = "diagnostics", test))] aggregate_tx_observer,
                    storage, config.minimum_exchange_micros, config.cost, config.block_ack_timing,
                ),
            },
            None => AccessPointNetworkTx::new(
                tx_storage,
                #[cfg(any(feature = "diagnostics", test))] aggregate_tx_observer,
            ),
        };
        #[cfg(feature = "diagnostics")]
        let services_started_micros = self.now().as_micros();
        let services = AccessPointDatapathServices {
            control: self,
            hardware,
            aggregate,
            network_tx,
            status_observer,
            security_material,
            set_link_state: |state| {
                network_link
                    .set_link_state(crate::datapath::network::AP_NETWORK_INTERFACE_ID, state)
            },
            #[cfg(any(feature = "diagnostics", test))]
            aggregate_tx_observer,
            #[cfg(feature = "diagnostics")]
            delivery_observer,
            last_status_revision,
            network_link_up: false,
            #[cfg(any(feature = "diagnostics", test))]
            block_ack_observation: BlockAckObservationState::default(),
            #[cfg(feature = "diagnostics")]
            network_backpressure_since_micros: None,
            #[cfg(feature = "diagnostics")]
            tx_pending_since_micros: Some(services_started_micros),
            #[cfg(feature = "diagnostics")]
            network_tx_pending: None,
            next_control_deadline: oer_time::Instant::EPOCH,
        };
        let mut runner = DatapathRunner::new(
            interrupts.mac_runtime(),
            network,
            crate::datapath::network::AP_NETWORK_INTERFACE_ID,
            services,
            clock,
        );
        let exit = await_stack_boundary!(runner.run_until(control())).map_err(|error| match error {
            AccessPointDatapathError::Control(error) => {
                AccessPointRunError::Control(error)
            }
            AccessPointDatapathError::Network(error) => {
                AccessPointRunError::Network(error)
            }
            AccessPointDatapathError::Airtime(error) => AccessPointRunError::Airtime(error),
            AccessPointDatapathError::Aggregate(error) => {
                AccessPointRunError::Aggregate(error)
            }
        })?;
        let (_, mut services) = runner.into_parts();
        match exit {
            crate::datapath::DatapathRunnerExit::Stopped => {}
            crate::datapath::DatapathRunnerExit::Role(exit) => match exit {},
        }
        #[cfg(feature = "diagnostics")]
        live_hardware_observer(services.hardware);
        // The runner is no longer polling RX, but its fully armed descriptor
        // ring is still live. Revoke AP admission before parking this logical
        // consumer; the physical IRQ route and walker cross the role boundary.
        // Do not assert the outer channel-stop request here. That request is
        // paired with a PHY retune and is legal only while withdrawing the
        // complete physical RX epoch, not during a same-channel role handoff.
        #[cfg(feature = "diagnostics")]
        log_access_point_queue_zero("logical-stop", services.hardware);
        oer_esp32s31_ieee80211_mac::ap_policy::disable_ap_receive_policy(
            services.hardware,
        );
        crate::time::wait_for(&**services.control, crate::time::MAC_STOP_SETTLE).await;
        while oer_esp32s31_ieee80211_mac::init::MacRuntimeStopHardware::mac_runtime_active_state(
            services.hardware,
        ) != 0
        {
            crate::time::wait_for(&**services.control, crate::time::MAC_STOP_POLL).await;
        }
        services.clear_block_ack_observation();
        drop(services);
        let _discarded_staged = self.protocol_rx.discard_queued();
        #[cfg(any(feature = "diagnostics", test))]
        let rx_scheduler = self.receive.scheduler_snapshot();
        observe_access_point!(self, observation, {
            observation.ignored_rx_frames = observation
                .ignored_rx_frames
                .saturating_add(u32::try_from(_discarded_staged).unwrap_or(u32::MAX));
            observation.retained_rx_descriptors = rx_scheduler
                .map(|snapshot| snapshot.observed_mask.count_ones())
                .unwrap_or(0);
        });
        let _ = platform;
        let interrupt_drain = interrupts.park();
        let _interrupt_drain =
            interrupt_drain.map_err(AccessPointRunError::InterruptQuiesce)?;
        loop {
            match self.stop(hardware) {
                Ok(()) => break,
                Err(AccessPointControlError::Receive(
                    oer_esp32s31_ieee80211_mac::rx::pool::RxStageTransactionError::Ring(
                        oer_esp32s31_ieee80211_mac::rx::RxRingError::Busy,
                    ),
                )) => yield_now().await,
                Err(error) => return Err(AccessPointRunError::Control(error)),
            }
        }
        Ok(AccessPointRunObservation {
            #[cfg(any(feature = "diagnostics", test))]
            interrupt_drain: _interrupt_drain,
            #[cfg(any(feature = "diagnostics", test))]
            rx_scheduler,
        })
    }

    /// Consume a quiescent AP service and return every reusable capability.
    /// Failure returns the exact service; no caller can manufacture stopped
    /// Wi-Fi while RX or TX remains active.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn try_finish<H>(
        self,
        hardware: &mut H,
    ) -> Result<
        AccessPointStopped<
            'storage,
            'beacon,
            'slot,
            P,
            E,
            T,
            R,
            C,
            COUNT,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
            TX_BUFFER_SIZE,
        >,
        Self,
    >
    where
        H: ApRuntimeHardware + RxDma,
        R: AccessPointRxProducer<H, COUNT>,
        C: AccessPointRxProtocolConsumer,
    {
        if self.protocol_rx.queued_frames() != 0 {
            return Err(self);
        }
        let Self {
            receive,
            protocol_rx,
            role,
        } = self;
        let (processor, (), (), ()) = role.into_parts();
        let AccessPointProtocolStopped {
            transmit,
            rx_frame,
            tx_frame,
            data_rx,
            rx_block_ack,
            rx_reorder,
            rx_reorder_storage,
            #[cfg(feature = "diagnostics")]
            observation_storage,
            engine,
        } = match processor.try_finish_paired(hardware) {
            Ok(stopped) => stopped,
            Err(processor) => {
                return Err(Self {
                    receive,
                    protocol_rx,
                    role: AccessPointRoleRuntime::standalone(processor),
                });
            }
        };
        Ok(AccessPointStopped {
            receive,
            protocol_rx,
            transmit,
            rx_frame,
            tx_frame,
            data_rx,
            rx_block_ack,
            rx_reorder,
            rx_reorder_storage,
            #[cfg(feature = "diagnostics")]
            observation_storage,
            engine,
        })
    }
}
