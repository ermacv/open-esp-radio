#[cfg(any(feature = "diagnostics", test))]
fn observe_ht_rx_data_frame(
    observation: &mut AccessPointControlObservation,
    signal: HtSignal,
) {
    observation.rx_ht_data_frames = observation.rx_ht_data_frames.saturating_add(1);
    if signal.aggregation {
        observation.rx_ht_mpdus_with_aggregation_bit = observation
            .rx_ht_mpdus_with_aggregation_bit
            .saturating_add(1);
    }
    match signal.ht_duplicate_mcs32_classification() {
        HtDuplicateRxClassification::Ht40(_) => {
            observation.rx_ht40_mcs32_frames = observation.rx_ht40_mcs32_frames.saturating_add(1);
        }
        HtDuplicateRxClassification::Mismatch { .. } => {
            observation.rx_ht_mcs32_width_mismatches =
                observation.rx_ht_mcs32_width_mismatches.saturating_add(1);
        }
        HtDuplicateRxClassification::NotMcs32 => {}
    }
    if signal.channel_width_mhz == 40
        && let Some(count) = observation.rx_ht40_mcs_frames.get_mut(signal.mcs as usize)
    {
        *count = count.saturating_add(1);
        let guard_interval_count = if signal.short_guard_interval {
            &mut observation.rx_ht40_short_gi_frames
        } else {
            &mut observation.rx_ht40_long_gi_frames
        };
        *guard_interval_count = guard_interval_count.saturating_add(1);
    }
}

fn ap_security_material_for_management<S>(
    link_protection: LinkProtection,
    request: Option<ApManagementRequest<'_>>,
    peer_phase: Option<ApPeerPhase>,
    source: &mut S,
) -> ([u8; 32], u64)
where
    S: FnMut() -> ([u8; 32], u64),
{
    if link_protection == LinkProtection::Ccmp
        && matches!(request, Some(ApManagementRequest::Association { .. }))
        && peer_phase == Some(ApPeerPhase::Authenticated)
    {
        source()
    } else {
        ([0; 32], 0)
    }
}

fn retain_ap_power_save_action(
    engine: &mut ApEngine<'_>,
    pending: &mut PendingApBufferedReleases,
    action: ApPowerSaveAction,
) -> Result<(), AccessPointControlError> {
    let release = match action {
        ApPowerSaveAction::ReleaseOne(release) => Some(release),
        ApPowerSaveAction::None | ApPowerSaveAction::StateChanged { .. } => None,
    };
    let Some(release) = release else {
        return Ok(());
    };
    if let Err(release) = pending.push(release) {
        engine.complete_buffered_unicast_release(release, false)?;
        return Err(AccessPointControlError::ProtocolActionCapacity);
    }
    Ok(())
}

fn observe_ap_rx_peer_activity(
    engine: &mut ApEngine<'_>,
    pending: &mut PendingApBufferedReleases,
    peer: [u8; 6],
    power_state: Option<ApPeerPowerState>,
    at: oer_time::Instant,
) -> Result<(), AccessPointControlError> {
    match power_state {
        Some(ApPeerPowerState::Active) => {
            let action =
                engine.observe_rx_peer_power_state(peer, ApPeerPowerState::Active, at)?;
            retain_ap_power_save_action(engine, pending, action)?;
        }
        Some(ApPeerPowerState::Sleeping) => {
            let action =
                engine.observe_rx_peer_power_state(peer, ApPeerPowerState::Sleeping, at)?;
            retain_ap_power_save_action(engine, pending, action)?;
        }
        None => engine.observe_peer_activity(peer, at)?,
    }
    Ok(())
}

#[inline(always)]
const fn admitted_ap_data_power_state(frame_control: u16) -> ApPeerPowerState {
    if frame_control & 0x1000 == 0 {
        ApPeerPowerState::Active
    } else {
        ApPeerPowerState::Sleeping
    }
}

