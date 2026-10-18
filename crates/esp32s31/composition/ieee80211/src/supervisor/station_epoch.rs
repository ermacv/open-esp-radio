//! Station role engine composition and finite owner restoration.

use super::*;

pub(super) struct ProductionStationEnginePort<O> {
    radio: &'static SharedRadio,
    mac_clock: StationMacClock,
    mode: ProductionStationMode,
    power_mode: StationPowerMode,
    access_point: ProductionAccessPointResources,
    monitor: ProductionMonitorResources,
    _owner: PhantomData<fn() -> O>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum ProductionStationMode {
    Service,
    PairedCutover,
}

/// The station's PMKSA cache: it outlives every station epoch of the Wi-Fi
/// owner, so a restarted station resumes a cached SAE association.
static STATION_PMKSA: oer_esp32s31_ieee80211_sta::attempt::StaSharedPmksa =
    oer_esp32s31_ieee80211_sta::attempt::StaSharedPmksa::new();

impl<O> ProductionStationEnginePort<O> {
    fn new(
        radio: &'static SharedRadio,
        mac_clock: StationMacClock,
        power_mode: StationPowerMode,
        access_point: ProductionAccessPointResources,
        monitor: ProductionMonitorResources,
    ) -> Self {
        Self {
            radio,
            mac_clock,
            mode: ProductionStationMode::Service,
            power_mode,
            access_point,
            monitor,
            _owner: PhantomData,
        }
    }

    fn paired_cutover(
        radio: &'static SharedRadio,
        mac_clock: StationMacClock,
        power_mode: StationPowerMode,
        access_point: ProductionAccessPointResources,
        monitor: ProductionMonitorResources,
    ) -> Self {
        Self {
            radio,
            mac_clock,
            mode: ProductionStationMode::PairedCutover,
            power_mode,
            access_point,
            monitor,
            _owner: PhantomData,
        }
    }

    pub(super) fn into_parked_roles(
        self,
    ) -> (ProductionAccessPointResources, ProductionMonitorResources) {
        (self.access_point, self.monitor)
    }
}

pub(super) fn production_station_runtime<'state>(
    role: WifiRoleOwner<EspHalWifiPlatform, WifiMembership>,
    interrupt_epoch: MacInterruptEpoch,
    dma: StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
    tx_storage: &'static mut TxStorage,
    scan_table: &'static mut ScanTable,
    frame: &'static mut [u8],
    ethernet: &'static mut [u8],
    board: ProductionStationBoardResources,
) -> ProductionStationRuntime<'state> {
    StationRuntimeResources::new(
        StationRadioResources::new(role, interrupt_epoch),
        StationStorageResources::new(dma, tx_storage, scan_table, frame, ethernet),
        board,
    )
}

impl<'state, 'security> ProductionStationEnginePort<ProductionStationOwner<'state, 'security>> {
    #[inline(never)]
    async fn run_initial_scan_epoch(
        &mut self,
        phase: StationInitialScanPhase<
            'security,
            ProductionStationRuntime<'state>,
            RadioRuntimeOwner,
            ScanRx<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>,
            WifiNetworkResources,
        >,
        discovery: StationDiscovery,
    ) -> StationInitialScanExit<
        'security,
        ProductionStationRuntime<'state>,
        RadioRuntimeOwner,
        ReceiveFrontier<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE>,
        WifiNetworkResources,
        ProductionStationOwner<'state, 'security>,
        StaAttemptStage,
        ProductionStationFault<'state, 'security>,
    > {
        let (mut runtime, hardware, receive, network, identity, mut security) = phase.into_parts();
        let (_, storage_resources, _) = runtime.split_mut();
        let (_, tx_storage, scan_table, frame, _) = storage_resources.parts_mut();
        let control = tx_storage
            .take_control()
            .expect("initial scan owns the ordinary TX owner");
        let scan_plan = StationScanPlan::new(discovery, None, security.policy());
        let scan_request = scan_plan.request(identity.station_address);
        let scan = run_esp32s31_station_scan(
            StationScanResources {
                radio: self.radio,
                phy_observer: NoopPhyTargetObserver,
                hardware,
                receive,
                control,
                table: scan_table,
                frame,
                scan_observer: ProductionScanObserver,
                sequence: security.sequences.non_qos_mut(),
                timer: super::reception_timer(self.mac_clock),
            },
            scan_request,
        )
        .await;
        let decision = scan.decision;
        let oer_esp32s31_ieee80211_runtime::roles::station::StationScanReturned {
            hardware,
            receive,
            control,
            table: _,
            frame: _,
            sequence: _,
            phy_observer: _,
            scan_observer: _,
            timer: _,
            telemetry: _,
            transmit: _,
        } = scan.returned;
        tx_storage
            .restore_control(control)
            .unwrap_or_else(|_| panic!("initial scan returned over a live TX owner"));
        complete_esp32s31_station_initial_scan(
            StationInitialScanReturned {
                runtime,
                hardware,
                receive,
                network,
                identity,
                security,
            },
            decision,
            |receive| receive.into_live().map(ReceiveFrontier::from_live),
            |runtime, hardware, receive, network, identity, security| {
                ProductionStationOwner::new(
                    runtime,
                    ProductionStationPhase::InitialScan {
                        hardware,
                        receive,
                        network,
                        identity,
                    },
                    security.into_role(),
                )
            },
            StationInitialScanFailures {
                no_candidate: StaAttemptStage::Candidate,
                receive_handoff: StaAttemptStage::Candidate,
                transaction: StaAttemptStage::Candidate,
                invalid_plan: StaAttemptStage::Candidate,
            },
        )
    }

