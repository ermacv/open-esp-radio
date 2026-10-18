use super::*;
use crate::roles::station::control_slot::PlacedConnectedControl;

impl ConnectedStaPort {
    /// Bind the selected connected peer to the allocation-free staged RX
    /// protocol. The caller chooses the network/HIL sink but cannot replace
    /// the station identity or dispatcher policy.
    pub fn build_rx_protocol<
        'queue,
        'pool,
        'scratch,
        'irq,
        M,
        S,
        K,
        const DEPTH: usize,
        const CAPACITY: usize,
        const SLOTS: usize,
        const REORDER_SLOTS: usize,
    >(
        plan: &mut ConnectedStaPlan,
        resources: ConnectedStaRxProtocolResources<
            'queue,
            'pool,
            'scratch,
            'irq,
            M,
            S,
            K,
            DEPTH,
            CAPACITY,
            SLOTS,
            REORDER_SLOTS,
        >,
    ) -> ConnectedReceiveProtocol<
        'queue,
        'pool,
        'scratch,
        'irq,
        M,
        S,
        K,
        DEPTH,
        CAPACITY,
        SLOTS,
        REORDER_SLOTS,
    >
    where
        M: RawMutex,
        S: ConnectedRxProtocolSink<CAPACITY, SLOTS>,
    {
        let ConnectedStaRxProtocolResources {
            frames,
            irq,
            sink,
            mpdu,
            ethernet,
            reorder_commands,
            reorder_storage,
            runtime,
            reorder_scratch,
            clock,
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer,
            #[cfg(any(feature = "diagnostics", test))]
            reorder_observer,
        } = resources;
        let processor = Self::build_rx_processor(
            plan,
            ConnectedStaRxProcessorResources {
                irq,
                sink,
                mpdu,
                ethernet,
                reorder_commands,
                reorder_storage,
                runtime,
                reorder_scratch,
                clock,
                #[cfg(any(feature = "diagnostics", test))]
                pipeline_observer,
                #[cfg(any(feature = "diagnostics", test))]
                reorder_observer,
            },
        );
        ConnectedReceiveProtocol::from_processor(frames, processor)
    }

    /// Build connected-station RX policy without choosing a source queue.
    ///
    /// This is the shared composition point for standalone STA and paired
    /// STA+AP. Both receive the same dispatcher, reorder, control mailbox and
    /// evidence bindings; only the outer physical producer differs.
    pub fn build_rx_processor<
        'queue,
        'pool,
        'scratch,
        'irq,
        M,
        S,
        K,
        const CAPACITY: usize,
        const SLOTS: usize,
        const REORDER_SLOTS: usize,
    >(
        plan: &mut ConnectedStaPlan,
        resources: ConnectedStaRxProcessorResources<
            'queue,
            'pool,
            'scratch,
            'irq,
            M,
            S,
            K,
            CAPACITY,
            SLOTS,
            REORDER_SLOTS,
        >,
    ) -> ConnectedRxProcessor<'queue, 'pool, 'scratch, 'irq, M, S, K, CAPACITY, SLOTS, REORDER_SLOTS>
    where
        M: RawMutex,
        S: ConnectedRxProtocolSink<CAPACITY, SLOTS>,
    {
        resources
            .runtime
            .try_reconfigure_dispatcher(plan.rx_config())
            .expect("returned connected RX protocol storage must be quiescent");
        if let Some(replay) = plan.take_ccmp_rx_replay() {
            resources
                .runtime
                .dispatcher_mut()
                .install_shared_ccmp_rx_replay(replay);
        }
        if let Some(epoch) = plan.disable_esp_now_rx() {
            resources
                .runtime
                .dispatcher_mut()
                .install_esp_now_rx_epoch(epoch);
        }
        #[cfg(any(feature = "diagnostics", test))]
        let mut processor = ConnectedRxProcessor::new_with_reorder_slots(
            resources.irq,
            resources.sink,
            resources.mpdu,
            resources.ethernet,
            resources.runtime,
            resources.clock,
        )
        .with_rx_reorder_commands(resources.reorder_commands)
        .with_rx_reorder_storage(resources.reorder_storage);
        #[cfg(not(any(feature = "diagnostics", test)))]
        let processor = ConnectedRxProcessor::new_with_reorder_slots(
            resources.irq,
            resources.sink,
            resources.mpdu,
            resources.ethernet,
            resources.runtime,
            resources.clock,
        )
        .with_rx_reorder_commands(resources.reorder_commands)
        .with_rx_reorder_storage(resources.reorder_storage);
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(counters) = resources.pipeline_observer {
            processor = processor.with_pipeline_observer(counters);
        }
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = resources.reorder_observer {
            processor = processor.with_reorder_observer(observer);
        }
        match resources.reorder_scratch {
            Some(scratch) => processor.with_rx_reorder_scratch(scratch),
            None => processor,
        }
    }