/// Consume the common in-order protected-data owner without borrowing the
/// AP ordinary-TX capability. Active and parked AP roles enter this same leaf.
/// `Ok(None)` is non-mutating for the staging owner and reorder state.
#[inline(never)]
fn try_service_ap_staged_rx_direct<'storage, F, const DMA_BUFFER_SIZE: usize>(
    engine: &mut ApEngine<'_>,
    data_rx: &mut ApRxDispatcher,
    rx_reorder: &mut AccessPointRxReorder<'storage, DMA_BUFFER_SIZE>,
    pending_buffered_releases: &mut PendingApBufferedReleases,
    rx_frame: &mut [u8],
    rx_batch_used: &mut usize,
    rx_batch_offset: &mut usize,
    staged_frame: &mut Option<F>,
    now: oer_time::Instant,
    #[cfg(any(feature = "diagnostics", test))]
    observation: &mut AccessPointControlObservation,
    #[cfg(feature = "diagnostics")] delivery_observer: Option<&dyn RxNetworkDeliveryObserver>,
) -> Result<Option<AccessPointRxProtocolClass>, AccessPointControlError>
where
    F: AccessPointStagedRxFrame,
{
    let segment = staged_frame
        .as_ref()
        .expect("current AP staged frame is live")
        .segment();
    if !data_rx.may_dispatch_ordinary_pairwise(segment) {
        return Ok(None);
    }
    let Some(key) = data_rx.reorder_key(segment) else {
        return Ok(None);
    };
    let Some(_reorder_progress) = rx_reorder.try_ingest_immediate(key, now)? else {
        return Ok(None);
    };

    #[cfg(feature = "task-poll-telemetry")]
    let mut core0_ap_rx = crate::diagnostics::core0_ap_rx_cycles::Core0ApRxCycleProfile::begin();
    #[cfg(feature = "task-poll-telemetry")]
    core0_ap_rx.view_complete();

    #[cfg(any(feature = "diagnostics", test))]
    let diagnostic_frame = view_normalized_rx_frame(
        &segment,
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: 0,
        },
    )
    .ok();
    #[cfg(any(feature = "diagnostics", test))]
    {
        observation.protected_data_frames = observation.protected_data_frames.saturating_add(1);
        if let Some(diagnostic_frame) = diagnostic_frame
            && let MacRxEvidence::HardwareObserved(rssi_dbm) = diagnostic_frame.metadata.rssi_dbm
        {
            if observation.rx_rssi_samples == 0 {
                observation.rx_rssi_min_dbm = rssi_dbm;
                observation.rx_rssi_max_dbm = rssi_dbm;
            } else {
                observation.rx_rssi_min_dbm = observation.rx_rssi_min_dbm.min(rssi_dbm);
                observation.rx_rssi_max_dbm = observation.rx_rssi_max_dbm.max(rssi_dbm);
            }
            observation.rx_rssi_samples = observation.rx_rssi_samples.saturating_add(1);
            observation.rx_rssi_sum_dbm = observation
                .rx_rssi_sum_dbm
                .saturating_add(i32::from(rssi_dbm));
        }
        if let Some(diagnostic_frame) = diagnostic_frame
            && let MacRxEvidence::HardwareObserved(phy) = diagnostic_frame.metadata.rate
            && let Some(signal) = phy.ht_signal()
        {
            observe_ht_rx_data_frame(observation, signal);
        }
        observation.rx_reorder_dispatched_mpdus = observation
            .rx_reorder_dispatched_mpdus
            .saturating_add(u32::from(_reorder_progress.dispatched));
    }

    let power_state = segment
        .buffer
        .get(oer_esp32s31_ieee80211_mac::rx::PUBLIC_HEADER_SIZE..)
        .and_then(|mpdu| mpdu.get(..2))
        .map(|bytes| admitted_ap_data_power_state(u16::from_le_bytes([bytes[0], bytes[1]])))
        .expect("ordinary AP data preflight validated its public frame control");
    let mut admitted_activity = None;
    #[cfg(any(feature = "diagnostics", test))]
    let mut produced_data = false;
    let in_place_publication = {
        let mut in_place = InPlaceAccessPointRxSink::new(segment.buffer);
        #[cfg(feature = "task-poll-telemetry")]
        let dispatch_started = crate::diagnostics::core0_rx_cycles::cycle_count();
        AccessPointProtectedFrameDispatch::dispatch_ordinary(
            data_rx,
            segment,
            |request| {
                let (admission, activity) = engine.admit_ordinary_pairwise_rx_with_activity(
                    request,
                    power_state,
                    now,
                );
                admitted_activity = Some(activity);
                admission
            },
            key.peer,
            #[cfg(any(feature = "diagnostics", test))]
            now,
            &mut in_place,
            #[cfg(any(feature = "diagnostics", test))]
            observation,
            #[cfg(any(feature = "diagnostics", test))]
            &mut produced_data,
            #[cfg(feature = "task-poll-telemetry")]
            &mut core0_ap_rx,
        );
        #[cfg(feature = "task-poll-telemetry")]
        core0_ap_rx.dispatch_complete(
            crate::diagnostics::core0_rx_cycles::cycle_count().wrapping_sub(dispatch_started),
        );
        if in_place.unsupported {
            return Err(AccessPointControlError::ReceiveBatchCapacity);
        }
        in_place.publication
    };

    if let Some(ethernet) = in_place_publication {
        #[cfg(any(feature = "diagnostics", test))]
        let raw = segment.buffer;
        #[cfg(any(feature = "diagnostics", test))]
        let payload =
            &raw[ethernet.payload_offset..ethernet.payload_offset + ethernet.payload_length];
        #[cfg(any(feature = "diagnostics", test))]
        let ethernet_frame = EthernetFrameParts {
            destination: ethernet.destination,
            source: ethernet.source,
            ether_type: ethernet.ether_type,
            payload,
        };
        #[cfg(any(feature = "diagnostics", test))]
        let protocol = ethernet_parts_protocol(ethernet_frame);
        #[cfg(feature = "diagnostics")]
        if let Some(observer) = delivery_observer {
            observer.admitted(RxNetworkDeliveryEvent::decoded(ethernet_frame, Some(raw)));
        }
        let frame = EthernetFrameParts {
            destination: ethernet.destination,
            source: ethernet.source,
            ether_type: ethernet.ether_type,
            payload: &segment.buffer
                [ethernet.payload_offset..ethernet.payload_offset + ethernet.payload_length],
        };
        let mut writer = crate::datapath::rx::ethernet::PackedEthernetWriter::new(rx_frame);
        writer
            .push(frame)
            .map_err(|_| AccessPointControlError::ReceiveBatchCapacity)?;
        *rx_batch_used = writer.used();
        *rx_batch_offset = 0;
        drop(
            staged_frame
                .take()
                .expect("direct AP publication owns the current staging frame"),
        );
        #[cfg(any(feature = "diagnostics", test))]
        {
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
    #[cfg(any(feature = "diagnostics", test))]
    if !produced_data {
        observation.ignored_rx_frames = observation.ignored_rx_frames.saturating_add(1);
    }
    #[cfg(feature = "task-poll-telemetry")]
    core0_ap_rx.publication_complete();

    if let Some(activity) = admitted_activity
        && let Some(action) = activity?
    {
        retain_ap_power_save_action(engine, pending_buffered_releases, action)?;
    }
    #[cfg(feature = "task-poll-telemetry")]
    core0_ap_rx.finish();
    Ok(Some(AccessPointRxProtocolClass::ProtectedData))
}

/// Consume every AP data-frame ordering case without borrowing the shared
/// ordinary-TX capability. The immediate in-order case stays on the small
/// leaf above; gaps, duplicates and releases enter the same role-local BA
/// state from active and parked AP owners.
fn try_service_ap_staged_rx_data<'storage, F, const DMA_BUFFER_SIZE: usize>(
    engine: &mut ApEngine<'_>,
    state: &mut AccessPointProtocolState<'storage, DMA_BUFFER_SIZE>,
    staged_frame: &mut Option<F>,
    now: oer_time::Instant,
    #[cfg(feature = "diagnostics")] delivery_observer: Option<&dyn RxNetworkDeliveryObserver>,
) -> Result<Option<AccessPointRxProtocolClass>, AccessPointControlError>
where
    F: AccessPointStagedRxFrame,
{
    if let Some(class) = try_service_ap_staged_rx_direct(
        engine,
        state.data_rx,
        state.rx_reorder,
        &mut state.pending_buffered_releases,
        state.rx_frame,
        &mut state.rx_batch_used,
        &mut state.rx_batch_offset,
        staged_frame,
        now,
        #[cfg(any(feature = "diagnostics", test))]
        &mut state.observer.observation,
        #[cfg(feature = "diagnostics")]
        delivery_observer,
    )? {
        return Ok(Some(class));
    }

    #[cfg(feature = "task-poll-telemetry")]
    let mut core0_ap_rx = crate::diagnostics::core0_ap_rx_cycles::Core0ApRxCycleProfile::begin();
    let segment = staged_frame
        .as_ref()
        .expect("current AP staged frame is live")
        .segment();
    let frame = match view_normalized_rx_frame(
        &segment,
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: 0,
        },
    ) {
        Ok(frame) => frame,
        Err(_error) => {
            observe_access_point!(state, observation, {
                match _error {
                    oer_esp32s31_ieee80211_mac::rx::RxError::MicFailure => {
                        observation.rx_mic_failures = observation.rx_mic_failures.saturating_add(1);
                    }
                    oer_esp32s31_ieee80211_mac::rx::RxError::Quarantined => {
                        let duplicate_or_stale = state
                            .data_rx
                            .reorder_key(segment)
                            .is_some_and(|key| state.rx_reorder.is_duplicate_or_stale(key));
                        if duplicate_or_stale {
                            observation.protected_data_duplicates =
                                observation.protected_data_duplicates.saturating_add(1);
                        } else {
                            observation.rx_quarantined_frames =
                                observation.rx_quarantined_frames.saturating_add(1);
                        }
                    }
                    _ => {
                        observation.rx_view_rejected =
                            observation.rx_view_rejected.saturating_add(1);
                    }
                }
                observation.ignored_rx_frames = observation.ignored_rx_frames.saturating_add(1);
            });
            return Ok(Some(AccessPointRxProtocolClass::Rejected));
        }
    };
    let frame_control = u16::from_le_bytes([frame.mpdu[0], frame.mpdu[1]]);
    let data_frame = frame_control & 0x000c == 0x0008;
    let protected = frame_control & 0x4000 != 0;
    if !data_frame || (engine.link_protection() != LinkProtection::Open && !protected) {
        return Ok(None);
    }

    #[cfg(feature = "task-poll-telemetry")]
    core0_ap_rx.view_complete();
    observe_access_point!(state, observation, {
        observation.protected_data_frames = observation.protected_data_frames.saturating_add(1);
        if let MacRxEvidence::HardwareObserved(rssi_dbm) = frame.metadata.rssi_dbm {
            if observation.rx_rssi_samples == 0 {
                observation.rx_rssi_min_dbm = rssi_dbm;
                observation.rx_rssi_max_dbm = rssi_dbm;
            } else {
                observation.rx_rssi_min_dbm = observation.rx_rssi_min_dbm.min(rssi_dbm);
                observation.rx_rssi_max_dbm = observation.rx_rssi_max_dbm.max(rssi_dbm);
            }
            observation.rx_rssi_samples = observation.rx_rssi_samples.saturating_add(1);
            observation.rx_rssi_sum_dbm = observation
                .rx_rssi_sum_dbm
                .saturating_add(i32::from(rssi_dbm));
        }
        if let MacRxEvidence::HardwareObserved(phy) = frame.metadata.rate
            && let Some(signal) = phy.ht_signal()
        {
            observe_ht_rx_data_frame(observation, signal);
        }
    });

    let power_state = Some(admitted_ap_data_power_state(frame_control));
    let ampdu_contained = matches!(
        frame.metadata.ampdu,
        MacRxEvidence::HardwareObserved(true) | MacRxEvidence::ProtocolValidated(true)
    );
    let ampdu_baseband_format = if ampdu_contained {
        match frame.metadata.rate {
            MacRxEvidence::HardwareObserved(phy) => Some(phy.baseband_format().raw()),
            _ => None,
        }
    } else {
        None
    };
    let mut activity_peer = None;
    #[cfg(feature = "task-poll-telemetry")]
    let mut protected_dispatch_cycles = 0_u32;
    let (reorder_progress, batch_used, batch_exhausted, in_place_publication, produced_data) = {
        let data_rx = &mut state.data_rx;
        #[cfg(any(feature = "diagnostics", test))]
        let report = &mut state.observer.observation;
        let mut deferred = DeferredAccessPointRxSink::new(state.rx_frame);
        let mut in_place = InPlaceAccessPointRxSink::new(segment.buffer);
        let mut produced_data = false;
        #[cfg(feature = "task-poll-telemetry")]
        let reorder_key_started = crate::diagnostics::core0_rx_cycles::cycle_count();
        let key = data_rx.reorder_key(segment);
        let peer = key.map(|key| key.peer).or_else(|| {
            frame
                .mpdu
                .get(10..16)
                .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())
        });
        #[cfg(feature = "task-poll-telemetry")]
        core0_ap_rx.record_reorder_key(
            crate::diagnostics::core0_rx_cycles::cycle_count().wrapping_sub(reorder_key_started),
        );
        let current_buffer = segment.buffer.as_ptr();
        let qos_control_offset = 24
            + if frame_control & 0x0300 == 0x0300 {
                6
            } else {
                0
            };
        let current_is_amsdu = frame_control & 0x0080 != 0
            && frame
                .mpdu
                .get(qos_control_offset)
                .is_some_and(|control| control & 0x80 != 0);
        let reorder_progress = {
            let mut dispatch = |ordered: oer_esp32s31_ieee80211_mac::rx::RxSegment<'_>| {
                #[cfg(feature = "task-poll-telemetry")]
                let dispatch_started = crate::diagnostics::core0_rx_cycles::cycle_count();
                AccessPointProtectedFrameDispatch::dispatch(
                    data_rx,
                    ordered,
                    |request| engine.admit_rx_data(request),
                    peer,
                    current_buffer as usize,
                    current_is_amsdu,
                    now,
                    &mut deferred,
                    &mut in_place,
                    #[cfg(any(feature = "diagnostics", test))]
                    report,
                    &mut activity_peer,
                    &mut produced_data,
                    #[cfg(feature = "task-poll-telemetry")]
                    &mut core0_ap_rx,
                );
                #[cfg(feature = "task-poll-telemetry")]
                {
                    protected_dispatch_cycles = protected_dispatch_cycles.wrapping_add(
                        crate::diagnostics::core0_rx_cycles::cycle_count()
                            .wrapping_sub(dispatch_started),
                    );
                }
            };
            if let Some(key) = key {
                state.rx_reorder.ingest(
                    state.rx_reorder_storage,
                    segment,
                    key,
                    ampdu_baseband_format,
                    now,
                    &mut dispatch,
                )
            } else {
                dispatch(segment);
                Ok(Default::default())
            }
        }?;
        (
            reorder_progress,
            deferred.used(),
            deferred.exhausted || in_place.unsupported,
            in_place.publication,
            produced_data,
        )
    };
    #[cfg(feature = "task-poll-telemetry")]
    core0_ap_rx.dispatch_complete(protected_dispatch_cycles);
    #[cfg(feature = "task-poll-telemetry")]
    crate::diagnostics::core0_ap_rx_cycles::CORE0_AP_RX_CYCLES.record_ingress_path(
        false,
        false,
        false,
        reorder_progress.buffered,
    );

    if let Some(reset) = reorder_progress.hardware_window_reset {
        observe_access_point!(state, observation, {
            observation.rx_reorder_hardware_window_resets = observation
                .rx_reorder_hardware_window_resets
                .saturating_add(1);
        });
        let agreement = state.rx_block_ack.snapshots_for(MacInterface::AccessPoint)
            [usize::from(reset.hardware_index)]
        .expect("AP reorder reset belongs to one live AP BlockAck agreement");
        state
            .protocol_actions
            .publisher()
            .try_publish(AccessPointProtocolAction::Hardware(
                AccessPointHardwareAction::ResetRxBlockAckWindow {
                    hardware_index: reset.hardware_index,
                    tid: agreement.tid,
                    starting_sequence: reset.starting_sequence,
                    window: RX_BLOCK_ACK_MAX_WINDOW,
                },
            ))
            .map_err(|_| AccessPointControlError::ProtocolActionCapacity)?;
    }
    observe_access_point!(state, observation, {
        if reorder_progress.duplicate {
            observation.protected_data_duplicates =
                observation.protected_data_duplicates.saturating_add(1);
        }
        if reorder_progress.buffered {
            observation.rx_reorder_buffered_mpdus =
                observation.rx_reorder_buffered_mpdus.saturating_add(1);
        }
        observation.rx_reorder_dispatched_mpdus = observation
            .rx_reorder_dispatched_mpdus
            .saturating_add(u32::from(reorder_progress.dispatched));
        if let Some(reason) = reorder_progress.dropped {
            let reason = match reason {
                rx_reorder::AccessPointRxReorderDrop::StorageExhausted => {
                    AccessPointRxRejectionReason::ReorderStorageExhausted
                }
                rx_reorder::AccessPointRxReorderDrop::FrameTooLong => {
                    AccessPointRxRejectionReason::ReorderFrameTooLong
                }
            };
            observation.record_rx_rejection(reason, segment, now.as_micros());
            observation.protected_data_protocol_rejected = observation
                .protected_data_protocol_rejected
                .saturating_add(1);
        }
    });
    if batch_exhausted {
        observe_access_point!(state, observation, {
            observation.protected_data_protocol_rejected = observation
                .protected_data_protocol_rejected
                .saturating_add(1);
        });
        return Err(AccessPointControlError::ReceiveBatchCapacity);
    }
    let mut batch_used = batch_used;
    if let Some(ethernet) = in_place_publication {
        #[cfg(any(feature = "diagnostics", test))]
        let raw = segment.buffer;
        #[cfg(any(feature = "diagnostics", test))]
        let payload =
            &raw[ethernet.payload_offset..ethernet.payload_offset + ethernet.payload_length];
        #[cfg(any(feature = "diagnostics", test))]
        let ethernet_frame = EthernetFrameParts {
            destination: ethernet.destination,
            source: ethernet.source,
            ether_type: ethernet.ether_type,
            payload,
        };
        #[cfg(any(feature = "diagnostics", test))]
        let protocol = ethernet_parts_protocol(ethernet_frame);
        #[cfg(feature = "diagnostics")]
        if let Some(observer) = delivery_observer {
            observer.admitted(RxNetworkDeliveryEvent::decoded(ethernet_frame, Some(raw)));
        }
        let frame = EthernetFrameParts {
            destination: ethernet.destination,
            source: ethernet.source,
            ether_type: ethernet.ether_type,
            payload: &segment.buffer
                [ethernet.payload_offset..ethernet.payload_offset + ethernet.payload_length],
        };
        let mut writer =
            crate::datapath::rx::ethernet::PackedEthernetWriter::resume(state.rx_frame, batch_used)
                .map_err(|_| AccessPointControlError::ReceiveBatchCapacity)?;
        writer
            .push(frame)
            .map_err(|_| AccessPointControlError::ReceiveBatchCapacity)?;
        batch_used = writer.used();
        drop(
            staged_frame
                .take()
                .expect("in-place AP publication owns the current staging frame"),
        );
        observe_access_point!(state, observation, {
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
        });
    }
    if batch_used != 0 {
        state.rx_batch_used = batch_used;
        state.rx_batch_offset = 0;
    }
    observe_access_point!(state, observation, {
        if !produced_data {
            observation.ignored_rx_frames = observation.ignored_rx_frames.saturating_add(1);
        }
    });
    #[cfg(not(any(feature = "diagnostics", test)))]
    let _ = produced_data;
    #[cfg(feature = "task-poll-telemetry")]
    core0_ap_rx.publication_complete();

    if let Some(peer) = activity_peer {
        observe_ap_rx_peer_activity(
            engine,
            &mut state.pending_buffered_releases,
            peer,
            power_state,
            now,
        )?;
    }
    #[cfg(feature = "task-poll-telemetry")]
    core0_ap_rx.finish();
    Ok(Some(AccessPointRxProtocolClass::ProtectedData))
}

