#![expect(
    clippy::result_large_err,
    reason = "AP teardown returns the exact physical and role owners on failure"
)]

//! Private ESP32-S31 access-point epoch composition.

#[cfg(feature = "diagnostics")]
use super::access_point_observation::{
    begin_access_point_observation, publish_stored_access_point_observation,
    store_access_point_rx_hardware_observation,
};

use super::*;
type ProductionAccessPointControl = AccessPointControl<
    'static,
    'static,
    'static,
    ProductionAccessPointRxProducer,
    ProductionAccessPointRxConsumer,
    PhyTxTargetPowerProfile,
    fn() -> u32,
    MacTimer,
    RX_DESCRIPTOR_COUNT,
    RX_BUFFER_SIZE,
    RX_BUFFER_STORAGE_SIZE,
    TX_BUFFER_SIZE,
>;
type ProductionWifiTxResources =
    WifiTxResources<'static, PhyTxTargetPowerProfile, fn() -> u32, MacTimer, TX_BUFFER_SIZE>;
type ProductionAccessPointStopped = EmbassyAccessPointStopped<
    'static,
    'static,
    'static,
    PhyTxTargetPowerProfile,
    fn() -> u32,
    MacTimer,
    ProductionAccessPointRxProducer,
    ProductionAccessPointRxConsumer,
    RX_DESCRIPTOR_COUNT,
    RX_BUFFER_SIZE,
    RX_BUFFER_STORAGE_SIZE,
    TX_BUFFER_SIZE,
>;
type ProductionAccessPointAmpdu =
    oer_esp32s31_ieee80211_runtime::roles::access_point::AccessPointAmpdu<
        'static,
        RadioTxBacking,
        { crate::resources::profile::ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT },
        0,
    >;
type ProductionAccessPointRxBlockAck =
    oer_esp32s31_ieee80211_runtime::roles::concurrent::StaApRxBlockAck;
type ProductionAccessPointRxReorder = AccessPointRxReorder<'static, RX_BUFFER_SIZE>;
type ProductionAccessPointRxReorderStorage =
    oer_esp32s31_ieee80211_runtime::datapath::rx::reorder::RxReorderFrameStorage<
        RX_BUFFER_SIZE,
        { oer_esp32s31_ieee80211_runtime::datapath::rx::reorder::RX_REORDER_BACKING_SLOT_COUNT },
    >;

type ProductionAccessPointTxStorage =
    oer_esp32s31_ieee80211_runtime::roles::access_point::network_tx::AccessPointTxStorage<
        RadioNetworkTxBacking,
    >;

pub(super) struct ProductionAccessPointParked {
    tx_storage: &'static mut ProductionAccessPointTxStorage,
    dma: StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
    tx_epoch: &'static mut TxStorage,
    station: ProductionStationRoleResources,
    monitor: ProductionMonitorResources,
    aggregate_tx: Option<RadioAmpduStorage>,
}

pub(super) struct ProductionAccessPointTask {
    channel: WifiChannel,
    owner: WifiRoleOwner<EspHalWifiPlatform, WifiMembership>,
    registers: RadioRuntimeOwner,
    interrupts: MacInterruptEpoch,
    service: ProductionAccessPointControl,
    aggregate: ProductionAccessPointAmpdu,
    parked: ProductionAccessPointParked,
}

pub(super) struct ProductionAccessPointPreflightFault {
    _owner: WifiRoleOwner<EspHalWifiPlatform, WifiMembership>,
    _registers: RadioRuntimeOwner,
    _interrupts: MacInterruptEpoch,
    _physical: ProductionWifiPhysicalResources,
    _station_role: ProductionStationRoleResources,
    _access_point: ProductionAccessPointResources,
    _monitor: ProductionMonitorResources,
    _detached_control: Option<ControlTx>,
    _ring: Option<ProductionRxRing>,
}

pub(super) struct ProductionAccessPointEngineFault {
    _owner: WifiRoleOwner<EspHalWifiPlatform, WifiMembership>,
    _registers: RadioRuntimeOwner,
    _interrupts: MacInterruptEpoch,
    _ring: ProductionRxRing,
    _transmit: ProductionWifiTxResources,
    _engine: oer_esp32s31_ieee80211_ap::engine::ApEngineStartFailure<'static>,
    _parked: ProductionAccessPointParked,
    _rx_dispatcher: &'static mut oer_esp32s31_ieee80211_ap::rx::ApRxDispatcher,
    _rx_block_ack: &'static ProductionAccessPointRxBlockAck,
    _rx_reorder: &'static mut ProductionAccessPointRxReorder,
    _rx_reorder_storage: &'static ProductionAccessPointRxReorderStorage,
    #[cfg(feature = "diagnostics")]
    _observation_storage: &'static mut oer_esp32s31_ieee80211_runtime::diagnostics::access_point::AccessPointObservationStorage,
    _rx_frame: &'static mut [u8],
    _tx_frame: &'static mut [u8],
}

pub(super) enum ProductionAccessPointPreparationFault {
    Preflight {
        _fault: ProductionAccessPointPreflightFault,
    },
    Engine {
        _fault: ProductionAccessPointEngineFault,
    },
}