    #[inline(never)]
    async fn run_connected_epoch(
        &mut self,
        phase: StationConnectedPhase<
            'security,
            ProductionStationRuntime<'state>,
            ProductionConnectedPhase,
        >,
        control: &mut StationCommandReceiver<'_, CriticalSectionRawMutex>,
    ) -> StaAttemptOutcome<
        ProductionStationOwner<'state, 'security>,
        StaAttemptStage,
        ProductionStationFault<'state, 'security>,
    > {
        let (runtime, connected, security) = phase.into_parts();
        if self.mode == ProductionStationMode::PairedCutover {
            return StaAttemptOutcome::Stopped {
                owner: ProductionStationOwner::new(
                    runtime,
                    ProductionStationPhase::Connected { connected },
                    security,
                ),
            };
        }
        let ProductionConnectedPhase {
            epoch,
            network,
            station,
            peer,
            installed_security,
        } = connected;
        let interface = runtime.board().interface;
        let returned = oer_ieee80211_runtime::await_stack_boundary!(run_connected(
            control,
            self.radio,
            self.mac_clock,
            ConnectedStationResources::new(
                runtime,
                epoch,
                network,
                interface,
                connected_config(self.power_mode),
                peer,
                installed_security,
                security,
            ),
        ));
        let returned = match returned {
            ConnectedStationRunExit::Returned(returned) => returned,
            ConnectedStationRunExit::Faulted(fault) => {
                return StaAttemptOutcome::Faulted {
                    fault: ProductionStationFault {
                        _connected: fault,
                        _station: station,
                    },
                };
            }
        };
        if let ConnectedStationOutcome::Disconnected(reason) = returned.outcome
            && reason.forgets_pmksa()
            && let Some(credentials) = returned.security.credentials()
        {
            credentials.pmksa().remove(station.access_point.bssid);
        }
        let owner = ProductionStationOwner::new(
            returned.runtime,
            ProductionStationPhase::RunningScan {
                disconnected: returned.disconnected,
                station,
            },
            returned.security,
        );
        match returned.outcome {
            ConnectedStationOutcome::Disconnected(_)
            | ConnectedStationOutcome::ReconnectRequested => StaAttemptOutcome::Disconnected {
                owner,
                next_candidate: StaNextCandidate::Refresh,
            },
            ConnectedStationOutcome::StationStopped(_) => StaAttemptOutcome::Stopped { owner },
            ConnectedStationOutcome::HardwareFailure => StaAttemptOutcome::Failed {
                owner,
                failure: StaAttemptFailure::new(
                    oer_ieee80211_sta::station::StaLifecycleStage::Hardware,
                    StaFailureDisposition::Terminal,
                    StaAttemptStage::ConnectedEntry,
                ),
            },
        }
    }

