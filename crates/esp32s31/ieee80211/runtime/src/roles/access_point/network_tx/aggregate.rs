//! Standby A-MPDU preparation over peer-bound retained network leases.
//! Publication and cancellation remain on the shared network TX owner.

use super::*;

pub(super) const fn aggregate_adapter_available(ordinary_publication_pending: bool) -> bool {
    !ordinary_publication_pending
}

impl<'observer, B, N> AccessPointNetworkTx<'observer, B, N>
where
    B: MaterializedTxFrame,
    N: SoftwareTxFrame,
{
    pub(in super::super) fn advance_prepared<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
        const SLOTS: usize,
        const BUFFER_SIZE: usize,
    >(
        &mut self,
        aggregate: &mut AccessPointAmpdu<'_, B, SLOTS, BUFFER_SIZE>,
        control: &mut AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        network: &impl SelectedBurstMaterializer<SoftwareFrame = N, PhysicalFrame = B>,
    ) -> Result<(), AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: oer_time::Timer,
    {
        self.prepare_ready_standby(aggregate, control, network)?;
        if TX_PHASE_TELEMETRY {
            self.record_partial_frontier(network);
        }
        Ok(())
    }

    pub(super) fn record_partial_frontier(
        &self,
        network: &impl SelectedBurstMaterializer<SoftwareFrame = N, PhysicalFrame = B>,
    ) {
        let Some(batch) = self.prepared_standby.as_ref() else {
            return;
        };
        if batch.admitted >= usize::from(batch.policy.frame_limit()) {
            return;
        }
        let key = ApTxFlowKey::associated(batch.admission.association());
        let matching_retained = self.active_frames.len_for(key)
            + usize::from(self.prepared_first_key == Some(key))
            + usize::from(self.prepared_second_key == Some(key));
        let retained = self.active_frames.len()
            + usize::from(self.prepared_first.is_some())
            + usize::from(self.prepared_second.is_some());
        CORE0_PERFORMANCE.record_ap_partial_frontier(
            matching_retained,
            retained.saturating_sub(matching_retained),
            network.queue_len(),
            batch.mismatch_claims.value(),
        );
        let ownership = network.ownership_snapshot();
        CORE0_PERFORMANCE.record_ap_partial_publication(
            batch.admitted,
            ownership.free,
            0,
            0,
            0,
            0,
            0,
            ownership.radio_owned,
            ownership
                .radio_owned
                .saturating_sub(batch.admitted.saturating_add(retained)),
        );
    }
    #[allow(clippy::too_many_arguments)]
    pub(in super::super) fn prepare<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
        const SLOTS: usize,
        const BUFFER_SIZE: usize,
    >(
        &mut self,
        aggregate: &mut AccessPointAmpdu<'_, B, SLOTS, BUFFER_SIZE>,
        control: &mut AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        frame: N,
        network: &impl SelectedBurstMaterializer<SoftwareFrame = N, PhysicalFrame = B>,
    ) -> Result<(), AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: oer_time::Timer,
    {
        assert!(
            (self.aggregate_pending() || self.has_prepared()) && aggregate.has_standby(),
            "DATAPATH must check AP standby ownership before claiming another ordered lease"
        );
        self.retain_active_frame(control.mac.engine_mut(), frame)?;

        // The AP-specific peer, power-save and key checks remain per frame.
        // Destination sources are pulled directly by advance_prepared;
        // FIFO-only sources enter the bounded radio retention path here.
        self.prepare_ready_standby(aggregate, control, network)?;
        Ok(())
    }

    pub(super) fn prepare_ready_standby<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
        const SLOTS: usize,
        const BUFFER_SIZE: usize,
    >(
        &mut self,
        aggregate: &mut AccessPointAmpdu<'_, B, SLOTS, BUFFER_SIZE>,
        control: &mut AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        network: &impl SelectedBurstMaterializer<SoftwareFrame = N, PhysicalFrame = B>,
    ) -> Result<(), AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: oer_time::Timer,
    {
        if self.prepared_standby.as_ref().is_some_and(|batch| {
            !control
                .mac
                .engine()
                .association_is_current(batch.admission.association())
        }) {
            aggregate
                .standby_mut()
                .expect("prepared batch owns standby arena")
                .cancel_build()
                .map_err(AccessPointDatapathError::Aggregate)?;
            let _ = self
                .prepared_standby
                .take()
                .expect("checked stale AP standby batch remains owned");
            if let Some(accounting) = self.airtime.as_mut() {
                accounting.cancel_standby()?;
            }
            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer {
                observer.observe(AggregateTxObservation::StandbyCancelled);
            }
        }
        self.reconsider_airtime_frontier()?;
        self.refresh_awake_demand(control.mac.engine());
        if (self.awake_buffered_peer.is_some()
            || (self
                .airtime
                .as_ref()
                .is_some_and(airtime::Accounting::selects_peers)
                && !self.aggregate_pending()))
            && self.prepared_standby.is_none()
            && self.prepared_first.is_none()
            && self.prepared_second.is_none()
            && self.prepared_buffered_release.is_none()
            && self.prepared_group_release.is_none()
            && self.active_group_release.is_none()
            && self.dtim_group_release_remaining == 0
            && !control.tx_pending()
        {
            // Wake-only work must make progress even without an A-MPDU
            // standby arena. First selection needs no physical reservation.
            match self.take_scheduled_active_or_network(control.mac.engine_mut(), network)? {
                Some(ApTxSelection::Buffered(release)) => {
                    self.prepared_buffered_release = Some(release);
                    return Ok(());
                }
                Some(ApTxSelection::Frame(key, frame)) => {
                    self.prepared_first_key = Some(key);
                    self.prepared_first = Some(frame);
                }
                None => {}
            }
        }
        while self.can_prepare(aggregate, control.tx_pending()) {
            if self.prepared_standby.is_some() {
                if !self.prepare_existing_standby_batch(aggregate, control, network)? {
                    break;
                }
                continue;
            }
            let selected = if self.prepared_first.is_some() {
                let key = self
                    .prepared_first_key
                    .expect("prepared AP frame retains its flow key");
                self.take_matching_active_or_network(control.mac.engine_mut(), key, network)?
                    .map(|frame| ApTxSelection::Frame(key, frame))
            } else {
                self.take_scheduled_active_or_network(control.mac.engine_mut(), network)?
            };
            let Some(selected) = selected else {
                break;
            };
            let (key, frame) = match selected {
                ApTxSelection::Frame(key, frame) => (key, frame),
                ApTxSelection::Buffered(release) => {
                    self.prepared_buffered_release = Some(release);
                    break;
                }
            };
            if !self.prepare_retained_one(aggregate, control, key, frame, network)? {
                break;
            }
        }
        Ok(())
    }

    fn prepare_existing_standby_batch<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
        const SLOTS: usize,
        const BUFFER_SIZE: usize,
    >(
        &mut self,
        aggregate: &mut AccessPointAmpdu<'_, B, SLOTS, BUFFER_SIZE>,
        control: &mut AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        network: &impl SelectedBurstMaterializer<SoftwareFrame = N, PhysicalFrame = B>,
    ) -> Result<bool, AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: oer_time::Timer,
    {
        let (admission, remaining) = {
            let batch = self
                .prepared_standby
                .as_ref()
                .expect("caller checked the prepared AP standby batch");
            (
                batch.admission,
                usize::from(batch.policy.frame_limit()).saturating_sub(batch.admitted),
            )
        };
        if remaining == 0 {
            return Ok(false);
        }
        let key = ApTxFlowKey::associated(admission.association());
        let mut frames = [const { None }; SLOTS];
        let burst_limit = remaining.min(SLOTS).min(network.materialization_capacity());
        if burst_limit == 0 {
            return Ok(false);
        }
        let mut length_budget = aggregate
            .standby_mut()
            .expect("checked standby arena")
            .length_budget(admission.rate())
            .map_err(AccessPointDatapathError::Aggregate)?;
        if let Some(accounting) = self.airtime.as_ref() {
            accounting.cap_standby_aggregate(admission.rate(), &mut length_budget)?;
        }
        let mut capacity_limited = false;
        let mut count = 0;
        while count < burst_limit {
            let Some(frame) =
                self.take_matching_active_or_network(control.mac.engine_mut(), key, network)?
            else {
                break;
            };
            debug_assert!(admission.accepts_ethernet(frame.as_slice()));
            if !length_budget
                .admit_ethernet(frame.as_slice())
                .map_err(AccessPointDatapathError::Aggregate)?
            {
                // The next unencoded owner is the closed batch's frontier.
                // Existing can_prepare ownership checks stop further pulls;
                // no independent full flag can disagree with that owner.
                debug_assert!(self.prepared_first.is_none());
                self.prepared_first_key = Some(key);
                self.prepared_first = Some(frame);
                capacity_limited = true;
                break;
            }
            frames[count] = Some(frame);
            count += 1;
        }
        if count == 0 {
            return Ok(false);
        }

        #[cfg(any(feature = "diagnostics", test))]
        let started = self.observer.map(AggregateTxObserver::now_micros);
        let mut promoted = [const { None }; SLOTS];
        if !network.try_materialize_batch(&mut frames, &mut promoted) {
            if capacity_limited {
                let frontier = self
                    .prepared_first
                    .take()
                    .expect("length refusal retains its owner");
                self.prepared_first_key = None;
                self.restore_active_frame_front(key, frontier);
            }
            for frame in frames[..count].iter_mut().rev().filter_map(Option::take) {
                self.restore_active_frame_front(key, frame);
            }
            return Ok(false);
        }

        let peer = admission.peer();
        for slot in promoted[..count].iter_mut() {
            let mut frame = slot
                .take()
                .expect("successful AP burst promotion publishes every DMA owner");
            let offset = frame.ethernet_offset();
            let length = frame.ethernet_length();
            let encoded = control
                .mac
                .engine_mut()
                .encode_aggregate_ethernet_in_place(
                    admission.binding(),
                    frame.storage_mut(),
                    offset,
                    length,
                )
                .map_err(|error| {
                    AccessPointDatapathError::Control(AccessPointControlError::from(error))
                })?;
            aggregate
                .standby_mut()
                .expect("checked standby arena")
                .push(peer, frame, encoded)
                .map_err(AccessPointDatapathError::Aggregate)?;
        }
        let frame_limit;
        {
            let batch = self
                .prepared_standby
                .as_mut()
                .expect("checked AP standby batch");
            batch.admitted += count;
            frame_limit = usize::from(batch.policy.frame_limit());
            #[cfg(any(feature = "diagnostics", test))]
            {
                batch.preparation_micros = batch.preparation_micros.saturating_add(
                    self.observer
                        .map(|observer| observer.now_micros().saturating_sub(started.unwrap_or(0)))
                        .unwrap_or(0),
                );
            }
        }
        let _ = frame_limit;
        Ok(true)
    }

    fn prepare_retained_one<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
        const SLOTS: usize,
        const BUFFER_SIZE: usize,
    >(
        &mut self,
        aggregate: &mut AccessPointAmpdu<'_, B, SLOTS, BUFFER_SIZE>,
        control: &mut AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        key: ApTxFlowKey,
        frame: N,
        network: &impl SelectedBurstMaterializer<SoftwareFrame = N, PhysicalFrame = B>,
    ) -> Result<bool, AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: oer_time::Timer,
    {
        let result = (|| {
            #[cfg(any(feature = "diagnostics", test))]
            let started = self.observer.map(AggregateTxObserver::now_micros);
            if let Some(batch) = self.prepared_standby.as_ref() {
                let admission = batch.admission;
                if key != ApTxFlowKey::associated(admission.association())
                    || !admission.accepts_ethernet(frame.as_slice())
                {
                    debug_assert!(self.prepared_first.is_none());
                    self.prepared_first_key = Some(key);
                    self.prepared_first = Some(frame);
                    return Ok(true);
                }
                let mut length_budget = aggregate
                    .standby_mut()
                    .expect("checked standby arena")
                    .length_budget(admission.rate())
                    .map_err(AccessPointDatapathError::Aggregate)?;
                if let Some(accounting) = self.airtime.as_ref() {
                    accounting.cap_standby_aggregate(admission.rate(), &mut length_budget)?;
                }
                if !length_budget
                    .admit_ethernet(frame.as_slice())
                    .map_err(AccessPointDatapathError::Aggregate)?
                {
                    debug_assert!(self.prepared_first.is_none());
                    self.prepared_first_key = Some(key);
                    self.prepared_first = Some(frame);
                    return Ok(false);
                }
                let mut frame = match network.try_materialize(frame) {
                    Ok(frame) => frame,
                    Err(frame) => {
                        self.restore_active_frame_front(key, frame);
                        return Ok(false);
                    }
                };
                let peer = admission.peer();
                let offset = frame.ethernet_offset();
                let length = frame.ethernet_length();
                let encoded = control
                    .mac
                    .engine_mut()
                    .encode_aggregate_ethernet_in_place(
                        admission.binding(),
                        frame.storage_mut(),
                        offset,
                        length,
                    )
                    .map_err(|error| {
                        AccessPointDatapathError::Control(AccessPointControlError::from(error))
                    })?;
                aggregate
                    .standby_mut()
                    .expect("checked standby arena")
                    .push(peer, frame, encoded)
                    .map_err(AccessPointDatapathError::Aggregate)?;
                let batch = self
                    .prepared_standby
                    .as_mut()
                    .expect("checked AP standby batch");
                batch.admitted += 1;
                #[cfg(any(feature = "diagnostics", test))]
                {
                    batch.preparation_micros = batch.preparation_micros.saturating_add(
                        self.observer
                            .map(|observer| {
                                observer.now_micros().saturating_sub(started.unwrap_or(0))
                            })
                            .unwrap_or(0),
                    );
                }
                return Ok(true);
            }

            let Some(first) = self.prepared_first.take() else {
                self.prepared_first_key = Some(key);
                self.prepared_first = Some(frame);
                return Ok(true);
            };
            let first_key = self
                .prepared_first_key
                .take()
                .expect("prepared AP frame retains its flow key");
            let admission = control.mac.aggregate_admission(first.as_slice());
            let Some(admission) = admission.filter(|admission| {
                first_key == key
                    && first_key == ApTxFlowKey::associated(admission.association())
                    && admission.accepts_ethernet(frame.as_slice())
            }) else {
                debug_assert!(self.prepared_second.is_none());
                self.prepared_first_key = Some(first_key);
                self.prepared_first = Some(first);
                self.prepared_second_key = Some(key);
                self.prepared_second = Some(frame);
                return Ok(true);
            };
            if let Some(accounting) = self.airtime.as_mut() {
                accounting.reserve_standby(control.mac.engine(), first_key)?;
            }
            let mut length_budget = aggregate
                .standby_mut()
                .expect("checked standby arena")
                .length_budget(admission.rate())
                .map_err(AccessPointDatapathError::Aggregate)?;
            if let Some(accounting) = self.airtime.as_ref() {
                accounting.cap_standby_aggregate(admission.rate(), &mut length_budget)?;
            }
            if !length_budget
                .admit_ethernet(first.as_slice())
                .map_err(AccessPointDatapathError::Aggregate)?
                || !length_budget
                    .admit_ethernet(frame.as_slice())
                    .map_err(AccessPointDatapathError::Aggregate)?
            {
                if let Some(accounting) = self.airtime.as_mut() {
                    accounting.defer_standby()?;
                }
                // Keep a publishable ordinary frontier. Restoring the pair into
                // active queues alone would retry this same impossible aggregate.
                self.prepared_first_key = Some(first_key);
                self.prepared_first = Some(first);
                self.prepared_second_key = Some(key);
                self.prepared_second = Some(frame);
                return Ok(false);
            }
            let peer = admission.peer();
            let (mut first, mut frame) = match network.try_materialize_pair(first, frame) {
                Ok(frames) => frames,
                Err((first, frame)) => {
                    self.restore_active_pair_front(first_key, first, frame);
                    if let Some(accounting) = self.airtime.as_mut() {
                        accounting.cancel_standby()?;
                    }
                    return Ok(false);
                }
            };
            let first_offset = first.ethernet_offset();
            let first_length = first.ethernet_length();
            let first_encoded = control
                .mac
                .engine_mut()
                .encode_aggregate_ethernet_in_place(
                    admission.binding(),
                    first.storage_mut(),
                    first_offset,
                    first_length,
                )
                .map_err(|error| {
                    AccessPointDatapathError::Control(AccessPointControlError::from(error))
                })?;
            let policy = admission
                .bind_policy(first_encoded.hardware_key_selector, SLOTS)
                .map_err(ApAmpduError::from)
                .map_err(AccessPointDatapathError::Aggregate)?;
            let standby = aggregate.standby_mut().expect("checked standby arena");
            standby
                .begin(
                    peer,
                    policy.rate(),
                    first_encoded.sequence_number,
                    policy.role().hardware_key_selector,
                    admission.agreement(),
                )
                .map_err(AccessPointDatapathError::Aggregate)?;
            self.prepared_standby = Some(PreparedStandby {
                admission,
                policy,
                admitted: 0,
                mismatch_claims: Core0Tally::new(),
                #[cfg(any(feature = "diagnostics", test))]
                preparation_micros: 0,
            });
            standby
                .push(peer, first, first_encoded)
                .map_err(AccessPointDatapathError::Aggregate)?;
            let offset = frame.ethernet_offset();
            let length = frame.ethernet_length();
            let encoded = control
                .mac
                .engine_mut()
                .encode_aggregate_ethernet_in_place(
                    admission.binding(),
                    frame.storage_mut(),
                    offset,
                    length,
                )
                .map_err(|error| {
                    AccessPointDatapathError::Control(AccessPointControlError::from(error))
                })?;
            aggregate
                .standby_mut()
                .expect("checked standby arena")
                .push(peer, frame, encoded)
                .map_err(AccessPointDatapathError::Aggregate)?;
            let batch = self
                .prepared_standby
                .as_mut()
                .expect("AP standby construction owns its metadata");
            batch.admitted = 2;
            #[cfg(any(feature = "diagnostics", test))]
            {
                batch.preparation_micros = self
                    .observer
                    .map(|observer| observer.now_micros().saturating_sub(started.unwrap_or(0)))
                    .unwrap_or(0);
            }

            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer {
                observer.observe(AggregateTxObservation::StandbyPrepared);
            }
            Ok(true)
        })();
        if result.is_err()
            && self.prepared_standby.is_none()
            && let Some(accounting) = self.airtime.as_mut()
        {
            accounting.cancel_standby()?;
            accounting.cancel_selection()?;
        }
        result
    }
}