pub(super) enum ProductionAccessPointTeardownFault {
    Aggregate {
        _owner: WifiRoleOwner<EspHalWifiPlatform, WifiMembership>,
        _registers: RadioRuntimeOwner,
        _interrupts: MacInterruptEpoch,
        _stopped: ProductionAccessPointStopped,
        _aggregate: ProductionAccessPointAmpdu,
        _parked: ProductionAccessPointParked,
    },
    TxRestore {
        _owner: WifiRoleOwner<EspHalWifiPlatform, WifiMembership>,
        _registers: RadioRuntimeOwner,
        _interrupts: MacInterruptEpoch,
        _ring: ProductionRxRing,
        _storage: &'static RxStorage,
        _rx_frame: &'static mut [u8],
        _tx_frame: &'static mut [u8],
        _data_rx: &'static mut oer_esp32s31_ieee80211_ap::rx::ApRxDispatcher,
        _rx_block_ack: &'static ProductionAccessPointRxBlockAck,
        _rx_reorder: &'static mut ProductionAccessPointRxReorder,
        _rx_reorder_storage: &'static ProductionAccessPointRxReorderStorage,
        #[cfg(feature = "diagnostics")]
        _observation_storage: &'static mut oer_esp32s31_ieee80211_runtime::diagnostics::access_point::AccessPointObservationStorage,
        _engine: oer_esp32s31_ieee80211_ap::engine::ApEngineStop<'static>,
        _parked: ProductionAccessPointParked,
        _returned_control: ControlTx,
    },
}

/// Static resources reserved for one exclusive AP epoch.
pub(super) struct ProductionAccessPointResources {
    pub(super) tx_storage: &'static mut ProductionAccessPointTxStorage,
    pub(super) address: [u8; 6],
    pub(super) beacon: &'static mut [u8; oer_ieee80211_mac::beacon::AP_BEACON_CAPACITY],
    pub(super) rx_frame: &'static mut [u8],
    pub(super) tx_frame: &'static mut [u8],
    pub(super) peer_storage: &'static mut oer_ieee80211_ap::AccessPointPeerStorage,
    pub(super) pairwise_storage:
        &'static mut oer_esp32s31_ieee80211_ap::security::ApPairwiseKeyStorage,
    pub(super) rx_dispatcher:
        &'static mut oer_esp32s31_ieee80211_ap::rx::ApRxDispatcher,
    pub(super) rx_block_ack: &'static ProductionAccessPointRxBlockAck,
    pub(super) rx_reorder: &'static mut AccessPointRxReorder<'static, RX_BUFFER_SIZE>,
    pub(super) rx_reorder_storage:
        &'static oer_esp32s31_ieee80211_runtime::datapath::rx::reorder::RxReorderFrameStorage<
            RX_BUFFER_SIZE,
            {
                oer_esp32s31_ieee80211_runtime::datapath::rx::reorder::RX_REORDER_BACKING_SLOT_COUNT
            },
        >,
    #[cfg(feature = "diagnostics")]
    pub(super) observation_storage: &'static mut oer_esp32s31_ieee80211_runtime::diagnostics::access_point::AccessPointObservationStorage,
}