    #[inline(never)]
    async fn run_running_scan_epoch(
        &mut self,
        phase: StationRunningScanPhase<
            'security,
            ProductionStationRuntime<'state>,
            ConnectedDisconnectedEpoch,
        >,
        discovery: StationDiscovery,
    ) -> StationRunningScanExit<
        'security,
        ProductionStationRuntime<'state>,
        ConnectedReconnectedEpoch,
        WifiNetworkResources,
        ProductionStationOwner<'state, 'security>,
        StaAttemptStage,
        ProductionStationFault<'state, 'security>,
    > {
        let (mut runtime, disconnected, station, mut security) = phase.into_parts();
        let (_, storage_resources, _) = runtime.split_mut();
        let (_, tx_storage, scan_table, frame, _) = storage_resources.parts_mut();
        let RunningScanEpochParts {
            retained,
            hardware,
            rx,
        } = disconnected.into_running_scan_parts();
        let control = tx_storage
            .take_control()
            .expect("connected teardown returned the ordinary TX owner");
        let scan_plan = StationScanPlan::new(discovery, None, security.policy());
        let scan_request = scan_plan.request(station.station_address);
        let scan = run_esp32s31_station_scan(
            StationScanResources {
                radio: self.radio,
                phy_observer: NoopPhyTargetObserver,
                hardware,
                receive: match rx {
                    ConnectedParkedRx::Live(rx) => RunningScanRx::from_parked(rx)
                        .unwrap_or_else(|_| panic!("parked station RX retained a staging lease")),
                    ConnectedParkedRx::Halted(rx) => RunningScanRx::from_stopped(rx),
                },
                control,
                table: scan_table,
                frame,
                scan_observer: ProductionScanObserver,
                sequence: security.sequences.non_qos_mut(),
                timer: super::reception_timer(self.mac_clock),
            },
            scan_request,
        )
        .await;
        let scan_result = match scan.decision {
            StationScanDecision::Selected { candidate, .. } => {
                StationRunningScanCompletion::Selected(candidate)
            }
            StationScanDecision::NoCandidate { .. } => StationRunningScanCompletion::Failed {
                disposition: StaFailureDisposition::RefreshCandidate,
                error: StaAttemptStage::Candidate,
            },
            StationScanDecision::Stopped { .. } => StationRunningScanCompletion::Stopped,
            StationScanDecision::Failed { error, .. } => {
                let disposition = esp32s31_station_scan_failure_disposition(&error);
                StationRunningScanCompletion::Failed {
                    disposition,
                    error: StaAttemptStage::Candidate,
                }
            }
            StationScanDecision::InvalidPlan { .. } => StationRunningScanCompletion::Failed {
                disposition: StaFailureDisposition::Terminal,
                error: StaAttemptStage::Candidate,
            },
        };
        let oer_esp32s31_ieee80211_runtime::roles::station::StationScanReturned {
            hardware,
            receive,
            control,
            table: _,
            frame: _,
            sequence: _,
            phy_observer: _,
            scan_observer: _,
            timer: _,
            telemetry: _,
            transmit: _,
        } = scan.returned;
        let rx = receive.into_parked().unwrap_or_else(|rx| {
            panic!(
                "running scan did not return a live RX owner: {:?}",
                rx.phase()
            )
        });
        tx_storage
            .restore_control(control)
            .unwrap_or_else(|_| panic!("running scan returned over a live TX owner"));
        let disconnected = retained.restore(hardware, ConnectedParkedRx::from_live(rx));
        complete_esp32s31_station_running_scan(
            runtime,
            disconnected,
            station,
            security,
            scan_result,
            |disconnected| {
                let (network, epoch) = disconnected.prepare_reconnect();
                (WifiNetworkResources::Running(network), epoch)
            },
            |runtime, disconnected, station, security| {
                ProductionStationOwner::new(
                    runtime,
                    ProductionStationPhase::RunningScan {
                        disconnected,
                        station,
                    },
                    security.into_role(),
                )
            },
        )
    }
}

