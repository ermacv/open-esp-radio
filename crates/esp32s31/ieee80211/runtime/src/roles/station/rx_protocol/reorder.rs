use super::*;
use oer_esp32s31_ieee80211_mac::rx::ampdu::RxBlockAckRequestKey;
#[cfg(any(feature = "diagnostics", test))]
use oer_ieee80211_mac::sequence::SequenceNumber;

use crate::diagnostics::core0_rx_reorder_cycles::{
    CORE0_REORDER_CYCLES, Core0DirectCycleProfile, Core0DirectPath, Core0ReorderPath,
};

impl<
    'queue,
    'pool,
    'scratch,
    'irq,
    M: RawMutex,
    S,
    K,
    const CAPACITY: usize,
    const SLOTS: usize,
    const REORDER_SLOTS: usize,
> ConnectedRxProcessor<'queue, 'pool, 'scratch, 'irq, M, S, K, CAPACITY, SLOTS, REORDER_SLOTS>
where
    S: ConnectedRxProtocolSink<CAPACITY, SLOTS>,
    K: Timer,
{
    /// Try the run-to-completion path for the common in-order protected QoS
    /// frame.
    ///
    /// Every rejection returns the untouched frame to the general async path.
    /// `try_ingest_immediate` is specifically non-mutating for gaps, buffered
    /// successors and window advances, so falling back cannot apply reorder
    /// state twice. A successful call consumes the reorder frontier and the
    /// retained staging owner synchronously.
    #[cfg_attr(
        target_arch = "riscv32",
        unsafe(link_section = ".hot.text.open_radio_sta_rx_direct")
    )]
    #[inline(never)]
    pub(super) fn try_dispatch_frame_direct(
        &mut self,
        frame: StagedRxFrame<'pool, CAPACITY, SLOTS>,
    ) -> Result<Option<ConnectedRxDispatch>, StagedRxFrame<'pool, CAPACITY, SLOTS>> {
        let mut direct_cycles = Core0DirectCycleProfile::begin();
        assert!(
            self.runtime.dispatcher_configured(),
            "stopped connected RX processor cannot dispatch another frame"
        );
        let preflight_rejected = self.sink.staged_rx_admission() != StagedRxAdmission::Immediate
            || self.runtime.dispatcher.may_publish_amsdu(frame.segment())
            || !self
                .runtime
                .dispatcher
                .may_publish_ethernet(frame.segment());
        direct_cycles.preflight_completed();
        if preflight_rejected {
            direct_cycles.finish(Core0DirectPath::PreflightRejected);
            return Err(frame);
        }

        let key = self.runtime.dispatcher.reorder_key(frame.segment());
        direct_cycles.key_completed();
        let Some(key) = key else {
            direct_cycles.finish(Core0DirectPath::KeyRejected);
            return Err(frame);
        };
        let bank = self.runtime.reorder_banks.find(
            oer_esp32s31_ieee80211_mac::MacInterface::Station,
            key.peer,
            key.tid,
        );
        direct_cycles.bank_completed();
        let Some(bank) = bank else {
            direct_cycles.finish(Core0DirectPath::BankRejected);
            return Err(frame);
        };
        #[cfg(any(feature = "diagnostics", test))]
        self.observe_first_reorder_frame(bank, key.tid, key.sequence);
        let immediate = RxAmpduMpdu {
            sequence: key.sequence,
            slot: RX_REORDER_CURRENT_SLOT as u8,
        };
        let ingest = self
            .runtime
            .reorder_banks
            .state_mut(bank)
            .expect("active TID was checked above")
            .try_ingest_immediate(immediate);
        direct_cycles.ingest_completed();
        match ingest {
            Ok(Some(_)) => {
                self.update_gap_deadline(bank);
                direct_cycles.deadline_completed();
                let dispatch = self.dispatch_owned_frame_direct(frame);
                direct_cycles.dispatch_completed();
                direct_cycles.finish(Core0DirectPath::Accepted);
                Ok(Some(dispatch))
            }
            Ok(None) => {
                direct_cycles.finish(Core0DirectPath::ReorderRejected);
                Err(frame)
            }
            Err(error) => {
                drop(frame);
                self.irq.notify_rx_capacity();
                direct_cycles.finish(Core0DirectPath::DuplicateOrIgnored);
                Ok(Some(
                    if matches!(error, RxAmpduError::DuplicateSequence(_)) {
                        ConnectedRxDispatch::Duplicate
                    } else {
                        ConnectedRxDispatch::Ignored
                    },
                ))
            }
        }
    }

    /// Consume the agreement's first-frame marker at the common direct/async
    /// boundary. Leaving it to the async fallback makes a later exceptional
    /// frame appear to be the first frame even though the direct hot path has
    /// already advanced the reorder window.
    #[cfg(any(feature = "diagnostics", test))]
    pub(super) fn observe_first_reorder_frame(
        &mut self,
        bank: usize,
        tid: u8,
        sequence: SequenceNumber,
    ) {
        let Some(start) = self.runtime.reorder_first_starts[bank].take() else {
            return;
        };
        if let Some(observer) = self.pipeline_observer {
            observer.observe(RxPipelineObservation::ReorderFirst {
                tid,
                start,
                sequence,
            });
        }
        if let Some(observer) = self.reorder_observer {
            observer.observe(RxReorderAgreementObservation::First {
                tid,
                start,
                sequence,
            });
        }
    }

    pub async fn dispatch_frame(
        &mut self,
        frame: StagedRxFrame<'pool, CAPACITY, SLOTS>,
    ) -> Option<ConnectedRxDispatch> {
        assert!(
            self.runtime.dispatcher_configured(),
            "stopped connected RX processor cannot dispatch another frame"
        );
        CORE0_REORDER_CYCLES.begin();
        #[cfg(any(feature = "diagnostics", test))]
        let reorder_started = self.pipeline_observer.map(|observer| observer.now_micros());
        let reorder_key = self.runtime.dispatcher.reorder_key(frame.segment());
        CORE0_REORDER_CYCLES.key_completed();
        let Some(key) = reorder_key else {
            #[cfg(any(feature = "diagnostics", test))]
            self.observe_reorder_prepared(reorder_started);
            CORE0_REORDER_CYCLES.prepared_observer_completed();
            CORE0_REORDER_CYCLES.finish(Core0ReorderPath::NoKey);
            if let Some(request) = self
                .runtime
                .dispatcher
                .block_ack_request_key(frame.segment())
            {
                drop(frame);
                self.irq.notify_rx_capacity();
                return Some(self.move_reorder_window(request).await);
            }
            return Some(self.dispatch_owned_frame(frame).await);
        };
        let bank = self.runtime.reorder_banks.find(
            oer_esp32s31_ieee80211_mac::MacInterface::Station,
            key.peer,
            key.tid,
        );
        CORE0_REORDER_CYCLES.bank_completed();
        let active = bank.is_some();
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.pipeline_observer {
            observer.observe(RxPipelineObservation::ReorderIngress {
                active,
                retry: key.retry,
            });
        }
        CORE0_REORDER_CYCLES.ingress_observer_completed();
        if !active {
            #[cfg(any(feature = "diagnostics", test))]
            self.observe_reorder_prepared(reorder_started);
            CORE0_REORDER_CYCLES.prepared_observer_completed();
            CORE0_REORDER_CYCLES.finish(Core0ReorderPath::Inactive);
            return Some(self.dispatch_owned_frame(frame).await);
        }
        let bank = bank.expect("active reorder has one hardware-bank identity");
        #[cfg(any(feature = "diagnostics", test))]
        self.observe_first_reorder_frame(bank, key.tid, key.sequence);
        CORE0_REORDER_CYCLES.first_completed();

        let immediate = RxAmpduMpdu {
            sequence: key.sequence,
            slot: RX_REORDER_CURRENT_SLOT as u8,
        };
        let immediate_result = self
            .runtime
            .reorder_banks
            .state_mut(bank)
            .expect("active TID was checked above")
            .try_ingest_immediate(immediate);
        CORE0_REORDER_CYCLES.ingest_completed();
        match immediate_result {
            Ok(Some(_)) => {
                self.update_gap_deadline(bank);
                CORE0_REORDER_CYCLES.deadline_completed();
                #[cfg(any(feature = "diagnostics", test))]
                if let Some(observer) = self.pipeline_observer {
                    observer.observe(RxPipelineObservation::ReorderReleased {
                        buffered: false,
                        released: 1,
                        missing: 0,
                        stale: false,
                    });
                }
                CORE0_REORDER_CYCLES.release_observer_completed();
                #[cfg(any(feature = "diagnostics", test))]
                self.record_reorder_occupied();
                CORE0_REORDER_CYCLES.occupied_observer_completed();
                #[cfg(any(feature = "diagnostics", test))]
                self.observe_reorder_prepared(reorder_started);
                CORE0_REORDER_CYCLES.prepared_observer_completed();
                CORE0_REORDER_CYCLES.finish(Core0ReorderPath::Immediate);
                return Some(self.dispatch_owned_frame(frame).await);
            }
            Ok(None) => {
                CORE0_REORDER_CYCLES.finish(Core0ReorderPath::Slow);
            }
            Err(error) => {
                #[cfg(any(feature = "diagnostics", test))]
                if let Some(observer) = self.pipeline_observer {
                    observer.observe(RxPipelineObservation::ReorderDiscarded);
                }
                drop(frame);
                self.irq.notify_rx_capacity();
                CORE0_REORDER_CYCLES.finish(Core0ReorderPath::Slow);
                return Some(if matches!(error, RxAmpduError::DuplicateSequence(_)) {
                    ConnectedRxDispatch::Duplicate
                } else {
                    ConnectedRxDispatch::Ignored
                });
            }
        }

        let retain = match self
            .runtime
            .reorder_banks
            .state(bank)
            .expect("active TID was checked above")
            .retains_on_ingest(key.sequence)
        {
            Ok(retain) => retain,
            Err(error) => {
                #[cfg(any(feature = "diagnostics", test))]
                if let Some(observer) = self.pipeline_observer {
                    observer.observe(RxPipelineObservation::ReorderDiscarded);
                }
                drop(frame);
                self.irq.notify_rx_capacity();
                return Some(if matches!(error, RxAmpduError::DuplicateSequence(_)) {
                    ConnectedRxDispatch::Duplicate
                } else {
                    ConnectedRxDispatch::Ignored
                });
            }
        };
        // A 64-slot hot pool can retain the maximum 31 out-of-order frames,
        // admit the next 32-descriptor hardware burst and still own the
        // current frontier frame. Smaller compositions keep the independent
        // cold backing so one sequence gap cannot exhaust DMA staging.
        let retain_hot = retain && SLOTS == RX_REORDER_BACKING_SLOT_COUNT;
        let reservation = if retain && !retain_hot {
            let Some(storage) = self.reorder_storage else {
                // An agreement must never retain the finite hot staging pool
                // when its independent backing was omitted by the composition.
                #[cfg(any(feature = "diagnostics", test))]
                if let Some(observer) = self.pipeline_observer {
                    observer.observe(RxPipelineObservation::ReorderDiscarded);
                }
                drop(frame);
                self.irq.notify_rx_capacity();
                return Some(ConnectedRxDispatch::Ignored);
            };
            match storage.try_reserve() {
                Ok(reservation) => Some(reservation),
                Err(_) => {
                    #[cfg(any(feature = "diagnostics", test))]
                    if let Some(observer) = self.pipeline_observer {
                        observer.observe(RxPipelineObservation::ReorderDiscarded);
                    }
                    drop(frame);
                    self.irq.notify_rx_capacity();
                    return Some(ConnectedRxDispatch::Ignored);
                }
            }
        } else {
            None
        };
        let slot = reservation.as_ref().map_or_else(
            || {
                if retain_hot {
                    frame.slot()
                } else {
                    RX_REORDER_CURRENT_SLOT
                }
            },
            |reservation| reservation.slot(),
        );
        let mpdu = RxAmpduMpdu {
            sequence: key.sequence,
            slot: slot as u8,
        };
        let release = match self
            .runtime
            .reorder_banks
            .state_mut(bank)
            .expect("active TID was checked above")
            .ingest(mpdu)
        {
            Ok(release) => release,
            Err(error) => {
                #[cfg(any(feature = "diagnostics", test))]
                if let Some(observer) = self.pipeline_observer {
                    observer.observe(RxPipelineObservation::ReorderDiscarded);
                }
                drop(reservation);
                drop(frame);
                self.irq.notify_rx_capacity();
                return Some(if matches!(error, RxAmpduError::DuplicateSequence(_)) {
                    ConnectedRxDispatch::Duplicate
                } else {
                    ConnectedRxDispatch::Ignored
                });
            }
        };
        self.update_gap_deadline(bank);
        #[cfg(any(feature = "diagnostics", test))]
        self.record_reorder_occupied();
        if release.buffered {
            if retain_hot {
                debug_assert!(slot < self.runtime.retained.len());
                debug_assert!(self.runtime.retained[slot].is_none());
                self.runtime.retained[slot] = Some(RetainedRxFrame::Hot(frame));
                return self.dispatch_release(release).await;
            }
            let reservation = reservation.expect("predicted retained frame owns backing");
            let retained = match reservation.copy_from(frame.segment()) {
                Ok(retained) => retained,
                Err((_error, reservation)) => {
                    #[cfg(any(feature = "diagnostics", test))]
                    if let Some(observer) = self.pipeline_observer {
                        observer.observe(RxPipelineObservation::ReorderDiscarded);
                    }
                    let rollback = self
                        .runtime
                        .reorder_banks
                        .stop_bank(bank)
                        .expect("active reorder owns the failed retained copy");
                    self.runtime.gap_deadlines[bank] = None;
                    self.runtime.reorder_first_starts[bank] = None;
                    drop(reservation);
                    return self
                        .dispatch_release_with_current(rollback, slot, frame)
                        .await
                        .or(Some(ConnectedRxDispatch::Ignored));
                }
            };
            debug_assert_eq!(retained.slot(), slot);
            debug_assert!(self.runtime.retained[slot].is_none());
            self.runtime.retained[slot] = Some(RetainedRxFrame::Cold(retained));
            drop(frame);
            self.irq.notify_rx_capacity();
            self.dispatch_release(release).await
        } else {
            drop(reservation);
            self.dispatch_release_with_current(release, slot, frame)
                .await
        }
    }

    pub(super) async fn apply_reorder_command(
        &mut self,
        command: RxReorderCommand,
    ) -> Option<ConnectedRxDispatch> {
        match command {
            RxReorderCommand::Start(agreement) => {
                let bank = usize::from(agreement.hardware_index);
                let released = self.runtime.reorder_banks.start(agreement).ok().flatten();
                if self.runtime.reorder_banks.identity(bank) != Some(agreement.identity()) {
                    return None;
                }
                self.runtime.reorder_first_starts[bank] = Some(agreement.starting_sequence);
                self.runtime.gap_deadlines[bank] = None;
                #[cfg(any(feature = "diagnostics", test))]
                if let Some(observer) = self.pipeline_observer {
                    observer.observe(RxPipelineObservation::ReorderStarted {
                        tid: agreement.tid,
                        starting_sequence: agreement.starting_sequence,
                        window: agreement.window,
                    });
                }
                #[cfg(any(feature = "diagnostics", test))]
                if let Some(observer) = self.reorder_observer {
                    observer.observe(RxReorderAgreementObservation::Started {
                        tid: agreement.tid,
                        starting_sequence: agreement.starting_sequence,
                        window: agreement.window,
                    });
                }
                match released {
                    Some(release) => self.dispatch_release(release).await,
                    None => None,
                }
            }
            RxReorderCommand::Stop(identity) => {
                let bank = usize::from(identity.hardware_index);
                if bank >= RX_BLOCK_ACK_BANK_COUNT
                    || self.runtime.reorder_banks.identity(bank) != Some(identity)
                {
                    None
                } else {
                    self.stop_reorder(bank).await
                }
            }
            RxReorderCommand::StopInterface(interface) => {
                let mut result = None;
                for bank in 0..RX_BLOCK_ACK_BANK_COUNT {
                    if self
                        .runtime
                        .reorder_banks
                        .identity(bank)
                        .is_some_and(|identity| identity.interface == interface)
                        && let Some(released) = self.stop_reorder(bank).await
                    {
                        result = Some(released);
                    }
                }
                result
            }
        }
    }

    async fn stop_reorder(&mut self, bank: usize) -> Option<ConnectedRxDispatch> {
        self.runtime.gap_deadlines[bank] = None;
        self.runtime.reorder_first_starts[bank] = None;
        let release = self.runtime.reorder_banks.stop_bank(bank)?;
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.pipeline_observer {
            observer.observe(RxPipelineObservation::ReorderStopped);
        }
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.reorder_observer {
            observer.observe(RxReorderAgreementObservation::Stopped);
        }
        #[cfg(any(feature = "diagnostics", test))]
        self.record_reorder_occupied();
        self.dispatch_release(release).await
    }

    pub(super) fn next_gap_deadline(&self) -> Option<(usize, Instant)> {
        self.runtime
            .gap_deadlines
            .iter()
            .copied()
            .enumerate()
            .filter_map(|(tid, deadline)| deadline.map(|deadline| (tid, deadline)))
            .min_by_key(|(_, deadline)| *deadline)
    }

    fn update_gap_deadline(&mut self, tid: usize) {
        if self
            .runtime
            .reorder_banks
            .state(tid)
            .is_some_and(|reorder| reorder.occupied() != 0)
        {
            self.runtime.gap_deadlines[tid].get_or_insert_with(|| {
                // An unrepresentable deadline is never reached.
                self.clock
                    .now()
                    .checked_add(Duration::from_micros(
                        oer_espressif_ieee80211_policy::block_ack::RX_REORDER_GAP_TIMEOUT_MICROS,
                    ))
                    .unwrap_or(Instant::from_micros(u64::MAX))
            });
        } else {
            self.runtime.gap_deadlines[tid] = None;
        }
    }

    /// Apply a BlockAckReq from the associated AP to its TID's reorder window.
    async fn move_reorder_window(&mut self, request: RxBlockAckRequestKey) -> ConnectedRxDispatch {
        let Some(bank) = self.runtime.reorder_banks.find(
            oer_esp32s31_ieee80211_mac::MacInterface::Station,
            request.peer,
            request.tid,
        ) else {
            return ConnectedRxDispatch::BlockAckRequest;
        };
        let Some(release) = self
            .runtime
            .reorder_banks
            .state_mut(bank)
            .and_then(|reorder| reorder.move_window_to(request.starting_sequence))
        else {
            return ConnectedRxDispatch::BlockAckRequest;
        };
        self.update_gap_deadline(bank);
        #[cfg(any(feature = "diagnostics", test))]
        self.record_reorder_occupied();
        self.dispatch_release(release).await;
        ConnectedRxDispatch::BlockAckRequest
    }

    pub(super) async fn expire_reorder_gap(&mut self, tid: usize) -> Option<ConnectedRxDispatch> {
        self.runtime.gap_deadlines[tid] = None;
        let release = self.runtime.reorder_banks.state_mut(tid)?.expire_gap();
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.pipeline_observer {
            observer.observe(RxPipelineObservation::ReorderGapExpired);
        }
        self.update_gap_deadline(tid);
        #[cfg(any(feature = "diagnostics", test))]
        self.record_reorder_occupied();
        self.dispatch_release(release).await
    }

    #[cfg(any(feature = "diagnostics", test))]
    fn observe_reorder_prepared(&self, started: Option<u64>) {
        if let (Some(observer), Some(started)) = (self.pipeline_observer, started) {
            observer.observe(RxPipelineObservation::ReorderPrepared {
                micros: observer.elapsed_micros_since(started),
            });
        }
    }

    #[cfg(any(feature = "diagnostics", test))]
    fn record_reorder_occupied(&self) {
        let Some(observer) = self.pipeline_observer else {
            return;
        };
        let occupied = self.runtime.reorder_banks.occupied();
        observer.observe(RxPipelineObservation::ReorderOccupied { occupied });
    }

    async fn dispatch_release(&mut self, release: RxAmpduRelease) -> Option<ConnectedRxDispatch> {
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.pipeline_observer {
            observer.observe(RxPipelineObservation::ReorderReleased {
                buffered: release.buffered,
                released: release.count,
                missing: release.missing,
                stale: release.rejected.is_some(),
            });
        }
        let mut result = None;
        for released in release.iter() {
            let slot = usize::from(released.slot);
            let frame = self.runtime.retained[slot]
                .take()
                .expect("reorder release must reference one retained frame lease");
            result = Some(self.dispatch_retained_frame(frame).await);
        }
        if let Some(rejected) = release.rejected {
            self.release_retained_slot(usize::from(rejected.slot));
            result = Some(ConnectedRxDispatch::Duplicate);
        }
        result
    }

    async fn dispatch_release_with_current(
        &mut self,
        release: RxAmpduRelease,
        current_slot: usize,
        current_frame: StagedRxFrame<'pool, CAPACITY, SLOTS>,
    ) -> Option<ConnectedRxDispatch> {
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.pipeline_observer {
            observer.observe(RxPipelineObservation::ReorderReleased {
                buffered: release.buffered,
                released: release.count,
                missing: release.missing,
                stale: release.rejected.is_some(),
            });
        }
        let mut current_frame = Some(current_frame);
        let mut result = None;
        for released in release.iter() {
            let slot = usize::from(released.slot);
            result = Some(if slot == current_slot {
                self.dispatch_owned_frame(
                    current_frame
                        .take()
                        .expect("current reorder release is unique"),
                )
                .await
            } else {
                let frame = self.runtime.retained[slot]
                    .take()
                    .expect("reorder release references retained cold backing");
                self.dispatch_retained_frame(frame).await
            });
        }
        if let Some(rejected) = release.rejected {
            let slot = usize::from(rejected.slot);
            if slot == current_slot {
                drop(current_frame.take());
                self.irq.notify_rx_capacity();
            } else {
                self.release_retained_slot(slot);
            }
            result = Some(ConnectedRxDispatch::Duplicate);
        }
        debug_assert!(current_frame.is_none());
        result
    }

    fn release_retained_slot(&mut self, slot: usize) {
        if let Some(frame) = self.runtime.retained[slot].take() {
            let hot = matches!(&frame, RetainedRxFrame::Hot(_));
            drop(frame);
            if hot {
                self.irq.notify_rx_capacity();
            }
        }
    }
}