impl ProductionWifiEpochRunner {
    pub(super) async fn prepare_access_point_task(
        &self,
        wifi: ProductionWifiOwner,
        physical: ProductionWifiPhysicalResources,
        station: ProductionStationRoleResources,
        access_point: ProductionAccessPointResources,
        monitor: ProductionMonitorResources,
        request: AccessPointRequest,
    ) -> Result<ProductionAccessPointTask, ProductionAccessPointPreparationFault> {
        let current_channel = wifi.current_channel();
        diagnostics_event!(
            "open-radio: AP prepare begin current_channel={current_channel:?} requested_channel={:?}",
            request.channel()
        );
        let mut materialized = materialize_production_wifi(wifi, physical);
        let requested_channel = request.channel();
        if requested_channel != current_channel {
            diagnostics_event!("open-radio: AP prepare channel switch begin");
            let lowered_channel = lower_wifi_channel(requested_channel);
            let mut channel = ScanPhy::new(NoopPhyTargetObserver, EmbassyClock);
            let selected = {
                let mut guard = self.radio.lock().await;
                let (lease, platform, _) = guard.parts();
                await_stack_boundary!(channel.select_channel(
                    lease,
                    platform,
                    lowered_channel.channel_or_frequency,
                    lowered_channel.cbw,
                    &mut materialized.registers,
                ))
            };
            if selected.is_err() {
                return Err(ProductionAccessPointPreparationFault::Preflight {
                    _fault: ProductionAccessPointPreflightFault {
                        _owner: materialized.owner,
                        _registers: materialized.registers,
                        _interrupts: materialized.interrupts,
                        _physical: materialized.resources,
                        _station_role: station,
                        _access_point: access_point,
                        _monitor: monitor,
                        _detached_control: None,
                        _ring: None,
                    },
                });
            }
            materialized.owner.set_current_channel(requested_channel);
            diagnostics_event!("open-radio: AP prepare channel switch complete");
        }

        let ProductionWifiPhysicalResources {
            dma,
            rx_ring,
            tx,
            aggregate_tx,
        } = materialized.resources;
        let ProductionStationRoleResources {
            scan_table,
            scan_frame,
            ethernet,
            mut resume,
            board,
            station_address,
        } = station;
        let power = materialized.owner.start_report().wifi.tx_power;
        let tx_epoch = self.initialize_tx_epoch(tx, power);
        let ring = match rx_ring {
            Some(ring) => ring,
            None => match ScanRx::prepare_initial(
                &mut materialized.registers,
                dma.storage(),
                dma.descriptor_base(),
                dma.buffer_addresses(),
            ) {
                Ok(receive) => ProductionRxRing::Halted(
                    receive
                        .into_halted()
                        .unwrap_or_else(|_| unreachable!("fresh AP RX is prepared but not live")),
                ),
                Err(_) => {
                    return Err(ProductionAccessPointPreparationFault::Preflight {
                        _fault: ProductionAccessPointPreflightFault {
                            _owner: materialized.owner,
                            _registers: materialized.registers,
                            _interrupts: materialized.interrupts,
                            _physical: ProductionWifiPhysicalResources {
                                dma,
                                rx_ring: None,
                                tx: ProductionOrdinaryTxResources::Epoch(tx_epoch),
                                aggregate_tx,
                            },
                            _station_role: ProductionStationRoleResources {
                                scan_table,
                                scan_frame,
                                ethernet,
                                resume,
                                board,
                                station_address,
                            },
                            _access_point: access_point,
                            _monitor: monitor,
                            _detached_control: None,
                            _ring: None,
                        },
                    });
                }
            },
        };
        diagnostics_event!("open-radio: AP prepare RX ring acquired");
        let control = match tx_epoch.take_control() {
            Ok(control) => control,
            Err(_) => {
                return Err(ProductionAccessPointPreparationFault::Preflight {
                    _fault: ProductionAccessPointPreflightFault {
                        _owner: materialized.owner,
                        _registers: materialized.registers,
                        _interrupts: materialized.interrupts,
                        _physical: ProductionWifiPhysicalResources {
                            dma,
                            rx_ring: None,
                            tx: ProductionOrdinaryTxResources::Epoch(tx_epoch),
                            aggregate_tx,
                        },
                        _station_role: ProductionStationRoleResources {
                            scan_table,
                            scan_frame,
                            ethernet,
                            resume,
                            board,
                            station_address,
                        },
                        _access_point: access_point,
                        _monitor: monitor,
                        _detached_control: None,
                        _ring: Some(ring),
                    },
                });
            }
        };
        let transmit = match control.try_into_resources() {
            Ok(resources) => resources,
            Err(control) => {
                return Err(ProductionAccessPointPreparationFault::Preflight {
                    _fault: ProductionAccessPointPreflightFault {
                        _owner: materialized.owner,
                        _registers: materialized.registers,
                        _interrupts: materialized.interrupts,
                        _physical: ProductionWifiPhysicalResources {
                            dma,
                            rx_ring: None,
                            tx: ProductionOrdinaryTxResources::Epoch(tx_epoch),
                            aggregate_tx,
                        },
                        _station_role: ProductionStationRoleResources {
                            scan_table,
                            scan_frame,
                            ethernet,
                            resume,
                            board,
                            station_address,
                        },
                        _access_point: access_point,
                        _monitor: monitor,
                        _detached_control: Some(control),
                        _ring: Some(ring),
                    },
                });
            }
        };
        diagnostics_event!("open-radio: AP prepare TX resources idle");

        let (ssid, security, channel, client_limit, inactive_timeout, beacon_interval, dtim_period) =
            request.into_parts();
        let ProductionAccessPointResources {
            tx_storage,
            address,
            beacon,
            rx_frame,
            tx_frame,
            peer_storage,
            pairwise_storage,
            rx_dispatcher,
            rx_block_ack,
            rx_reorder,
            rx_reorder_storage,
            #[cfg(feature = "diagnostics")]
            observation_storage,
        } = access_point;
        let service = match security {
            AccessPointSecurity::Open => {
                AccessPointService::new_open(address, client_limit, inactive_timeout, peer_storage)
            }
            AccessPointSecurity::Wpa2Personal(pmk) => {
                let mut gtk_key = [0_u8; 16];
                for word in gtk_key.chunks_exact_mut(4) {
                    word.copy_from_slice(&self.trng.random().to_le_bytes());
                }
                let gtk = RsnGtk::new(1, true, gtk_key)
                    .unwrap_or_else(|_| unreachable!("production GTK key id is valid"));
                AccessPointService::new(
                    address,
                    pmk,
                    gtk,
                    client_limit,
                    inactive_timeout,
                    peer_storage,
                )
            }
        };
        let engine = match ApEngine::start(
            &mut materialized.registers,
            service,
            beacon,
            pairwise_storage,
            &ssid,
            channel,
            beacon_interval.tu(),
            dtim_period.get(),
            self.mac_clock.tsf_epoch(),
        ) {
            Ok(engine) => engine,
            Err(engine) => {
                return Err(ProductionAccessPointPreparationFault::Engine {
                    _fault: ProductionAccessPointEngineFault {
                        _owner: materialized.owner,
                        _registers: materialized.registers,
                        _interrupts: materialized.interrupts,
                        _ring: ring,
                        _transmit: transmit,
                        _engine: engine,
                        _parked: ProductionAccessPointParked {
                            tx_storage,
                            dma,
                            tx_epoch,
                            station: ProductionStationRoleResources {
                                scan_table,
                                scan_frame,
                                ethernet,
                                resume,
                                board,
                                station_address,
                            },
                            monitor,
                            aggregate_tx: Some(aggregate_tx),
                        },
                        _rx_dispatcher: rx_dispatcher,
                        _rx_block_ack: rx_block_ack,
                        _rx_reorder: rx_reorder,
                        _rx_reorder_storage: rx_reorder_storage,
                        #[cfg(feature = "diagnostics")]
                        _observation_storage: observation_storage,
                        _rx_frame: rx_frame,
                        _tx_frame: tx_frame,
                    },
                });
            }
        };
        diagnostics_event!("open-radio: AP prepare engine started");
        let maximum_aggregate_bytes = transmit.policy.ht_ampdu().maximum_aggregate_bytes();
        let aggregate = ProductionAccessPointAmpdu::new(aggregate_tx, maximum_aggregate_bytes);
        let mac = ApMac::new(
            engine,
            transmit,
            ApTxConfig {
                publication_timeout: TX_COMPLETION_TIMEOUT,
            },
        );
        let (receive, protocol_rx) = access_point_rx_pipeline(
            ring,
            dma.storage(),
            resume.take_retained_rx(),
            #[cfg(feature = "diagnostics")]
            board
                .diagnostics
                .expect("diagnostics AP retains its pipeline observer")
                .rx_pipeline,
        );
        let service = AccessPointControl::new(
            receive,
            protocol_rx,
            mac,
            rx_frame,
            tx_frame,
            rx_dispatcher,
            rx_block_ack,
            rx_reorder,
            rx_reorder_storage,
            #[cfg(feature = "diagnostics")]
            observation_storage,
        );
        #[cfg(feature = "diagnostics")]
        let service = service.with_terminal_observer(begin_access_point_observation());
        diagnostics_event!("open-radio: AP prepare complete");
        Ok(ProductionAccessPointTask {
            channel,
            owner: materialized.owner,
            registers: materialized.registers,
            interrupts: materialized.interrupts,
            service,
            aggregate,
            parked: ProductionAccessPointParked {
                tx_storage,
                dma,
                tx_epoch,
                station: ProductionStationRoleResources {
                    scan_table,
                    scan_frame,
                    ethernet,
                    resume,
                    board,
                    station_address,
                },
                monitor,
                aggregate_tx: None,
            },
        })
    }