    /// Move a quiescent control TX owner into its connected ordinary/A-MPDU
    /// owner. A busy control owner returns every authority unchanged.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn build_tx<
        'slot,
        'resources,
        M,
        P,
        E,
        T,
        const FRAME_CAPACITY: usize,
        const HEADROOM: usize,
        const TRAILER: usize,
        const QUEUE_DEPTH: usize,
        const AGGREGATE_SLOTS: usize,
        const AGGREGATE_BUFFER_SIZE: usize,
        const ORDINARY_BUFFER_SIZE: usize,
    >(
        plan: &mut ConnectedStaPlan,
        resources: ConnectedStaTxResources<
            'slot,
            'resources,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            AGGREGATE_SLOTS,
            AGGREGATE_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
    ) -> Result<
        ConnectedTx<
            'slot,
            'resources,
            crate::datapath::PinnedTxFrame<
                'resources,
                M,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                QUEUE_DEPTH,
            >,
            P,
            E,
            T,
            AGGREGATE_SLOTS,
            AGGREGATE_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        ConnectedStaTxHandoffFailure<
            'slot,
            'resources,
            PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
            P,
            E,
            T,
            AGGREGATE_SLOTS,
            AGGREGATE_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
    >
    where
        M: RawMutex,
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: oer_time::Timer,
    {
        assert_eq!(
            resources.aggregate.primary().state(),
            TxSlotState::Free,
            "a connected epoch requires returned idle aggregate storage"
        );
        if let Some(standby) = resources.aggregate.standby() {
            assert_eq!(
                standby.state(),
                TxSlotState::Free,
                "a connected epoch requires returned idle standby aggregate storage"
            );
        }
        let link_protection = resources.security.mode();
        let handoff = ConnectedTxHandoff {
            security: resources.security,
            sequences: resources.sequences,
            config: plan.single_mpdu_tx_config().for_security(link_protection),
        };
        let ordinary = match resources.control.try_into_connected(handoff) {
            Ok(ordinary) => ordinary,
            Err((control, handoff)) => {
                return Err(ConnectedStaTxHandoffFailure {
                    control,
                    handoff,
                    aggregate: resources.aggregate,
                    #[cfg(any(feature = "diagnostics", test))]
                    aggregate_tx_observer: resources.aggregate_tx_observer,
                    tx_block_ack_status_sink: resources.tx_block_ack_status_sink,
                });
            }
        };
        let he_trigger_based = plan.config.tx.he_trigger_based;
        let rate_control = plan
            .rate_control
            .take()
            .expect("a connected STA plan transfers rate control exactly once");
        let mut tx = ConnectedTx::new(
            ordinary,
            resources.aggregate,
            plan.aggregate_tx_config(),
            rate_control,
            plan.data_rate_policy,
            plan.aggregate_rate_policy,
        )
        .expect("connected STA config and idle aggregate storage were validated before handoff")
        .with_he_trigger_based(he_trigger_based);
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = resources.aggregate_tx_observer {
            tx = tx.with_observer(observer);
        }
        if let Some(sink) = resources.tx_block_ack_status_sink {
            tx = tx.with_block_ack_status_sink(sink);
        }
        Ok(tx)
    }

