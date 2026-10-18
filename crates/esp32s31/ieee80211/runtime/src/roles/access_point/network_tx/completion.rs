//! Deadline waits, hardware completion and aggregate terminal observations.
//! These methods borrow the same owner used for publication and power save.

use super::*;

use oer_ieee80211_softmac::MacTxWork;

impl<'observer, B, N> AccessPointNetworkTx<'observer, B, N>
where
    B: MaterializedTxFrame,
    N: SoftwareTxFrame,
{
    pub(in super::super) async fn wait_deadline<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
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
    ) where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: oer_time::Timer,
    {
        if matches!(
            self.aggregate_phase,
            Some(AggregateServicePhase::Unaggregating | AggregateServicePhase::RequestingBlockAck)
        ) {
            let (_, ordinary) = control
                .mac
                .try_aggregate_adapter()
                .expect("an unaggregating retry owns ordinary AP TX");
            ordinary.wait_deadline().await;
        } else if let Some(phase) = self.aggregate_phase {
            let deadline = phase.deadline();
            let (_, ordinary) = control
                .mac
                .try_aggregate_adapter()
                .expect("aggregate publication leaves ordinary AP TX idle");
            oer_time::Timer::wait_until(&*ordinary, deadline).await;
        } else {
            control.wait_tx_deadline().await;
        }
    }

    pub(in super::super) fn service<
        P,
        E,
        T,
        H,
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
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: oer_time::Timer + crate::mac_clock::MacClockReader,
        H: TxHardware
            + ApRuntimeHardware
            + RxBlockAckHardware
            + oer_esp32s31_ieee80211_mac::tx::ampdu::HtAmpduHardware,
    {
        if self.aggregate_phase.is_none() {
            #[cfg(any(feature = "diagnostics", test))]
            let data = control.mac.pending_publication_kind()
                == Some(oer_esp32s31_ieee80211_ap::transaction::ApPendingPublicationKind::Data);
            let progress = match control.service_tx(hardware, wake) {
                Ok(progress) => progress,
                Err(error) => {
                    if self.active_group_release.is_some() {
                        self.complete_active_group_release(control, false)?;
                    }
                    if self.active_buffered_release.is_some() {
                        self.complete_active_buffered_release(control, false)?;
                    }
                    return Err(AccessPointDatapathError::Control(error));
                }
            };
            if progress == WifiTxProgress::Complete {
                let airtime_result = if let Some(accounting) = self.airtime.as_mut() {
                    accounting.complete_active(control.mac.work())
                } else {
                    Ok(())
                };
                #[cfg(any(feature = "diagnostics", test))]
                if data && let Some(observer) = self.observer {
                    observer.observe(AggregateTxObservation::OrdinaryWorkCompleted {
                        work: control.mac.work(),
                    });
                }
                let succeeded = control.take_last_terminal_tx_succeeded().unwrap_or(false);
                if self.active_group_release.is_some() {
                    // A group MPDU has no ACK. `succeeded` is only terminal
                    // hardware publication success for the one-attempt basic-
                    // rate transaction.
                    self.complete_active_group_release(control, succeeded)?;
                }
                if self.active_buffered_release.is_some() {
                    self.complete_active_buffered_release(control, succeeded)?;
                }
                let _ = self.stage_dtim_group_release(control)?;
                if self.prepared_group_release.is_none() {
                    let _ = self.refresh_power_save_demand(control)?;
                }
                airtime_result?;
            }
            return Ok(progress);
        }

        let phase = self
            .aggregate_phase
            .expect("ordinary service returned above");
        match phase {
            AggregateServicePhase::Unaggregating => {
                return self.service_unaggregating(aggregate, control, hardware, wake);
            }
            AggregateServicePhase::RequestingBlockAck => {
                return self.service_block_ack_request(aggregate, control, hardware, wake);
            }
            _ => {}
        }
        let action = phase.action(wake, || {
            let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                AccessPointDatapathError::Control(AccessPointControlError::Mac(error))
            })?;
            Ok(oer_time::Clock::now(&*ordinary))
        })?;
        let service_event = match action {
            AggregateServiceAction::Wait => return Ok(WifiTxProgress::Pending),
            AggregateServiceAction::Observe(event) => event,
            AggregateServiceAction::FinishAbort => {
                let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                    AccessPointDatapathError::Control(AccessPointControlError::Mac(error))
                })?;
                aggregate
                    .active_mut()
                    .finish_timeout_abort(hardware)
                    .map_err(AccessPointDatapathError::Aggregate)?;
                ordinary.reset_aggregate_contention();
                self.aggregate_phase = None;
                let work = self.take_exchange_work(aggregate);
                #[cfg(any(feature = "diagnostics", test))]
                {
                    self.exchange_started_micros = None;
                }
                #[cfg(any(feature = "diagnostics", test))]
                if let Some(observer) = self.observer {
                    observer.observe(AggregateTxObservation::HardwareTimeout);
                    observer.observe(AggregateTxObservation::WorkCompleted { work });
                }
                if let Some(accounting) = self.airtime.as_mut() {
                    accounting.complete_active(work)?;
                }
                return Ok(WifiTxProgress::Complete);
            }
        };
        if service_event == AggregateTxServiceEvent::Collision {
            let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                AccessPointDatapathError::Control(AccessPointControlError::Mac(error))
            })?;
            if !aggregate
                .active_mut()
                .abort_collision(hardware)
                .map_err(AccessPointDatapathError::Aggregate)?
            {
                return Err(AccessPointDatapathError::Aggregate(
                    ApAmpduError::HardwareDidNotDetach,
                ));
            }
            ordinary.reset_aggregate_contention();
            self.aggregate_phase = None;
            let work = self.take_exchange_work(aggregate);
            #[cfg(any(feature = "diagnostics", test))]
            {
                self.exchange_started_micros = None;
            }
            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer {
                observer.observe(AggregateTxObservation::Collision);
                observer.observe(AggregateTxObservation::WorkCompleted { work });
            }
            if let Some(accounting) = self.airtime.as_mut() {
                accounting.complete_active(work)?;
            }
            return Ok(WifiTxProgress::Complete);
        }
        if matches!(
            service_event,
            AggregateTxServiceEvent::HardwareTimeout | AggregateTxServiceEvent::ExecutorDeadline
        ) {
            if !aggregate
                .active_mut()
                .begin_timeout_abort(hardware)
                .map_err(AccessPointDatapathError::Aggregate)?
            {
                return Err(AccessPointDatapathError::Aggregate(
                    ApAmpduError::HardwareDidNotDetach,
                ));
            }
            let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                AccessPointDatapathError::Control(AccessPointControlError::Mac(error))
            })?;
            // Sample after the abort request; no wait future owns this phase.
            self.aggregate_phase = Some(AggregateServicePhase::ResetRequired);
            self.aggregate_phase = Some(AggregateServicePhase::after_abort(oer_time::Clock::now(
                &*ordinary,
            ))?);
            return Ok(WifiTxProgress::Pending);
        }

        let aggregate_progress = {
            #[cfg(any(feature = "diagnostics", test))]
            let completion_started = self.observer.map(AggregateTxObserver::now_micros);
            let (engine, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                AccessPointDatapathError::Control(AccessPointControlError::Mac(error))
            })?;
            let block_ack_operational = aggregate
                .active_mut()
                .published_agreement()
                .is_some_and(|agreement| engine.tx_block_ack_holds(agreement));
            let stamp = radio_stamp(ordinary.timer())?;
            let progress = aggregate
                .active_mut()
                .service_completion(ordinary, hardware, block_ack_operational, stamp)
                .map_err(AccessPointDatapathError::Aggregate)?;
            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer {
                let finished = observer.now_micros();
                let started = completion_started.unwrap_or(finished);
                match progress {
                    ApAmpduProgress::Republished(_) => {
                        observer.observe(AggregateTxObservation::Published {
                            at_micros: started,
                            program_micros: finished.saturating_sub(started),
                        });
                    }
                    ApAmpduProgress::CompletionReady(_)
                    | ApAmpduProgress::Unaggregate(_)
                    | ApAmpduProgress::RequestingBlockAck(_) => {
                        observer.observe(AggregateTxObservation::CompletionCoreCompleted {
                            micros: finished.saturating_sub(started),
                        });
                    }
                    ApAmpduProgress::Pending => {}
                }
            }
            progress
        };
        self.handle_aggregate_progress(
            aggregate,
            control,
            hardware,
            aggregate_progress,
            service_event,
            true,
        )
    }

    /// Continue the aggregate exchange after one completion or BlockAckReq
    /// resort of the aggregate owner.
    fn handle_aggregate_progress<
        P,
        E,
        T,
        H,
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
        hardware: &mut H,
        aggregate_progress: ApAmpduProgress,
        service_event: AggregateTxServiceEvent,
        block_ack_sample: bool,
    ) -> Result<WifiTxProgress, AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: oer_time::Timer,
        H: TxHardware + oer_esp32s31_ieee80211_mac::tx::ampdu::HtAmpduHardware,
    {
        #[cfg(not(any(feature = "diagnostics", test)))]
        let _ = block_ack_sample;
        match aggregate_progress {
            ApAmpduProgress::CompletionReady(completion) => {
                #[cfg(any(feature = "diagnostics", test))]
                {
                    self.observe_completion_details(completion, false, block_ack_sample);
                }
                #[cfg(not(any(feature = "diagnostics", test)))]
                let _ = completion;
                #[cfg(any(feature = "diagnostics", test))]
                let release_started = self.observer.map(AggregateTxObserver::now_micros);
                aggregate
                    .active_mut()
                    .release_completed()
                    .map_err(AccessPointDatapathError::Aggregate)?;
                self.aggregate_phase = None;
                #[cfg(any(feature = "diagnostics", test))]
                if let Some(observer) = self.observer {
                    let finished = observer.now_micros();
                    observer.observe(AggregateTxObservation::BackingReleaseCompleted {
                        micros: finished.saturating_sub(release_started.unwrap_or(finished)),
                    });
                }
                #[cfg(any(feature = "diagnostics", test))]
                {
                    debug_assert!(self.terminal_acknowledged.is_none());
                    self.terminal_acknowledged = Some(completion.acknowledged);
                }
                #[cfg(any(feature = "diagnostics", test))]
                {
                    self.exchange_started_micros = None;
                }
                let work = self.take_exchange_work(aggregate);
                #[cfg(any(feature = "diagnostics", test))]
                if let Some(observer) = self.observer {
                    observer.observe(AggregateTxObservation::WorkCompleted { work });
                }
                if let Some(accounting) = self.airtime.as_mut() {
                    accounting.complete_active(work)?;
                }
                Ok(WifiTxProgress::Complete)
            }
            ApAmpduProgress::Unaggregate(completion) => {
                #[cfg(any(feature = "diagnostics", test))]
                self.observe_completion_details(completion, false, block_ack_sample);
                #[cfg(not(any(feature = "diagnostics", test)))]
                let _ = completion;
                let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                    AccessPointDatapathError::Control(AccessPointControlError::Mac(error))
                })?;
                aggregate
                    .active_mut()
                    .start_next_unaggregated(ordinary, hardware)
                    .map_err(AccessPointDatapathError::Aggregate)?;
                self.aggregate_phase = Some(AggregateServicePhase::Unaggregating);
                Ok(WifiTxProgress::Pending)
            }
            ApAmpduProgress::RequestingBlockAck(completion) => {
                #[cfg(any(feature = "diagnostics", test))]
                self.observe_completion_details(completion, true, block_ack_sample);
                #[cfg(not(any(feature = "diagnostics", test)))]
                let _ = completion;
                self.aggregate_phase = Some(AggregateServicePhase::RequestingBlockAck);
                Ok(WifiTxProgress::Pending)
            }
            ApAmpduProgress::Republished(completion) => {
                #[cfg(any(feature = "diagnostics", test))]
                self.observe_completion_details(completion, true, block_ack_sample);
                #[cfg(not(any(feature = "diagnostics", test)))]
                let _ = completion;
                let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                    AccessPointDatapathError::Control(AccessPointControlError::Mac(error))
                })?;
                self.aggregate_phase = Some(AggregateServicePhase::Published(
                    oer_time::Clock::now(&*ordinary).saturating_add(ordinary.publication_timeout()),
                ));
                Ok(WifiTxProgress::Pending)
            }
            ApAmpduProgress::Pending => {
                if service_event == AggregateTxServiceEvent::Completion {
                    return Err(AccessPointDatapathError::Aggregate(
                        ApAmpduError::CompletionInterruptWithoutState,
                    ));
                }
                Ok(WifiTxProgress::Pending)
            }
        }
    }

    /// Resort the retained aggregate once its BlockAckReq completed: by the
    /// BlockAck received, or with every MPDU missing when none arrived.
    fn service_block_ack_request<
        P,
        E,
        T,
        H,
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
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: oer_time::Timer + crate::mac_clock::MacClockReader,
        H: TxHardware + oer_esp32s31_ieee80211_mac::tx::ampdu::HtAmpduHardware,
    {
        let (engine, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
            AccessPointDatapathError::Control(AccessPointControlError::Mac(error))
        })?;
        let stamp = radio_stamp(ordinary.timer())?;
        aggregate
            .active_mut()
            .validate_block_ack_request_stamp(stamp)
            .map_err(AccessPointDatapathError::Aggregate)?;
        let progress = ordinary
            .service(hardware, wake)
            .map_err(|error| AccessPointDatapathError::Aggregate(ApAmpduError::Ordinary(error)))?;
        if progress == WifiTxProgress::Pending {
            return Ok(progress);
        }
        let block_ack = ordinary
            .take_last_outcome()
            .and_then(|outcome| outcome.report().block_ack);
        self.exchange_ordinary_work.absorb(ordinary.work());
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer {
            observer.observe(AggregateTxObservation::BlockAckRequestCompleted {
                answered: block_ack.is_some(),
            });
        }
        let block_ack_operational = aggregate
            .active_mut()
            .published_agreement()
            .is_some_and(|agreement| engine.tx_block_ack_holds(agreement));
        #[cfg(any(feature = "diagnostics", test))]
        let resort_started = self.observer.map(AggregateTxObserver::now_micros);
        let aggregate_progress = aggregate
            .active_mut()
            .observe_block_ack_request(ordinary, hardware, block_ack, block_ack_operational, stamp)
            .map_err(AccessPointDatapathError::Aggregate)?;
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer
            && matches!(aggregate_progress, ApAmpduProgress::Republished(_))
        {
            let finished = observer.now_micros();
            let started = resort_started.unwrap_or(finished);
            observer.observe(AggregateTxObservation::Published {
                at_micros: started,
                program_micros: finished.saturating_sub(started),
            });
        }
        // The BlockAckReq's answer was observed above; it is not a sample of
        // an aggregate publication's BlockAck.
        self.handle_aggregate_progress(
            aggregate,
            control,
            hardware,
            aggregate_progress,
            AggregateTxServiceEvent::Pending,
            false,
        )
    }

    /// The aggregate's own work plus the ordinary transmissions made for
    /// its exchange, which are reset for the next exchange.
    fn take_exchange_work<const SLOTS: usize, const BUFFER_SIZE: usize>(
        &mut self,
        aggregate: &mut AccessPointAmpdu<'_, B, SLOTS, BUFFER_SIZE>,
    ) -> MacTxWork {
        let mut work = aggregate.active_mut().work();
        work.absorb(core::mem::take(&mut self.exchange_ordinary_work));
        work
    }

    /// Advance the ordinary retry of one MPDU taken out of an aggregate whose
    /// agreement ended; start the next one, or finish the aggregate's
    /// exchange after the last.
    fn service_unaggregating<
        P,
        E,
        T,
        H,
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
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: oer_time::Timer,
        H: TxHardware + oer_esp32s31_ieee80211_mac::tx::ampdu::HtAmpduHardware,
    {
        let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
            AccessPointDatapathError::Control(AccessPointControlError::Mac(error))
        })?;
        let progress = ordinary
            .service(hardware, wake)
            .map_err(|error| AccessPointDatapathError::Aggregate(ApAmpduError::Ordinary(error)))?;
        if progress == WifiTxProgress::Pending {
            return Ok(progress);
        }
        let _ = ordinary.take_last_outcome();
        self.exchange_ordinary_work.absorb(ordinary.work());
        if aggregate.active_mut().is_unaggregating() {
            aggregate
                .active_mut()
                .start_next_unaggregated(ordinary, hardware)
                .map_err(AccessPointDatapathError::Aggregate)?;
            self.aggregate_phase = Some(AggregateServicePhase::Unaggregating);
            return Ok(WifiTxProgress::Pending);
        }
        self.aggregate_phase = None;
        let work = self.take_exchange_work(aggregate);
        #[cfg(any(feature = "diagnostics", test))]
        {
            self.exchange_started_micros = None;
        }
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer {
            observer.observe(AggregateTxObservation::WorkCompleted { work });
        }
        if let Some(accounting) = self.airtime.as_mut() {
            accounting.complete_active(work)?;
        }
        Ok(WifiTxProgress::Complete)
    }

    #[cfg(any(feature = "diagnostics", test))]
    fn observe_completion_details(
        &self,
        completion: ApAmpduCompletion,
        republished: bool,
        block_ack_sample: bool,
    ) {
        let Some(observer) = self.observer else {
            return;
        };
        if block_ack_sample {
            observer.observe(AggregateTxObservation::BlockAckProcessed {
                tx_status: completion.tx_status,
                block_ack_received: completion.block_ack_received,
                control: completion.block_ack_control,
                first_sequence: completion.first_sequence,
                starting_sequence: completion.starting_sequence,
                subframes: completion.subframes,
                missing_original_indices: completion.missing_original_indices,
                block_ack_snr_db: completion.block_ack_snr_db,
            });
        }
        if !republished && let Some(started) = self.exchange_started_micros {
            observer.observe(AggregateTxObservation::ExchangeCompleted {
                micros: observer.now_micros().saturating_sub(started),
                publications: completion.aggregate_attempts,
            });
        }
    }
}