    pub(super) fn finish_access_point_task(
        &self,
        task: ProductionAccessPointTask,
    ) -> Result<ProductionSupervisorStopped, ProductionWifiFault> {
        let ProductionAccessPointTask {
            channel,
            owner,
            mut registers,
            interrupts,
            service,
            aggregate,
            parked,
        } = task;
        let stopped = match service.try_finish(&mut registers) {
            Ok(stopped) => stopped,
            Err(service) => {
                return Err(ProductionWifiFault::AccessPointRuntime {
                    _task: ProductionAccessPointTask {
                        channel,
                        owner,
                        registers,
                        interrupts,
                        service,
                        aggregate,
                        parked,
                    },
                });
            }
        };
        let ProductionAccessPointParked {
            tx_storage,
            dma,
            tx_epoch,
            mut station,
            monitor,
            aggregate_tx: parked_aggregate,
        } = parked;
        debug_assert!(parked_aggregate.is_none());
        let aggregate_tx = match aggregate.try_into_resources() {
            Ok(resources) => resources,
            Err(aggregate) => {
                return Err(ProductionWifiFault::AccessPointTeardown {
                    _fault: ProductionAccessPointTeardownFault::Aggregate {
                        _owner: owner,
                        _registers: registers,
                        _interrupts: interrupts,
                        _stopped: stopped,
                        _aggregate: aggregate,
                        _parked: ProductionAccessPointParked {
                            tx_storage,
                            dma,
                            tx_epoch,
                            station,
                            monitor,
                            aggregate_tx: None,
                        },
                    },
                });
            }
        };
        // The AP consumer must release its endpoint before the same affine
        // producer is detached from this role and restored to the station
        // resume frontier.
        drop(stopped.protocol_rx);
        let (ring, retained_rx) = match stopped.receive.try_into_live_epoch_parts() {
            Ok(parts) => parts,
            Err(_) => unreachable!("completed AP run returns a parked live staged-RX producer"),
        };
        station.resume.restore_retained_rx(retained_rx);
        if let Err((_error, returned_control)) = tx_epoch.restore_resources(stopped.transmit) {
            return Err(ProductionWifiFault::AccessPointTeardown {
                _fault: ProductionAccessPointTeardownFault::TxRestore {
                    _owner: owner,
                    _registers: registers,
                    _interrupts: interrupts,
                    _ring: ProductionRxRing::Live(ring),
                    _storage: dma.storage(),
                    _rx_frame: stopped.rx_frame,
                    _tx_frame: stopped.tx_frame,
                    _data_rx: stopped.data_rx,
                    _rx_block_ack: stopped.rx_block_ack,
                    _rx_reorder: stopped.rx_reorder,
                    _rx_reorder_storage: stopped.rx_reorder_storage,
                    #[cfg(feature = "diagnostics")]
                    _observation_storage: stopped.observation_storage,
                    _engine: stopped.engine,
                    _parked: ProductionAccessPointParked {
                        tx_storage,
                        dma,
                        tx_epoch,
                        station,
                        monitor,
                        aggregate_tx: Some(aggregate_tx),
                    },
                    _returned_control: returned_control,
                },
            });
        }
        // No fallible AP owner transition remains after TX restoration.
        // Park the radio frontier before constructing the independent role
        // resource aggregates, so the live IRQ/register owner cannot overlap
        // their terminal assembly in this machine frame.
        let wifi = park_production_wifi(owner, registers, interrupts);
        let oer_esp32s31_ieee80211_ap::engine::ApEngineStop {
            service,
            beacon_storage,
            pairwise_storage,
            security: _,
        } = stopped.engine;
        let address = service.address();
        let peer_storage = service.into_peer_storage();
        let access_point = ProductionAccessPointResources {
            tx_storage,
            address,
            beacon: beacon_storage,
            rx_frame: stopped.rx_frame,
            tx_frame: stopped.tx_frame,
            peer_storage,
            pairwise_storage,
            rx_dispatcher: stopped.data_rx,
            rx_block_ack: stopped.rx_block_ack,
            rx_reorder: stopped.rx_reorder,
            rx_reorder_storage: stopped.rx_reorder_storage,
            #[cfg(feature = "diagnostics")]
            observation_storage: stopped.observation_storage,
        };
        let physical = ProductionWifiPhysicalResources {
            dma,
            rx_ring: Some(ProductionRxRing::Live(ring)),
            tx: ProductionOrdinaryTxResources::Epoch(tx_epoch),
            aggregate_tx,
        };
        Ok(WifiSupervisorStopped::new(
            wifi,
            physical,
            station,
            access_point,
            monitor,
        ))
    }
}