impl<'state, 'security> ProductionStationEnginePort<ProductionStationOwner<'state, 'security>> {
    #[inline(never)]
    async fn run_initial_join_epoch<'a>(
        &'a mut self,
        phase: StationInitialJoinPhase<
            'security,
            ProductionStationRuntime<'state>,
            RadioRuntimeOwner,
            ReceiveFrontier<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE>,
            WifiNetworkResources,
        >,
        context: StaAttemptContext,
    ) -> StationJoinExit<
        'security,
        ProductionStationRuntime<'state>,
        ProductionConnectedPhase,
        ProductionStationOwner<'state, 'security>,
        StaAttemptStage,
        ProductionStationFault<'state, 'security>,
    >
    where
        'security: 'a,
        'state: 'a,
    {
        diagnostics_event!(
            "open-radio: station lifecycle attempt generation={} attempt={}",
            context.generation,
            context.attempt
        );
        let (mut runtime, mut hardware, receive, network, station, security) = phase.into_parts();
        let (_, storage_resources, _) = runtime.split_mut();
        let (dma, tx_storage, _, frame, _) = storage_resources.parts_mut();
        let join = run_esp32s31_station_join::<
            _,
            _,
            _,
            _,
            EmbassyClock,
            _,
            (),
            _,
            RX_DESCRIPTOR_COUNT,
            RX_BUFFER_SIZE,
            RX_BUFFER_STORAGE_SIZE,
        >(StationJoinResources {
            hardware: &mut hardware,
            radio: self.radio,
            phy_observer: NoopPhyTargetObserver,
            receive,
            rx_storage: dma.storage(),
            transmit: tx_storage
                .control_mut()
                .expect("station attempt owns ordinary TX"),
            frame,
            station,
            listen_interval: self.power_mode.listen_interval(),
            security,
            attempt_observer: ProductionAttemptObserver,
        })
        .await;
        match join {
            StationJoinOutcome::Failed {
                returned,
                stage,
                disposition,
                error,
                progress,
                ..
            } => {
                diagnostics_event!(
                    "open-radio: station attempt failed stage={stage:?} \
                     disposition={disposition:?} completed={} error={error:?}",
                    progress.completed_count()
                );
                StationJoinExit::complete(StaAttemptOutcome::Failed {
                    owner: ProductionStationOwner::new(
                        runtime,
                        ProductionStationPhase::InitialJoin {
                            hardware,
                            receive: returned.receive,
                            network,
                            station: returned.station,
                        },
                        returned.security,
                    ),
                    failure: StaAttemptFailure::new(stage.lifecycle_stage(), disposition, stage),
                })
            }
            StationJoinOutcome::Connected {
                returned,
                peer,
                installed_security,
                report,
                progress,
            } => {
                diagnostics_event!(
                    "open-radio: station joined phases={} auth={} pmksa={} assoc={} wpa2={} m4={}",
                    progress.completed_count(),
                    report.authentication.is_some(),
                    report.pmksa_resumed,
                    report.association.is_some(),
                    report.wpa2.is_some(),
                    report.message4.is_some()
                );
                StationJoinExit::connected_ready(
                    runtime,
                    ProductionConnectedPhase {
                        epoch: ConnectedStationEpoch::Initial {
                            hardware,
                            receive: returned.receive,
                        },
                        network,
                        station: returned.station,
                        peer,
                        installed_security,
                    },
                    returned.security,
                )
            }
        }
    }

    #[inline(never)]
    async fn run_reconnected_epoch<'a>(
        &'a mut self,
        phase: StationReconnectedPhase<
            'security,
            ProductionStationRuntime<'state>,
            ConnectedReconnectedEpoch,
            WifiNetworkResources,
        >,
        context: StaAttemptContext,
    ) -> StationJoinExit<
        'security,
        ProductionStationRuntime<'state>,
        ProductionConnectedPhase,
        ProductionStationOwner<'state, 'security>,
        StaAttemptStage,
        ProductionStationFault<'state, 'security>,
    >
    where
        'security: 'a,
        'state: 'a,
    {
        diagnostics_event!(
            "open-radio: station lifecycle attempt generation={} attempt={}",
            context.generation,
            context.attempt
        );
        let (mut runtime, mut reconnect, network, station, security) = phase.into_parts();
        let (hardware, receive_slot) = reconnect.hardware_and_rx_mut();
        let receive = match receive_slot.take() {
            Ok(receive) => receive,
            Err(_) => {
                return StationJoinExit::complete(StaAttemptOutcome::Failed {
                    owner: ProductionStationOwner::new(
                        runtime,
                        ProductionStationPhase::Reconnected {
                            epoch: reconnect,
                            network,
                            station,
                        },
                        security,
                    ),
                    failure: StaAttemptFailure::new(
                        oer_ieee80211_sta::station::StaLifecycleStage::Hardware,
                        oer_ieee80211_sta::station::StaFailureDisposition::Terminal,
                        StaAttemptStage::Candidate,
                    ),
                });
            }
        };
        let (_, storage_resources, _) = runtime.split_mut();
        let (dma, tx_storage, _, frame, _) = storage_resources.parts_mut();
        let join = run_esp32s31_station_join::<
            _,
            _,
            _,
            _,
            EmbassyClock,
            _,
            (),
            _,
            RX_DESCRIPTOR_COUNT,
            RX_BUFFER_SIZE,
            RX_BUFFER_STORAGE_SIZE,
        >(StationJoinResources {
            hardware,
            radio: self.radio,
            phy_observer: NoopPhyTargetObserver,
            receive,
            rx_storage: dma.storage(),
            transmit: tx_storage
                .control_mut()
                .expect("station attempt owns ordinary TX"),
            frame,
            station,
            listen_interval: self.power_mode.listen_interval(),
            security,
            attempt_observer: ProductionAttemptObserver,
        })
        .await;
        match join {
            StationJoinOutcome::Failed {
                returned,
                stage,
                disposition,
                error,
                progress,
                ..
            } => {
                diagnostics_event!(
                    "open-radio: reconnect attempt failed stage={stage:?} \
                     disposition={disposition:?} completed={} error={error:?}",
                    progress.completed_count()
                );
                let (_, receive_slot) = reconnect.hardware_and_rx_mut();
                *receive_slot = returned.receive;
                StationJoinExit::complete(StaAttemptOutcome::Failed {
                    owner: ProductionStationOwner::new(
                        runtime,
                        ProductionStationPhase::Reconnected {
                            epoch: reconnect,
                            network,
                            station: returned.station,
                        },
                        returned.security,
                    ),
                    failure: StaAttemptFailure::new(stage.lifecycle_stage(), disposition, stage),
                })
            }
            StationJoinOutcome::Connected {
                returned,
                peer,
                installed_security,
                report,
                progress,
            } => {
                diagnostics_event!(
                    "open-radio: station rejoined phases={} auth={} pmksa={} assoc={} wpa2={} m4={}",
                    progress.completed_count(),
                    report.authentication.is_some(),
                    report.pmksa_resumed,
                    report.association.is_some(),
                    report.wpa2.is_some(),
                    report.message4.is_some()
                );
                let (_, receive_slot) = reconnect.hardware_and_rx_mut();
                *receive_slot = returned.receive;
                StationJoinExit::connected_ready(
                    runtime,
                    ProductionConnectedPhase {
                        epoch: ConnectedStationEpoch::Reconnected(reconnect),
                        network,
                        station: returned.station,
                        peer,
                        installed_security,
                    },
                    returned.security,
                )
            }
        }
    }
}

