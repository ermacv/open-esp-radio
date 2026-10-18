#![expect(
    clippy::large_enum_variant,
    reason = "no-alloc paired frontier retains the complete connected or stopped owner graph"
)]
#![expect(
    clippy::too_many_arguments,
    reason = "the paired active transaction exposes each independent role, endpoint, and generation owner"
)]

//! Same-channel production STA+AP lifecycle transaction.
//!
//! This module is the only composition root allowed to turn a connected
//! station frontier into the paired DATAPATH graph. It must retain one physical
//! RX producer, one physical TX owner and one MAC interrupt epoch, then return
//! every owner through the ordinary stopped supervisor frontier.

use core::future::ready;

use crate::{
    interrupts::IRQ_RUNTIME,
    resources::profile::{
        ESP32S31_DEFAULT_RX_REORDER_WINDOW as RX_REORDER_WINDOW,
        ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT as TX_AMPDU_FRAME_COUNT,
    },
    supervisor::station::{
        ConnectedStationReplaySetupFailure, RX_REORDER_COMMANDS, RX_REORDER_STORAGE, RX_STAGE_POOL,
        STA_AP_STAGED_RX_QUEUE, STA_CCMP_RX_REPLAY, STAGED_RX_QUEUE,
    },
};

use embassy_time::Timer;
use oer_time_embassy::EmbassyClock;

use oer_radio::wifi::{
    StationAccessPointRequest, StationRequest, StationScanChannels, StationScanPolicy,
};

use oer_esp32s31_ieee80211::datapath::lifecycle::{
    StaApLifecycle, StaApReceiveIdentities, apply_sta_ap_register_action, sta_ap_register_action,
};

use oer_esp32s31_ieee80211_ap::protocol::AccessPointServiceStatus;

use oer_esp32s31_ieee80211_runtime::{
    datapath::{
        DatapathRunnerExit,
        network::{DatapathNetwork, DatapathNetworkLink},
        paired::DatapathPairRole,
        rx::dma::{RxEpochResources, StagedRxEpoch},
        services::SingleRoleServices,
    },
    roles::{
        access_point::{
            AccessPointAmpdu, AccessPointProtocolProcessor, AccessPointRoleRuntime,
            access_point_network_link_state, finish_sta_ap_access_point_role,
            network_tx::AccessPointNetworkTx, park_sta_ap_access_point_role,
        },
        concurrent::{
            StaApControlExit, StaApRxConsumer, StaApRxService, StaApStationRxSink,
            compose_sta_ap_datapath_runner, compose_sta_ap_datapath_services,
        },
        station::{
            connected::{
                AlreadyParkedRx, ConnectedEpochResources, ConnectedEpochStarted,
                ConnectedNetworkStartedParts, ConnectedStaControlResources,
                ConnectedStaDriverParts, ConnectedStaGroupSecurity, ConnectedStaNetworkTxDomain,
                ConnectedStaPort, ConnectedStaRxProcessorResources, ConnectedStaTeardownPort,
                ConnectedStaTxResources, ConnectedWpa2Security, activate_esp32s31_connected_epoch,
                prepare_esp32s31_connected_service, start_esp32s31_initial_connected_epoch,
                start_esp32s31_reconnected_connected_epoch,
            },
            runtime::{StaApStationPrepared, finish_sta_ap_station, prepare_sta_ap_station},
        },
    },
};

use oer_esp32s31_ieee80211_esp_hal::mac_interrupt_epoch::{
    prepare_active_connected_sta_without_power_save,
    prepare_active_connected_sta_without_power_save_with_access,
};

use oer_esp32s31_ieee80211_mac::sta_ap_registers::{
    disable_access_point_receive_registers, disable_station_receive_registers,
};

use oer_esp32s31_ieee80211_sta::{
    attempt::{StaAttemptSecurityMaterial, StaInstalledSecurity},
    single_mpdu_tx::ConnectedTxSecurity,
};

use oer_ieee80211_mac::vif::StaApRxAddresses;

use oer_ieee80211_runtime::station_network::RunningStationNetwork;

use super::*;

/// Result of advancing the finite station lifecycle to its paired cutover
/// edge. A normal pre-connect terminal edge remains reusable; a hardware
/// contradiction remains quarantined in the existing production fault type.
enum ProductionPairedStationFrontier {
    Connected {
        resources: ConnectedStationResources<'static, 'static>,
        station: StaAttemptStation,
        access_point: ProductionAccessPointResources,
        monitor: ProductionMonitorResources,
    },
    NotConnected(ProductionSupervisorStopped),
}

fn constrain_station_to_paired_channel(
    request: StationRequest,
    channel: WifiChannel,
) -> StationRequest {
    let (discovery, security, reconnect, power_mode) = request.into_parts();
    let scan = discovery.scan();
    let channels = StationScanChannels::from_primary_channels(&[channel.primary()])
        .expect("validated 2.4-GHz AP channel is a valid station scan channel");
    StationRequest::new(
        discovery.ssid(),
        security,
        reconnect,
        StationScanPolicy::new(channels, scan.dwell(), scan.association_preference()),
    )
    .with_power_mode(power_mode)
}