pub(super) async fn wait_for_active_wifi_role_stop(
    endpoint: &mut WifiSupervisorEndpoint<'_, CriticalSectionRawMutex, RadioError>,
) {
    loop {
        match endpoint.receive().await {
            WifiSupervisorCommand::Stop => return,
            WifiSupervisorCommand::RestartRadio => {
                endpoint
                    .respond(WifiSupervisorResponse::RestartRadio(Err(
                        RadioError::RoleActive(WifiStartKind::WholeRadioRestart),
                    )))
                    .await;
            }
            WifiSupervisorCommand::Scan(request) => {
                endpoint
                    .respond(WifiSupervisorResponse::Scan(Err(
                        WifiScanFailure::Rejected {
                            request,
                            error: RadioError::RoleActive(WifiStartKind::StandaloneScan),
                        },
                    )))
                    .await;
            }
            WifiSupervisorCommand::StartStation(request) => {
                endpoint
                    .respond(WifiSupervisorResponse::Station(Err(
                        WifiStartFailure::rejected(
                            request,
                            RadioError::RoleActive(WifiStartKind::Station),
                        ),
                    )))
                    .await;
            }
            WifiSupervisorCommand::StartAccessPoint(request) => {
                endpoint
                    .respond(WifiSupervisorResponse::AccessPoint(Err(
                        WifiStartFailure::rejected(
                            request,
                            RadioError::RoleActive(WifiStartKind::AccessPoint),
                        ),
                    )))
                    .await;
            }
            WifiSupervisorCommand::StartStationAccessPoint(request) => {
                endpoint
                    .respond(WifiSupervisorResponse::StationAccessPoint(Err(
                        WifiStartFailure::rejected(
                            request,
                            RadioError::RoleActive(WifiStartKind::StationAccessPoint),
                        ),
                    )))
                    .await;
            }
            WifiSupervisorCommand::StartMonitor(request) => {
                endpoint
                    .respond(WifiSupervisorResponse::Monitor(Err(
                        WifiStartFailure::rejected(
                            request,
                            RadioError::RoleActive(WifiStartKind::StandaloneMonitor),
                        ),
                    )))
                    .await;
            }
        }
    }
}
impl ProductionWifiEpochRunner {
    pub(super) async fn run_access_point_service(
        &mut self,
        endpoint: &mut WifiSupervisorEndpoint<'_, CriticalSectionRawMutex, RadioError>,
        stopped: ProductionSupervisorStopped,
        request: AccessPointRequest,
        generation: oer_radio::wifi::RadioSubsystemGeneration,
    ) -> WifiRoleEpochOutcome<ProductionSupervisorStopped, ProductionWifiFault> {
        let (wifi, physical, station, access_point, monitor) = stopped.into_parts();
        let mut task = match await_stack_boundary!(self.prepare_access_point_task(
            wifi,
            physical,
            station,
            access_point,
            monitor,
            request,
        )) {
            Ok(task) => task,
            Err(fault) => {
                let faulted = ProductionWifiFault::AccessPointPreparation { _fault: fault };
                endpoint
                    .respond(WifiSupervisorResponse::AccessPoint(Err(
                        WifiStartFailure::faulted(self.fault_error(&faulted)),
                    )))
                    .await;
                return WifiRoleEpochOutcome::Faulted(faulted);
            }
        };
        // Only a successfully prepared fresh role reaches this edge. A failed
        // detach quarantines its board resources and cannot reset this ledger.
        if let Some(airtime) = task.parked.station.board.access_point_airtime.as_mut() {
            airtime.storage = oer_esp32s31_ieee80211_runtime::roles::access_point::network_tx::AccessPointAirtimeStorage::new(airtime.configuration.quantum_micros).with_observer(airtime.configuration.observer);
        }
        let mut started = false;
        #[cfg(feature = "diagnostics")]
        let rx_statistics_before = task.registers.receive_statistics_snapshot();
        #[cfg(feature = "diagnostics")]
        let tx_statistics_before = task.registers.transmit_statistics_snapshot();
        #[cfg(feature = "diagnostics")]
        let rx_policy_before = task.registers.access_point_receive_policy_snapshot();
        #[cfg(feature = "diagnostics")]
        let rx_delivery_observer = task
            .parked
            .station
            .board
            .diagnostics
            .and_then(|hooks| hooks.rx_delivery);
        let result = {
            let network = task.parked.station.resume.radio_runner_mut();
            let platform = task.owner.platform_mut();
            await_stack_boundary!(
                task.service.run_until_stopped(
                    &mut task.registers,
                    &mut task.interrupts,
                    &*platform,
                    network,
                    &mut task.aggregate,
                    &mut *task.parked.tx_storage,
                    task.parked.station.board.access_point_airtime.as_mut().map(|airtime| (&mut airtime.storage, airtime.configuration)),
                    #[cfg(feature = "diagnostics")]
                    task.parked
                        .station
                        .board
                        .diagnostics
                        .map(|hooks| hooks.aggregate_tx),
                    #[cfg(feature = "diagnostics")]
                    rx_delivery_observer,
                    #[cfg(feature = "diagnostics")]
                    |hardware| {
                        let priority = hardware.coex_priority_snapshot();
                        diagnostics_event!(
                            "open-radio: access-point live COEX priority rx_active={} rx_ack={} wifi_default={}",
                            priority.rx_active,
                            priority.rx_ack,
                            priority.wifi_default,
                        );
                        for index in 0..8 {
                            if let Some(snapshot) = hardware.rx_block_ack_entry_snapshot(index)
                                && snapshot.write_enabled
                            {
                                diagnostics_event!(
                                    "open-radio: access-point live RX BA bank={} enabled={} tid={} write={} valid={} control_clean={} peer={:02x?} interface={:?} window={} current={} loaded_start={} bitmap_status={:016x} bitmap_load={:016x}",
                                    index,
                                    snapshot.enabled,
                                    snapshot.tid,
                                    snapshot.write_enabled,
                                    snapshot.valid,
                                    snapshot.control_unknown_clear,
                                    snapshot.peer,
                                    snapshot.interface,
                                    snapshot.window,
                                    snapshot.current_sequence,
                                    snapshot.loaded_start_sequence,
                                    snapshot.bitmap_status,
                                    snapshot.bitmap_load,
                                );
                            }
                        }
                    },
                    || async {
                        // The runtime constructs this future only after RX IRQ
                        // activation and the first beacon publication succeed.
                        endpoint.respond(WifiSupervisorResponse::AccessPoint(Ok(
                            WifiStartReport::new(generation),
                        ))).await;
                        started = true;
                        wait_for_active_wifi_role_stop(endpoint).await;
                    },
                    |status| {
                        crate::status::publish_access_point_status(generation, status);
                    },
                    || {
                        let mut nonce = [0_u8; 32];
                        for word in nonce.chunks_exact_mut(4) {
                            word.copy_from_slice(&self.trng.random().to_le_bytes());
                        }
                        let replay =
                            (u64::from(self.trng.random()) << 32) | u64::from(self.trng.random());
                        (nonce, replay)
                    },
                    oer_time_embassy::EmbassyClock,
                )
            )
        };
        crate::status::publish_access_point_stopped();
        #[cfg(feature = "diagnostics")]
        if let Err(error) = &result {
            // Publish the typed owner failure before the larger terminal RX
            // snapshot. A saturated AP can fill the bounded diagnostic log;
            // the cause must not be displaced by secondary state evidence.
            log::error!("open-radio: access-point runtime fault: {error:?}");
        }
        #[cfg(feature = "diagnostics")]
        {
            // A rare repeated STA/AP lifecycle failure leaves the AP receive
            // path after exactly one completed descriptor. Capture the
            // hardware-owned frontier before teardown republishes the ring;
            // production images compile this one-shot diagnostic out.
            let rx_dma = task.registers.receive_dma_snapshot();
            let rx_statistics_after = task.registers.receive_statistics_snapshot();
            let tx_delta = task
                .registers
                .transmit_statistics_snapshot()
                .wrapping_delta_since(tx_statistics_before);
            let rx_delta = rx_statistics_after
                .primary
                .wrapping_delta_since(rx_statistics_before.primary);
            let rx_decode_delta = rx_statistics_after
                .decode_errors
                .wrapping_delta_since(rx_statistics_before.decode_errors);
            let rx_hang_delta = rx_statistics_after
                .hang
                .wrapping_delta_since(rx_statistics_before.hang);
            let rx_policy_after = task.registers.access_point_receive_policy_snapshot();
            let rx_match_after = task.registers.he_trigger_receive_diagnostics();
            // Keep the compact discriminating counters ahead of descriptor
            // and register dumps.  Saturated AP diagnostics use a bounded
            // logger; placing these observations at the tail silently lost
            // the only TX/RX-response correlation in precisely the overload
            // runs for which it is needed.
            diagnostics_event!(
                "open-radio: access-point TX hardware delta txrts={} txcts={} track={} trcts={}",
                tx_delta.tx_rts,
                tx_delta.tx_cts,
                tx_delta.track,
                tx_delta.trcts,
            );
            diagnostics_event!(
                "open-radio: access-point RX hardware faults buffer_full={} fifo_overflow={} tkip={} bt_block={} freq_hop={} last_unmatched={} ack_irq={} rts_irq={}",
                rx_delta.buffer_full,
                rx_delta.fifo_overflow,
                rx_delta.tkip_error,
                rx_delta.bt_block_error,
                rx_delta.frequency_hop_error,
                rx_delta.last_unmatched_error,
                rx_delta.ack_interrupt,
                rx_delta.rts_interrupt,
            );
            for (edge, policy) in [("before", rx_policy_before), ("after", rx_policy_after)] {
                diagnostics_event!(
                    "open-radio: access-point RX policy edge={} bssid={:02x?} address_check={} soft_ap={} enabled={}",
                    edge,
                    policy.bssid,
                    policy.bssid_address_check_enabled,
                    policy.interface_is_soft_ap,
                    policy.interface_rx_policy_enabled,
                );
            }
            diagnostics_event!(
                "open-radio: access-point RX match ax_bssid1={} ax_bssid0={} color_valid={} ampdu_auto_ack_valid={}",
                rx_match_after.ax_match_bssid1,
                rx_match_after.ax_match_bssid0,
                rx_match_after.bss_color_valid,
                rx_match_after.rx_ampdu_auto_ack_valid,
            );
            diagnostics_event!(
                "open-radio: access-point RX decode delta brx_agc={} brx={} nrx={} nrx_abort={} nrx_agc_exit={} nrx_baseband_off={} nrx_fdm_watchdog={} nrx_restart={}",
                rx_decode_delta.brx_agc,
                rx_decode_delta.brx,
                rx_decode_delta.nrx,
                rx_decode_delta.nrx_abort,
                rx_decode_delta.nrx_agc_exit,
                rx_decode_delta.nrx_baseband_off,
                rx_decode_delta.nrx_fdm_watchdog,
                rx_decode_delta.nrx_restart,
            );
            diagnostics_event!(
                "open-radio: access-point RX decode format nrx_service={} nrx_tx_over={} nrx_unsupported={} nrx_he_format={} nrx_ht_sig={} nrx_he_unsupported={} nrx_he_sig_a_crc={}",
                rx_decode_delta.nrx_service,
                rx_decode_delta.nrx_tx_over,
                rx_decode_delta.nrx_unsupported,
                rx_decode_delta.nrx_he_format,
                rx_decode_delta.nrx_ht_sig,
                rx_decode_delta.nrx_he_unsupported,
                rx_decode_delta.nrx_he_sig_a_crc,
            );
            diagnostics_event!(
                "open-radio: access-point RX hang delta rx={} tx={} rx_tx_hang={} rx_tx_panic={}",
                rx_hang_delta.rx,
                rx_hang_delta.tx,
                rx_hang_delta.rx_tx_hang,
                rx_hang_delta.rx_tx_panic,
            );
            for index in 0..8 {
                if let Some(snapshot) = task.registers.rx_block_ack_entry_snapshot(index)
                    && snapshot.write_enabled
                {
                    diagnostics_event!(
                        "open-radio: access-point RX BA bank={} enabled={} tid={} write={} valid={} control_clean={} peer={:02x?} interface={:?} window={} current={} loaded_start={} bitmap_status={:016x} bitmap_load={:016x}",
                        index,
                        snapshot.enabled,
                        snapshot.tid,
                        snapshot.write_enabled,
                        snapshot.valid,
                        snapshot.control_unknown_clear,
                        snapshot.peer,
                        snapshot.interface,
                        snapshot.window,
                        snapshot.current_sequence,
                        snapshot.loaded_start_sequence,
                        snapshot.bitmap_status,
                        snapshot.bitmap_load,
                    );
                }
            }
            let rx_head = task.service.rx_descriptor_snapshot(0);
            let rx_second = task.service.rx_descriptor_snapshot(1);
            let rx_tail = task
                .service
                .rx_descriptor_snapshot(RX_DESCRIPTOR_COUNT.saturating_sub(1));
            let descriptor_base_low = rx_head.map(|descriptor| descriptor.address & 0x000f_ffff);
            let descriptor_index = |low: u32| {
                let offset = low.checked_sub(descriptor_base_low?)?;
                // ESP32-S31 Wi-Fi DMA descriptors are exactly three words.
                (offset % 12 == 0)
                    .then(|| usize::try_from(offset / 12).ok())
                    .flatten()
                    .filter(|index| *index < RX_DESCRIPTOR_COUNT)
            };
            let rx_base = descriptor_index(rx_dma.descriptor_base & 0x000f_ffff)
                .and_then(|index| task.service.rx_descriptor_snapshot(index));
            let rx_next = descriptor_index(rx_dma.next_descriptor_low)
                .and_then(|index| task.service.rx_descriptor_snapshot(index));
            let rx_last = descriptor_index(rx_dma.last_descriptor_low)
                .and_then(|index| task.service.rx_descriptor_snapshot(index));
            diagnostics_event!(
                "open-radio: access-point RX DMA stop walker={} reload={} base={:#010x} next={:#07x} last={:#07x}",
                rx_dma.walker_enabled,
                rx_dma.reload_pending,
                rx_dma.descriptor_base,
                rx_dma.next_descriptor_low,
                rx_dma.last_descriptor_low,
            );
            for (position, descriptor) in [
                ("head", rx_head),
                ("second", rx_second),
                ("tail", rx_tail),
                ("hardware-base", rx_base),
                ("hardware-next", rx_next),
                ("hardware-last", rx_last),
            ] {
                match descriptor {
                    Some(descriptor) => diagnostics_event!(
                        "open-radio: access-point RX descriptor position={} index={} address={:#010x} word0={:#010x} buffer={:#010x} next={:#010x}",
                        position,
                        descriptor.index,
                        descriptor.address,
                        descriptor.word0,
                        descriptor.buffer_address,
                        descriptor.next_address,
                    ),
                    None => diagnostics_event!(
                        "open-radio: access-point RX descriptor position={} unavailable",
                        position
                    ),
                }
            }
            diagnostics_event!(
                "open-radio: access-point RX hardware delta mpdu={} data={} other_unicast={} fcs={} abort={} abort_fcs_pass={} power_drop={} he_sig_b={} same_bm={} signal_field={} end={}",
                rx_delta.mpdu_count,
                rx_delta.data_success,
                rx_delta.other_unicast,
                rx_delta.fcs_error,
                rx_delta.abort,
                rx_delta.abort_fcs_pass,
                rx_delta.power_drop_error,
                rx_delta.he_sig_b_error,
                rx_delta.same_bm_error,
                rx_delta.signal_field,
                rx_delta.end,
            );
            store_access_point_rx_hardware_observation(crate::DiagnosticRxStatistics::from_deltas(
                rx_delta,
                rx_decode_delta,
                rx_hang_delta,
            ));
        };
        #[cfg(feature = "diagnostics")]
        if let Ok(report) = &result {
            if let Some(scheduler) = report.rx_scheduler {
                diagnostics_event!(
                    "open-radio: access-point RX scheduler stop recycle_start={} accepted_tail={} observed={:?}",
                    scheduler.recycle_start,
                    scheduler.accepted_tail,
                    scheduler.observed_mask,
                );
                let topology = scheduler.topology;
                diagnostics_event!(
                    "open-radio: access-point RX topology stop base={:#010x} start={} head={:#010x} head_next={:#010x} tail_index={} tail={:#010x} visited={} terminal={} valid={}",
                    topology.descriptor_base,
                    topology.start_index,
                    topology.head_address,
                    topology.head_next_address,
                    topology.tail_index,
                    topology.tail_address,
                    topology.visited_descriptors,
                    topology.terminal_descriptors,
                    topology.valid,
                );
            } else {
                diagnostics_event!("open-radio: access-point RX scheduler stop unavailable");
            }
        }
        if result.is_err() {
            let faulted = ProductionWifiFault::AccessPointRuntime { _task: task };
            let error = self.fault_error(&faulted);
            let response = if started {
                WifiSupervisorResponse::Stop(Err(error))
            } else {
                WifiSupervisorResponse::AccessPoint(Err(WifiStartFailure::faulted(error)))
            };
            endpoint.respond(response).await;
            return WifiRoleEpochOutcome::Faulted(faulted);
        }
        #[cfg(feature = "diagnostics")]
        let diagnostic_destination = (task.parked.station.board.diagnostics, task.channel);
        let stopped = match self.finish_access_point_task(task) {
            Ok(stopped) => stopped,
            Err(faulted) => {
                endpoint
                    .respond(WifiSupervisorResponse::Stop(
                        Err(self.fault_error(&faulted)),
                    ))
                    .await;
                return WifiRoleEpochOutcome::Faulted(faulted);
            }
        };
        #[cfg(feature = "diagnostics")]
        if let (Some(hooks), channel) = diagnostic_destination {
            // `finish_access_point_task` emits the terminal protocol/MAC
            // observation. The larger register snapshot stays in its static
            // value slot, so only this small destination crosses the affine
            // owner-return boundary.
            publish_stored_access_point_observation(hooks.access_point, channel);
        }
        endpoint
            .respond(WifiSupervisorResponse::Stop(Ok(WifiStopReport::new(
                generation,
            ))))
            .await;
        WifiRoleEpochOutcome::Stopped(stopped)
    }
}