#[cfg(not(feature = "diagnostics"))]
pub(super) type ProductionStationRunner<'state, 'security> = StationEngine<
    'security,
    ProductionStationEnginePort<ProductionStationOwner<'state, 'security>>,
>;

#[cfg(feature = "diagnostics")]
pub(super) type ProductionStationRunner<'state, 'security> = StationEngine<
    'security,
    ProductionStationEnginePort<ProductionStationOwner<'state, 'security>>,
    ProductionStationObserver,
>;

#[cfg(feature = "diagnostics")]
#[derive(Clone, Copy)]
pub(super) struct ProductionStationObserver {
    station_attempt: Option<fn(crate::StationAttemptObservation)>,
}

#[cfg(feature = "diagnostics")]
impl<'state, 'security>
    StationEngineObserver<
        'security,
        CriticalSectionRawMutex,
        ProductionStationEnginePort<ProductionStationOwner<'state, 'security>>,
    > for ProductionStationObserver
{
    fn backoff_started(&mut self, _delay_millis: u32, reason: StaBackoffReason) {
        let StaBackoffReason::AttemptFailed { stage, attempt } = reason else {
            return;
        };
        if let Some(station_attempt) = self.station_attempt {
            station_attempt(crate::StationAttemptObservation::AttemptFailed { attempt, stage });
        }
    }
}