impl ProductionWifiEpochRunner {
    async fn advance_station_to_paired_frontier(
        &self,
        stopped: ProductionSupervisorStopped,
        request: StationRequest,
    ) -> Result<ProductionPairedStationFrontier, ProductionWifiFault> {
        let (_control, mut task) =
            self.prepare_station_task(stopped, request, ProductionStationMode::PairedCutover)?;
        let exit = await_stack_boundary!(task.run());
        let resources = match exit {
            StationExit::Stopped {
                resources,
                reason: oer_esp32s31_ieee80211_runtime::roles::station::StationStopReason::Backend,
                ..
            } => resources,
            StationExit::Stopped { resources, .. }
            | StationExit::RetryExhausted { resources, .. }
            | StationExit::Terminal { resources, .. } => {
                return restore_production_station_frontier(resources)
                    .map(ProductionPairedStationFrontier::NotConnected);
            }
            StationExit::Faulted { fault, runner, .. } => {
                return Err(ProductionWifiFault::Station {
                    _fault: fault,
                    _runner: runner,
                });
            }
        };
        let (owner, runner) = resources.into_parts();
        let (runtime, phase, security) = owner.into_parts();
        let ProductionStationPhase::Connected { connected } = phase else {
            return Err(ProductionWifiFault::PairedStationPhase {
                _owner: ProductionStationOwner::new(runtime, phase, security),
                _runner: runner,
            });
        };
        let ProductionConnectedPhase {
            epoch,
            network,
            station,
            peer,
            installed_security,
        } = connected;
        let (access_point, monitor) = runner.into_port().into_parked_roles();
        let interface = runtime.board().interface;
        Ok(ProductionPairedStationFrontier::Connected {
            resources: ConnectedStationResources::new(
                runtime,
                epoch,
                network,
                interface,
                // Same-radio SoftAP cannot remain available while the STA
                // sleeps. Association keeps the requested listen interval,
                // and paired operation runs no station power management.
                connected_config(StationPowerMode::None),
                peer,
                installed_security,
                security,
            ),
            station,
            access_point,
            monitor,
        })
    }

    /// Run the one production same-channel STA+AP epoch.
    ///
    /// The published simultaneous-STA+AP capability is backed by this exact
    /// production transaction and its inverse. HIL exercises this owner graph,
    /// not a shadow composition.
    pub(super) async fn run_station_access_point_service(
        &mut self,
        endpoint: &mut WifiSupervisorEndpoint<'_, CriticalSectionRawMutex, RadioError>,
        stopped: ProductionSupervisorStopped,
        request: StationAccessPointRequest,
        generation: oer_radio::wifi::RadioSubsystemGeneration,
    ) -> WifiRoleEpochOutcome<ProductionSupervisorStopped, ProductionWifiFault> {
        let (station_request, access_point_request) = request.into_parts();
        let station_request =
            constrain_station_to_paired_channel(station_request, access_point_request.channel());
        let frontier = match await_stack_boundary!(
            self.advance_station_to_paired_frontier(stopped, station_request)
        ) {
            Ok(frontier) => frontier,
            Err(faulted) => {
                endpoint
                    .respond(WifiSupervisorResponse::StationAccessPoint(Err(
                        WifiStartFailure::faulted(self.fault_error(&faulted)),
                    )))
                    .await;
                return WifiRoleEpochOutcome::Faulted(faulted);
            }
        };
        let ProductionPairedStationFrontier::Connected {
            resources,
            station,
            access_point,
            monitor,
        } = frontier
        else {
            let ProductionPairedStationFrontier::NotConnected(stopped) = frontier else {
                unreachable!("paired station frontier has two exhaustive states")
            };
            endpoint
                .respond(WifiSupervisorResponse::StationAccessPoint(Err(
                    WifiStartFailure::faulted(RadioError::HardwareFault),
                )))
                .await;
            diagnostics_event!("open-radio: paired cutover failed before connected frontier");
            return WifiRoleEpochOutcome::NotStarted(stopped);
        };

        let parts = resources.into_parts();
        let prepared = match prepare_esp32s31_connected_service::<
            TX_AMPDU_FRAME_COUNT,
            RX_REORDER_WINDOW,
            _,
            _,
            _,
        >(
            oer_esp32s31_ieee80211_runtime::roles::station::connected::ConnectedServiceResources::new(
                parts.runtime,
                parts.epoch,
                parts.network,
                parts.interface,
                parts.config,
                parts.peer,
                parts.installed_security,
                parts.security,
            ),
        ) {
            Ok(prepared) => prepared,
            Err(failure) => {
                // The station lifecycle used this same validated config to
                // produce the connected frontier. A contradiction here is a
                // non-reusable software invariant, not a recoverable request.
                endpoint
                    .respond(WifiSupervisorResponse::StationAccessPoint(Err(
                        WifiStartFailure::faulted(RadioError::HardwareFault),
                    )))
                    .await;
                diagnostics_event!(
                    "open-radio: paired cutover rejected connected service policy: {:?}",
                    failure.error
                );
                let error = failure.error;
                return WifiRoleEpochOutcome::Faulted(
                    ProductionWifiFault::PairedConnected {
                        _fault: ConnectedStationFault::InvalidConnectedPolicy {
                            _resources: failure.into_resources(),
                            _error: error,
                        },
                        _station: station,
                        _access_point: access_point,
                        _monitor: monitor,
                    },
                );
            }
        };
        let mut started = prepared.start_network(|_runtime, (), _plan| ((), ()));
        let station_channel = station.selected_channel().unwrap_or_else(|_| {
            let (runtime, _) = started.runtime_and_epoch_mut();
            runtime.radio().owner().current_channel()
        });
        diagnostics_event!(
            "open-radio: paired channel station={station_channel:?} requested={:?}",
            access_point_request.channel()
        );
        if station_channel != access_point_request.channel() {
            endpoint
                .respond(WifiSupervisorResponse::StationAccessPoint(Err(
                    WifiStartFailure::faulted(RadioError::Planning(
                        WifiServicePlanningError::Request(
                            oer_radio::wifi::WifiServiceRequestError::StationAccessPointChannelMismatch,
                        ),
                    )),
                )))
                .await;
            diagnostics_event!("open-radio: paired cutover channel mismatch");
            return WifiRoleEpochOutcome::Faulted(ProductionWifiFault::PairedChannelMismatch {
                _started: started,
                _station: station,
                _access_point: access_point,
                _monitor: monitor,
            });
        }
        let activation = {
            let (runtime, epoch) = started.runtime_and_epoch_mut();
            let (radio, _storage, _board) = runtime.split_mut();
            let (owner, interrupt) = radio.parts_mut();
            let platform = owner.platform_mut();
            let prepared = if interrupt.is_active() {
                match epoch {
                    ConnectedEpochResources::Initial { hardware, .. } => {
                        prepare_active_connected_sta_without_power_save(hardware)
                    }
                    ConnectedEpochResources::Reconnected(reconnected) => {
                        let access = reconnected.hardware_mut().register_access();
                        prepare_active_connected_sta_without_power_save_with_access(&access)
                    }
                }
                .map_err(|_| ())
            } else {
                match interrupt.setup_mut() {
                    Ok(setup) => match epoch {
                        ConnectedEpochResources::Initial { hardware, .. } => {
                            Ok(setup.prepare_connected_sta_without_power_save(hardware))
                        }
                        ConnectedEpochResources::Reconnected(reconnected) => reconnected
                            .hardware_mut()
                            .register_access()
                            .try_prepare_connected_sta_without_power_save(setup)
                            .map_err(|_| ()),
                    },
                    Err(_) => Err(()),
                }
            };
            prepared.and_then(|prepared| {
                activate_esp32s31_connected_epoch(interrupt, platform, prepared).map_err(|_| ())
            })
        };
        if activation.is_err() {
            endpoint
                .respond(WifiSupervisorResponse::StationAccessPoint(Err(
                    WifiStartFailure::faulted(RadioError::HardwareFault),
                )))
                .await;
            return WifiRoleEpochOutcome::Faulted(ProductionWifiFault::PairedConnected {
                _fault: ConnectedStationFault::InterruptActivation { _started: started },
                _station: station,
                _access_point: access_point,
                _monitor: monitor,
            });
        }

        await_stack_boundary!(self.run_station_access_point_active(
            endpoint,
            started,
            station,
            access_point,
            monitor,
            access_point_request,
            station_channel,
            generation,
        ))
    }

