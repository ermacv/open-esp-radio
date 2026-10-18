use super::*;
use oer_ieee80211_mac::sequence::SequenceNumber;
use oer_time::Clock as _;

use crate::diagnostics::core0_rx_performance::{
    CORE0_PERFORMANCE, Core0PerformanceSample, Core0TxPhase,
};

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
    pub(super) fn radio_stamp(
        &self,
    ) -> Result<oer_ieee80211_lower_mac::Ieee80211Stamp, AggregateTxError> {
        crate::mac_clock::MacClockReader::sample(self.ordinary.timer())
            .map(|sample| sample.stamp())
            .ok_or(AggregateTxError::RadioClockUnavailable)
    }

    /// Admit a selected request through its source before the ordinary STA
    /// classifier, BlockAck, retry and DMA ownership transitions.
    ///
    /// The request need not own Ethernet bytes. Its source constructs the
    /// first final frame on admission and supplies any subsequent frames only
    /// as the existing physical publication path asks for them.
    ///
    /// A busy owner or a source unable to construct its first frame returns
    /// the original request. Once construction succeeds, radio errors follow
    /// the existing physical-frame ownership contract.
    pub fn start_request<H, I>(
        &mut self,
        hardware: &mut H,
        request: I::Request,
        source: &I,
    ) -> Result<WifiTxProgress, RequestTxError<I::Request>>
    where
        H: HtAmpduHardware,
        I: oer_ieee80211_datapath::TxRequestSource<Frame = B>,
    {
        if self.active() || self.has_prepared_network_tx() {
            return Err(RequestTxError::Busy(request));
        }
        let first = source
            .try_materialize(request)
            .map_err(RequestTxError::Unmaterialized)?;
        self.start_network(hardware, first, source)
            .map_err(RequestTxError::Radio)
    }

    /// Materialize the first selected request into software-owned standby.
    /// Unavailable standby storage or failed first-frame construction returns
    /// the request. DMA publication remains a separate transition.
    pub fn prepare_request_standby<I>(
        &mut self,
        request: I::Request,
        source: &I,
    ) -> Result<(), I::Request>
    where
        I: oer_ieee80211_datapath::TxRequestSource<Frame = B>,
    {
        if !self.can_prepare_network_tx() {
            return Err(request);
        }
        let first = source.try_materialize(request)?;
        self.prepare_network_standby(first, source);
        Ok(())
    }

    pub fn start_network<H, I>(
        &mut self,
        hardware: &mut H,
        first: B,
        network: &I,
    ) -> Result<WifiTxProgress, AggregateTxError>
    where
        H: HtAmpduHardware,
        I: PhysicalTxSource<Frame = B>,
    {
        if self.active() || self.has_prepared_network_tx() {
            return Err(AggregateTxError::ActiveTransaction);
        }
        // Rate-control observations affect only the next fresh exchange.
        // An already published or retained retry keeps its original typed
        // rate and descriptor program until terminal completion.
        self.config.rate = self.rate_control.ampdu_tx_rate(self.aggregate_rate_policy);
        self.last_aggregate_status = None;
        let selected = self.ordinary.select_network_traffic(first.ethernet())?;
        let aggregate_rate = !matches!(self.config.rate, TxPhyRate::Legacy(_));
        let ht_requires_pair = matches!(self.config.rate, TxPhyRate::Ht(_));
        if !aggregate_rate {
            return self.start_network_ordinary(
                hardware,
                first,
                selected,
                NetworkSingleMpduReason::LegacyRate,
            );
        }
        if !self.block_ack_operational(selected.tid()) {
            return self.start_network_ordinary(
                hardware,
                first,
                selected,
                NetworkSingleMpduReason::BlockAckUnavailable,
            );
        }
        if ht_requires_pair && network.pending_frames() == 0 {
            return self.start_network_ordinary(
                hardware,
                first,
                selected,
                NetworkSingleMpduReason::HtNeedsPair,
            );
        }

        let traffic = match self.aggregate_traffic(selected) {
            Ok(traffic) => traffic,
            Err(_) => {
                return self.start_network_ordinary(
                    hardware,
                    first,
                    selected,
                    NetworkSingleMpduReason::FreshAggregateCapacity,
                );
            }
        };

        // Ordinary TX validates Ethernet length before consuming sequence or
        // CCMP state. Establish the extra in-place A-MPDU prefix geometry at
        // the same pre-mutation boundary; otherwise a malformed netstack
        // lease could consume aggregate metadata and only then fail encode.
        if !self.frame_has_aggregate_geometry(&first) {
            return self.start_network_ordinary(
                hardware,
                first,
                selected,
                NetworkSingleMpduReason::FreshAggregateCapacity,
            );
        }

        // BlockAck eligibility does not imply that every network frame fits
        // the peer/rate/TXOP ceiling of a fresh aggregate. In particular,
        // control-plane traffic can arrive immediately after ADDBA. Such a
        // frame remains a valid ordinary QoS MPDU and must not terminate the
        // complete radio runner with `AggregateFull`.
        if !self.first_frame_fits_fresh_aggregate(first.ethernet_length(), traffic)? {
            return self.start_network_ordinary(
                hardware,
                first,
                selected,
                NetworkSingleMpduReason::FreshAggregateCapacity,
            );
        }

        #[cfg(any(feature = "diagnostics", test))]
        let preparation_started = self.observer.map(|_| self.ordinary.now());
        let prepared = self.prepare_aggregate(first, network, traffic)?;
        #[cfg(any(feature = "diagnostics", test))]
        self.observe_prepared(&prepared);
        #[cfg(any(feature = "diagnostics", test))]
        if let (Some(observer), Some(started)) = (self.observer, preparation_started) {
            observer.observe(AggregateTxObservation::PreparationCompleted {
                micros: self
                    .ordinary
                    .now()
                    .saturating_duration_since(started)
                    .as_micros(),
            });
        }
        self.activate_prepared(prepared)?;
        let progress = self.publish_initial(hardware)?;
        self.prepare_standby(network);
        Ok(progress)
    }

    /// Rate and publication budget of the next network data MPDU, taken
    /// from the ordinary schedule the rate control currently selects.
    pub(super) fn data_tx_selection(&self) -> StaDataTxSelection {
        StaDataTxSelection {
            rate: self.rate_control.tx_rate(self.data_rate_policy),
            publication_limit: schedule_publication_limit(self.rate_control.current_schedule()),
        }
    }

    fn start_network_ordinary<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        first: B,
        traffic: WifiTxTraffic,
        _reason: NetworkSingleMpduReason,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        #[cfg(any(feature = "diagnostics", test))]
        let ethernet_length = first.ethernet_length();
        let selection = self.data_tx_selection();
        let progress =
            self.ordinary
                .start_with_traffic(hardware, first.ethernet(), traffic, selection)?;
        drop(first);
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer {
            observer.observe(AggregateTxObservation::NetworkSingleMpdu {
                reason: _reason,
                ethernet_length,
            });
        }
        self.active = ConnectedTxActive::Ordinary;
        self.network_ordinary = true;
        self.network_power.started = true;
        Ok(progress)
    }

    fn first_frame_fits_fresh_aggregate(
        &self,
        ethernet_length: usize,
        traffic: AggregateTraffic,
    ) -> Result<bool, AggregateTxError> {
        let frame_length = ethernet_length
            .checked_add(STA_PROTECTED_QOS_ETHERNET_OVERHEAD)
            .ok_or(AggregateTxError::BufferSizeOverflow)?;
        let dma_capacity = B::MIN_STORAGE_CAPACITY;
        let hardware_mic_length = oer_esp32s31_ieee80211::ordinary_tx::TX_CCMP_MIC_SIZE as u8;
        let frame_size = AmpduFrameSize::new(frame_length, hardware_mic_length);
        let maximum_aggregate_bytes = self.ordinary.policy().ht_ampdu().maximum_aggregate_bytes();
        match self.config.rate {
            TxPhyRate::Ht(rate) => Ok(self.ampdu.active().can_fit_fresh_referenced_ht_frame(
                frame_length,
                hardware_mic_length,
                rate,
                maximum_aggregate_bytes,
                dma_capacity,
            )?),
            TxPhyRate::He(rate) => Ok(self.ampdu.active().can_fit_fresh_referenced_he_frame(
                frame_size,
                HeAmpduPolicy::new(
                    rate,
                    self.ordinary.policy().ht_ampdu().density(),
                    traffic.he_txop_limit,
                ),
                maximum_aggregate_bytes,
                dma_capacity,
            )?),
            TxPhyRate::Legacy(_) => Err(AggregateTxError::UnsupportedRate),
        }
    }

    fn aggregate_traffic(
        &self,
        selected: WifiTxTraffic,
    ) -> Result<AggregateTraffic, WmmTxopUnsupported> {
        let he_txop_limit = match self.config.rate {
            TxPhyRate::Ht(_) => {
                selected.require_ht_txop_support()?;
                self.config.he_txop_limit
            }
            TxPhyRate::He(_) => selected.he_txop_limit(self.config.he_txop_limit)?,
            TxPhyRate::Legacy(_) => self.config.he_txop_limit,
        };
        Ok(AggregateTraffic {
            selected,
            he_txop_limit,
        })
    }

    fn frame_matches_traffic(&self, frame: &B, traffic: AggregateTraffic) -> bool {
        self.frame_has_aggregate_geometry(frame)
            && self
                .ordinary
                .select_network_traffic(frame.ethernet())
                .is_ok_and(|selected| {
                    selected.tid() == traffic.tid()
                        && selected.access_category == traffic.selected.access_category
                })
    }

    fn frame_has_aggregate_geometry(&self, frame: &B) -> bool {
        frame.ethernet_length() >= 14
            && frame.ethernet_offset()
                >= STA_PROTECTED_QOS_ETHERNET_HEADROOM
                    + oer_esp32s31_ieee80211_mac::tx::ampdu::TX_AMPDU_METADATA_SIZE
    }

    fn defer_network_frame(&mut self, frame: B) {
        assert!(
            self.deferred_network.replace(frame).is_none(),
            "one immutable FIFO boundary may retain only its immediate successor"
        );
    }

    fn prepare_aggregate<I>(
        &mut self,
        first: B,
        network: &I,
        traffic: AggregateTraffic,
    ) -> Result<AggregatePrepared<SLOTS>, AggregateTxError>
    where
        I: PhysicalTxSource<Frame = B>,
    {
        let first_sequence = self
            .ordinary
            .peek_qos_sequence(traffic.tid())
            .ok_or(AggregateTxError::MissingQosSequence(traffic.tid()))?;
        // Association policy is owned outside the DMA-visible descriptor
        // arena. Reinstall its byte ceiling at every Free -> Reserved edge so
        // a new batch cannot depend on cold scalar contents retained beside
        // hardware-owned words.
        self.ampdu.active_mut().configure_max_aggregate_bytes(
            self.ordinary.policy().ht_ampdu().maximum_aggregate_bytes(),
        )?;
        let cookie = self.ampdu.active_mut().begin()?;
        self.cookie = Some(cookie);

        let result = self.prepare_reserved(first, network, first_sequence, cookie, traffic);
        if result.is_err() {
            self.cancel_current_reservation();
        }
        result
    }

    fn prepare_reserved<I>(
        &mut self,
        first: B,
        network: &I,
        first_sequence: SequenceNumber,
        cookie: TxCookie,
        traffic: AggregateTraffic,
    ) -> Result<AggregatePrepared<SLOTS>, AggregateTxError>
    where
        I: PhysicalTxSource<Frame = B>,
    {
        self.push_candidate(first, network, AggregateFrameAdmission::FreshExact, traffic)?;
        let frame_limit = self.aggregate_frame_limit(traffic.tid());

        let build_stop = loop {
            if self.deferred_network.is_some() {
                break AggregateBuildStop::QueueEmpty;
            }
            if self.ampdu.active().held_backing_count() >= frame_limit {
                break AggregateBuildStop::FrameLimit;
            }
            if !self.can_push(B::MAX_ETHERNET_LENGTH, traffic)? {
                break AggregateBuildStop::CapacityLimit;
            }
            let Some(frame) = network.try_take_physical() else {
                break AggregateBuildStop::QueueEmpty;
            };
            if !self.frame_matches_traffic(&frame, traffic) {
                self.defer_network_frame(frame);
                break AggregateBuildStop::QueueEmpty;
            }
            let admission = match self.config.rate {
                TxPhyRate::Ht(_) => AggregateFrameAdmission::HtQueueCapacity,
                TxPhyRate::He(_) => AggregateFrameAdmission::NeedsExactCheck,
                TxPhyRate::Legacy(_) => return Err(AggregateTxError::UnsupportedRate),
            };
            self.push_candidate(frame, network, admission, traffic)?;
        };

        let aggregate = self.ampdu.active().prepared_aggregate(cookie)?;
        let retry = AmpduRetryState::<SLOTS>::new(
            first_sequence,
            aggregate.subframes,
            AmpduRetryPolicy {
                lifetime: oer_time::RadioDuration::from_micros(
                    VENDOR_AMPDU_MSDU_LIFETIME_MICROS as u64,
                ),
                // An A-MSDU can exceed the ordinary descriptor's copy
                // buffer. Retain even one missing A-MSDU in the aggregate
                // owner so retry preserves its pinned backing, sequence and
                // PN instead of attempting an impossible ordinary detach.
                retain_single_mpdu: matches!(self.config.rate, TxPhyRate::He(_))
                    || self.block_ack_amsdu(traffic.tid()),
            },
            self.radio_stamp()?,
        )?;
        let prepared = AggregatePrepared {
            traffic,
            block_ack_generation: self
                .block_ack_generation(traffic.tid())
                .ok_or(AggregateTxError::MissingQosSequence(traffic.tid()))?,
            aggregate_length: aggregate.bytes,
            retry,
            original_subframes: aggregate.subframes,
            first_sequence,
            build_stop,
            #[cfg(any(feature = "diagnostics", test))]
            preparation_micros: 0,
        };
        Ok(prepared)
    }

    fn extend_reserved<I>(
        &mut self,
        first: B,
        network: &I,
        mut prepared: AggregatePrepared<SLOTS>,
    ) -> Result<AggregatePrepared<SLOTS>, AggregateTxError>
    where
        I: PhysicalTxSource<Frame = B>,
    {
        let traffic = prepared.traffic;
        if !self.frame_matches_traffic(&first, traffic) {
            self.defer_network_frame(first);
            prepared.build_stop = AggregateBuildStop::QueueEmpty;
            return Ok(prepared);
        }
        let admission = match self.config.rate {
            TxPhyRate::Ht(_) => AggregateFrameAdmission::HtQueueCapacity,
            TxPhyRate::He(_) => AggregateFrameAdmission::NeedsExactCheck,
            TxPhyRate::Legacy(_) => return Err(AggregateTxError::UnsupportedRate),
        };
        self.push_candidate(first, network, admission, traffic)?;
        let frame_limit = self.aggregate_frame_limit(traffic.tid());
        let build_stop = loop {
            if self.deferred_network.is_some() {
                break AggregateBuildStop::QueueEmpty;
            }
            if self.ampdu.active().held_backing_count() >= frame_limit {
                break AggregateBuildStop::FrameLimit;
            }
            if !self.can_push(B::MAX_ETHERNET_LENGTH, traffic)? {
                break AggregateBuildStop::CapacityLimit;
            }
            let Some(frame) = network.try_take_physical() else {
                break AggregateBuildStop::QueueEmpty;
            };
            if !self.frame_matches_traffic(&frame, traffic) {
                self.defer_network_frame(frame);
                break AggregateBuildStop::QueueEmpty;
            }
            self.push_candidate(frame, network, admission, traffic)?;
        };
        let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
        let aggregate = self.ampdu.active().prepared_aggregate(cookie)?;
        prepared.aggregate_length = aggregate.bytes;
        prepared.original_subframes = aggregate.subframes;
        prepared.build_stop = build_stop;
        prepared.retry = AmpduRetryState::<SLOTS>::new(
            prepared.first_sequence,
            aggregate.subframes,
            AmpduRetryPolicy {
                lifetime: oer_time::RadioDuration::from_micros(
                    VENDOR_AMPDU_MSDU_LIFETIME_MICROS as u64,
                ),
                retain_single_mpdu: matches!(self.config.rate, TxPhyRate::He(_))
                    || self.block_ack_amsdu(traffic.tid()),
            },
            self.radio_stamp()?,
        )?;
        Ok(prepared)
    }

    #[cfg(any(feature = "diagnostics", test))]
    fn observe_prepared(&self, prepared: &AggregatePrepared<SLOTS>) {
        if let Some(observer) = self.observer {
            let bandwidth_mhz = match self.config.rate {
                TxPhyRate::Ht(rate) => match rate.channel_width {
                    oer_esp32s31_ieee80211_mac::tx::HtChannelWidth::Mhz20 => 20,
                    oer_esp32s31_ieee80211_mac::tx::HtChannelWidth::Mhz40 => 40,
                },
                TxPhyRate::He(_) | TxPhyRate::Legacy(_) => 20,
            };
            observer.observe(AggregateTxObservation::RateSelected {
                bandwidth_mhz,
                nominal_kbps: self.config.rate.nominal_kbps(),
            });
            observer.observe(AggregateTxObservation::Prepared {
                subframes: prepared.original_subframes,
                stop: prepared.build_stop,
            });
        }
    }

    fn activate_prepared(
        &mut self,
        prepared: AggregatePrepared<SLOTS>,
    ) -> Result<(), AggregateTxError> {
        let config = self.publication_config(
            prepared.aggregate_length,
            prepared.original_subframes,
            prepared.traffic,
        )?;
        self.active = ConnectedTxActive::Aggregate(AggregateActive {
            traffic: prepared.traffic,
            block_ack_generation: prepared.block_ack_generation,
            config,
            retry: prepared.retry,
            original_subframes: prepared.original_subframes,
            deadline: oer_time::Instant::EPOCH,
            #[cfg(any(feature = "diagnostics", test))]
            first_publication_micros: None,
            #[cfg(feature = "tx-wait-probe")]
            wait_probe: wait_probe::Schedule::default(),
            #[cfg(feature = "tx-wait-probe")]
            first_sequence: prepared.first_sequence,
        });
        Ok(())
    }

    /// Fill the software-owned second arena after the current aggregate has
    /// already been published. No descriptor from this arena becomes visible
    /// to MAC hardware at this edge.
    fn prepare_standby<I>(&mut self, network: &I)
    where
        I: PhysicalTxSource<Frame = B>,
    {
        if !self.can_prepare_network_tx() {
            return;
        }
        let minimum_frames = if matches!(self.config.rate, TxPhyRate::Ht(_)) {
            2
        } else {
            1
        };
        if network.pending_frames() < minimum_frames {
            return;
        }
        let Some(first) = network.try_take_physical() else {
            return;
        };

        self.prepare_network_standby(first, network);
    }

    /// Whether a standby network transaction can be prepared now.
    pub fn can_prepare_network_tx(&self) -> bool {
        // The second arena pipelines only the batch after an aggregate that
        // is already hardware-owned. An ordinary transaction may be a
        // control frame sharing sequence/key policy with this owner; letting
        // network preparation cross that boundary would mutate the next data
        // batch while connected control still owns the transaction.
        let base = matches!(self.active, ConnectedTxActive::Aggregate(_))
            && self.ampdu.has_standby()
            && self.standby_error.is_none()
            && self.deferred_network.is_none()
            && !matches!(self.config.rate, TxPhyRate::Legacy(_));
        if !base {
            return false;
        }
        match self.standby_prepared.as_ref() {
            // Classification needs the immutable Ethernet lease. The
            // handoff retains it as `deferred_network` if this rate/TID
            // cannot extend the aggregate, so no FIFO entry is lost here.
            None => true,
            Some(prepared) => {
                let traffic = prepared.traffic;
                let frame_limit = self.aggregate_frame_limit(traffic.tid());
                self.block_ack_generation(traffic.tid()) == Some(prepared.block_ack_generation)
                    && self.ampdu.standby().is_some_and(|owner| {
                        owner.held_backing_count() < frame_limit
                            && owner.held_backing_count() < SLOTS
                    })
                    && matches!(
                        self.can_push_standby(B::MAX_ETHERNET_LENGTH, traffic),
                        Ok(true)
                    )
            }
        }
    }

    fn can_push_standby(
        &self,
        ethernet_length: usize,
        traffic: AggregateTraffic,
    ) -> Result<bool, AggregateTxError> {
        let ampdu = self
            .ampdu
            .standby()
            .ok_or(AggregateTxError::InvalidPublicationState)?;
        let cookie = self.standby_cookie.ok_or(AggregateTxError::MissingCookie)?;
        let frame_length = ethernet_length
            .checked_add(STA_PROTECTED_QOS_ETHERNET_OVERHEAD)
            .ok_or(AggregateTxError::BufferSizeOverflow)?;
        let dma_capacity = B::MIN_STORAGE_CAPACITY;
        let hardware_mic_length = oer_esp32s31_ieee80211::ordinary_tx::TX_CCMP_MIC_SIZE as u8;
        match self.config.rate {
            TxPhyRate::Ht(rate) => Ok(ampdu.can_commit_referenced_ht_frame(
                cookie,
                frame_length,
                hardware_mic_length,
                0,
                rate,
                dma_capacity,
            )?),
            TxPhyRate::He(rate) => Ok(ampdu.can_commit_referenced_he_frame(
                cookie,
                AmpduFrameSize::new(frame_length, hardware_mic_length),
                HeAmpduPolicy::new(
                    rate,
                    self.ordinary.policy().ht_ampdu().density(),
                    traffic.he_txop_limit,
                ),
                dma_capacity,
            )?),
            TxPhyRate::Legacy(_) => Err(AggregateTxError::UnsupportedRate),
        }
    }

    pub(super) fn prepare_network_standby<I>(&mut self, first: B, network: &I)
    where
        I: PhysicalTxSource<Frame = B>,
    {
        if !self.can_prepare_network_tx() {
            drop(first);
            return;
        }

        // Retain an invalid immediate successor at the immutable FIFO
        // boundary. The ordinary path will report its exact validation error
        // after the hardware-owned predecessor completes, without spending a
        // QoS sequence number or packet number in the standby arena.
        if !self.frame_has_aggregate_geometry(&first) {
            self.defer_network_frame(first);
            return;
        }

        let selected = match self.ordinary.select_network_traffic(first.ethernet()) {
            Ok(selected) => selected,
            Err(error) => {
                drop(first);
                self.standby_error = Some(error.into());
                return;
            }
        };
        let traffic = match self.aggregate_traffic(selected) {
            Ok(traffic) => traffic,
            Err(_) => {
                self.defer_network_frame(first);
                return;
            }
        };
        if !self.block_ack_operational(traffic.tid())
            || (self.standby_prepared.is_none()
                && matches!(self.config.rate, TxPhyRate::Ht(_))
                && network.pending_frames() == 0)
            || !matches!(
                self.first_frame_fits_fresh_aggregate(first.ethernet_length(), traffic),
                Ok(true)
            )
        {
            self.defer_network_frame(first);
            return;
        }
        if self.standby_prepared.as_ref().is_some_and(|prepared| {
            prepared.traffic.tid() != traffic.tid()
                || prepared.traffic.queue() != traffic.queue()
                || prepared.traffic.he_txop_limit != traffic.he_txop_limit
        }) {
            self.defer_network_frame(first);
            return;
        }

        #[cfg(any(feature = "diagnostics", test))]
        let started = self.observer.map(|_| self.ordinary.now());
        assert!(
            self.ampdu.swap_active_standby(),
            "standby presence checked before preparation"
        );
        core::mem::swap(&mut self.cookie, &mut self.standby_cookie);
        let previous = self.standby_prepared.take();
        #[cfg(any(feature = "diagnostics", test))]
        let extending = previous.is_some();
        let result = match previous {
            Some(prepared) => self.extend_reserved(first, network, prepared),
            None => self.prepare_aggregate(first, network, traffic),
        };
        #[cfg(any(feature = "diagnostics", test))]
        let elapsed = started.map(|started| {
            self.ordinary
                .now()
                .saturating_duration_since(started)
                .as_micros()
        });
        if result.is_err() && self.cookie.is_some() {
            self.cancel_current_reservation();
        }
        core::mem::swap(&mut self.cookie, &mut self.standby_cookie);
        assert!(
            self.ampdu.swap_active_standby(),
            "temporary standby selection preserves both arenas"
        );

        match result {
            Ok(prepared) => {
                #[cfg(any(feature = "diagnostics", test))]
                let mut prepared = prepared;
                #[cfg(any(feature = "diagnostics", test))]
                {
                    prepared.preparation_micros = prepared
                        .preparation_micros
                        .wrapping_add(elapsed.unwrap_or(0));
                }
                self.standby_prepared = Some(prepared);
                #[cfg(any(feature = "diagnostics", test))]
                if !extending && let Some(observer) = self.observer {
                    observer.observe(AggregateTxObservation::StandbyPrepared);
                }
            }
            Err(error) => self.standby_error = Some(error),
        }
    }

    pub(super) fn start_prepared_network<H, I>(
        &mut self,
        hardware: &mut H,
        network: &I,
    ) -> Result<WifiTxProgress, AggregateTxError>
    where
        H: HtAmpduHardware,
        I: PhysicalTxSource<Frame = B>,
    {
        if self.active() {
            return Err(AggregateTxError::ActiveTransaction);
        }
        if let Some(error) = self.standby_error.take() {
            return Err(error);
        }
        if let Some(prepared) = self.standby_prepared.as_ref()
            && self.block_ack_generation(prepared.traffic.tid())
                != Some(prepared.block_ack_generation)
        {
            let tid = prepared.traffic.tid();
            self.cancel_prepared_network()?;
            return Err(AggregateTxError::BlockAckAgreementChanged { tid });
        }
        if let Some(prepared) = &self.standby_prepared {
            prepared.retry.validate_stamp(self.radio_stamp()?)?;
        }
        let Some(prepared) = self.standby_prepared.take() else {
            let first = self
                .deferred_network
                .take()
                .ok_or(AggregateTxError::InvalidPublicationState)?;
            return self.start_network(hardware, first, network);
        };
        #[cfg(any(feature = "diagnostics", test))]
        self.observe_prepared(&prepared);
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer {
            observer.observe(AggregateTxObservation::PreparationCompleted {
                micros: prepared.preparation_micros,
            });
        }
        assert!(
            self.ampdu.swap_active_standby(),
            "prepared standby retains its arena"
        );
        core::mem::swap(&mut self.cookie, &mut self.standby_cookie);
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer {
            observer.observe(AggregateTxObservation::StandbyPublished);
        }
        self.activate_prepared(prepared)?;
        let progress = self.publish_initial(hardware)?;
        self.prepare_standby(network);
        Ok(progress)
    }

    fn can_push(
        &self,
        ethernet_length: usize,
        traffic: AggregateTraffic,
    ) -> Result<bool, AggregateTxError> {
        let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
        let frame_length = ethernet_length
            .checked_add(STA_PROTECTED_QOS_ETHERNET_OVERHEAD)
            .ok_or(AggregateTxError::BufferSizeOverflow)?;
        let dma_capacity = B::MIN_STORAGE_CAPACITY;
        let hardware_mic_length = oer_esp32s31_ieee80211::ordinary_tx::TX_CCMP_MIC_SIZE as u8;
        let frame_size = AmpduFrameSize::new(frame_length, hardware_mic_length);
        match self.config.rate {
            TxPhyRate::Ht(rate) => Ok(self.ampdu.active().can_commit_referenced_ht_frame(
                cookie,
                frame_length,
                hardware_mic_length,
                0,
                rate,
                dma_capacity,
            )?),
            TxPhyRate::He(rate) => Ok(self.ampdu.active().can_commit_referenced_he_frame(
                cookie,
                frame_size,
                HeAmpduPolicy::new(
                    rate,
                    self.ordinary.policy().ht_ampdu().density(),
                    traffic.he_txop_limit,
                ),
                dma_capacity,
            )?),
            TxPhyRate::Legacy(_) => Err(AggregateTxError::UnsupportedRate),
        }
    }

    fn can_push_amsdu_pair(
        &self,
        first_ethernet_length: usize,
        second_ethernet_length: usize,
        traffic: AggregateTraffic,
    ) -> Result<bool, AggregateTxError> {
        let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
        let frame_length =
            sta_protected_amsdu_pair_frame_length(first_ethernet_length, second_ethernet_length)
                .map_err(AggregateTxError::Encode)?;
        let dma_capacity = B::MIN_STORAGE_CAPACITY;
        let hardware_mic_length = oer_esp32s31_ieee80211::ordinary_tx::TX_CCMP_MIC_SIZE as u8;
        let frame_size = AmpduFrameSize::new(frame_length, hardware_mic_length);
        match self.config.rate {
            TxPhyRate::Ht(rate) => Ok(self.ampdu.active().can_commit_referenced_ht_frame(
                cookie,
                frame_length,
                hardware_mic_length,
                0,
                rate,
                dma_capacity,
            )?),
            TxPhyRate::He(rate) => Ok(self.ampdu.active().can_commit_referenced_he_frame(
                cookie,
                frame_size,
                HeAmpduPolicy::new(
                    rate,
                    self.ordinary.policy().ht_ampdu().density(),
                    traffic.he_txop_limit,
                ),
                dma_capacity,
            )?),
            TxPhyRate::Legacy(_) => Err(AggregateTxError::UnsupportedRate),
        }
    }

    fn push_candidate<I>(
        &mut self,
        first: B,
        network: &I,
        admission: AggregateFrameAdmission,
        traffic: AggregateTraffic,
    ) -> Result<(), AggregateTxError>
    where
        I: PhysicalTxSource<Frame = B>,
    {
        if self.block_ack_amsdu(traffic.tid())
            && self.can_push_amsdu_pair(first.ethernet_length(), B::MAX_ETHERNET_LENGTH, traffic)?
            && let Some(second) = network.try_take_physical()
        {
            if self.frame_matches_traffic(&second, traffic) {
                return self.push_amsdu_pair(first, second, traffic);
            }
            self.defer_network_frame(second);
        }
        self.push_frame(first, admission, traffic)
    }

    fn push_amsdu_pair(
        &mut self,
        mut first: B,
        second: B,
        traffic: AggregateTraffic,
    ) -> Result<(), AggregateTxError> {
        if self.ampdu.active().held_backing_count() >= SLOTS {
            return Err(HtAmpduTxError::AggregateFull.into());
        }
        let encode_started = Core0PerformanceSample::read();
        let metadata = self
            .ordinary
            .take_protected_metadata(traffic.tid())
            .map_err(SingleMpduTxError::from)?
            .ok_or(AggregateTxError::MissingQosSequence(traffic.tid()))?;
        let ethernet_offset = first.ethernet_offset();
        let ethernet_length = first.ethernet_length();
        let encoded = metadata
            .encode_amsdu_pair_in_place(
                first.storage_mut(),
                ethernet_offset,
                ethernet_length,
                second.ethernet(),
            )
            .map_err(AggregateTxError::Encode)?;
        CORE0_PERFORMANCE.record_tx_phase(
            Core0TxPhase::Encode,
            encode_started,
            Core0PerformanceSample::read(),
        );
        let metadata_size = oer_esp32s31_ieee80211_mac::tx::ampdu::TX_AMPDU_METADATA_SIZE;
        let dma_offset = encoded.offset.checked_sub(metadata_size).ok_or(
            AggregateTxError::DmaPrefixGeometry {
                encoded_offset: encoded.offset,
                metadata_size,
            },
        )?;
        let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
        let hardware_mic_length = oer_esp32s31_ieee80211::ordinary_tx::TX_CCMP_MIC_SIZE as u8;
        let layout = AmpduFrameLayout::new(
            dma_offset,
            AmpduFrameSize::new(encoded.length, hardware_mic_length),
        )
        .ok_or(AggregateTxError::DmaPrefixGeometry {
            encoded_offset: encoded.offset,
            metadata_size,
        })?;
        let commit_started = Core0PerformanceSample::read();
        match self.config.rate {
            TxPhyRate::Ht(rate) => self.ampdu.active_mut().commit_ht(
                cookie,
                first,
                HtAmpduFrameRequest::new(layout, 0, rate),
            )?,
            TxPhyRate::He(rate) => self.ampdu.active_mut().commit_he(
                cookie,
                first,
                HeAmpduFrameRequest::new(
                    layout,
                    HeAmpduPolicy::new(
                        rate,
                        self.ordinary.policy().ht_ampdu().density(),
                        traffic.he_txop_limit,
                    ),
                ),
            )?,
            TxPhyRate::Legacy(_) => return Err(AggregateTxError::UnsupportedRate),
        }
        CORE0_PERFORMANCE.record_tx_phase(
            Core0TxPhase::Commit,
            commit_started,
            Core0PerformanceSample::read(),
        );
        drop(second);
        Ok(())
    }

    fn push_frame(
        &mut self,
        mut frame: B,
        admission: AggregateFrameAdmission,
        traffic: AggregateTraffic,
    ) -> Result<(), AggregateTxError> {
        if self.ampdu.active().held_backing_count() >= SLOTS
            || (admission == AggregateFrameAdmission::NeedsExactCheck
                && !self.can_push(frame.ethernet_length(), traffic)?)
        {
            return Err(HtAmpduTxError::AggregateFull.into());
        }
        let encode_started = Core0PerformanceSample::read();
        let metadata = self
            .ordinary
            .take_protected_metadata(traffic.tid())
            .map_err(SingleMpduTxError::from)?
            .ok_or(AggregateTxError::MissingQosSequence(traffic.tid()))?;
        let ethernet_offset = frame.ethernet_offset();
        let ethernet_length = frame.ethernet_length();
        let encoded = metadata
            .encode_in_place(
                frame.storage_mut(),
                ethernet_offset,
                ethernet_length,
                DataHeControl::Disabled,
            )
            .map_err(AggregateTxError::Encode)?;
        CORE0_PERFORMANCE.record_tx_phase(
            Core0TxPhase::Encode,
            encode_started,
            Core0PerformanceSample::read(),
        );
        let metadata_size = oer_esp32s31_ieee80211_mac::tx::ampdu::TX_AMPDU_METADATA_SIZE;
        let dma_offset = encoded.offset.checked_sub(metadata_size).ok_or(
            AggregateTxError::DmaPrefixGeometry {
                encoded_offset: encoded.offset,
                metadata_size,
            },
        )?;
        let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
        let hardware_mic_length = oer_esp32s31_ieee80211::ordinary_tx::TX_CCMP_MIC_SIZE as u8;
        let frame_size = AmpduFrameSize::new(encoded.length, hardware_mic_length);
        let layout = AmpduFrameLayout::new(dma_offset, frame_size).ok_or(
            AggregateTxError::DmaPrefixGeometry {
                encoded_offset: encoded.offset,
                metadata_size,
            },
        )?;
        let commit_started = Core0PerformanceSample::read();
        match self.config.rate {
            TxPhyRate::Ht(rate) => self.ampdu.active_mut().commit_ht(
                cookie,
                frame,
                HtAmpduFrameRequest::new(layout, 0, rate),
            )?,
            TxPhyRate::He(rate) => self.ampdu.active_mut().commit_he(
                cookie,
                frame,
                HeAmpduFrameRequest::new(
                    layout,
                    HeAmpduPolicy::new(
                        rate,
                        self.ordinary.policy().ht_ampdu().density(),
                        traffic.he_txop_limit,
                    ),
                ),
            )?,
            TxPhyRate::Legacy(_) => return Err(AggregateTxError::UnsupportedRate),
        }
        CORE0_PERFORMANCE.record_tx_phase(
            Core0TxPhase::Commit,
            commit_started,
            Core0PerformanceSample::read(),
        );
        Ok(())
    }

    fn publication_config(
        &mut self,
        aggregate_length: u16,
        subframes: u8,
        traffic: AggregateTraffic,
    ) -> Result<AmpduTxConfig, AggregateTxError> {
        let queue = traffic.queue();
        let key = self.ordinary.hardware_key_selector();
        let (contention, contention_window) = self.ordinary.contention_publication(queue);
        match self.config.rate {
            TxPhyRate::Ht(rate) => {
                let role_policy = self
                    .ht_role_policy(traffic.tid())?
                    .ok_or(AggregateTxError::InvalidPublicationState)?;
                let data_power = self
                    .ordinary
                    .power_profile()
                    .power_pair(rate.power_lookup_code());
                let (_, control) = self.ordinary.control_frame_for(ProtectedPpdu {
                    rate: TxPhyRate::Ht(rate),
                    receiver: TxReceiver::Individual,
                    psdu: TxPsdu::Ampdu {
                        length: u32::from(aggregate_length),
                    },
                });
                let config = ht_ampdu_publication_config(
                    role_policy.role(),
                    HtAmpduPublicationInputs {
                        rate: role_policy.rate(),
                        aggregate_length,
                        subframes,
                        protection_spacing: self.ordinary.policy().ht_ampdu().protection_spacing(),
                        data_power_primary: data_power.primary as u8,
                        data_power_alternate: data_power.alternate as u8,
                        control,
                        aifsn: contention.aifsn(),
                        contention_window,
                        scheduler_priority: queue.vendor_data_scheduler_priority(),
                        packet_priority: queue.vendor_data_packet_priority(),
                    },
                )
                .ok_or(AggregateTxError::BufferSizeOverflow)?;
                Ok(AmpduTxConfig::Ht(config))
            }
            TxPhyRate::He(rate) => {
                let mut config = HeAmpduTxConfig::new_with_txop(
                    rate,
                    self.ordinary.policy().he_bss_color(),
                    aggregate_length,
                    subframes,
                    self.ordinary.policy().ht_ampdu().density(),
                    traffic.he_txop_limit,
                )
                .ok_or(AggregateTxError::BufferSizeOverflow)?;
                let data_power = self
                    .ordinary
                    .power_profile()
                    .power_pair(rate.power_lookup_code());
                config.data_power_primary = data_power.primary as u8;
                config.data_power_alternate = data_power.alternate as u8;
                config.control = self
                    .ordinary
                    .control_frame_for(ProtectedPpdu {
                        rate: TxPhyRate::He(rate),
                        receiver: TxReceiver::Individual,
                        psdu: TxPsdu::Ampdu {
                            length: u32::from(aggregate_length),
                        },
                    })
                    .1;
                config.aifsn = contention.aifsn();
                config.contention_window = contention_window;
                config.scheduler_priority = queue.vendor_data_scheduler_priority();
                config.pti = queue.vendor_data_packet_priority();
                config.pti_count = 1;
                config.hardware_key_selector = key;
                // Trigger response policy was negotiated for the existing
                // BE/TID-0 owner. Other WMM queues remain ordinary HE-SU
                // aggregates until a per-TID HE-TB contract is reviewed.
                if traffic.tid() == HE_TRIGGER_DATA_TID
                    && queue == LegacyTxQueue::BestEffort
                    && let Some(trigger_based) = self.he_trigger_based
                {
                    config = config.with_trigger_based(trigger_based);
                }
                Ok(AmpduTxConfig::He(config))
            }
            TxPhyRate::Legacy(_) => Err(AggregateTxError::UnsupportedRate),
        }
    }

    fn publish_initial<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        if let ConnectedTxActive::Aggregate(active) = &self.active {
            active.retry.validate_stamp(self.radio_stamp()?)?;
        }
        let active = mem::replace(&mut self.active, ConnectedTxActive::Idle);
        let ConnectedTxActive::Aggregate(mut active) = active else {
            return Err(AggregateTxError::InvalidPublicationState);
        };
        if let Err(error) = self.publish_attempt(hardware, &mut active) {
            self.cancel_prepared();
            return Err(error);
        }
        self.last_aggregate_status = None;
        self.active = ConnectedTxActive::Aggregate(active);
        self.network_power.started = true;
        Ok(WifiTxProgress::Pending)
    }

    pub(super) fn publish_attempt<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        active: &mut AggregateActive<SLOTS>,
    ) -> Result<(), AggregateTxError> {
        let publication_started = self.ordinary.now();
        let deadline = publication_started
            .checked_add(self.config.completion_timeout)
            .ok_or(AggregateTxError::DeadlineOverflow)?;
        match active.config {
            AmpduTxConfig::Ht(config) => self.ampdu.active_mut().submit(
                hardware,
                self.cookie.ok_or(AggregateTxError::MissingCookie)?,
                active.traffic.queue(),
                config,
            )?,
            AmpduTxConfig::He(config) => self.ampdu.active_mut().submit_he(
                hardware,
                self.cookie.ok_or(AggregateTxError::MissingCookie)?,
                active.traffic.queue(),
                config,
            )?,
        }
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer {
            let publication_finished = self.ordinary.now();
            if active.first_publication_micros.is_none() {
                active.first_publication_micros = Some(publication_started.as_micros());
            }
            #[cfg(feature = "tx-wait-probe")]
            {
                active.wait_probe = wait_probe::Schedule::new(publication_started);
            }
            observer.observe(AggregateTxObservation::Published {
                at_micros: publication_started.as_micros(),
                program_micros: publication_finished
                    .saturating_duration_since(publication_started)
                    .as_micros(),
            });
        }
        active.deadline = deadline;
        Ok(())
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub(crate) fn mark_prepared_scheduler_phase(
        &mut self,
        phase: PreparedTxSchedulerPhase,
        at_micros: u64,
    ) {
        // Every boundary is observed, including those without a prepared
        // successor: the observer classifies publications that did not enter
        // through the prepared path instead of silently losing them.
        if let Some(observer) = self.observer {
            observer.observe(AggregateTxObservation::PreparedSchedulerPhase { phase, at_micros });
        }
    }
}