    /// Construct BlockAck, beacon-loss, RX-reorder and power control from the
    /// same connected plan used by RX and TX, placed in the resources' slot
    /// and configured there, never moved by value.
    pub fn build_control<'resources, M: RawMutex, const CAPACITY: usize>(
        plan: &ConnectedStaPlan,
        resources: ConnectedStaControlResources<'resources, M, CAPACITY>,
    ) -> PlacedConnectedControl<'resources, M, CAPACITY> {
        let tx_block_ack = plan
            .config
            .block_ack
            .tx_block_ack_originator()
            .expect("connected STA plan validated TX BlockAck policy");
        let mut control = resources.slot.place(ConnectedControl::new_shared(
            resources.receiver,
            plan.link.bssid,
            plan.link.association_phy == PhyMode::He20,
            tx_block_ack,
            resources.rx_block_ack,
            resources.tsf_epoch,
        ));
        control.set_he_trigger_based(plan.config.tx.he_trigger_based);
        control
            .set_rx_block_ack_maximum_window(plan.config.block_ack.rx_block_ack_maximum_window)
            .expect("connected STA plan validated RX BlockAck policy");
        control.set_rx_reorder_commands(resources.reorder_commands);
        control.enable_beacon_loss(plan.beacon_loss);
        control
            .enable_hardware_beacon_monitor_frontier(
                oer_esp32s31_ieee80211_sta::hardware::beacon_monitor::StationBeaconMonitorBinding::new(
                    plan.link.bssid,
                    StaAssociationId::new(plan.link.association_id)
                        .expect("connected plan validated the infrastructure association ID"),
                ),
                plan.beacon_loss,
            )
            .expect("a fresh connected control owner has no beacon-monitor epoch");
        if let Some(binding) = resources.power {
            control.enable_power_management(
                plan.config.power.into(),
                JoinBeacon {
                    beacon: PmBeacon {
                        timestamp_tsf: plan.link.beacon_timestamp_tsf,
                        interval_tu: plan.link.beacon_interval_tu,
                        tim: None,
                    },
                    received_at: plan.link.beacon_received_at,
                },
                binding,
            );
        }
        if plan.link_protection() == oer_ieee80211_mac::security::LinkProtection::Ccmp
            && plan.config.block_ack.request_initial_tx_block_ack
            && matches!(plan.aggregate_tx_rate, TxPhyRate::Ht(_) | TxPhyRate::He(_))
        {
            control.queue_initial_tx_block_ack(
                plan.config.block_ack.tx_block_ack_negotiation_attempt_limit,
            );
        }
        control
    }

    /// Atomically compose the complete connected driver graph.
    ///
    /// TX is acquired first because it is the only fallible owner handoff
    /// after plan validation. If it is still active, every untouched
    /// hardware/RX/protocol/control owner is returned alongside the exact TX
    /// frontier. Only a successful TX handoff may consume scratch and mailbox
    /// resources into the long-running graph.
    #[allow(
        clippy::too_many_arguments,
        clippy::type_complexity,
        clippy::result_large_err
    )]
    pub fn compose<
        'slot,
        'resources,
        'queue,
        'pool,
        'scratch,
        'irq,
        'control,
        M,
        S,
        K,
        P,
        E,
        T,
        H,
        R,
        const RX_DEPTH: usize,
        const RX_CAPACITY: usize,
        const RX_SLOTS: usize,
        const REORDER_SLOTS: usize,
        const FRAME_CAPACITY: usize,
        const HEADROOM: usize,
        const TRAILER: usize,
        const TX_QUEUE_DEPTH: usize,
        const AGGREGATE_SLOTS: usize,
        const AGGREGATE_BUFFER_SIZE: usize,
        const ORDINARY_BUFFER_SIZE: usize,
        const CONTROL_CAPACITY: usize,
    >(
        mut plan: ConnectedStaPlan,
        mut hardware: H,
        rx: R,
        protocol: ConnectedStaRxProtocolResources<
            'queue,
            'pool,
            'scratch,
            'irq,
            M,
            S,
            K,
            RX_DEPTH,
            RX_CAPACITY,
            RX_SLOTS,
            REORDER_SLOTS,
        >,
        tx: ConnectedStaTxResources<
            'slot,
            'resources,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
            AGGREGATE_SLOTS,
            AGGREGATE_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        control: ConnectedStaControlResources<'control, M, CONTROL_CAPACITY>,
    ) -> Result<
        ConnectedStaDrivers<
            H,
            R,
            ConnectedTx<
                'slot,
                'resources,
                crate::datapath::PinnedTxFrame<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    TX_QUEUE_DEPTH,
                >,
                P,
                E,
                T,
                AGGREGATE_SLOTS,
                AGGREGATE_BUFFER_SIZE,
                ORDINARY_BUFFER_SIZE,
            >,
            PlacedConnectedControl<'control, M, CONTROL_CAPACITY>,
            ConnectedReceiveProtocol<
                'queue,
                'pool,
                'scratch,
                'irq,
                M,
                S,
                K,
                RX_DEPTH,
                RX_CAPACITY,
                RX_SLOTS,
                REORDER_SLOTS,
            >,
        >,
        ConnectedStaCompositionFailure<
            H,
            R,
            ConnectedStaRxProtocolResources<
                'queue,
                'pool,
                'scratch,
                'irq,
                M,
                S,
                K,
                RX_DEPTH,
                RX_CAPACITY,
                RX_SLOTS,
                REORDER_SLOTS,
            >,
            ConnectedStaControlResources<'control, M, CONTROL_CAPACITY>,
            ConnectedStaTxHandoffFailure<
                'slot,
                'resources,
                PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
                P,
                E,
                T,
                AGGREGATE_SLOTS,
                AGGREGATE_BUFFER_SIZE,
                ORDINARY_BUFFER_SIZE,
            >,
        >,
    >
    where
        M: RawMutex,
        S: ConnectedRxProtocolSink<RX_CAPACITY, RX_SLOTS>,
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: oer_time::Timer,
        H: StaEspNowRxPolicyHardware,
    {
        let tx = match Self::build_tx(&mut plan, tx) {
            Ok(tx) => tx,
            Err(tx) => {
                return Err(ConnectedStaCompositionFailure {
                    plan,
                    hardware,
                    rx,
                    protocol,
                    control,
                    tx,
                });
            }
        };
        if plan.esp_now_rx_enabled() {
            configure_sta_esp_now_receive_policy(&mut hardware, plan.link.bssid);
        }
        let protocol = Self::build_rx_protocol(&mut plan, protocol);
        let control = Self::build_control(&plan, control);
        Ok(Self::assemble(
            plan,
            ConnectedStaDriverParts {
                hardware,
                rx,
                tx,
                control,
                protocol,
            },
        ))
    }

    /// Join the already prepared hardware/RX/TX/control owners into the only
    /// services accepted by [`crate::datapath::DatapathRunner`].
    pub fn assemble<H, R, X, C, P>(
        plan: ConnectedStaPlan,
        parts: ConnectedStaDriverParts<H, R, X, C, P>,
    ) -> ConnectedStaDrivers<H, R, X, C, P> {
        ConnectedStaDrivers {
            services: SingleRoleServices::with_control(
                parts.hardware,
                crate::roles::station::connected::ConnectedStaRxService::new(
                    parts.rx,
                    parts.protocol,
                ),
                parts.tx,
                parts.control,
            ),
            report: ConnectedStaReport {
                link: plan.link,
                data_tx_rate: plan.data_tx_rate,
                aggregate_tx_rate: plan.aggregate_tx_rate,
                ht_duplicate_tx_selection: plan.ht_duplicate_tx_selection,
            },
        }
    }
}
