//! Supervisor command dispatch and finite standalone role epochs.

use super::*;

fn standalone_scan_report(
    table: &ScanTable<SCAN_RECORD_CAPACITY>,
    generation: oer_radio::wifi::RadioSubsystemGeneration,
) -> WifiScanReport {
    let summary = table.summary();
    let mut results = [WifiScanResult::EMPTY; WIFI_SCAN_RESULT_CAPACITY];
    for (destination, source) in results.iter_mut().zip(table.records()) {
        *destination = WifiScanResult::new(
            source.ssid,
            source.ssid_len,
            source.bssid,
            source.channel,
            source.rssi,
            source.privacy,
            source.rsn,
            source.legacy_wpa,
            source.ht_capability_ie_present,
            source.he_capability_ie_len != 0,
        );
    }
    WifiScanReport::new(
        generation,
        results,
        summary.records as u8,
        summary.observed_frames,
        summary.dropped_unique_bss,
    )
}
impl ProductionWifiEpochRunner {
    async fn run_standalone_scan(
        &mut self,
        endpoint: &mut WifiSupervisorEndpoint<'_, CriticalSectionRawMutex, RadioError>,
        stopped: ProductionSupervisorStopped,
        request: WifiScanRequest,
        generation: oer_radio::wifi::RadioSubsystemGeneration,
    ) -> WifiRoleEpochOutcome<ProductionSupervisorStopped, ProductionWifiFault> {
        let (wifi, physical, mut station, access_point, monitor) = stopped.into_parts();
        let mut materialized = materialize_production_wifi(wifi, physical);
        let (dma, rx_ring, tx, aggregate_tx) = materialized.resources.into_parts();
        let tx_epoch =
            self.initialize_tx_epoch(tx, materialized.owner.start_report().wifi.tx_power);
        activate_promiscuous_receive(&mut materialized.registers);
        let receive = match rx_ring {
            Some(ring) => ring.into_scan(dma.storage()),
            None => match ScanRx::prepare_initial(
                &mut materialized.registers,
                dma.storage(),
                dma.descriptor_base(),
                dma.buffer_addresses(),
            ) {
                Ok(receive) => receive,
                Err(error) => {
                    let faulted = ProductionWifiFault::StandaloneScanInitialRx {
                        _owner: materialized.owner,
                        _registers: materialized.registers,
                        _interrupts: materialized.interrupts,
                        _physical: ProductionWifiPhysicalResources::new(
                            dma,
                            None,
                            ProductionOrdinaryTxResources::Epoch(tx_epoch),
                            aggregate_tx,
                        ),
                        _station: station,
                        _access_point: access_point,
                        _monitor: monitor,
                        _error: error,
                    };
                    endpoint
                        .respond(WifiSupervisorResponse::Scan(Err(
                            WifiScanFailure::Faulted {
                                error: self.fault_error(&faulted),
                            },
                        )))
                        .await;
                    return WifiRoleEpochOutcome::Faulted(faulted);
                }
            },
        };
        let control = tx_epoch
            .take_control()
            .expect("standalone scan starts from an idle ordinary TX owner");
        let mut channels = [0_u8; 14];
        let mut channel_count = 0_usize;
        for channel in request.channels().primary_channels() {
            channels[channel_count] = channel;
            channel_count += 1;
        }
        let scan_request = StationScanRequest::new(
            oer_esp32s31_ieee80211_sta::scan::StaScanConfig::new(request.dwell_millis())
                .expect("WifiScanRequest stores a nonzero dwell"),
            &channels[..channel_count],
            station.station_address(),
            &[],
            &ESP32S31_STATION_PROBE_RATES,
            oer_ieee80211_mac::security::StaSecurityPolicy::Wpa2Personal,
        )
        .with_descriptor_capacity(ESP32S31_STATION_PROBE_DESCRIPTOR_CAPACITY)
        .without_candidate_selection();
        let mut sequence = oer_ieee80211_mac::station::StaSequenceCounter::new(
            oer_ieee80211_mac::sequence::SequenceNumber::from_low_bits(self.trng.random() as u16),
        );
        let (scan_table, scan_frame) = station.scan_storage();
        let scan = run_esp32s31_station_scan(
            StationScanResources {
                radio: self.radio,
                phy_observer: NoopPhyTargetObserver,
                hardware: materialized.registers,
                receive,
                control,
                table: scan_table,
                frame: scan_frame,
                scan_observer: ProductionScanObserver,
                sequence: &mut sequence,
                timer: super::reception_timer(self.mac_clock),
            },
            scan_request,
        )
        .await;
        let completed = matches!(scan.decision, StationScanDecision::NoCandidate { .. });
        let oer_esp32s31_ieee80211_runtime::roles::station::StationScanReturned {
            hardware: registers,
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
        if let Err((error, returned_control)) = tx_epoch.restore_control(control) {
            let faulted = ProductionWifiFault::StandaloneScanReturn {
                _fault: ProductionStandaloneScanReturnFault::TxRestore {
                    _owner: materialized.owner,
                    _registers: registers,
                    _interrupts: materialized.interrupts,
                    _dma: dma,
                    _receive: receive,
                    _tx_epoch: tx_epoch,
                    _aggregate_tx: aggregate_tx,
                    _station: station,
                    _access_point: access_point,
                    _monitor: monitor,
                    _error: error,
                    _returned_control: returned_control,
                },
            };
            endpoint
                .respond(WifiSupervisorResponse::Scan(Err(
                    WifiScanFailure::Faulted {
                        error: self.fault_error(&faulted),
                    },
                )))
                .await;
            return WifiRoleEpochOutcome::Faulted(faulted);
        }
        let ring = match receive.into_live() {
            Ok(ring) => ring,
            Err(receive) => {
                let faulted = ProductionWifiFault::StandaloneScanReturn {
                    _fault: ProductionStandaloneScanReturnFault::ReceiveNotLive {
                        _owner: materialized.owner,
                        _registers: registers,
                        _interrupts: materialized.interrupts,
                        _dma: dma,
                        _receive: receive,
                        _tx_epoch: tx_epoch,
                        _aggregate_tx: aggregate_tx,
                        _station: station,
                        _access_point: access_point,
                        _monitor: monitor,
                    },
                };
                endpoint
                    .respond(WifiSupervisorResponse::Scan(Err(
                        WifiScanFailure::Faulted {
                            error: self.fault_error(&faulted),
                        },
                    )))
                    .await;
                return WifiRoleEpochOutcome::Faulted(faulted);
            }
        };
        let report = standalone_scan_report(station.scan_table(), generation);
        let wifi = park_production_wifi(materialized.owner, registers, materialized.interrupts);
        let stopped = WifiSupervisorStopped::new(
            wifi,
            ProductionWifiPhysicalResources::new(
                dma,
                Some(ProductionRxRing::Live(ring)),
                ProductionOrdinaryTxResources::Epoch(tx_epoch),
                aggregate_tx,
            ),
            station,
            access_point,
            monitor,
        );
        let response = if completed {
            Ok(report)
        } else {
            Err(WifiScanFailure::Returned {
                request,
                error: RadioError::HardwareFault,
            })
        };
        endpoint
            .respond(WifiSupervisorResponse::Scan(response))
            .await;
        WifiRoleEpochOutcome::Stopped(stopped)
    }

    async fn run_station_service(
        &self,
        endpoint: &mut WifiSupervisorEndpoint<'_, CriticalSectionRawMutex, RadioError>,
        stopped: ProductionSupervisorStopped,
        request: StationRequest,
        generation: oer_radio::wifi::RadioSubsystemGeneration,
    ) -> WifiRoleEpochOutcome<ProductionSupervisorStopped, ProductionWifiFault> {
        await_stack_boundary!(run_esp32s31_station_supervisor_epoch(
            endpoint,
            StationSupervisorEpoch::new(stopped, request, generation),
            |stopped, request| {
                self.prepare_station_task(stopped, request, ProductionStationMode::Service)
            },
            StationSupervisorHooks::new(
                |output: ProductionStationExit| {
                    let resources = match output {
                        StationExit::Stopped {
                            resources,
                            progress,
                            reason,
                        } => {
                            diagnostics_event!(
                                "open-radio: station epoch stopped attempts={} connected_epochs={} reason={reason:?}",
                                progress.attempts_started,
                                progress.connected_epochs,
                            );
                            resources
                        }
                        StationExit::RetryExhausted {
                            resources,
                            progress,
                            failure,
                        } => {
                            diagnostics_event!(
                                "open-radio: station epoch exhausted attempts={} stage={:?}",
                                progress.attempts_started,
                                failure.stage,
                            );
                            #[cfg(feature = "diagnostics")]
                            if let Some(hooks) = resources.owner().runtime.board().diagnostics {
                                (hooks.station_attempt)(
                                    crate::StationAttemptObservation::RetryExhausted {
                                        attempts: progress.final_generation_attempt,
                                        stage: failure.stage,
                                    },
                                );
                            }
                            resources
                        }
                        StationExit::Terminal {
                            resources,
                            progress,
                            failure,
                        } => {
                            diagnostics_event!(
                                "open-radio: station epoch ended attempts={} stage={:?}",
                                progress.attempts_started,
                                failure.stage,
                            );
                            resources
                        }
                        StationExit::Faulted { fault, runner, .. } => {
                            return WifiRoleFrontier::Faulted(ProductionWifiFault::Station {
                                _fault: fault,
                                _runner: runner,
                            });
                        }
                    };
                    match restore_production_station_frontier(resources) {
                        Ok(stopped) => WifiRoleFrontier::Stopped(stopped),
                        Err(faulted) => WifiRoleFrontier::Faulted(faulted),
                    }
                },
                RadioError::RoleActive,
                |_faulted: &ProductionWifiFault| RadioError::HardwareFault,
            ),
        ))
    }
}
impl WifiRoleEpochRunner<CriticalSectionRawMutex> for ProductionWifiEpochRunner {
    type Stopped = ProductionSupervisorStopped;
    type Faulted = ProductionWifiFault;
    type LifecycleFaulted = &'static mut ProductionRadioLifecycleFault;
    type Error = RadioError;

