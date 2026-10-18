use super::*;
use oer_time::Clock as _;

impl<
    'slot,
    'ampdu,
    B,
    P,
    E,
    T,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> ConnectedTx<'slot, 'ampdu, B, P, E, T, SLOTS, AMPDU_BUFFER_SIZE, ORDINARY_BUFFER_SIZE>
where
    B: MaterializedTxFrame,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + crate::mac_clock::MacClockReader,
{
    /// Service one captured event synchronously. Pending timeout-abort keeps
    /// the aggregate owners in this state machine, never in a suspended future.
    /// The executor or fused owner uses `next_deadline` to wait.
    pub fn service<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        #[cfg(any(feature = "diagnostics", test))]
        if matches!(wake, WifiTxWake::Interrupt { .. })
            && let Some(observer) = self.observer
        {
            observer.observe(AggregateTxObservation::InterruptServiceStarted {
                at_micros: self.ordinary.now().as_micros(),
            });
        }
        let radio_stamp = match &self.active {
            ConnectedTxActive::Aggregate(active)
            | ConnectedTxActive::RequestingBlockAck(active) => {
                let stamp = self.radio_stamp()?;
                active.retry.validate_stamp(stamp)?;
                Some(stamp)
            }
            _ => None,
        };
        let active = mem::replace(&mut self.active, ConnectedTxActive::Idle);
        match active {
            ConnectedTxActive::Idle => Err(AggregateTxError::InactiveTransaction),
            ConnectedTxActive::Ordinary => {
                let progress = self.ordinary.service(hardware, wake)?;
                if progress == WifiTxProgress::Pending {
                    self.active = ConnectedTxActive::Ordinary;
                } else {
                    if core::mem::take(&mut self.network_ordinary) {
                        self.network_power.completed = Some(
                            self.ordinary
                                .last_outcome()
                                .is_some_and(|outcome| outcome.is_success()),
                        );
                    }
                    self.observe_ordinary_rate_control();
                    #[cfg(any(feature = "diagnostics", test))]
                    if let Some(observer) = self.observer {
                        observer.observe_station_ordinary(self.ordinary.last_outcome());
                        observer.observe(AggregateTxObservation::OrdinaryWorkCompleted {
                            work: self.ordinary.work(),
                        });
                    }
                }
                Ok(progress)
            }
            ConnectedTxActive::Unaggregating(unaggregating) => {
                self.service_unaggregating(hardware, wake, unaggregating)
            }
            ConnectedTxActive::RequestingBlockAck(active) => self.service_block_ack_request(
                hardware,
                wake,
                active,
                radio_stamp.expect("aggregate sample validated"),
            ),
            ConnectedTxActive::AbortSettling(active) => self.service_abort_settle(hardware, active),
            ConnectedTxActive::Aggregate(active) => self.service_aggregate(
                hardware,
                wake,
                active,
                radio_stamp.expect("aggregate sample validated"),
            ),
        }
    }

    /// Copy the next missing MPDU out of the retained aggregate and send it
    /// as an ordinary frame; the copy of the last one releases the aggregate.
    fn start_unaggregated_retry<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        mut unaggregating: Unaggregating<SLOTS>,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
        let index = unaggregating.remaining.trailing_zeros() as u8;
        unaggregating.remaining &= unaggregating.remaining - 1;
        let (frame_length, hardware_mic_length) = {
            let (encoded, mic) = self.ampdu.active_mut().completed_frame(cookie, index)?;
            (self.ordinary.copy_encoded_retry(encoded)?, usize::from(mic))
        };
        if unaggregating.remaining == 0 {
            self.release_completed()?;
        }
        // SOURCE: `libpp[pp.o]::ppResortTxAMPDU` hands a missing MPDU of an
        // ended agreement to `libpp[pp_he.o]::ppHEAMPDU2Normal`, which clears the MPDU, short
        // and long retry counters and reselects the ordinary data schedule
        // through `libpp[trc.o]::rcGetSched`: the MPDU starts a fresh ordinary exchange.
        let selection = self.data_tx_selection();
        let access_category = unaggregating.aggregate.traffic.selected.access_category;
        let progress = self.ordinary.start_prepared_encoded_retry_for_category(
            hardware,
            frame_length,
            hardware_mic_length,
            selection,
            access_category,
        )?;
        debug_assert_eq!(progress, WifiTxProgress::Pending);
        self.active = ConnectedTxActive::Unaggregating(unaggregating);
        Ok(progress)
    }

    fn service_unaggregating<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
        mut unaggregating: Unaggregating<SLOTS>,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        let progress = self.ordinary.service(hardware, wake)?;
        if progress == WifiTxProgress::Pending {
            self.active = ConnectedTxActive::Unaggregating(unaggregating);
            return Ok(progress);
        }
        self.observe_ordinary_rate_control();
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer {
            observer.observe_station_ordinary(self.ordinary.last_outcome());
            observer.observe(AggregateTxObservation::OrdinaryWorkCompleted {
                work: self.ordinary.work(),
            });
        }
        let ordinary = self
            .ordinary
            .last_outcome()
            .ok_or(AggregateTxError::MissingOrdinaryRetryStatus)?
            .report()
            .status;
        if !matches!(ordinary.result, MacTxResult::Transmitted) {
            unaggregating.status.result = MacAmpduTxResult::Incomplete;
        }
        unaggregating.status.individual_retries.record(ordinary);
        if unaggregating.remaining != 0 {
            return self.start_unaggregated_retry(hardware, unaggregating);
        }
        self.record_terminal_status(unaggregating.status);
        Ok(progress)
    }

    fn record_terminal_status(&mut self, status: MacAmpduTxStatus<TxPhyRate>) {
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer {
            observer.observe_station_terminal(status);
        }
        self.last_aggregate_status = Some(status);
        self.network_power.completed = Some(
            matches!(status.result, MacAmpduTxResult::Delivered)
                || status.block_acknowledged_subframes != 0,
        );
    }

    fn service_abort_settle<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        active: AggregateActive<SLOTS>,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        if self.ordinary.now() < active.deadline {
            self.active = ConnectedTxActive::AbortSettling(active);
            return Ok(WifiTxProgress::Pending);
        }
        let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
        self.ampdu
            .active_mut()
            .finish_timeout_abort(hardware, cookie)?;
        self.release_frames();
        self.cookie = None;
        self.ordinary
            .reset_terminal_exchange(active.traffic.queue());
        self.record_terminal_status(MacAmpduTxStatus {
            result: MacAmpduTxResult::HardwareTimeout,
            original_subframes: u16::from(active.original_subframes),
            aggregate_attempts: active.retry.aggregate_attempts(),
            aggregate_rate: active.config.rate(),
            block_acknowledged_subframes: u16::from(active.retry.acknowledged()),
            individual_retries: MacIndividualRetries::NONE,
            ack_snr_db: None,
        });
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer {
            observer.observe(AggregateTxObservation::HardwareTimeout);
            self.observe_terminal_exchange(observer, &active, self.ordinary.now().as_micros());
        }
        Ok(WifiTxProgress::Complete)
    }

    fn service_aggregate<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
        mut active: AggregateActive<SLOTS>,
        radio_stamp: oer_ieee80211_lower_mac::Ieee80211Stamp,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        #[cfg(feature = "tx-wait-probe")]
        if let Some(observer) = self.observer {
            let now = self.ordinary.now();
            if let Some((elapsed_micros, timer_lateness_micros)) = active.wait_probe.sample(now) {
                let queue = active.traffic.queue().hardware_index();
                observer.observe_wait_probe(crate::diagnostics::aggregate_tx::TxWaitSample {
                    deadline_wake: matches!(wake, WifiTxWake::Deadline),
                    at_micros: now.as_micros(),
                    elapsed_micros,
                    timer_lateness_micros,
                    first_sequence: active.first_sequence,
                    queue,
                    snapshot: hardware.ordinary_tx_queue_snapshot(queue),
                });
            }
        }
        let service_event = match AggregateTxServiceEvent::classify(wake) {
            Ok(event) => event,
            Err(error) => {
                return self.reset_required(AggregateTxResetReason::ConflictingInterruptEvents(
                    error.events,
                ));
            }
        };

        let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
        let block_ack_operational =
            self.block_ack_generation(active.traffic.tid()) == Some(active.block_ack_generation);
        if let Some(observed) = self.ampdu.active_mut().observe_retry_completion(
            hardware,
            cookie,
            &mut active.retry,
            radio_stamp,
            block_ack_operational,
        )? {
            let completion = observed.completion;
            let current_subframes = observed.subframes;
            #[cfg(any(feature = "diagnostics", test))]
            let current_first_sequence = observed.first_sequence;
            let decision = observed.decision;
            self.rate_control.observe_tx_completion(completion.tx);
            // The retry owner has already validated both counts. An absent
            // A-MPDU rate arena disables adaptation but cannot alter DMA or
            // retry ownership; malformed counts are therefore impossible at
            // this typed boundary and remain diagnostic-only.
            let acknowledged = current_subframes.saturating_sub(decision.missing());
            let observation = self.rate_control.observe_ampdu_block_ack(
                self.ordinary.now().as_micros() as u32,
                u16::from(current_subframes),
                u16::from(acknowledged),
            );
            debug_assert!(matches!(
                observation,
                Ok(_) | Err(AmpduRateObservationError::Unavailable)
            ));
            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer {
                observer.observe(AggregateTxObservation::BlockAckProcessed {
                    tx_status: completion.tx.status(),
                    block_ack_received: completion.block_ack_received,
                    control: completion.block_ack.control,
                    first_sequence: current_first_sequence,
                    starting_sequence: completion.block_ack.block_ack.starting_sequence,
                    subframes: current_subframes,
                    missing_original_indices: active.retry.missing_original_indices(),
                    block_ack_snr_db: completion.tx.ack_snr_sample(),
                });
            }
            return self.apply_retry_decision(
                hardware,
                active,
                decision,
                completion.tx.ack_snr_sample(),
            );
        }

        if service_event == AggregateTxServiceEvent::Completion {
            return self.reset_required(AggregateTxResetReason::CompletionInterruptWithoutState);
        }
        if matches!(
            service_event,
            AggregateTxServiceEvent::HardwareTimeout | AggregateTxServiceEvent::ExecutorDeadline
        ) {
            if service_event == AggregateTxServiceEvent::ExecutorDeadline
                && self.ordinary.now() < active.deadline
            {
                self.active = ConnectedTxActive::Aggregate(active);
                return Ok(WifiTxProgress::Pending);
            }
            let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
            if !self
                .ampdu
                .active_mut()
                .begin_timeout_abort(hardware, cookie)?
            {
                return self.reset_required(
                    if service_event == AggregateTxServiceEvent::ExecutorDeadline {
                        AggregateTxResetReason::ExecutorDeadline
                    } else {
                        AggregateTxResetReason::TimeoutInterruptWithoutState
                    },
                );
            }
            let Some(deadline) = self.ordinary.now().checked_add(AMPDU_ABORT_SETTLE) else {
                self.ampdu.active_mut().require_reset(cookie)?;
                return Err(AggregateTxError::DeadlineOverflow);
            };
            active.deadline = deadline;
            self.active = ConnectedTxActive::AbortSettling(active);
            return Ok(WifiTxProgress::Pending);
        }
        if service_event == AggregateTxServiceEvent::Collision {
            let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
            if !self.ampdu.active_mut().abort_collision(hardware, cookie)? {
                return self.reset_required(AggregateTxResetReason::CollisionInterruptWithoutState);
            }
            self.release_frames();
            self.cookie = None;
            self.ordinary
                .reset_terminal_exchange(active.traffic.queue());
            self.record_terminal_status(MacAmpduTxStatus {
                result: MacAmpduTxResult::CollisionLimit,
                original_subframes: u16::from(active.original_subframes),
                aggregate_attempts: active.retry.aggregate_attempts(),
                aggregate_rate: active.config.rate(),
                block_acknowledged_subframes: u16::from(active.retry.acknowledged()),
                individual_retries: MacIndividualRetries::NONE,
                ack_snr_db: None,
            });
            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer {
                observer.observe(AggregateTxObservation::Collision);
                self.observe_terminal_exchange(observer, &active, self.ordinary.now().as_micros());
            }
            return Ok(WifiTxProgress::Complete);
        }

        self.active = ConnectedTxActive::Aggregate(active);
        Ok(WifiTxProgress::Pending)
    }

    /// Act on one retry decision for the retained aggregate: publish it
    /// again, ask for its BlockAck, send its missing MPDUs individually or
    /// end it.
    fn apply_retry_decision<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        mut active: AggregateActive<SLOTS>,
        decision: AmpduRetryDecision,
        ack_snr_db: Option<i8>,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
        let republication = match decision {
            AmpduRetryDecision::RetainAggregate { retry_mask } => {
                Some((retry_mask, AmpduRepublication::Retransmission))
            }
            AmpduRetryDecision::RepublishUnchanged { retry_mask } => {
                Some((retry_mask, AmpduRepublication::AfterProtectionFailure))
            }
            AmpduRetryDecision::RequestBlockAck {
                starting_sequence, ..
            } => {
                return self.start_block_ack_request(hardware, active, starting_sequence);
            }
            AmpduRetryDecision::Unaggregate { .. }
            | AmpduRetryDecision::Finish { .. }
            | AmpduRetryDecision::FinishTriggerFlow => None,
        };
        if let Some((retry_mask, republication)) = republication {
            let queue = active.traffic.queue();
            let aggregate = self.ampdu.active_mut().retain_for_ampdu_retry(
                cookie,
                retry_mask,
                republication,
            )?;
            self.ordinary.record_retry_failure(queue);
            let (_, contention_window) = self.ordinary.contention_publication(queue);
            let (_, control) = self.ordinary.control_frame_for(ProtectedPpdu {
                rate: active.config.rate(),
                receiver: TxReceiver::Individual,
                psdu: TxPsdu::Ampdu {
                    length: u32::from(aggregate.bytes),
                },
            });
            active.config.update_retained_retry(
                aggregate.bytes,
                aggregate.subframes,
                contention_window,
                control,
            );
            if let Err(error) = self.publish_attempt(hardware, &mut active) {
                self.cancel_prepared();
                return Err(error);
            }
            self.active = ConnectedTxActive::Aggregate(active);
            return Ok(WifiTxProgress::Pending);
        }

        let retry_mask = decision.retry_mask();
        let missing = decision.missing();
        let queue = active.traffic.queue();
        if missing == 0 {
            self.ordinary.record_success(queue);
        } else {
            self.ordinary.reset_terminal_exchange(queue);
        }

        if matches!(decision, AmpduRetryDecision::Unaggregate { .. }) {
            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer {
                observer.observe(AggregateTxObservation::Completed {
                    acknowledged: active.retry.acknowledged(),
                    individual_retry: true,
                });
                self.observe_terminal_exchange(observer, &active, self.ordinary.now().as_micros());
            }
            let status = MacAmpduTxStatus {
                // Delivered only once every missing MPDU was transmitted.
                result: MacAmpduTxResult::Delivered,
                original_subframes: u16::from(active.original_subframes),
                aggregate_attempts: active.retry.aggregate_attempts(),
                aggregate_rate: active.config.rate(),
                block_acknowledged_subframes: u16::from(active.retry.acknowledged()),
                individual_retries: MacIndividualRetries::NONE,
                ack_snr_db,
            };
            return self.start_unaggregated_retry(
                hardware,
                Unaggregating {
                    aggregate: active,
                    remaining: retry_mask,
                    status,
                },
            );
        }

        self.release_completed()?;
        let acknowledged = active.retry.acknowledged();
        self.record_terminal_status(MacAmpduTxStatus {
            result: if acknowledged == active.original_subframes {
                MacAmpduTxResult::Delivered
            } else {
                MacAmpduTxResult::Incomplete
            },
            original_subframes: u16::from(active.original_subframes),
            aggregate_attempts: active.retry.aggregate_attempts(),
            aggregate_rate: active.config.rate(),
            block_acknowledged_subframes: u16::from(acknowledged),
            individual_retries: MacIndividualRetries::NONE,
            ack_snr_db,
        });
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer {
            observer.observe(AggregateTxObservation::Completed {
                acknowledged: active.retry.acknowledged(),
                individual_retry: false,
            });
            self.observe_terminal_exchange(observer, &active, self.ordinary.now().as_micros());
        }
        Ok(WifiTxProgress::Complete)
    }

    /// Keep the aggregate whose protection exchange failed at every attempt
    /// and send a BlockAckReq for its head through the ordinary owner.
    fn start_block_ack_request<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        active: AggregateActive<SLOTS>,
        starting_sequence: SequenceNumber,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        self.ordinary
            .reset_terminal_exchange(active.traffic.queue());
        let progress = self.ordinary.start_block_ack_request(
            hardware,
            active.traffic.tid(),
            starting_sequence,
        )?;
        self.active = ConnectedTxActive::RequestingBlockAck(active);
        Ok(progress)
    }

    /// Resort the retained aggregate by the BlockAck its BlockAckReq got.
    /// An unanswered request resorts it with every MPDU still missing.
    fn service_block_ack_request<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
        mut active: AggregateActive<SLOTS>,
        radio_stamp: oer_ieee80211_lower_mac::Ieee80211Stamp,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        let progress = self.ordinary.service(hardware, wake)?;
        if progress == WifiTxProgress::Pending {
            self.active = ConnectedTxActive::RequestingBlockAck(active);
            return Ok(progress);
        }
        let report = self
            .ordinary
            .last_outcome()
            .ok_or(AggregateTxError::MissingOrdinaryRetryStatus)?
            .report();
        let block_ack = report.block_ack;
        let ack_snr_db = report
            .completion
            .and_then(|completion| completion.ack_snr_sample());
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer {
            observer.observe(AggregateTxObservation::BlockAckRequestCompleted {
                answered: block_ack.is_some(),
            });
        }
        let block_ack_operational =
            self.block_ack_generation(active.traffic.tid()) == Some(active.block_ack_generation);
        let decision = active.retry.observe_block_ack_request(
            block_ack,
            radio_stamp,
            block_ack_operational,
        )?;
        self.apply_retry_decision(hardware, active, decision, ack_snr_db)
    }

    fn observe_ordinary_rate_control(&mut self) {
        let Some(outcome) = self.ordinary.last_outcome() else {
            return;
        };
        let report = outcome.report();
        if let Some(completion) = report.completion {
            self.rate_control.observe_tx_completion(completion);
        }
        self.rate_control
            .update_tx_per(u32::from(report.status.attempts.saturating_sub(1)));
    }

    #[cfg(any(feature = "diagnostics", test))]
    fn observe_terminal_exchange(
        &self,
        observer: &dyn AggregateTxObserver,
        active: &AggregateActive<SLOTS>,
        finished_micros: u64,
    ) {
        observer.observe(AggregateTxObservation::WorkCompleted {
            work: self.ampdu.active().work(),
        });
        if let Some(started_micros) = active.first_publication_micros {
            observer.observe(AggregateTxObservation::ExchangeCompleted {
                micros: finished_micros.wrapping_sub(started_micros),
                publications: active.retry.aggregate_attempts(),
            });
        }
    }
}