impl<'storage, 'beacon, const DMA_BUFFER_SIZE: usize>
    AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>
{
    pub(super) fn rx_reorder_work_due(&self, now: oer_time::Instant) -> bool {
        self.state.rx_reorder.work_due(now)
    }

    /// Try the common protected-data RX leaf while the physical ordinary-TX
    /// owner remains parked. Any frame outside that exact leaf is returned
    /// unchanged so the caller can acquire TX and enter the complete AP role
    /// graph.
    pub fn service_routed_rx_while_parked<F>(
        &mut self,
        frame: F,
        now: oer_time::Instant,
        #[cfg(feature = "diagnostics")] delivery_observer: Option<&dyn RxNetworkDeliveryObserver>,
    ) -> Result<
        crate::roles::concurrent::RoutedRxDisposition<F>,
        AccessPointControlError,
    >
    where
        F: AccessPointStagedRxFrame,
    {
        if self.rx_batch_pending()
            || self.state.rx_reorder.work_due(now)
            || self.state.protocol_actions.remaining_capacity() < AP_PROTOCOL_ACTIONS_PER_RX_FRAME
        {
            return Ok(crate::roles::concurrent::RoutedRxDisposition::Deferred(frame));
        }

        let mut frame = Some(frame);
        let mac = &mut self.mac;
        let state = &mut self.state;
        let serviced = try_service_ap_staged_rx_data(
            mac.engine_mut(),
            state,
            &mut frame,
            now,
            #[cfg(feature = "diagnostics")]
            delivery_observer,
        )?;
        if serviced.is_some() {
            state.serviced_rx_frames = state.serviced_rx_frames.saturating_add(1);
            Ok(crate::roles::concurrent::RoutedRxDisposition::Processed)
        } else {
            Ok(
                crate::roles::concurrent::RoutedRxDisposition::Deferred(
                    frame.expect("non-mutating direct AP miss retains the staging owner"),
                ),
            )
        }
    }
}