impl<'state, 'security> StationEnginePort<'security, CriticalSectionRawMutex>
    for ProductionStationEnginePort<ProductionStationOwner<'state, 'security>>
{
    type Runtime = ProductionStationRuntime<'state>;
    type InitialHardware = RadioRuntimeOwner;
    type InitialScanRx =
        ScanRx<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>;
    type RxFrontier = ReceiveFrontier<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE>;
    type Network = WifiNetworkResources;
    type Disconnected = ConnectedDisconnectedEpoch;
    type Reconnected = ConnectedReconnectedEpoch;
    type Connected = ProductionConnectedPhase;
    type Error = StaAttemptStage;
    type Fault = ProductionStationFault<'state, 'security>;
    type Timer = oer_time_embassy::EmbassyClock;

    fn timer(&self) -> &Self::Timer {
        &oer_time_embassy::EmbassyClock
    }

    fn run_initial_scan<'a>(
        &'a mut self,
        phase: StationInitialScanPhase<
            'security,
            Self::Runtime,
            Self::InitialHardware,
            Self::InitialScanRx,
            Self::Network,
        >,
        discovery: StationDiscovery,
        _context: StaAttemptContext,
        _control: &'a mut StationCommandReceiver<'_, CriticalSectionRawMutex>,
    ) -> impl Future<
        Output = StationInitialScanExit<
            'security,
            Self::Runtime,
            Self::InitialHardware,
            Self::RxFrontier,
            Self::Network,
            ProductionStationOwner<'state, 'security>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        'security: 'a,
        'state: 'a,
    {
        self.run_initial_scan_epoch(phase, discovery)
    }

    fn run_initial_join<'a>(
        &'a mut self,
        phase: StationInitialJoinPhase<
            'security,
            Self::Runtime,
            Self::InitialHardware,
            Self::RxFrontier,
            Self::Network,
        >,
        context: StaAttemptContext,
        _control: &'a mut StationCommandReceiver<'_, CriticalSectionRawMutex>,
    ) -> impl Future<
        Output = StationJoinExit<
            'security,
            Self::Runtime,
            Self::Connected,
            ProductionStationOwner<'state, 'security>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        'security: 'a,
        'state: 'a,
    {
        self.run_initial_join_epoch(phase, context)
    }

    fn run_running_scan<'a>(
        &'a mut self,
        phase: StationRunningScanPhase<'security, Self::Runtime, Self::Disconnected>,
        discovery: StationDiscovery,
        _context: StaAttemptContext,
        _control: &'a mut StationCommandReceiver<'_, CriticalSectionRawMutex>,
    ) -> impl Future<
        Output = StationRunningScanExit<
            'security,
            Self::Runtime,
            Self::Reconnected,
            Self::Network,
            ProductionStationOwner<'state, 'security>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        'security: 'a,
        'state: 'a,
    {
        self.run_running_scan_epoch(phase, discovery)
    }

    fn run_reconnected<'a>(
        &'a mut self,
        phase: StationReconnectedPhase<'security, Self::Runtime, Self::Reconnected, Self::Network>,
        context: StaAttemptContext,
        _control: &'a mut StationCommandReceiver<'_, CriticalSectionRawMutex>,
    ) -> impl Future<
        Output = StationJoinExit<
            'security,
            Self::Runtime,
            Self::Connected,
            ProductionStationOwner<'state, 'security>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        'security: 'a,
        'state: 'a,
    {
        self.run_reconnected_epoch(phase, context)
    }

    fn run_connected<'a>(
        &'a mut self,
        phase: StationConnectedPhase<'security, Self::Runtime, Self::Connected>,
        _context: StaAttemptContext,
        control: &'a mut StationCommandReceiver<'_, CriticalSectionRawMutex>,
    ) -> impl Future<
        Output = StaAttemptOutcome<
            ProductionStationOwner<'state, 'security>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        'security: 'a,
        'state: 'a,
    {
        self.run_connected_epoch(phase, control)
    }

    fn candidate_refresh_contract_error(&mut self) -> Self::Error {
        StaAttemptStage::Candidate
    }
}

pub(super) type ProductionStationTask =
    StationTask<'static, CriticalSectionRawMutex, ProductionStationRunner<'static, 'static>>;
pub(super) type ProductionStationControl = StationController<'static, CriticalSectionRawMutex>;
pub(super) type ProductionStationExit = StationExit<
    ProductionStationOwner<'static, 'static>,
    ProductionStationRunner<'static, 'static>,
    StaAttemptStage,
    ProductionStationFault<'static, 'static>,
>;

pub(super) fn restore_production_station_frontier(
    resources: oer_esp32s31_ieee80211_runtime::roles::station::StationReturnedResources<
        ProductionStationOwner<'static, 'static>,
        ProductionStationRunner<'static, 'static>,
    >,
) -> Result<ProductionSupervisorStopped, ProductionWifiFault> {
    let (owner, runner) = resources.into_parts();
    match try_reclaim_production_station(owner) {
        Ok(stopped) => {
            let (access_point, monitor) = runner.into_port().into_parked_roles();
            let resources = ProductionWifiStoppedResources::Returned(stopped.resources);
            match try_split_wifi_stopped_resources(resources) {
                Ok((physical, station)) => Ok(WifiSupervisorStopped::new(
                    stopped.wifi,
                    physical,
                    station,
                    access_point,
                    monitor,
                )),
                Err(resources) => Err(ProductionWifiFault::StoppedOwner {
                    _wifi: stopped.wifi,
                    _resources: resources,
                    _access_point: access_point,
                    _monitor: monitor,
                }),
            }
        }
        Err(failure) => Err(ProductionWifiFault::Reclaim {
            _station: failure,
            _runner: runner,
        }),
    }
}