    fn planning_error(&mut self, error: WifiServicePlanningError) -> Self::Error {
        RadioError::Planning(error)
    }

    fn fault_error(&mut self, faulted: &Self::Faulted) -> Self::Error {
        let _ = faulted;
        RadioError::HardwareFault
    }

    fn lifecycle_fault_error(&mut self, faulted: &Self::LifecycleFaulted) -> Self::Error {
        let _ = faulted;
        RadioError::HardwareFault
    }

    fn run_epoch<'a>(
        &'a mut self,
        endpoint: &'a mut WifiSupervisorEndpoint<'_, CriticalSectionRawMutex, Self::Error>,
        stopped: Self::Stopped,
        service: WifiServiceRequest,
        generation: oer_radio::wifi::RadioSubsystemGeneration,
    ) -> impl Future<Output = WifiRoleEpochOutcome<Self::Stopped, Self::Faulted>> + 'a {
        async move {
            let outcome = match service {
                WifiServiceRequest::StandaloneScan { request, .. } => {
                    await_stack_boundary!(
                        self.run_standalone_scan(endpoint, stopped, request, generation)
                    )
                }
                WifiServiceRequest::StandaloneMonitor { plan, request } => {
                    let Some(monitor_plan) = plan.standalone_monitor() else {
                        endpoint
                            .respond(WifiSupervisorResponse::Monitor(Err(
                                WifiStartFailure::rejected(
                                    request,
                                    RadioError::Planning(
                                        WifiServicePlanningError::Request(
                                            oer_radio::wifi::WifiServiceRequestError::NotStandaloneMonitorTopology,
                                        ),
                                    ),
                                ),
                            )))
                            .await;
                        return WifiRoleEpochOutcome::NotStarted(stopped);
                    };
                    let channel_policy = request.channel_policy();
                    let channel = channel_policy.initial_channel();
                    let snapshot_length = request.capture_policy().snapshot_length();
                    let (
                        wifi,
                        physical_resources,
                        station_resources,
                        access_point_resources,
                        monitor_resources,
                    ) = stopped.into_parts();
                    let (physical_resources, rx_ring) = physical_resources.take_rx_ring();
                    let materialized = materialize_production_wifi(wifi, ());
                    let radio = MonitorRadio::new(
                        materialized.owner,
                        materialized.registers,
                        materialized.interrupts,
                    );
                    let discarded = self.monitor_capture.discard_queued();
                    crate::monitor::record_discarded_monitor_frames(discarded);
                    let (mut controller, mut task) = match prepare_esp32s31_monitor_task(
                        monitor_plan,
                        radio,
                        monitor_resources.bind(generation, snapshot_length, rx_ring),
                    ) {
                        Ok(prepared) => prepared,
                        Err(failure) => {
                            let faulted = ProductionWifiFault::MonitorBuild {
                                _failure: failure,
                                _physical: physical_resources,
                                _station: station_resources,
                            };
                            endpoint
                                .respond(WifiSupervisorResponse::Monitor(Err(
                                    WifiStartFailure::faulted(self.fault_error(&faulted)),
                                )))
                                .await;
                            return WifiRoleEpochOutcome::Faulted(faulted);
                        }
                    };
                    let mut observer = NoopPhyTargetObserver;
                    if let Err(error) =
                        await_stack_boundary!(task.switch_channel::<EmbassyClock, _, _, _>(
                            self.radio,
                            channel,
                            &mut observer
                        ),)
                    {
                        let faulted = ProductionWifiFault::MonitorChannel {
                            _error: error,
                            _task: task,
                            _physical: physical_resources,
                            _station: station_resources,
                        };
                        endpoint
                            .respond(WifiSupervisorResponse::Monitor(Err(
                                WifiStartFailure::faulted(self.fault_error(&faulted)),
                            )))
                            .await;
                        return WifiRoleEpochOutcome::Faulted(faulted);
                    }
                    endpoint
                        .respond(WifiSupervisorResponse::Monitor(Ok(WifiStartReport::new(
                            generation,
                        ))))
                        .await;
                    let exit = await_stack_boundary!(drive_esp32s31_monitor_role::<
                        _,
                        _,
                        _,
                        _,
                        _,
                        _,
                        _,
                        EmbassyClock,
                        _,
                        _,
                        _,
                        RX_DESCRIPTOR_COUNT,
                        RX_BUFFER_SIZE,
                        RX_BUFFER_STORAGE_SIZE,
                    >(
                        endpoint,
                        &mut controller,
                        self.radio,
                        task,
                        channel_policy,
                        &mut observer,
                        RadioError::RoleActive,
                    ));
                    let frontier = await_stack_boundary!(finish_wifi_active_role(
                        endpoint,
                        generation,
                        exit,
                        |output| match output {
                            MonitorTaskExit::Stopped { stopped, .. } => {
                                let discarded = self.monitor_capture.discard_queued();
                                crate::monitor::record_discarded_monitor_frames(discarded);
                                let (monitor, rx_ring) =
                                    ProductionMonitorResources::from_stopped(stopped.resources);
                                let radio = stopped.radio;
                                WifiRoleFrontier::Stopped(WifiSupervisorStopped::new(
                                    ProductionWifiOwner::Live {
                                        owner: radio.owner,
                                        registers: radio.registers,
                                        interrupts: radio.interrupts,
                                    },
                                    physical_resources.restore_rx_ring(rx_ring),
                                    station_resources,
                                    access_point_resources,
                                    monitor,
                                ))
                            }
                            MonitorTaskExit::Faulted { task, .. } => {
                                WifiRoleFrontier::Faulted(ProductionWifiFault::MonitorRuntime {
                                    _task: task,
                                    _physical: physical_resources,
                                    _station: station_resources,
                                })
                            }
                        },
                        |_faulted| RadioError::HardwareFault,
                    ));
                    match frontier {
                        WifiRoleFrontier::Stopped(stopped) => {
                            WifiRoleEpochOutcome::Stopped(stopped)
                        }
                        WifiRoleFrontier::Faulted(faulted) => {
                            WifiRoleEpochOutcome::Faulted(faulted)
                        }
                    }
                }
                WifiServiceRequest::Station { request, .. } => {
                    await_stack_boundary!(
                        self.run_station_service(endpoint, stopped, request, generation)
                    )
                }
                WifiServiceRequest::AccessPoint { request, .. } => {
                    await_stack_boundary!(
                        self.run_access_point_service(endpoint, stopped, request, generation)
                    )
                }
                WifiServiceRequest::StationAccessPoint { request, .. } => {
                    await_stack_boundary!(
                        self.run_station_access_point_service(
                            endpoint, stopped, request, generation
                        )
                    )
                }
            };
            if let WifiRoleEpochOutcome::Faulted(faulted) = &outcome
                && let Some(reason) = faulted.station_fault_reason()
            {
                crate::status::publish_station_faulted(reason);
            }
            outcome
        }
    }

    // CAPABILITY: radio-cold-restart
    fn restart_radio<'a>(
        &'a mut self,
        slot: &'a mut Option<Self::Stopped>,
    ) -> impl Future<Output = Result<WifiRadioRestartRf, Self::LifecycleFaulted>> + 'a {
        async move {
            let protection = self.watchdog.shutdown();
            let stopped = slot
                .take()
                .expect("supervisor retains stopped owner between role epochs");
            let frontier = match await_stack_boundary!(shutdown::quiesce_for_shutdown(stopped)) {
                Ok(frontier) => frontier,
                Err(failure) => {
                    diagnostics_event!("open-radio: Wi-Fi shutdown failed: {}", failure.reason());
                    failure.enforce_disposition();
                    crate::WatchdogConfig::complete(protection);
                    return Err(RADIO_LIFECYCLE_FAULT.init(
                        ProductionRadioLifecycleFault::ShutdownQuiesce { _failure: failure },
                    ));
                }
            };
            let (wifi, resources) = frontier.into_parts();
            let (released, coex, rf) = {
                let mut guard = self.radio.lock().await;
                // The vendor `wifi_hw_stop` disables coexistence before the
                // PHY, so the client releases outside coexistence; a timer
                // that cannot be withdrawn stops the radio.
                let (membership, wifi) = wifi.leave_membership();
                let WifiMembership { power, coexisting } = membership;
                let coex = match guard.disable_coex(coexisting) {
                    Ok(absent) => absent,
                    Err(CoexLeaveFailure { proof, error }) => {
                        let fault = ProductionRadioLifecycleFault::CoexDisable {
                            _error: error,
                            _membership: WifiMembership {
                                power,
                                coexisting: proof,
                            },
                            _wifi: wifi,
                            _resources: resources,
                        };
                        enforce_lifecycle(&fault, true);
                        crate::WatchdogConfig::complete(protection);
                        return Err(RADIO_LIFECYCLE_FAULT.init(fault));
                    }
                };
                let (lease, _, _) = guard.parts();
                let released = match wifi.release(lease, power, &coex) {
                    Ok(released) => released,
                    Err(failure) => {
                        let ambiguous = !matches!(
                            failure.error(),
                            WifiReleaseError::Hardware(_) | WifiReleaseError::Phy(_)
                        );
                        enforce_lifecycle(&failure, ambiguous);
                        crate::WatchdogConfig::complete(protection);
                        return Err(RADIO_LIFECYCLE_FAULT.init(
                            ProductionRadioLifecycleFault::Release {
                                _failure: failure,
                                _coex: coex,
                                _resources: resources,
                            },
                        ));
                    }
                };
                // The last client's `esp_phy_disable` closes RF; another client
                // keeps it open.
                let rf = match await_stack_boundary!(guard.close_phy_if_idle()) {
                    Ok(true) => WifiRadioRestartRf::ClosedAndWoken,
                    Ok(false) => WifiRadioRestartRf::KeptOpen,
                    Err(error) => {
                        let fault = ProductionRadioLifecycleFault::RfClose {
                            _error: error,
                            _partition: released.partition,
                            _platform: released.platform,
                            _coex: coex,
                            _resources: resources,
                        };
                        enforce_lifecycle(
                            &fault,
                            !matches!(error, ConcurrentRfError::Recoverable(_)),
                        );
                        crate::WatchdogConfig::complete(protection);
                        return Err(RADIO_LIFECYCLE_FAULT.init(fault));
                    }
                };
                (released, coex, rf)
            };
            crate::WatchdogConfig::complete(protection);
            let protection = self.watchdog.startup();
            let wifi = match await_stack_boundary!(join_shared_radio(
                self.radio,
                released.partition,
                released.platform,
                coex,
                self.radio_start,
            )) {
                Ok((_phy, wifi, mac_clock)) => {
                    // The restarted MAC reads a fresh clock; the epochs of
                    // the earlier start have ended.
                    self.mac_clock = mac_clock;
                    wifi
                }
                Err(failure) => {
                    crate::WatchdogConfig::complete(protection);
                    return Err(
                        RADIO_LIFECYCLE_FAULT.init(ProductionRadioLifecycleFault::Join {
                            _failure: failure,
                            _resources: Some(resources),
                        }),
                    );
                }
            };
            crate::WatchdogConfig::complete(protection);
            let (physical, station, access_point, monitor) = resources;
            *slot = Some(WifiSupervisorStopped::new(
                ProductionWifiOwner::Cold(wifi),
                physical,
                station,
                access_point,
                monitor,
            ));
            Ok(rf)
        }
    }
}

fn enforce_lifecycle<E: ?Sized>(failure: &E, hardware_ambiguous: bool) {
    use oer_esp32s31_phy::tracking::fail_stop::SharedPhyFailStop;
    crate::lifecycle_policy::enforce(
        failure,
        SharedPhyFailStop::from_ambiguous_lifecycle(hardware_ambiguous),
    );
}