#[cfg(test)]
#[path = "protocol_service/ht_rx_observation_tests.rs"]
mod ht_rx_observation_tests;

impl<'storage, 'beacon, 'slot, P, E, T, const DMA_BUFFER_SIZE: usize, const TX_BUFFER_SIZE: usize>
    AccessPointProtocolProcessor<
        'storage,
        'beacon,
        'slot,
        P,
        E,
        T,
        DMA_BUFFER_SIZE,
        TX_BUFFER_SIZE,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
{
    /// Construct AP protocol/control state without binding a DMA producer or
    /// private RX queue. Same-channel STA+AP composition owns those physical
    /// resources at its common DATAPATH boundary.
    pub fn new(
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
        let access_point = mac.engine().service_address();
        let security = mac.engine().link_protection();
        data_rx.reset(ApRxConfig {
            access_point,
            ingress: RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
            security,
        });
        rx_block_ack
            .prepare_interface(MacInterface::AccessPoint)
            .expect("AP start requires its previous RX BlockAck epoch to be quiescent");
        let discarded_reorder_frames = rx_reorder.discard_all();
        debug_assert_eq!(discarded_reorder_frames, 0);
        #[cfg(feature = "diagnostics")]
        observation_storage.reset();
        Self {
            mac,
            state: AccessPointProtocolState {
                rx_frame,
                tx_frame,
                data_rx,
                rx_block_ack,
                rx_reorder,
                rx_reorder_storage,
                rx_addba_in_flight: None,
                protocol_actions: AccessPointProtocolMailbox::new(),
                pending_buffered_releases: PendingApBufferedReleases::new(),
                pending_dtim_group_frames: None,
                rx_batch_used: 0,
                rx_batch_offset: 0,
                serviced_rx_frames: 0,
                serviced_rx_descriptors: 0,
                last_terminal_tx_succeeded: None,
                #[cfg(feature = "diagnostics")]
                observer: observation_storage,
                #[cfg(all(test, not(feature = "diagnostics")))]
                observer: AccessPointObservationStorage::default(),
                #[cfg(any(feature = "diagnostics", test))]
                terminal_observer: None,
            },
        }
    }

    /// Attach the non-owning terminal observer for this AP role epoch.
    #[cfg(any(feature = "diagnostics", test))]
    pub fn with_terminal_observer(
        mut self,
        observer: &'static dyn AccessPointTerminalObserver,
    ) -> Self {
        self.terminal_observer = Some(observer);
        self
    }

    /// Remove the unique ordinary-TX capability at an idle transaction edge.
    /// A pending publication fails closed and returns the complete processor
    /// unchanged, so the caller cannot lose either hardware or protocol state.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn try_park(
        self,
    ) -> Result<
        (
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>,
        ),
        Self,
    > {
        let Self { mac, state } = self;
        match mac.try_park() {
            Ok((resources, mac)) => Ok((
                resources,
                AccessPointProtocolProcessorParked { mac, state },
            )),
            Err(mac) => Err(Self { mac, state }),
        }
    }

    /// Reconstitute the AP processor from its exact role state and the sole
    /// ordinary-TX capability owned by the paired physical transaction.
    pub fn resume(
        resources: WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
        parked: AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>,
    ) -> Self {
        let AccessPointProtocolProcessorParked { mac, state } = parked;
        Self {
            mac: ApMac::resume(resources, mac),
            state,
        }
    }

    /// Consume one frame already classified for the AP by the common physical
    /// RX dispatcher.
    ///
    /// This path owns no DMA operation and never reads an AP-private queue. It
    /// is the protocol boundary used by same-channel STA+AP composition. If
    /// ordering or an active hardware TX prevents safe processing, the exact
    /// staging lease is returned instead of copied or dropped.
    pub fn service_routed_rx<H, F, S>(
        &mut self,
        hardware: &mut H,
        frame: F,
        security_material: &mut S,
        now: oer_time::Instant,
        #[cfg(feature = "diagnostics")] delivery_observer: Option<&dyn RxNetworkDeliveryObserver>,
    ) -> Result<
        crate::roles::concurrent::RoutedRxDisposition<F>,
        AccessPointControlError,
    >
    where
        H: TxHardware + ApRuntimeHardware + RxBlockAckHardware,
        F: AccessPointStagedRxFrame,
        S: FnMut() -> ([u8; 32], u64),
    {
        let tx_pending = self.mac.tx_pending();
        self.apply_protocol_actions(hardware)?;
        if self.rx_batch_pending() || self.service_rx_reorder_expiry(now)? {
            return Ok(crate::roles::concurrent::RoutedRxDisposition::Deferred(frame));
        }

        if tx_pending
            && !rx_pipeline::can_process_ap_frame_during_tx(
                frame.segment(),
                self.mac.engine().link_protection(),
            )
        {
            return Ok(crate::roles::concurrent::RoutedRxDisposition::Deferred(frame));
        }

        self.serviced_rx_frames = self.serviced_rx_frames.saturating_add(1);
        #[cfg(feature = "diagnostics")]
        self.sample_rx_block_ack_hardware(hardware);
        #[cfg(feature = "diagnostics")]
        let protocol_started = self.now().as_micros();
        let protocol_class = self.service_staged_rx(
            rx_protocol_consumer_has_hardware(tx_pending).then_some(hardware),
            frame,
            security_material,
            now,
            #[cfg(feature = "diagnostics")]
            delivery_observer,
        )?;
        self.apply_protocol_actions(hardware)?;

        #[cfg(not(feature = "diagnostics"))]
        let _ = protocol_class;
        #[cfg(feature = "diagnostics")]
        self.observe_rx_protocol_service(
            protocol_class,
            self.now().as_micros().saturating_sub(protocol_started),
        );
        Ok(crate::roles::concurrent::RoutedRxDisposition::Processed)
    }

    #[cfg(feature = "diagnostics")]
    pub(super) fn sample_rx_block_ack_hardware<H: RxBlockAckHardware>(&mut self, hardware: &mut H) {
        // Four or five samples cover a ten-second HT40 ceiling interval while
        // keeping the bounded UART log well below per-frame instrumentation.
        if self.serviced_rx_frames < self.observer.next_rx_block_ack_hardware_sample {
            return;
        }
        self.observer.next_rx_block_ack_hardware_sample = self
            .serviced_rx_frames
            .saturating_div(8_192)
            .saturating_add(1)
            .saturating_mul(8_192);
        for agreement in self
            .rx_block_ack
            .snapshots_for(MacInterface::AccessPoint)
            .into_iter()
            .flatten()
        {
            let Some(snapshot) =
                hardware.diagnostic_rx_block_ack_entry_snapshot(agreement.hardware_index)
            else {
                log::warn!(
                    "open-radio: AP live RX BA sample={} bank={} unavailable",
                    self.serviced_rx_frames,
                    agreement.hardware_index,
                );
                continue;
            };
            let configuration_matches = snapshot.enabled
                && snapshot.write_enabled
                && snapshot.valid
                && snapshot.control_unknown_clear
                && snapshot.peer == agreement.peer
                && snapshot.interface == agreement.interface
                && snapshot.tid == agreement.tid
                && u16::from(snapshot.window) == RX_BLOCK_ACK_MAX_WINDOW;
            log::info!(
                "open-radio: AP live RX BA sample={} bank={} matches={} enabled={} write={} valid={} clean={} interface={:?} tid={} window={} current={} loaded_start={} bitmap_status={:016x} bitmap_load={:016x}",
                self.serviced_rx_frames,
                agreement.hardware_index,
                configuration_matches,
                snapshot.enabled,
                snapshot.write_enabled,
                snapshot.valid,
                snapshot.control_unknown_clear,
                snapshot.interface,
                snapshot.tid,
                snapshot.window,
                snapshot.current_sequence,
                snapshot.loaded_start_sequence,
                snapshot.bitmap_status,
                snapshot.bitmap_load,
            );
        }
    }

    /// Consume only protected data while another transaction owns the
    /// physical TX domain.
    ///
    /// The frame parser may update role-local reorder/report state and append
    /// value-only mailbox actions. It cannot borrow MMIO or publish a frame;
    /// management and EAPOL owners are returned unchanged for the first idle
    /// transaction boundary.
    pub fn service_routed_rx_during_tx<H, F, S>(
        &mut self,
        frame: F,
        security_material: &mut S,
        now: oer_time::Instant,
        #[cfg(feature = "diagnostics")] delivery_observer: Option<&dyn RxNetworkDeliveryObserver>,
    ) -> Result<
        crate::roles::concurrent::RoutedRxDisposition<F>,
        AccessPointControlError,
    >
    where
        H: TxHardware + ApRuntimeHardware + RxBlockAckHardware,
        F: AccessPointStagedRxFrame,
        S: FnMut() -> ([u8; 32], u64),
    {
        if self.rx_batch_pending()
            || !rx_pipeline::can_process_ap_frame_during_tx(
                frame.segment(),
                self.mac.engine().link_protection(),
            )
        {
            return Ok(crate::roles::concurrent::RoutedRxDisposition::Deferred(frame));
        }
        // Do not consume the affine staging lease unless every value-only
        // action this frame can produce has a slot. A long physical TX may
        // admit several DMA/protocol turns before hardware can drain the
        // mailbox; the exact ordered head remains queued instead of turning
        // bounded backpressure into a role fault.
        if self.protocol_actions.remaining_capacity() < AP_PROTOCOL_ACTIONS_PER_RX_FRAME {
            return Ok(crate::roles::concurrent::RoutedRxDisposition::Deferred(frame));
        }
        if self
            .rx_reorder
            .next_deadline()
            .is_some_and(|deadline| deadline <= now)
        {
            return Ok(crate::roles::concurrent::RoutedRxDisposition::Deferred(frame));
        }

        self.serviced_rx_frames = self.serviced_rx_frames.saturating_add(1);
        #[cfg(feature = "diagnostics")]
        let protocol_started = self.now().as_micros();
        let protocol_class = self.service_staged_rx::<H, _, _>(
            None,
            frame,
            security_material,
            now,
            #[cfg(feature = "diagnostics")]
            delivery_observer,
        )?;
        #[cfg(not(feature = "diagnostics"))]
        let _ = protocol_class;
        #[cfg(feature = "diagnostics")]
        self.observe_rx_protocol_service(
            protocol_class,
            self.now().as_micros().saturating_sub(protocol_started),
        );
        Ok(crate::roles::concurrent::RoutedRxDisposition::Processed)
    }

    /// Execute value-only RX actions after the physical transaction owner has
    /// returned. This is the sole paired-role mailbox drain edge.
    pub fn apply_pending_protocol_actions<H>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), AccessPointControlError>
    where
        H: RxBlockAckHardware,
    {
        #[cfg(feature = "diagnostics")]
        self.sample_rx_block_ack_hardware(hardware);
        self.apply_protocol_actions(hardware)
    }

    fn retain_power_save_action(
        &mut self,
        action: ApPowerSaveAction,
    ) -> Result<(), AccessPointControlError> {
        let mac = &mut self.mac;
        let state = &mut self.state;
        retain_ap_power_save_action(
            mac.engine_mut(),
            &mut state.pending_buffered_releases,
            action,
        )
    }

    pub(super) fn take_pending_buffered_release(&mut self) -> Option<ApBufferedUnicastRelease> {
        self.state.pending_buffered_releases.pop_current(self.mac.engine())
    }

    pub(super) fn rollback_pending_buffered_releases(
        &mut self,
    ) -> Result<(), AccessPointControlError> {
        while let Some(release) = self.take_pending_buffered_release() {
            self.mac
                .engine_mut()
                .complete_buffered_unicast_release(release, false)?;
        }
        Ok(())
    }

    #[cfg(feature = "diagnostics")]
    fn observe_rx_protocol_service(
        &mut self,
        protocol_class: AccessPointRxProtocolClass,
        elapsed: u64,
    ) {
        self.observer.observation.maximum_rx_protocol_service_micros = self
            .observer
            .observation
            .maximum_rx_protocol_service_micros
            .max(u32::try_from(elapsed).unwrap_or(u32::MAX));
        let elapsed = u32::try_from(elapsed).unwrap_or(u32::MAX);
        let class_maximum = match protocol_class {
            AccessPointRxProtocolClass::ProtectedData => {
                self.observer
                    .observation
                    .total_rx_protected_data_service_micros = self
                    .observer
                    .observation
                    .total_rx_protected_data_service_micros
                    .saturating_add(elapsed);
                Some(
                    &mut self
                        .observer
                        .observation
                        .maximum_rx_protected_data_service_micros,
                )
            }
            AccessPointRxProtocolClass::Management => Some(
                &mut self
                    .observer
                    .observation
                    .maximum_rx_management_service_micros,
            ),
            AccessPointRxProtocolClass::Eapol => {
                Some(&mut self.observer.observation.maximum_rx_eapol_service_micros)
            }
            AccessPointRxProtocolClass::Other | AccessPointRxProtocolClass::Rejected => None,
        };
        if let Some(class_maximum) = class_maximum {
            *class_maximum = (*class_maximum).max(elapsed);
        }
    }

    /// Consume the complete TX-independent AP data path through the same
    /// role-local state used by a parked AP role.
    fn try_service_staged_rx_data<F>(
        &mut self,
        staged_frame: &mut Option<F>,
        now: oer_time::Instant,
        #[cfg(feature = "diagnostics")] delivery_observer: Option<&dyn RxNetworkDeliveryObserver>,
    ) -> Result<Option<AccessPointRxProtocolClass>, AccessPointControlError>
    where
        F: AccessPointStagedRxFrame,
    {
        let mac = &mut self.mac;
        let state = &mut self.state;
        try_service_ap_staged_rx_data(
            mac.engine_mut(),
            state,
            staged_frame,
            now,
            #[cfg(feature = "diagnostics")]
            delivery_observer,
        )
    }

    /// Consume one staged AP RX owner on the protocol hot path.
    ///
    /// Saturated AP RX keeps this routine resident for most of the radio-task
    /// budget.  The S31 PSRAM-code profile therefore places the routine in the
    /// semantic hot-text class; the board linker decides whether that class is
    /// backed by internal executable SRAM.  This does not make the protocol
    /// routine interrupt-safe and does not change its ownership semantics.
    #[cfg_attr(
        all(target_arch = "riscv32", not(feature = "task-poll-telemetry")),
        unsafe(link_section = ".hot.text.open_radio_ap_rx")
    )]
    #[inline(never)]
    fn service_staged_rx<H, F, S>(
        &mut self,
        mut hardware: Option<&mut H>,
        staged_frame: F,
        security_material: &mut S,
        now: oer_time::Instant,
        #[cfg(feature = "diagnostics")] delivery_observer: Option<&dyn RxNetworkDeliveryObserver>,
    ) -> Result<AccessPointRxProtocolClass, AccessPointControlError>
    where
        H: TxHardware + ApRuntimeHardware + RxBlockAckHardware,
        F: AccessPointStagedRxFrame,
        S: FnMut() -> ([u8; 32], u64),
    {
        let mut staged_frame = Some(staged_frame);
        if let Some(protocol_class) = self.try_service_staged_rx_data(
            &mut staged_frame,
            now,
            #[cfg(feature = "diagnostics")]
            delivery_observer,
        )? {
            return Ok(protocol_class);
        }
        let segment = staged_frame
            .as_ref()
            .expect("current AP staged frame is live")
            .segment();
        let frame = match view_normalized_rx_frame(
            &segment,
            RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
        ) {
            Ok(frame) => frame,
            Err(_error) => {
                #[cfg(any(feature = "diagnostics", test))]
                let duplicate_or_stale = matches!(
                    _error,
                    oer_esp32s31_ieee80211_mac::rx::RxError::Quarantined
                ) && self
                    .state
                    .data_rx
                    .reorder_key(segment)
                    .is_some_and(|key| self.state.rx_reorder.is_duplicate_or_stale(key));
                observe_access_point!(self, observation, {
                    match _error {
                        oer_esp32s31_ieee80211_mac::rx::RxError::MicFailure => {
                            observation.rx_mic_failures =
                                observation.rx_mic_failures.saturating_add(1);
                        }
                        oer_esp32s31_ieee80211_mac::rx::RxError::Quarantined => {
                            if duplicate_or_stale {
                                observation.protected_data_duplicates =
                                    observation.protected_data_duplicates.saturating_add(1);
                            } else {
                                observation.rx_quarantined_frames =
                                    observation.rx_quarantined_frames.saturating_add(1);
                            }
                        }
                        _ => {
                            observation.rx_view_rejected =
                                observation.rx_view_rejected.saturating_add(1);
                        }
                    }
                    observation.ignored_rx_frames = observation.ignored_rx_frames.saturating_add(1);
                });
                return Ok(AccessPointRxProtocolClass::Rejected);
            }
        };
        let frame_control = u16::from_le_bytes([frame.mpdu[0], frame.mpdu[1]]);
        let data_frame = frame_control & 0x000c == 0x0008;
        let power_save_observation =
            observe_ap_power_save_for_access_point(frame.mpdu, self.mac.engine().service_address());
        let null_data_power_save_observation = observe_ap_null_data_power_save_for_access_point(
            frame.mpdu,
            self.mac.engine().service_address(),
        );
        let protocol_class = if frame_control & 0x000c == 0 {
            let hardware = hardware
                .take()
                .ok_or(AccessPointControlError::ProtocolFrameRequiresHardware)?;
            if self.service_management(hardware, frame.mpdu, security_material, now)? {
                observe_access_point!(self, observation, {
                    observation.control_frames_staged =
                        observation.control_frames_staged.saturating_add(1);
                });
            }
            AccessPointRxProtocolClass::Management
        } else if data_frame {
            let hardware = hardware
                .take()
                .ok_or(AccessPointControlError::ProtocolFrameRequiresHardware)?;
            if self.service_eapol(hardware, frame.mpdu, now)? {
                AccessPointRxProtocolClass::Eapol
            } else {
                observe_access_point!(self, observation, {
                    observation.security_mode_mismatches =
                        observation.security_mode_mismatches.saturating_add(1);
                });
                AccessPointRxProtocolClass::Rejected
            }
        } else if let Some(request) = self.state.data_rx.block_ack_request_key(segment) {
            // A client BlockAckReq moves its receive window; the frames it
            // releases leave through the ordered pending-release queue.
            let _moved = self.state.rx_reorder.move_window(request, now);
            observe_access_point!(self, observation, {
                observation.rx_block_ack_requests =
                    observation.rx_block_ack_requests.saturating_add(1);
                if let Some(moved) = _moved {
                    observation.rx_reorder_bar_released_mpdus = observation
                        .rx_reorder_bar_released_mpdus
                        .saturating_add(u32::from(moved.released));
                    observation.rx_reorder_bar_missing_sequences = observation
                        .rx_reorder_bar_missing_sequences
                        .saturating_add(u32::from(moved.missing));
                }
            });
            AccessPointRxProtocolClass::Other
        } else {
            observe_access_point!(self, observation, {
                observation.ignored_rx_frames = observation.ignored_rx_frames.saturating_add(1);
            });
            AccessPointRxProtocolClass::Other
        };
        let null_data_activity = match null_data_power_save_observation {
            Some(ApPowerSaveObservation::Sleeping { peer })
                if self.mac.engine().is_authorized_peer(peer) =>
            {
                Some((peer, Some(ApPeerPowerState::Sleeping)))
            }
            Some(ApPowerSaveObservation::Active { peer })
                if self.mac.engine().is_authorized_peer(peer) =>
            {
                Some((peer, Some(ApPeerPowerState::Active)))
            }
            _ => None,
        };
        if let Some((peer, power_state)) = null_data_activity {
            self.observe_rx_peer_activity(peer, power_state, now)?;
        }
        if let Some(observation @ ApPowerSaveObservation::PsPoll { .. }) = power_save_observation {
            let action = self
                .mac
                .engine_mut()
                .observe_power_save(observation, now)?;
            self.retain_power_save_action(action)?;
        }
        Ok(protocol_class)
    }

    fn apply_protocol_actions<H>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), AccessPointControlError>
    where
        H: RxBlockAckHardware,
    {
        while let Some(action) = self.protocol_actions.receiver().try_receive() {
            match action {
                AccessPointProtocolAction::Hardware(
                    AccessPointHardwareAction::ResetRxBlockAckWindow {
                        hardware_index,
                        tid,
                        starting_sequence,
                        window,
                    },
                ) => hardware.reset_rx_block_ack_window(
                    hardware_index,
                    tid,
                    starting_sequence,
                    window,
                )?,
            }
        }
        Ok(())
    }

    fn observe_rx_peer_activity(
        &mut self,
        peer: [u8; 6],
        power_state: Option<ApPeerPowerState>,
        at: oer_time::Instant,
    ) -> Result<(), AccessPointControlError> {
        let mac = &mut self.mac;
        let state = &mut self.state;
        observe_ap_rx_peer_activity(
            mac.engine_mut(),
            &mut state.pending_buffered_releases,
            peer,
            power_state,
            at,
        )
    }

    fn service_management<H, S>(
        &mut self,
        hardware: &mut H,
        mpdu: &[u8],
        security_material: &mut S,
        now: oer_time::Instant,
    ) -> Result<bool, AccessPointControlError>
    where
        H: TxHardware + ApRuntimeHardware + RxBlockAckHardware,
        S: FnMut() -> ([u8; 32], u64),
    {
        let request = parse_ap_management_request(
            &oer_esp32s31_ieee80211_ap::profile::ADVERTISEMENT,
            mpdu,
            self.mac.engine().service_address(),
        );
        if let Some(ApManagementRequest::BlockAck { peer, action }) = request {
            match action {
                BlockAckAction::AddbaRequest {
                    dialog_token,
                    tid,
                    immediate,
                    window,
                    timeout_tu,
                    starting_sequence,
                    ..
                } if self.mac.engine().is_authorized_peer(peer) => {
                    if self.mac.engine().link_protection() == LinkProtection::Open {
                        self.publish_declined_rx_addba(hardware, peer, dialog_token, tid, window)?;
                        return Ok(true);
                    }
                    let offered = self.rx_block_ack.offer(RxBlockAckRequest {
                        interface: MacInterface::AccessPoint,
                        peer,
                        dialog_token,
                        tid,
                        immediate,
                        requested_window: window,
                        timeout_tu,
                        starting_sequence,
                    });
                    if offered.is_err() {
                        self.publish_declined_rx_addba(hardware, peer, dialog_token, tid, window)?;
                        return Ok(true);
                    }
                    let activation = match self.rx_block_ack.begin_pending() {
                        Ok(Some(activation)) => activation,
                        Ok(None) => return Ok(false),
                        Err(RxBlockAckSessionsError::NoFreeHardwareBank) => {
                            let discarded = self.rx_block_ack.discard_pending(
                                MacInterface::AccessPoint,
                                peer,
                                tid,
                            );
                            debug_assert!(discarded);
                            self.publish_declined_rx_addba(
                                hardware,
                                peer,
                                dialog_token,
                                tid,
                                window,
                            )?;
                            return Ok(true);
                        }
                        Err(error) => return Err(error.into()),
                    };
                    self.start_rx_addba_response(hardware, activation, now)?;
                    return Ok(true);
                }
                BlockAckAction::Delba {
                    tid,
                    initiator: true,
                    ..
                } => {
                    if let Some(agreement) =
                        self.rx_block_ack.stop(MacInterface::AccessPoint, peer, tid)
                    {
                        self.release_rx_reorder(agreement.identity(), now)?;
                        hardware.clear_rx_block_ack(agreement.hardware_index)?;
                    }
                    return Ok(false);
                }
                _ => {}
            }
        }

        // Entropy belongs to the transition which creates a fresh WPA2
        // authenticator, not to generic RX ingress.  The old call topology
        // generated ten TRNG words before classifying every MPDU, including
        // ordinary protected UDP data.  Only a new WPA2 association from an
        // authenticated peer can consume these values; retries in Securing
        // phase preserve their existing handshake epoch.
        let peer_phase = match request {
            Some(ApManagementRequest::Association { peer, .. }) => self
                .mac
                .engine()
                .peer_status(peer)
                .map(|status| status.phase),
            _ => None,
        };
        let (authenticator_nonce, initial_replay_counter) = ap_security_material_for_management(
            self.mac.engine().link_protection(),
            request,
            peer_phase,
            security_material,
        );
        let mac = &mut self.mac;
        let tx_frame = &mut *self.state.tx_frame;
        let outcome = mac.publish_management(
            hardware,
            mpdu,
            authenticator_nonce,
            initial_replay_counter,
            now,
            tx_frame,
        )?;
        if let oer_esp32s31_ieee80211_ap::engine::ApManagementOutcome::PeerRemoved {
            peer,
        } = outcome
        {
            self.discard_rx_peer(hardware, peer)?;
        }
        Ok(matches!(
            outcome,
            oer_esp32s31_ieee80211_ap::engine::ApManagementOutcome::Response { .. }
        ))
    }

    fn publish_declined_rx_addba<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        dialog_token: u8,
        tid: u8,
        requested_window: u16,
    ) -> Result<(), AccessPointControlError> {
        let mut body = [0_u8; 9];
        write_declined_addba_response(&mut body, dialog_token, tid, requested_window)
            .map_err(RxBlockAckSessionsError::Response)?;
        let mac = &mut self.mac;
        let tx_frame = &mut *self.state.tx_frame;
        mac.publish_rx_block_ack_response(hardware, peer, &body, tx_frame)?;
        Ok(())
    }

    fn start_rx_addba_response<H>(
        &mut self,
        hardware: &mut H,
        activation: RxBlockAckActivation,
        now: oer_time::Instant,
    ) -> Result<(), AccessPointControlError>
    where
        H: TxHardware + ApRuntimeHardware + RxBlockAckHardware,
    {
        debug_assert!(self.rx_addba_in_flight.is_none());
        if let Some(replaced) = activation.replaced() {
            if let Err(error) = self.release_rx_reorder(replaced.identity(), now) {
                self.rx_block_ack.cancel(activation)?;
                return Err(error);
            }
            if let Err(error) = hardware.clear_rx_block_ack(replaced.hardware_index) {
                self.rx_block_ack.cancel(activation)?;
                return Err(error.into());
            }
        }
        let negotiated = activation.negotiated();
        // SOURCE: complete vendor `ht_recv_action_ba_addba_request` first
        // enqueues the successful ADDBA response through
        // `ieee80211_send_action`, then publishes the receive agreement via
        // `ic_add_rx_ba`. The direct bank must not become
        // visible before the response publication edge.
        let mac = &mut self.mac;
        let tx_frame = &mut *self.state.tx_frame;
        if let Err(error) = mac.publish_rx_block_ack_response(
            hardware,
            negotiated.peer,
            activation.response_body(),
            tx_frame,
        ) {
            self.rx_block_ack.cancel(activation)?;
            return Err(error.into());
        }
        if let Err(error) = hardware.program_rx_block_ack(activation.hardware()) {
            self.rx_block_ack.cancel(activation)?;
            return Err(error.into());
        }
        if let Err(error) = self.rx_reorder.start(negotiated, |_| {}) {
            let clear = hardware.clear_rx_block_ack(negotiated.hardware_index);
            self.rx_block_ack.cancel(activation)?;
            clear?;
            return Err(error.into());
        }
        self.rx_addba_in_flight = Some(activation);
        Ok(())
    }

    fn release_rx_reorder(
        &mut self,
        identity: oer_esp32s31_ieee80211_mac::rx::ampdu::RxBlockAckIdentity,
        now: oer_time::Instant,
    ) -> Result<(), AccessPointControlError> {
        let processor = &mut *self;
        let mac = &mut processor.mac;
        let state = &mut processor.state;
        let data_rx = &mut state.data_rx;
        #[cfg(any(feature = "diagnostics", test))]
        let report = &mut state.observer.observation;
        let mut activity_peer = None;
        let mut sink = DeferredAccessPointRxSink::new(state.rx_frame);
        let _ = state.rx_reorder.stop(identity, |segment| {
            let peer = data_rx.reorder_key(segment).map(|key| key.peer);
            let outcome = data_rx.dispatch_at(
                segment,
                now,
                |request| mac.engine_mut().admit_rx_data(request),
                &mut sink,
            );
            #[cfg(any(feature = "diagnostics", test))]
            {
                report.record_dispatch_rejection(outcome, segment, now.as_micros());
                if sink.exhausted {
                    report.record_rx_rejection(
                        AccessPointRxRejectionReason::DeferredOutputCapacity,
                        segment,
                        now.as_micros(),
                    );
                }
            }
            let _ = observe_protected_dispatch(
                outcome,
                peer,
                #[cfg(any(feature = "diagnostics", test))]
                report,
                &mut activity_peer,
            );
        });
        if sink.exhausted {
            return Err(AccessPointControlError::ReceiveBatchCapacity);
        }
        if let Some(peer) = activity_peer {
            processor
                .mac
                .engine_mut()
                .observe_peer_activity(peer, now)?;
        }
        let used = sink.used();
        if used != 0 {
            state.rx_batch_used = used;
            state.rx_batch_offset = 0;
        }
        Ok(())
    }

    fn discard_rx_peer<H: RxBlockAckHardware>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
    ) -> Result<(), AccessPointControlError> {
        for agreement in self
            .rx_block_ack
            .stop_peer(MacInterface::AccessPoint, peer)
            .into_iter()
            .flatten()
        {
            let _ = self.rx_reorder.stop_discard(agreement.identity());
            hardware.clear_rx_block_ack(agreement.hardware_index)?;
        }
        self.data_rx.forget_peer(peer);
        Ok(())
    }

    pub(super) fn service_rx_reorder_expiry(
        &mut self,
        now: oer_time::Instant,
    ) -> Result<bool, AccessPointControlError> {
        let processor = &mut *self;
        let mac = &mut processor.mac;
        let state = &mut processor.state;
        let data_rx = &mut state.data_rx;
        #[cfg(any(feature = "diagnostics", test))]
        let report = &mut state.observer.observation;
        let mut activity_peer = None;
        let mut sink = DeferredAccessPointRxSink::new(state.rx_frame);
        let pending_dispatched = state.rx_reorder.dispatch_pending(|segment| {
            let peer = data_rx.reorder_key(segment).map(|key| key.peer);
            let outcome = data_rx.dispatch_at(
                segment,
                now,
                |request| mac.engine_mut().admit_rx_data(request),
                &mut sink,
            );
            #[cfg(any(feature = "diagnostics", test))]
            {
                report.record_dispatch_rejection(outcome, segment, now.as_micros());
                if sink.exhausted {
                    report.record_rx_rejection(
                        AccessPointRxRejectionReason::DeferredOutputCapacity,
                        segment,
                        now.as_micros(),
                    );
                }
            }
            let _ = observe_protected_dispatch(
                outcome,
                peer,
                #[cfg(any(feature = "diagnostics", test))]
                report,
                &mut activity_peer,
            );
        });
        let (dispatched, _gap_timeout) = if pending_dispatched {
            (1, false)
        } else {
            let dispatched = state.rx_reorder.expire_due(now, |segment| {
                let peer = data_rx.reorder_key(segment).map(|key| key.peer);
                let outcome = data_rx.dispatch_at(
                    segment,
                    now,
                    |request| mac.engine_mut().admit_rx_data(request),
                    &mut sink,
                );
                #[cfg(any(feature = "diagnostics", test))]
                {
                    report.record_dispatch_rejection(outcome, segment, now.as_micros());
                    if sink.exhausted {
                        report.record_rx_rejection(
                            AccessPointRxRejectionReason::DeferredOutputCapacity,
                            segment,
                            now.as_micros(),
                        );
                    }
                }
                let _ = observe_protected_dispatch(
                    outcome,
                    peer,
                    #[cfg(any(feature = "diagnostics", test))]
                    report,
                    &mut activity_peer,
                );
            });
            (dispatched, dispatched != 0)
        };
        observe_access_point!(state, observation, {
            if _gap_timeout {
                observation.rx_reorder_gap_timeouts =
                    observation.rx_reorder_gap_timeouts.saturating_add(1);
            }
            observation.rx_reorder_dispatched_mpdus = observation
                .rx_reorder_dispatched_mpdus
                .saturating_add(u32::from(dispatched));
        });
        if sink.exhausted {
            return Err(AccessPointControlError::ReceiveBatchCapacity);
        }
        if let Some(peer) = activity_peer {
            processor
                .mac
                .engine_mut()
                .observe_peer_activity(peer, now)?;
        }
        let used = sink.used();
        if used != 0 {
            state.rx_batch_used = used;
            state.rx_batch_offset = 0;
            return Ok(true);
        }
        Ok(dispatched != 0)
    }

    pub const fn rx_batch_pending(&self) -> bool {
        self.state.rx_batch_offset < self.state.rx_batch_used
    }

    /// Software-owned ordered RX work that must run before a newer AP MPDU.
    ///
    /// A window advance can release more frames than fit in the one-frame
    /// publication batch. Those releases retain their cold backing and their
    /// sequence position independently of a fresh MAC interrupt. Treat them
    /// exactly like a due reorder timer so the parked fast path cannot publish
    /// a newer current frame ahead of an older released owner.
    pub(super) fn rx_reorder_work_due(&self, now: oer_time::Instant) -> bool {
        self.state.rx_reorder.work_due(now)
    }

    #[cfg(any(feature = "diagnostics", test))]
    fn observe_ht_aggregate(&mut self, rate: HtRate, association_id: u16, subframes: usize) {
        observe_access_point!(self, observation, {
            if let Some(fill) = usize::from(association_id)
                .checked_sub(1)
                .and_then(|index| observation.tx_aggregate_fill.get_mut(index))
            {
                fill.record(association_id, u8::try_from(subframes).unwrap_or(u8::MAX));
            }
            observation.tx_ht_aggregates = observation.tx_ht_aggregates.saturating_add(1);
            if rate.channel_width == HtChannelWidth::Mhz40 && rate.mcs == HtMcs::Mcs7 {
                observation.tx_ht40_mcs7_aggregates =
                    observation.tx_ht40_mcs7_aggregates.saturating_add(1);
            }
        });
    }
}