impl ProductionWifiEpochRunner {
    pub(super) fn initialize_tx_epoch(
        &self,
        tx: ProductionOrdinaryTxResources,
        power: PhyTxTargetPowerProfile,
    ) -> &'static mut TxStorage {
        match tx {
            ProductionOrdinaryTxResources::Uninitialized(tx_slot) => TX_STATE.init_with(|| {
                TxStorage::from_slot(
                    tx_slot,
                    power,
                    tx_entropy as fn() -> u32,
                    reception_timer(self.mac_clock),
                    ControlTxConfig {
                        completion_timeout: TX_COMPLETION_TIMEOUT,
                        poll_interval: oer_time::Duration::from_micros(1),
                    },
                    self.rts_length_threshold,
                )
            }),
            ProductionOrdinaryTxResources::Epoch(tx) => {
                // The physical lifecycle returned the descriptor to this
                // storage before a new MAC start. Rebind the radio capability
                // without discarding its ordinary TX policy or terminal state.
                tx.control_mut()
                    .expect("the stopped radio returned its control TX owner")
                    .rebind_timer(reception_timer(self.mac_clock))
                    .unwrap_or_else(|_| panic!("a stopped radio has no active TX publication"));
                tx
            }
        }
    }

    fn fresh_security(&self, security: StationSecurity) -> StaAttemptSecurity<'static> {
        let sequences = StaTxSequenceCounters::new(
            oer_ieee80211_mac::sequence::SequenceNumber::from_low_bits(self.trng.random() as u16),
        );
        match security {
            StationSecurity::Open => StaAttemptSecurity::open(sequences),
            StationSecurity::Wpa2Personal { pmk, password } => StaAttemptSecurity::new(
                StaPersonalCredentials::wpa2(pmk, password, super::tx_entropy, &STATION_PMKSA),
                self.supplicant_nonce(),
                sequences,
                oer_esp32s31_ieee80211_sta::wpa2::Wpa2Message4Protection::Unprotected,
            ),
            StationSecurity::Wpa3Personal(password) => StaAttemptSecurity::new(
                StaPersonalCredentials::wpa3(password, super::tx_entropy, &STATION_PMKSA),
                self.supplicant_nonce(),
                sequences,
                oer_esp32s31_ieee80211_sta::wpa2::Wpa2Message4Protection::Unprotected,
            ),
        }
    }

    fn supplicant_nonce(&self) -> [u8; 32] {
        let mut supplicant_nonce = [0; 32];
        for word in supplicant_nonce.chunks_exact_mut(4) {
            word.copy_from_slice(&self.trng.random().to_le_bytes());
        }
        supplicant_nonce
    }

    pub(super) fn prepare_station_task(
        &self,
        stopped: ProductionSupervisorStopped,
        request: StationRequest,
        mode: ProductionStationMode,
    ) -> Result<(ProductionStationControl, ProductionStationTask), ProductionWifiFault> {
        let (discovery, security, reconnect, power_mode) = request.into_parts();
        let (wifi, physical_resources, station_role, access_point_resources, monitor_resources) =
            stopped.into_parts();
        let station_resources = join_station_activation_resources(physical_resources, station_role);
        let security = self.fresh_security(security);
        let requested_security = security.policy();
        let owner = match station_resources {
            ProductionWifiStoppedResources::Fresh(fresh) => {
                let materialized = materialize_production_wifi(wifi, fresh);
                let ProductionWifiFreshResources {
                    dma,
                    rx_ring,
                    tx,
                    scan_table,
                    scan_frame,
                    ethernet,
                    network,
                    board,
                    station_address,
                } = materialized.resources;
                let mut registers = materialized.registers;
                let tx_storage =
                    self.initialize_tx_epoch(tx, materialized.owner.start_report().wifi.tx_power);
                let scan_rx = match rx_ring {
                    Some(ring) => ring.into_scan(dma.storage()),
                    None => match ScanRx::prepare_initial(
                        &mut registers,
                        dma.storage(),
                        dma.descriptor_base(),
                        dma.buffer_addresses(),
                    ) {
                        Ok(scan_rx) => scan_rx,
                        Err(error) => {
                            return Err(ProductionWifiFault::InitialRx {
                                _fault: ProductionInitialRxFault {
                                    _error: error,
                                    _owner: materialized.owner,
                                    _registers: registers,
                                    _interrupts: materialized.interrupts,
                                    _dma: dma,
                                    _tx_storage: tx_storage,
                                    _scan_table: scan_table,
                                    _scan_frame: scan_frame,
                                    _ethernet: ethernet,
                                    _network: network,
                                    _board: board,
                                    _station_address: station_address,
                                    _security: security,
                                },
                            });
                        }
                    },
                };
                ProductionStationOwner::new(
                    production_station_runtime(
                        materialized.owner,
                        materialized.interrupts,
                        dma,
                        tx_storage,
                        scan_table,
                        scan_frame,
                        ethernet,
                        board,
                    ),
                    ProductionStationPhase::InitialScan {
                        hardware: registers,
                        receive: scan_rx,
                        network,
                        identity: StaIdentity {
                            station_address,
                            association_preference: discovery.scan().association_preference(),
                            security: requested_security,
                        },
                    },
                    security,
                )
            }
            ProductionWifiStoppedResources::Returned(returned) => {
                let materialized = materialize_production_wifi(wifi, returned);
                let ProductionWifiReusableResources {
                    storage,
                    board,
                    phase,
                    security: previous_security,
                } = materialized.resources;
                let rx_storage = storage.parts().0.storage();
                let identity = StaIdentity {
                    station_address: board.interface.interface.address,
                    association_preference: discovery.scan().association_preference(),
                    security: requested_security,
                };
                let phase = match try_rebind_esp32s31_station_phase(
                    phase,
                    rx_storage,
                    identity,
                    ConnectedParkedRx::from_live,
                ) {
                    Ok(phase) => phase,
                    Err(failure) => {
                        return Err(ProductionWifiFault::Resume {
                            _fault: ProductionStationResumeFault {
                                _owner: materialized.owner,
                                _registers: materialized.registers,
                                _interrupts: materialized.interrupts,
                                _storage: storage,
                                _board: board,
                                _phase: failure.resources,
                                _previous_security: previous_security,
                                _requested_security: security,
                            },
                        });
                    }
                };
                let phase = match try_restore_esp32s31_station_phase(materialized.registers, phase)
                {
                    Ok(phase) => phase,
                    Err(failure) => {
                        return Err(ProductionWifiFault::Resume {
                            _fault: ProductionStationResumeFault {
                                _owner: materialized.owner,
                                _registers: failure.registers,
                                _interrupts: materialized.interrupts,
                                _storage: storage,
                                _board: board,
                                _phase: failure.resources,
                                _previous_security: previous_security,
                                _requested_security: security,
                            },
                        });
                    }
                };
                let interrupt = materialized.interrupts;
                // Dropping the previous security value here zeroizes its PMK
                // only after the old finite station task returned completely.
                drop(previous_security);
                ProductionStationOwner::new(
                    StationRuntimeResources::new(
                        StationRadioResources::new(materialized.owner, interrupt),
                        storage,
                        board,
                    ),
                    phase,
                    security,
                )
            }
        };
        let port = match mode {
            ProductionStationMode::Service => ProductionStationEnginePort::new(
                self.radio,
                self.mac_clock,
                power_mode,
                access_point_resources,
                monitor_resources,
            ),
            ProductionStationMode::PairedCutover => ProductionStationEnginePort::paired_cutover(
                self.radio,
                self.mac_clock,
                power_mode,
                access_point_resources,
                monitor_resources,
            ),
        };
        #[cfg(feature = "diagnostics")]
        let runner = ProductionStationRunner::with_observer(
            port,
            discovery,
            ProductionStationObserver {
                station_attempt: owner
                    .runtime
                    .board()
                    .diagnostics
                    .map(|hooks| hooks.station_attempt),
            },
        );
        #[cfg(not(feature = "diagnostics"))]
        let runner = ProductionStationRunner::new(port, discovery);
        prepare_esp32s31_station_task(
            StationConfiguration::new(reconnect),
            StationStartResources::new(owner),
            self.station_control,
            runner,
        )
        .map_err(|failure| ProductionWifiFault::TaskPreparation { _failure: failure })
    }
}