    /// Own the active paired epoch after association, channel validation and
    /// interrupt activation. Keeping this as a separate poll boundary avoids
    /// retaining the large station-association transaction on the CPU stack
    /// while the long-running dual-interface DATAPATH graph is active.
    async fn run_station_access_point_active(
        &mut self,
        endpoint: &mut WifiSupervisorEndpoint<'_, CriticalSectionRawMutex, RadioError>,
        started: crate::supervisor::station::ConnectedNetworkStarted<'static, 'static>,
        station: StaAttemptStation,
        access_point: ProductionAccessPointResources,
        monitor: ProductionMonitorResources,
        access_point_request: AccessPointRequest,
        station_channel: oer_ieee80211_mac::channel::WifiChannel,
        generation: oer_radio::wifi::RadioSubsystemGeneration,
    ) -> WifiRoleEpochOutcome<ProductionSupervisorStopped, ProductionWifiFault> {
        let (installed_mode, material_mode) = started.security_modes();
        if installed_mode != material_mode {
            endpoint
                .respond(WifiSupervisorResponse::StationAccessPoint(Err(
                    WifiStartFailure::faulted(RadioError::HardwareFault),
                )))
                .await;
            return WifiRoleEpochOutcome::Faulted(ProductionWifiFault::PairedSecurityMismatch {
                _started: started,
                _station: station,
                _access_point: access_point,
                _monitor: monitor,
                _access_point_request: access_point_request,
            });
        }
        let ConnectedNetworkStartedParts {
            runtime,
            epoch,
            stack: (),
            network: network_runner,
            initial_network_task,
            mut plan,
            installed_security,
            security,
        } = started.into_parts();
        debug_assert!(initial_network_task.is_none());
        let _ = initial_network_task;
        let runtime = runtime.into_parts();
        let (mut role, mut interrupt_epoch) = runtime.radio.into_parts();
        role.set_current_channel(station_channel);
        let (dma, tx_storage, scan_table, frame, ethernet) = runtime.storage.into_parts();
        let mut board = runtime.board;

        let (start, standalone_receiver) = match epoch {
            ConnectedStationEpoch::Initial { hardware, receive } => {
                let (standalone_sender, standalone_receiver) = STAGED_RX_QUEUE.split();
                let initial = board
                    .initial_connected
                    .take()
                    .expect("paired initial connected frontier owns static resources");
                let rx = RxEpochResources::new(
                    dma.storage(),
                    &RX_STAGE_POOL,
                    standalone_sender,
                    EmbassyClock,
                );
                (
                    start_esp32s31_initial_connected_epoch(hardware, receive, initial.with_rx(rx))
                        .await,
                    Some(standalone_receiver),
                )
            }
            ConnectedStationEpoch::Reconnected(epoch) => (
                start_esp32s31_reconnected_connected_epoch(epoch).await,
                None,
            ),
        };
        let ConnectedEpochStarted {
            hardware,
            rx,
            aggregate_tx: aggregate,
            control: control_resources,
        } = match start {
            Ok(started) => started,
            Err(failure) => {
                endpoint
                    .respond(WifiSupervisorResponse::StationAccessPoint(Err(
                        WifiStartFailure::faulted(RadioError::HardwareFault),
                    )))
                    .await;
                return WifiRoleEpochOutcome::Faulted(ProductionWifiFault::PairedConnected {
                    _fault: ConnectedStationFault::EpochStart {
                        _runtime: production_station_runtime(
                            role,
                            interrupt_epoch,
                            dma,
                            tx_storage,
                            scan_table,
                            frame,
                            ethernet,
                            board,
                        ),
                        _failure: failure,
                        _stack: (),
                        _network: network_runner,
                        _initial_network_task: initial_network_task,
                        _plan: plan,
                        _installed_security: installed_security,
                        _security: security,
                    },
                    _station: station,
                    _access_point: access_point,
                    _monitor: monitor,
                });
            }
        };
        let standalone_receiver = standalone_receiver.unwrap_or_else(|| {
            rx.try_resume_standalone_receiver()
                .expect("reconnected paired cutover retains its standalone producer")
        });
        let control_tx = tx_storage
            .take_control()
            .unwrap_or_else(|_| unreachable!("paired cutover starts with idle ordinary TX"));
        let (sequences, mut station_security_material) = security.into_parts();
        let material_is_open =
            matches!(&station_security_material, StaAttemptSecurityMaterial::Open);
        let material_is_wpa2 = matches!(
            &station_security_material,
            StaAttemptSecurityMaterial::Personal { .. }
        );
        let (tx_security, mut group_security) = match installed_security {
            StaInstalledSecurity::Open if material_is_open => (
                ConnectedTxSecurity::Open,
                Some(ConnectedStaGroupSecurity::Open),
            ),
            StaInstalledSecurity::Wpa2Personal {
                pairwise,
                group,
                group_material,
                replay,
                management,
            } if material_is_wpa2 => {
                let (replay_rx, replay_control) = match STA_CCMP_RX_REPLAY.start(replay) {
                    Ok(endpoints) => endpoints,
                    Err(failure) => {
                        endpoint
                            .respond(WifiSupervisorResponse::StationAccessPoint(Err(
                                WifiStartFailure::faulted(RadioError::HardwareFault),
                            )))
                            .await;
                        return WifiRoleEpochOutcome::Faulted(
                            ProductionWifiFault::PairedConnected {
                                _fault: ConnectedStationFault::ReplaySetup {
                                    _runtime: production_station_runtime(
                                        role,
                                        interrupt_epoch,
                                        dma,
                                        tx_storage,
                                        scan_table,
                                        frame,
                                        ethernet,
                                        board,
                                    ),
                                    _started: ConnectedEpochStarted {
                                        hardware,
                                        rx,
                                        aggregate_tx: aggregate,
                                        control: control_resources,
                                    },
                                    _stack: (),
                                    _network: network_runner,
                                    _initial_network_task: initial_network_task,
                                    _plan: plan,
                                    _failure: ConnectedStationReplaySetupFailure::Start {
                                        _failure: failure,
                                        _pairwise: pairwise,
                                        _group: group,
                                        _group_material: group_material,
                                    },
                                    _sequences: sequences,
                                    _material: station_security_material,
                                    _control_tx: control_tx,
                                },
                                _station: station,
                                _access_point: access_point,
                                _monitor: monitor,
                            },
                        );
                    }
                };
                let tx_security = ConnectedTxSecurity::Wpa2Personal(pairwise);
                let group_security = ConnectedStaGroupSecurity::Wpa2PersonalRekey {
                    group,
                    material: group_material,
                    replay: replay_control,
                    management,
                };
                if let Err(failure) = plan.enable_ccmp_rx_replay(replay_rx) {
                    endpoint
                        .respond(WifiSupervisorResponse::StationAccessPoint(Err(
                            WifiStartFailure::faulted(RadioError::HardwareFault),
                        )))
                        .await;
                    return WifiRoleEpochOutcome::Faulted(ProductionWifiFault::PairedConnected {
                        _fault: ConnectedStationFault::ReplaySetup {
                            _runtime: production_station_runtime(
                                role,
                                interrupt_epoch,
                                dma,
                                tx_storage,
                                scan_table,
                                frame,
                                ethernet,
                                board,
                            ),
                            _started: ConnectedEpochStarted {
                                hardware,
                                rx,
                                aggregate_tx: aggregate,
                                control: control_resources,
                            },
                            _stack: (),
                            _network: network_runner,
                            _initial_network_task: initial_network_task,
                            _plan: plan,
                            _failure: ConnectedStationReplaySetupFailure::Plan {
                                _failure: failure,
                                _tx_security: tx_security,
                                _group_security: group_security,
                            },
                            _sequences: sequences,
                            _material: station_security_material,
                            _control_tx: control_tx,
                        },
                        _station: station,
                        _access_point: access_point,
                        _monitor: monitor,
                    });
                }
                (tx_security, Some(group_security))
            }
            _ => unreachable!("paired security modes were validated before owner split"),
        };
        drop(standalone_receiver);
        let (paired_sender, paired_receiver) = STA_AP_STAGED_RX_QUEUE.split();
        let paired_consumer = StaApRxConsumer::new(paired_receiver);
        let receive_identities = StaApReceiveIdentities {
            station_address: plan.link().station_address,
            station_bssid: plan.link().bssid,
            access_point_address: access_point.address,
        };
        let paired_rx = StagedRxEpoch::try_from_live_sta_ap(
            rx,
            paired_sender,
            plan.rx_config().ingress,
            StaApRxAddresses {
                station: receive_identities.station_address,
                station_bssid: receive_identities.station_bssid,
                access_point: receive_identities.access_point_address,
            },
        )
        .unwrap_or_else(|_| unreachable!("new connected RX has no queued standalone leases"));
        let common_rx = StaApRxService::new(paired_rx, paired_consumer);

        let ProductionStationBoardResources {
            access_point_airtime,
            interface,
            connected_datapath,
            rx_protocol_runtime,
            sta_ap_rx_batch,
            initial_connected,
            #[cfg(feature = "diagnostics")]
            diagnostics,
        } = board;
        let super::station::ConnectedControlStatics {
            queue: control_resources,
            slot: control_slot,
        } = control_resources;
        let (control_publisher, control_receiver) = control_resources.split();
        let station_sink = StaApStationRxSink::new(sta_ap_rx_batch, control_publisher);
        let (reorder_sender, reorder_receiver) = RX_REORDER_COMMANDS.split();
        let station_rx = ConnectedStaPort::build_rx_processor(
            &mut plan,
            ConnectedStaRxProcessorResources {
                irq: &IRQ_RUNTIME,
                sink: station_sink,
                mpdu: frame,
                ethernet,
                reorder_commands: reorder_receiver,
                reorder_storage: &RX_REORDER_STORAGE,
                runtime: rx_protocol_runtime,
                reorder_scratch: None,
                clock: EmbassyClock,
                #[cfg(feature = "diagnostics")]
                pipeline_observer: diagnostics.and_then(|hooks| hooks.rx_pipeline),
                #[cfg(feature = "diagnostics")]
                reorder_observer: diagnostics.and_then(|hooks| hooks.rx_reorder),
            },
        );
        let station_tx = ConnectedStaPort::build_tx(
            &mut plan,
            ConnectedStaTxResources {
                control: control_tx,
                aggregate,
                security: tx_security,
                sequences,
                #[cfg(feature = "diagnostics")]
                aggregate_tx_observer: diagnostics.map(|hooks| hooks.aggregate_tx),
                tx_block_ack_status_sink: Some(crate::status::publish_station_tx_block_ack),
                network_domain: ConnectedStaNetworkTxDomain::new(),
            },
        )
        .unwrap_or_else(|_| unreachable!("paired cutover starts with quiescent connected TX"));
        let mut station_control = ConnectedStaPort::build_control(
            &plan,
            ConnectedStaControlResources {
                slot: control_slot,
                receiver: control_receiver,
                reorder_commands: reorder_sender,
                rx_block_ack: &super::PRODUCTION_RX_BLOCK_ACK,
                tsf_epoch: self.mac_clock.tsf_epoch(),
                power: None,
            },
        );
        if let StaAttemptSecurityMaterial::Personal { connected, .. } =
            &mut station_security_material
        {
            let ConnectedStaGroupSecurity::Wpa2PersonalRekey {
                group,
                material: group_material,
                replay,
                management,
            } = group_security
                .take()
                .expect("paired WPA2 mode retains its group-key owner")
            else {
                unreachable!("paired security modes were validated before owner split")
            };
            station_control
                .install_wpa2_security(
                    ConnectedWpa2Security::new(
                        connected
                            .take()
                            .expect("installed station keys retain supplicant state"),
                        group,
                        group_material,
                        replay,
                        management,
                    ),
                    tx_entropy,
                )
                .unwrap_or_else(|_| unreachable!("fresh station control has no security session"));
        }
        let station_drivers = ConnectedStaPort::assemble(
            plan,
            ConnectedStaDriverParts {
                hardware,
                rx: (),
                tx: station_tx,
                control: station_control,
                protocol: station_rx,
            },
        );
        let prepared_station = prepare_sta_ap_station(station_drivers).unwrap_or_else(|_| {
            unreachable!("new connected TX can be parked before DATAPATH runs")
        });

        let StaApStationPrepared {
            mut hardware,
            physical_rx: (),
            station: station_role,
            mut physical_tx,
            report: station_report,
        } = prepared_station;
        let (ordinary, aggregate_resources) = physical_tx
            .try_lend(DatapathPairRole::Second)
            .unwrap_or_else(|_| unreachable!("station preparation returns available physical TX"));
        let (
            ssid,
            access_point_security,
            channel,
            client_limit,
            inactive_timeout,
            beacon_interval,
            dtim_period,
        ) = access_point_request.into_parts();
        let ProductionAccessPointResources {
            tx_storage: ap_tx_storage,
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
        assert!(
            core::ptr::eq(&super::PRODUCTION_RX_BLOCK_ACK, rx_block_ack),
            "paired STA and AP must share the one ordinary RX BlockAck bank owner",
        );
        let access_point_service = match access_point_security {
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
        let engine = ApEngine::start(
            &mut hardware,
            access_point_service,
            beacon,
            pairwise_storage,
            &ssid,
            channel,
            beacon_interval.tu(),
            dtim_period.get(),
            self.mac_clock.tsf_epoch(),
        )
        .unwrap_or_else(|_| {
            unreachable!("validated paired AP resources must start on the associated channel")
        });
        let mut lifecycle = StaApLifecycle::new();
        let station_transition = lifecycle
            .start_station(channel)
            .unwrap_or_else(|_| unreachable!("fresh paired lifecycle accepts its station owner"));
        debug_assert!(matches!(
            sta_ap_register_action(station_transition, receive_identities),
            oer_esp32s31_ieee80211::datapath::lifecycle::StaApRegisterAction::None
        ));
        let access_point_transition = lifecycle
            .start_access_point(channel)
            .unwrap_or_else(|_| unreachable!("validated paired roles share one exact channel"));
        apply_sta_ap_register_action(
            &mut hardware,
            sta_ap_register_action(access_point_transition, receive_identities),
        );
        let maximum_aggregate_bytes = ordinary.policy.ht_ampdu().maximum_aggregate_bytes();
        let access_point_aggregate =
            AccessPointAmpdu::new(aggregate_resources, maximum_aggregate_bytes);
        let access_point_mac = ApMac::new(
            engine,
            ordinary,
            ApTxConfig {
                publication_timeout: TX_COMPLETION_TIMEOUT,
            },
        );
        let access_point_processor = AccessPointProtocolProcessor::new(
            access_point_mac,
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
        let access_point_processor = access_point_processor.with_terminal_observer(
            super::access_point_observation::begin_access_point_observation(),
        );
        let access_point_network_tx =
            AccessPointNetworkTx::<RadioTxBacking, RadioNetworkTxBacking>::new(
                ap_tx_storage,
                #[cfg(feature = "diagnostics")]
                diagnostics.map(|hooks| hooks.aggregate_tx),
            );
        let access_point_network_link = network_runner.link_controller();
        let access_point_role = AccessPointRoleRuntime::new(
            access_point_processor,
            access_point_network_tx,
            || {
                let mut nonce = [0_u8; 32];
                for word in nonce.chunks_exact_mut(4) {
                    word.copy_from_slice(&Rng::new().random().to_le_bytes());
                }
                let replay =
                    (u64::from(Rng::new().random()) << 32) | u64::from(Rng::new().random());
                (nonce, replay)
            },
            move |status: AccessPointServiceStatus| {
                let link_state = access_point_network_link_state(status.authorized);
                if matches!(link_state, oer_network_interface::LinkState::Down) {
                    access_point_network_link.set_link_state(
                        oer_esp32s31_ieee80211_runtime::datapath::network::AP_NETWORK_INTERFACE_ID,
                        link_state,
                    );
                }
                if matches!(link_state, oer_network_interface::LinkState::Up) {
                    access_point_network_link.set_link_state(
                        oer_esp32s31_ieee80211_runtime::datapath::network::AP_NETWORK_INTERFACE_ID,
                        link_state,
                    );
                }
                crate::status::publish_access_point_status(generation, status);
            },
        );
        let access_point_role = park_sta_ap_access_point_role(
            access_point_role,
            access_point_aggregate,
            &mut physical_tx,
        )
        .unwrap_or_else(|_| unreachable!("new paired AP TX has no active transaction"));

        let services = compose_sta_ap_datapath_services(
            hardware,
            physical_tx,
            common_rx,
            station_role,
            access_point_role,
            EmbassyClock,
        );
        let mut paired_runner =
            compose_sta_ap_datapath_runner(&IRQ_RUNTIME, network_runner, services, EmbassyClock);
        #[cfg(feature = "diagnostics")]
        let rx_statistics_before = paired_runner
            .services()
            .hardware()
            .receive_statistics_snapshot();
        {
            let (hardware, rx) = paired_runner.services_mut().hardware_and_rx_mut();
            rx.start(hardware).await.unwrap_or_else(|_| {
                unreachable!("paired RX restarts the just-stopped descriptor epoch")
            });
        }
        endpoint
            .respond(WifiSupervisorResponse::StationAccessPoint(Ok(
                WifiStartReport::new(generation),
            )))
            .await;
        let run_exit = await_stack_boundary!(paired_runner.run_until(
            super::access_point::wait_for_active_wifi_role_stop(endpoint),
        ));
        let (stop_requested, active_fault, station_disconnect) = match run_exit {
            Ok(DatapathRunnerExit::Stopped) => (true, false, None),
            Ok(DatapathRunnerExit::Role(StaApControlExit::Station(reason))) => {
                (false, false, Some(reason))
            }
            Err(error) => {
                diagnostics_event!("open-radio: paired DATAPATH service failed: {error:?}");
                (false, true, None)
            }
        };
        if let Some(reason) = station_disconnect {
            diagnostics_event!("open-radio: paired station disconnected reason={reason:?}");
            crate::status::publish_station_disconnected(reason);
        }
        if !stop_requested {
            // Beacon loss or another station control exit tears down the
            // whole same-channel pair. The AP never survives on a channel
            // whose upstream station association no longer owns.
            loop {
                match await_stack_boundary!(paired_runner.run_until(ready(()))) {
                    Ok(DatapathRunnerExit::Stopped) => break,
                    Ok(DatapathRunnerExit::Role(_)) => {}
                    Err(error) => {
                        diagnostics_event!(
                            "open-radio: paired DATAPATH rollback still pending: {error:?}"
                        );
                        Timer::after_millis(1).await;
                    }
                }
            }
        }

        // Both logical roles are leaving this one physical RX epoch. Revoke
        // their disjoint admission banks while the common descriptor ring is
        // still armed and the IRQ bottom half still has a valid owner.
        {
            let hardware = paired_runner.services_mut().hardware_mut();
            disable_access_point_receive_registers(hardware);
            disable_station_receive_registers(hardware);
        }

        loop {
            match interrupt_epoch.park() {
                Ok(_) => break,
                Err(oer_esp32s31_ieee80211_runtime::datapath::irq::MacInterruptEpochQuiesceError::AlreadyQuiesced) => {
                    break;
                }
                Err(oer_esp32s31_ieee80211_runtime::datapath::irq::MacInterruptEpochQuiesceError::Route(error)) => {
                    diagnostics_event!(
                        "open-radio: paired MAC interrupt park unexpectedly failed: {error:?}"
                    );
                    Timer::after_millis(1).await;
                }
            }
        }
        // The teardown moves every owner by value. Outside this poll frame,
        // those moves do not add to the async state machine's stack budget.
        let stopped = match outside_poll_frame(move || {
            let (network_runner, services) = paired_runner.into_parts();
            let (
                mut hardware,
                physical_tx,
                common_rx,
                station_role,
                access_point_role,
                _control_arbiter,
            ) = services.into_parts();
            let route_report = common_rx.route_report();
            diagnostics_event!(
                "open-radio: paired RX routes total={} sta={} ap={} foreign={} ambiguous={} malformed={} hardware_error={}",
                route_report.total(),
                route_report.station,
                route_report.access_point,
                route_report.foreign,
                route_report.ambiguous,
                route_report.malformed,
                route_report.hardware_error,
            );
            #[cfg(feature = "diagnostics")]
            {
                let after = hardware.receive_statistics_snapshot();
                super::access_point_observation::store_access_point_rx_hardware_observation(
                    crate::DiagnosticRxStatistics::from_deltas(
                        after
                            .primary
                            .wrapping_delta_since(rx_statistics_before.primary),
                        after
                            .decode_errors
                            .wrapping_delta_since(rx_statistics_before.decode_errors),
                        after.hang.wrapping_delta_since(rx_statistics_before.hang),
                    ),
                )
            }
            let (paired_rx, mut paired_consumer) = common_rx.into_parts();
            diagnostics_event!(
                "open-radio: paired RX producer serviced_descriptors={}",
                paired_rx.serviced_descriptors(),
            );
            let discarded = paired_consumer.discard_queued();
            diagnostics_debug!(
                "open-radio: paired RX stopped discarded={} serviced={}",
                discarded,
                paired_rx.serviced_descriptors(),
            );
            let (standalone_sender, _standalone_receiver) = STAGED_RX_QUEUE.split();
            let parked_rx = paired_rx
                .try_into_standalone_live(standalone_sender)
                .unwrap_or_else(|_| {
                    unreachable!("paired queue was drained before standalone restore")
                });

            let access_point_finished =
                finish_sta_ap_access_point_role(access_point_role, physical_tx, &mut hardware)
                    .unwrap_or_else(|_| {
                        unreachable!("paired DATAPATH stop parks every AP TX owner")
                    });
            let access_point_transition = lifecycle
                .stop_access_point()
                .unwrap_or_else(|_| unreachable!("active paired lifecycle retains its AP role"));
            apply_sta_ap_register_action(
                &mut hardware,
                sta_ap_register_action(access_point_transition, receive_identities),
            );
            lifecycle
                .stop_station()
                .unwrap_or_else(|_| unreachable!("paired teardown retains its final station role"));
            let oer_esp32s31_ieee80211_runtime::roles::access_point::StaApAccessPointFinished {
                stopped: access_point_stopped,
                network_tx: _access_point_network_tx,
                security_material: _access_point_security_material,
                physical_tx,
            } = access_point_finished;
            // All physical AP publications have detached. Return software leases
            // before handing the reusable storage back to the stopped AP owner.
            let ap_tx_storage = _access_point_network_tx.into_storage();
            crate::status::publish_access_point_stopped();
            #[cfg(feature = "diagnostics")]
            if let Some(hooks) = diagnostics {
                super::access_point_observation::publish_stored_access_point_observation(
                    hooks.access_point,
                    station_channel,
                );
            }
            let prepared_station = StaApStationPrepared {
                hardware,
                physical_rx: (),
                station: station_role,
                physical_tx,
                report: station_report,
            };
            let station_drivers = finish_sta_ap_station(prepared_station).unwrap_or_else(|_| {
                unreachable!("paired DATAPATH stop parks every station TX owner")
            });

            let oer_esp32s31_ieee80211_runtime::roles::station::connected::ConnectedStaDrivers {
                services: station_services,
                report: _,
            } = station_drivers;
            let (hardware, station_rx, station_tx, mut station_control) =
                station_services.into_parts();
            let ((), station_protocol) = station_rx.into_parts();
            let group_security = match &mut station_security_material {
                StaAttemptSecurityMaterial::Open => group_security
                    .take()
                    .expect("paired Open station retains its no-key group marker"),
                StaAttemptSecurityMaterial::Personal { connected, .. } => {
                    let station_security = station_control
                        .take_wpa2_security()
                        .expect("paired WPA2 station control returns its security owner");
                    let (returned_connected, group) = station_security.into_parts();
                    *connected = Some(returned_connected);
                    ConnectedStaGroupSecurity::Wpa2Personal(group)
                }
            };
            let (stopped_protocol, station_sink) = station_protocol.into_stopped_with_sink();
            let (sta_ap_rx_batch, _control_publisher) = station_sink.into_parts();
            let (frame, ethernet, rx_protocol_runtime) = stopped_protocol.into_parts();
            let teardown = ConnectedStaTeardownPort::try_teardown(
                SingleRoleServices::with_control(
                    hardware,
                    AlreadyParkedRx::new(parked_rx),
                    station_tx,
                    station_control,
                ),
                group_security,
            )
            .unwrap_or_else(|_| unreachable!("paired DATAPATH stop returns idle station services"));
            tx_storage
                .restore_resources(teardown.tx_resources)
                .unwrap_or_else(|_| {
                    unreachable!("paired station returns the detached ordinary TX owner")
                });
            let disconnected = ConnectedDisconnectedEpoch::new(
                RunningStationNetwork::new((), network_runner),
                teardown.hardware,
                ConnectedParkedRx::from_live(teardown.parked_rx),
                teardown.aggregate,
                super::station::ConnectedControlStatics {
                    queue: control_resources,
                    slot: teardown.released_control,
                },
            );
            let station_security = match station_security_material {
                StaAttemptSecurityMaterial::Open => StaAttemptSecurity::open(teardown.sequences),
                StaAttemptSecurityMaterial::Personal {
                    credentials,
                    supplicant_nonce,
                    message4_protection,
                    ..
                } => StaAttemptSecurity::new(
                    credentials,
                    supplicant_nonce,
                    teardown.sequences,
                    message4_protection,
                ),
            };
            let station_owner = ProductionStationOwner::new(
                production_station_runtime(
                    role,
                    interrupt_epoch,
                    dma,
                    tx_storage,
                    scan_table,
                    frame,
                    ethernet,
                    ProductionStationBoardResources {
                        access_point_airtime,
                        interface,
                        connected_datapath,
                        rx_protocol_runtime,
                        sta_ap_rx_batch,
                        initial_connected,
                        #[cfg(feature = "diagnostics")]
                        diagnostics,
                    },
                ),
                ProductionStationPhase::RunningScan {
                    disconnected,
                    station,
                },
                station_security,
            );
            let oer_esp32s31_ieee80211_ap::engine::ApEngineStop {
                service,
                beacon_storage,
                pairwise_storage,
                security: _,
            } = access_point_stopped.engine;
            let access_point = ProductionAccessPointResources {
                tx_storage: ap_tx_storage,
                address: service.address(),
                beacon: beacon_storage,
                rx_frame: access_point_stopped.rx_frame,
                tx_frame: access_point_stopped.tx_frame,
                peer_storage: service.into_peer_storage(),
                pairwise_storage,
                rx_dispatcher: access_point_stopped.data_rx,
                rx_block_ack: access_point_stopped.rx_block_ack,
                rx_reorder: access_point_stopped.rx_reorder,
                rx_reorder_storage: access_point_stopped.rx_reorder_storage,
                #[cfg(feature = "diagnostics")]
                observation_storage: access_point_stopped.observation_storage,
            };
            let stopped = match try_reclaim_production_station(station_owner) {
                Ok(stopped) => stopped,
                Err(failure) => {
                    return Err(WifiRoleEpochOutcome::Faulted(
                        ProductionWifiFault::PairedReclaim {
                            _station: failure,
                            _access_point: access_point,
                            _monitor: monitor,
                        },
                    ));
                }
            };
            let resources = ProductionWifiStoppedResources::Returned(stopped.resources);
            let (physical, station) = match try_split_wifi_stopped_resources(resources) {
                Ok(resources) => resources,
                Err(resources) => {
                    return Err(WifiRoleEpochOutcome::Faulted(
                        ProductionWifiFault::StoppedOwner {
                            _wifi: stopped.wifi,
                            _resources: resources,
                            _access_point: access_point,
                            _monitor: monitor,
                        },
                    ));
                }
            };
            Ok(WifiSupervisorStopped::new(
                stopped.wifi,
                physical,
                station,
                access_point,
                monitor,
            ))
        }) {
            Ok(stopped) => stopped,
            Err(outcome) => return outcome,
        };
        if active_fault {
            return WifiRoleEpochOutcome::Faulted(ProductionWifiFault::PairedStopped {
                _stopped: stopped,
            });
        }
        if stop_requested {
            endpoint
                .respond(WifiSupervisorResponse::Stop(Ok(WifiStopReport::new(
                    generation,
                ))))
                .await;
        }
        WifiRoleEpochOutcome::Stopped(stopped)
    }
}
