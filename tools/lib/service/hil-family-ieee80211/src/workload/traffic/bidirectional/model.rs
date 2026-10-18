use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub(super) struct ThroughputSample {
    pub(super) datagrams: u64,
    pub(super) elapsed_us: u64,
    pub(super) throughput_kbps: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub(super) struct TxSample {
    pub(super) throughput_kbps: u64,
    pub(super) bandwidth_mhz: u16,
    pub(super) rate_kbps: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub(super) struct AmpduSample {
    pub(super) aggregates: u64,
    pub(super) publications: u64,
    pub(super) completed: u64,
    pub(super) subframes: u64,
    pub(super) acknowledged: u64,
    pub(super) single: u64,
    pub(super) individual_retry: u64,
    pub(super) timeout: u64,
    pub(super) collision: u64,
    pub(super) minimum: u8,
    pub(super) maximum: u8,
    pub(super) stop_frame: u64,
    pub(super) stop_capacity: u64,
    pub(super) stop_empty: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub(super) struct AmpduHistogramSample {
    pub(super) one: u64,
    pub(super) two_three: u64,
    pub(super) four_seven: u64,
    pub(super) eight_fifteen: u64,
    pub(super) sixteen_twentythree: u64,
    pub(super) twentyfour_thirty: u64,
    pub(super) thirtyone: u64,
    pub(super) full32: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub(super) struct AmpduTimingSample {
    pub(super) preparation_us: u64,
    pub(super) preparation_max_us: u64,
    pub(super) publication_us: u64,
    pub(super) publication_max_us: u64,
    pub(super) exchange_us: u64,
    pub(super) exchange_max_us: u64,
    pub(super) first_exchanges: u64,
    pub(super) first_exchange_us: u64,
    pub(super) first_exchange_max_us: u64,
    pub(super) retried_exchanges: u64,
    pub(super) retry_publications: u64,
    pub(super) retry_exchange_us: u64,
    pub(super) retry_exchange_max_us: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub(super) struct TxIrqTimingSample {
    pub(super) tx_irq_epochs: u64,
    pub(super) tx_irq_samples: u64,
    pub(super) tx_irq_skew: u64,
    pub(super) tx_irq_service_us: u64,
    pub(super) tx_irq_service_max_us: u64,
    pub(super) tx_flight_samples: u64,
    pub(super) tx_flight_us: u64,
    pub(super) tx_flight_max_us: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub(super) struct AmpduBlockAckSample {
    pub(super) samples: u64,
    pub(super) received: u64,
    pub(super) success_without: u64,
    pub(super) nonzero_control: u64,
    pub(super) start_outside: u64,
    pub(super) start_lag_max: u64,
    pub(super) full: u64,
    pub(super) partial: u64,
    pub(super) empty: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct AmpduEvidence {
    pub publications_pending_start: u64,
    pub publications_pending_end: u64,
    pub aggregates: u64,
    pub publications: u64,
    pub completed: u64,
    pub subframes: u64,
    pub acknowledged: u64,
    pub single: u64,
    pub individual_retry: u64,
    pub timeout: u64,
    pub collision: u64,
    pub minimum: u8,
    pub maximum: u8,
    pub stop_frame: u64,
    pub stop_capacity: u64,
    pub stop_empty: u64,
    pub one: u64,
    pub two_three: u64,
    pub four_seven: u64,
    pub eight_fifteen: u64,
    pub sixteen_twentythree: u64,
    pub twentyfour_thirty: u64,
    pub thirtyone: u64,
    pub full32: u64,
    pub preparation_us: u64,
    pub preparation_max_us: u64,
    pub publication_us: u64,
    pub publication_max_us: u64,
    pub exchange_us: u64,
    pub exchange_max_us: u64,
    pub first_exchanges: u64,
    pub first_exchange_us: u64,
    pub first_exchange_max_us: u64,
    pub retried_exchanges: u64,
    pub retry_publications: u64,
    pub retry_exchange_us: u64,
    pub retry_exchange_max_us: u64,
    pub tx_irq_epochs: u64,
    pub tx_irq_samples: u64,
    pub tx_irq_skew: u64,
    pub tx_irq_service_us: u64,
    pub tx_irq_service_max_us: u64,
    pub tx_flight_samples: u64,
    pub tx_flight_us: u64,
    pub tx_flight_max_us: u64,
    pub standby_prepared: u64,
    pub standby_published: u64,
    pub standby_cancelled: u64,
    pub block_ack_samples: u64,
    pub block_ack_received: u64,
    pub block_ack_success_without: u64,
    pub block_ack_nonzero_control: u64,
    pub block_ack_start_outside: u64,
    pub block_ack_start_lag_max: u64,
    pub full_block_ack: u64,
    pub partial_block_ack: u64,
    pub empty_block_ack: u64,
    pub block_ack_requests: u64,
    pub block_ack_requests_answered: u64,
}

impl AmpduEvidence {
    pub fn from_typed(typed: TxRadioEvidence, timing: TxAggregateTimingEvidence) -> Self {
        Self {
            aggregates: u64::from(typed.aggregates_prepared),
            publications: u64::from(typed.aggregate_publications),
            publications_pending_start: u64::from(typed.publications_pending_start),
            publications_pending_end: u64::from(typed.publications_pending_end),
            completed: u64::from(typed.aggregates_completed),
            subframes: u64::from(typed.subframes_prepared),
            acknowledged: u64::from(typed.subframes_acknowledged),
            individual_retry: u64::from(typed.individual_retries),
            timeout: u64::from(typed.hardware_timeouts),
            collision: u64::from(typed.collisions),
            minimum: typed.minimum_subframes,
            maximum: typed.maximum_subframes,
            one: u64::from(typed.prepared_histogram[0]),
            two_three: u64::from(typed.prepared_histogram[1]),
            four_seven: u64::from(typed.prepared_histogram[2]),
            eight_fifteen: u64::from(typed.prepared_histogram[3]),
            sixteen_twentythree: u64::from(typed.prepared_histogram[4]),
            twentyfour_thirty: u64::from(typed.prepared_histogram[5]),
            thirtyone: u64::from(typed.prepared_histogram[6]),
            full32: u64::from(typed.prepared_histogram[7]),
            stop_frame: u64::from(typed.stopped_at_frame_limit),
            stop_capacity: u64::from(typed.stopped_at_capacity_limit),
            stop_empty: u64::from(typed.stopped_on_empty_queue),
            preparation_us: u64::from(timing.preparation_micros),
            preparation_max_us: u64::from(timing.preparation_max_micros),
            publication_us: u64::from(timing.publication_micros),
            publication_max_us: u64::from(timing.publication_max_micros),
            exchange_us: u64::from(timing.exchange_micros),
            exchange_max_us: u64::from(timing.exchange_max_micros),
            first_exchanges: u64::from(timing.first_exchanges),
            first_exchange_us: u64::from(timing.first_exchange_micros),
            first_exchange_max_us: u64::from(timing.first_exchange_max_micros),
            retried_exchanges: u64::from(timing.retried_exchanges),
            retry_publications: u64::from(timing.retry_publications),
            retry_exchange_us: u64::from(timing.retry_exchange_micros),
            retry_exchange_max_us: u64::from(timing.retry_exchange_max_micros),
            block_ack_samples: u64::from(typed.block_ack_samples),
            block_ack_received: u64::from(typed.block_ack_received),
            block_ack_success_without: u64::from(typed.success_without_block_ack),
            block_ack_nonzero_control: u64::from(typed.nonzero_block_ack_control),
            full_block_ack: u64::from(typed.full_block_ack),
            partial_block_ack: u64::from(typed.partial_block_ack),
            empty_block_ack: u64::from(typed.empty_block_ack),
            block_ack_requests: u64::from(typed.block_ack_requests),
            block_ack_requests_answered: u64::from(typed.block_ack_requests_answered),
            tx_irq_epochs: u64::from(typed.tx_irq_epochs),
            tx_irq_samples: u64::from(typed.tx_irq_service_samples),
            tx_irq_skew: u64::from(typed.tx_irq_clock_skew_samples),
            tx_irq_service_us: u64::from(timing.tx_irq_service_micros),
            tx_irq_service_max_us: u64::from(timing.tx_irq_service_max_micros),
            tx_flight_samples: u64::from(typed.tx_publication_to_irq_samples),
            tx_flight_us: u64::from(timing.tx_publication_to_irq_micros),
            tx_flight_max_us: u64::from(timing.tx_publication_to_irq_max_micros),
            standby_prepared: u64::from(timing.standby_prepared),
            standby_published: u64::from(timing.standby_published),
            standby_cancelled: u64::from(timing.standby_cancelled),
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(super) fn from_report(report: &DeviceReport) -> Self {
        let mut evidence = Self::default();
        for sample in &report.ampdu {
            evidence.aggregates = evidence.aggregates.saturating_add(sample.aggregates);
            evidence.publications = evidence.publications.saturating_add(sample.publications);
            evidence.completed = evidence.completed.saturating_add(sample.completed);
            evidence.subframes = evidence.subframes.saturating_add(sample.subframes);
            evidence.acknowledged = evidence.acknowledged.saturating_add(sample.acknowledged);
            evidence.single = evidence.single.saturating_add(sample.single);
            evidence.individual_retry = evidence
                .individual_retry
                .saturating_add(sample.individual_retry);
            evidence.timeout = evidence.timeout.saturating_add(sample.timeout);
            evidence.collision = evidence.collision.saturating_add(sample.collision);
            if sample.minimum != 0 {
                evidence.minimum = if evidence.minimum == 0 {
                    sample.minimum
                } else {
                    evidence.minimum.min(sample.minimum)
                };
            }
            evidence.maximum = evidence.maximum.max(sample.maximum);
            evidence.stop_frame = evidence.stop_frame.saturating_add(sample.stop_frame);
            evidence.stop_capacity = evidence.stop_capacity.saturating_add(sample.stop_capacity);
            evidence.stop_empty = evidence.stop_empty.saturating_add(sample.stop_empty);
        }
        for sample in &report.ampdu_histograms {
            evidence.one = evidence.one.saturating_add(sample.one);
            evidence.two_three = evidence.two_three.saturating_add(sample.two_three);
            evidence.four_seven = evidence.four_seven.saturating_add(sample.four_seven);
            evidence.eight_fifteen = evidence.eight_fifteen.saturating_add(sample.eight_fifteen);
            evidence.sixteen_twentythree = evidence
                .sixteen_twentythree
                .saturating_add(sample.sixteen_twentythree);
            evidence.twentyfour_thirty = evidence
                .twentyfour_thirty
                .saturating_add(sample.twentyfour_thirty);
            evidence.thirtyone = evidence.thirtyone.saturating_add(sample.thirtyone);
            evidence.full32 = evidence.full32.saturating_add(sample.full32);
        }
        for sample in &report.ampdu_timings {
            evidence.preparation_us = evidence
                .preparation_us
                .saturating_add(sample.preparation_us);
            evidence.preparation_max_us =
                evidence.preparation_max_us.max(sample.preparation_max_us);
            evidence.publication_us = evidence
                .publication_us
                .saturating_add(sample.publication_us);
            evidence.publication_max_us =
                evidence.publication_max_us.max(sample.publication_max_us);
            evidence.exchange_us = evidence.exchange_us.saturating_add(sample.exchange_us);
            evidence.exchange_max_us = evidence.exchange_max_us.max(sample.exchange_max_us);
            evidence.first_exchanges = evidence
                .first_exchanges
                .saturating_add(sample.first_exchanges);
            evidence.first_exchange_us = evidence
                .first_exchange_us
                .saturating_add(sample.first_exchange_us);
            evidence.first_exchange_max_us = evidence
                .first_exchange_max_us
                .max(sample.first_exchange_max_us);
            evidence.retried_exchanges = evidence
                .retried_exchanges
                .saturating_add(sample.retried_exchanges);
            evidence.retry_publications = evidence
                .retry_publications
                .saturating_add(sample.retry_publications);
            evidence.retry_exchange_us = evidence
                .retry_exchange_us
                .saturating_add(sample.retry_exchange_us);
            evidence.retry_exchange_max_us = evidence
                .retry_exchange_max_us
                .max(sample.retry_exchange_max_us);
        }
        for sample in &report.tx_irq_timings {
            evidence.tx_irq_epochs = evidence.tx_irq_epochs.saturating_add(sample.tx_irq_epochs);
            evidence.tx_irq_samples = evidence
                .tx_irq_samples
                .saturating_add(sample.tx_irq_samples);
            evidence.tx_irq_skew = evidence.tx_irq_skew.saturating_add(sample.tx_irq_skew);
            evidence.tx_irq_service_us = evidence
                .tx_irq_service_us
                .saturating_add(sample.tx_irq_service_us);
            evidence.tx_irq_service_max_us = evidence
                .tx_irq_service_max_us
                .max(sample.tx_irq_service_max_us);
            evidence.tx_flight_samples = evidence
                .tx_flight_samples
                .saturating_add(sample.tx_flight_samples);
            evidence.tx_flight_us = evidence.tx_flight_us.saturating_add(sample.tx_flight_us);
            evidence.tx_flight_max_us = evidence.tx_flight_max_us.max(sample.tx_flight_max_us);
        }
        for sample in &report.ampdu_block_acks {
            evidence.block_ack_samples = evidence.block_ack_samples.saturating_add(sample.samples);
            evidence.block_ack_received =
                evidence.block_ack_received.saturating_add(sample.received);
            evidence.block_ack_success_without = evidence
                .block_ack_success_without
                .saturating_add(sample.success_without);
            evidence.block_ack_nonzero_control = evidence
                .block_ack_nonzero_control
                .saturating_add(sample.nonzero_control);
            evidence.block_ack_start_outside = evidence
                .block_ack_start_outside
                .saturating_add(sample.start_outside);
            evidence.block_ack_start_lag_max =
                evidence.block_ack_start_lag_max.max(sample.start_lag_max);
            evidence.full_block_ack = evidence.full_block_ack.saturating_add(sample.full);
            evidence.partial_block_ack = evidence.partial_block_ack.saturating_add(sample.partial);
            evidence.empty_block_ack = evidence.empty_block_ack.saturating_add(sample.empty);
        }
        evidence
    }

    #[cfg(test)]
    pub(super) fn histogram_total(self) -> u64 {
        self.one
            .saturating_add(self.two_three)
            .saturating_add(self.four_seven)
            .saturating_add(self.eight_fifteen)
            .saturating_add(self.sixteen_twentythree)
            .saturating_add(self.twentyfour_thirty)
            .saturating_add(self.thirtyone)
            .saturating_add(self.full32)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub struct TxQualification {
    pub throughput_floor_kbps: u64,
    pub sample_count: usize,
    pub ampdu: AmpduEvidence,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct RxQualification {
    pub throughput_median_kbps: u64,
    pub sample_count: usize,
    pub received_datagrams: u64,
    pub enqueued: u64,
    pub dropped: u64,
    pub he_mcs_histogram: [u64; 12],
    pub other_phy_frames: u64,
    pub ht40_long_gi_mcs: [u64; 8],
    pub ht40_short_gi_mcs: [u64; 8],
    pub ht20_mcs: [u64; 8],
    pub ht_other_frames: u64,
    pub s_mpdu: RxSmpduEvidence,
    pub ampdu: RxAmpduEvidence,
    pub pipeline: RxPipelineEvidence,
    pub irq: MacIrqEvidence,
    pub task_polls: TaskPollSet,
    pub sequence: UdpSequenceEvidence,
    pub order: RxOrderEvidence,
    pub reorder: RxReorderEvidence,
    pub buffer_full: u64,
    pub fifo_overflow: u64,
}

impl RxQualification {
    pub fn from_typed(transport: TransportEvidence, radio: RxRadioEvidence) -> Self {
        let throughput_median_kbps = transport
            .rx_bytes
            .saturating_mul(8)
            .saturating_mul(1_000)
            .checked_div(transport.elapsed_micros.max(1))
            .unwrap_or(0);
        Self {
            throughput_median_kbps,
            sample_count: 1,
            received_datagrams: transport.rx_units,
            enqueued: transport.rx_units,
            dropped: u64::from(radio.network_dropped),
            ht40_long_gi_mcs: {
                let mut histogram = [0; 8];
                if radio.ht40_below_mcs7_frames == 0 {
                    histogram[7] = u64::from(radio.ht40_long_gi_frames);
                }
                histogram
            },
            ht40_short_gi_mcs: {
                let mut histogram = [0; 8];
                if radio.ht40_below_mcs7_frames == 0 {
                    histogram[7] = u64::from(radio.ht40_short_gi_frames);
                }
                histogram
            },
            ht_other_frames: u64::from(radio.ht_invalid_frames),
            s_mpdu: RxSmpduEvidence {
                s_mpdu_datagrams: u64::from(radio.s_mpdu_datagrams),
                not_s_mpdu_datagrams: u64::from(radio.not_s_mpdu_datagrams),
                unavailable_datagrams: u64::from(radio.s_mpdu_unavailable_datagrams),
                s_mpdu_beacons: u64::from(radio.s_mpdu_beacons),
                not_s_mpdu_beacons: u64::from(radio.not_s_mpdu_beacons),
                unavailable_beacons: u64::from(radio.s_mpdu_unavailable_beacons),
            },
            ampdu: RxAmpduEvidence {
                ampdu_datagrams: u64::from(radio.ampdu_datagrams),
                not_ampdu_datagrams: u64::from(radio.not_ampdu_datagrams),
                hardware_ampdu_datagrams: u64::from(radio.hardware_ampdu_datagrams),
                hardware_not_ampdu_datagrams: u64::from(radio.hardware_not_ampdu_datagrams),
                protocol_ampdu_datagrams: u64::from(radio.protocol_ampdu_datagrams),
                protocol_not_ampdu_datagrams: u64::from(radio.protocol_not_ampdu_datagrams),
                unavailable_datagrams: u64::from(radio.ampdu_unavailable_datagrams),
            },
            sequence: UdpSequenceEvidence {
                intervals: 1,
                first: radio.sequence_first.map(u64::from),
                highest: radio.sequence_highest.map(u64::from),
                next: radio.sequence_highest.map(|value| u64::from(value) + 1),
                gap_events: u64::from(radio.sequence_gap_events),
                forward_missing: u64::from(radio.sequence_forward_missing),
                backward: u64::from(radio.sequence_backward),
                adjacent_duplicates: u64::from(radio.sequence_duplicates),
                unsequenced: u64::from(radio.sequence_unsequenced),
                ..UdpSequenceEvidence::default()
            },
            reorder: RxReorderEvidence {
                intervals: 1,
                last_start_tid: u64::from(radio.reorder_tid),
                window: u64::from(radio.reorder_window),
                first_samples: u64::from(radio.reorder_first_samples),
                first_tid: u64::from(radio.reorder_first_tid),
                first_start_sequence: u64::from(radio.reorder_first_start),
                first_frame_sequence: u64::from(radio.reorder_first_sequence),
                first_distance: u64::from(radio.reorder_first_distance),
                occupied: u64::from(radio.reorder_current_occupied),
                maximum_occupied: u64::from(radio.reorder_maximum_occupied),
                ..RxReorderEvidence::default()
            },
            buffer_full: u64::from(radio.dma_buffer_full),
            fifo_overflow: u64::from(radio.dma_fifo_overflow),
            ..Self::default()
        }
    }

    /// Overlay authoritative compact radio evidence onto optional text
    /// diagnostics. Text owns phase/cycle detail and need not duplicate the
    /// complete per-frame HT vector contract.
    pub fn with_typed_radio(mut self, typed: &Self) -> Self {
        self.ht40_long_gi_mcs = typed.ht40_long_gi_mcs;
        self.ht40_short_gi_mcs = typed.ht40_short_gi_mcs;
        self.ht20_mcs = typed.ht20_mcs;
        self.ht_other_frames = typed.ht_other_frames;
        self.s_mpdu = typed.s_mpdu;
        self.ampdu = typed.ampdu;
        self.buffer_full = typed.buffer_full;
        self.fifo_overflow = typed.fifo_overflow;
        self
    }
}

pub fn validate_ht40_rx_vector(
    radio: &RxRadioEvidence,
    minimum_mcs: Option<u8>,
    guard_interval: HtGuardIntervalExpectation,
) -> Result<()> {
    let long_total = u64::from(radio.ht40_long_gi_frames);
    let short_total = u64::from(radio.ht40_short_gi_frames);
    let ht40_total = long_total.saturating_add(short_total);
    let vector_total = ht40_total.saturating_add(u64::from(radio.ht_invalid_frames));
    let observed_udp = u64::from(radio.s_mpdu_datagrams)
        .saturating_add(u64::from(radio.not_s_mpdu_datagrams))
        .saturating_add(u64::from(radio.s_mpdu_unavailable_datagrams));
    if vector_total == 0 || vector_total != observed_udp {
        return Err(format!(
            "incomplete HT RX-vector interval: vectors={vector_total} benchmark_udp={observed_udp}"
        )
        .into());
    }
    if ht40_total == 0 {
        return Err("HT40 RX interval did not contain an HT40 benchmark vector".into());
    }
    if let Some(minimum_mcs) = minimum_mcs {
        if minimum_mcs != 7 {
            return Err(format!(
                "typed HT interval evidence currently supports only the MCS7 floor, requested MCS{minimum_mcs}"
            )
            .into());
        }
        if radio.ht40_below_mcs7_frames != 0 || radio.ht_invalid_frames != 0 {
            return Err(format!(
                "HT40 RX did not remain at MCS7: below={} non_ht40={}",
                radio.ht40_below_mcs7_frames, radio.ht_invalid_frames,
            )
            .into());
        }
    }
    match guard_interval {
        HtGuardIntervalExpectation::Any => {}
        HtGuardIntervalExpectation::Long if long_total != 0 && short_total == 0 => {}
        HtGuardIntervalExpectation::Short if short_total != 0 && long_total == 0 => {}
        expected => {
            return Err(format!(
                "HT40 RX guard interval mismatch: required={} long={long_total} short={short_total}",
                expected.id(),
            )
            .into());
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct RxSmpduEvidence {
    pub s_mpdu_datagrams: u64,
    pub not_s_mpdu_datagrams: u64,
    pub unavailable_datagrams: u64,
    pub s_mpdu_beacons: u64,
    pub not_s_mpdu_beacons: u64,
    pub unavailable_beacons: u64,
}

impl RxSmpduEvidence {
    pub(super) fn merge(&mut self, sample: Self) {
        self.s_mpdu_datagrams = self
            .s_mpdu_datagrams
            .saturating_add(sample.s_mpdu_datagrams);
        self.not_s_mpdu_datagrams = self
            .not_s_mpdu_datagrams
            .saturating_add(sample.not_s_mpdu_datagrams);
        self.unavailable_datagrams = self
            .unavailable_datagrams
            .saturating_add(sample.unavailable_datagrams);
        self.s_mpdu_beacons = self.s_mpdu_beacons.saturating_add(sample.s_mpdu_beacons);
        self.not_s_mpdu_beacons = self
            .not_s_mpdu_beacons
            .saturating_add(sample.not_s_mpdu_beacons);
        self.unavailable_beacons = self
            .unavailable_beacons
            .saturating_add(sample.unavailable_beacons);
    }

    pub(super) fn observed_datagrams(self) -> u64 {
        self.s_mpdu_datagrams
            .saturating_add(self.not_s_mpdu_datagrams)
    }

    pub(super) fn observed_beacons(self) -> u64 {
        self.s_mpdu_beacons.saturating_add(self.not_s_mpdu_beacons)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct RxAmpduEvidence {
    pub ampdu_datagrams: u64,
    pub not_ampdu_datagrams: u64,
    pub hardware_ampdu_datagrams: u64,
    pub hardware_not_ampdu_datagrams: u64,
    pub protocol_ampdu_datagrams: u64,
    pub protocol_not_ampdu_datagrams: u64,
    pub unavailable_datagrams: u64,
}

impl RxAmpduEvidence {
    pub(super) fn merge(&mut self, sample: Self) {
        self.ampdu_datagrams = self.ampdu_datagrams.saturating_add(sample.ampdu_datagrams);
        self.not_ampdu_datagrams = self
            .not_ampdu_datagrams
            .saturating_add(sample.not_ampdu_datagrams);
        self.hardware_ampdu_datagrams = self
            .hardware_ampdu_datagrams
            .saturating_add(sample.hardware_ampdu_datagrams);
        self.hardware_not_ampdu_datagrams = self
            .hardware_not_ampdu_datagrams
            .saturating_add(sample.hardware_not_ampdu_datagrams);
        self.protocol_ampdu_datagrams = self
            .protocol_ampdu_datagrams
            .saturating_add(sample.protocol_ampdu_datagrams);
        self.protocol_not_ampdu_datagrams = self
            .protocol_not_ampdu_datagrams
            .saturating_add(sample.protocol_not_ampdu_datagrams);
        self.unavailable_datagrams = self
            .unavailable_datagrams
            .saturating_add(sample.unavailable_datagrams);
    }

    pub(super) fn hardware_observed_datagrams(self) -> u64 {
        self.hardware_ampdu_datagrams
            .saturating_add(self.hardware_not_ampdu_datagrams)
    }
}

#[derive(serde::Serialize)]
pub struct RxAssessment {
    pub rx: RxQualification,
    pub failure: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct UdpSequenceEvidence {
    pub intervals: u64,
    pub first: Option<u64>,
    pub highest: Option<u64>,
    pub next: Option<u64>,
    pub gap_events: u64,
    pub forward_missing: u64,
    pub maximum_gap: u64,
    pub maximum_gap_at: Option<u64>,
    pub first_gap_at: Option<u64>,
    pub last_gap_at: Option<u64>,
    pub backward: u64,
    pub adjacent_duplicates: u64,
    pub unsequenced: u64,
    pub maximum_interarrival_us: u64,
    pub maximum_interarrival_at: Option<u64>,
}

impl UdpSequenceEvidence {
    pub(super) fn merge(&mut self, sample: Self) {
        self.intervals = self.intervals.saturating_add(sample.intervals);
        if self.first.is_none() {
            self.first = sample.first;
        }
        self.highest = self.highest.max(sample.highest);
        self.next = self.next.max(sample.next);
        self.gap_events = self.gap_events.saturating_add(sample.gap_events);
        self.forward_missing = self.forward_missing.saturating_add(sample.forward_missing);
        if sample.maximum_gap > self.maximum_gap {
            self.maximum_gap = sample.maximum_gap;
            self.maximum_gap_at = sample.maximum_gap_at;
        }
        if self.first_gap_at.is_none() {
            self.first_gap_at = sample.first_gap_at;
        }
        if sample.last_gap_at.is_some() {
            self.last_gap_at = sample.last_gap_at;
        }
        self.backward = self.backward.saturating_add(sample.backward);
        self.adjacent_duplicates = self
            .adjacent_duplicates
            .saturating_add(sample.adjacent_duplicates);
        self.unsequenced = self.unsequenced.saturating_add(sample.unsequenced);
        if sample.maximum_interarrival_us >= self.maximum_interarrival_us
            && sample.maximum_interarrival_at.is_some()
        {
            self.maximum_interarrival_us = sample.maximum_interarrival_us;
            self.maximum_interarrival_at = sample.maximum_interarrival_at;
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct RxOrderEvidence {
    pub intervals: u64,
    pub gap_events: u64,
    pub forward_missing: u64,
    pub backward: u64,
    pub adjacent_duplicates: u64,
    pub backward_mac_backward: u64,
    pub backward_mac_same: u64,
    pub backward_mac_forward: u64,
    pub backward_mac_other_tid: u64,
    pub backward_mac_unavailable: u64,
}

impl RxOrderEvidence {
    pub(super) fn merge(&mut self, sample: Self) {
        self.intervals = self.intervals.saturating_add(sample.intervals);
        self.gap_events = self.gap_events.saturating_add(sample.gap_events);
        self.forward_missing = self.forward_missing.saturating_add(sample.forward_missing);
        self.backward = self.backward.saturating_add(sample.backward);
        self.adjacent_duplicates = self
            .adjacent_duplicates
            .saturating_add(sample.adjacent_duplicates);
        self.backward_mac_backward = self
            .backward_mac_backward
            .saturating_add(sample.backward_mac_backward);
        self.backward_mac_same = self
            .backward_mac_same
            .saturating_add(sample.backward_mac_same);
        self.backward_mac_forward = self
            .backward_mac_forward
            .saturating_add(sample.backward_mac_forward);
        self.backward_mac_other_tid = self
            .backward_mac_other_tid
            .saturating_add(sample.backward_mac_other_tid);
        self.backward_mac_unavailable = self
            .backward_mac_unavailable
            .saturating_add(sample.backward_mac_unavailable);
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct RxReorderEvidence {
    pub intervals: u64,
    pub starts: u64,
    pub stops: u64,
    pub last_start_tid: u64,
    pub last_start_sequence: u64,
    pub window: u64,
    pub first_samples: u64,
    pub first_tid: u64,
    pub first_start_sequence: u64,
    pub first_frame_sequence: u64,
    pub first_distance: u64,
    pub buffered: u64,
    pub released: u64,
    pub missing: u64,
    pub stale: u64,
    pub gap_expiries: u64,
    pub occupied: u64,
    pub maximum_occupied: u64,
}

impl RxReorderEvidence {
    pub(super) fn merge(&mut self, sample: Self) {
        self.intervals = self.intervals.saturating_add(sample.intervals);
        self.starts = self.starts.saturating_add(sample.starts);
        self.stops = self.stops.saturating_add(sample.stops);
        if sample.window != 0 {
            self.last_start_tid = sample.last_start_tid;
            self.last_start_sequence = sample.last_start_sequence;
            self.window = sample.window;
        }
        if sample.first_samples != 0 {
            self.first_tid = sample.first_tid;
            self.first_start_sequence = sample.first_start_sequence;
            self.first_frame_sequence = sample.first_frame_sequence;
            self.first_distance = sample.first_distance;
        }
        self.first_samples = self.first_samples.saturating_add(sample.first_samples);
        self.buffered = self.buffered.saturating_add(sample.buffered);
        self.released = self.released.saturating_add(sample.released);
        self.missing = self.missing.saturating_add(sample.missing);
        self.stale = self.stale.saturating_add(sample.stale);
        self.gap_expiries = self.gap_expiries.saturating_add(sample.gap_expiries);
        self.occupied = sample.occupied;
        self.maximum_occupied = self.maximum_occupied.max(sample.maximum_occupied);
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct TaskPollEvidence {
    pub intervals: u64,
    pub polls: u64,
    pub poll_us: u64,
    pub poll_boot_max_us: u64,
    pub over_100us: u64,
    pub over_500us: u64,
    pub over_1000us: u64,
    pub over_5000us: u64,
}

impl TaskPollEvidence {
    pub(super) fn merge(&mut self, sample: Self) {
        self.intervals = self.intervals.saturating_add(sample.intervals);
        self.polls = self.polls.saturating_add(sample.polls);
        self.poll_us = self.poll_us.saturating_add(sample.poll_us);
        self.poll_boot_max_us = self.poll_boot_max_us.max(sample.poll_boot_max_us);
        self.over_100us = self.over_100us.saturating_add(sample.over_100us);
        self.over_500us = self.over_500us.saturating_add(sample.over_500us);
        self.over_1000us = self.over_1000us.saturating_add(sample.over_1000us);
        self.over_5000us = self.over_5000us.saturating_add(sample.over_5000us);
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct TaskPollSet {
    pub network: TaskPollEvidence,
    pub radio: TaskPollEvidence,
    pub udp_rx: TaskPollEvidence,
    pub udp_tx: TaskPollEvidence,
    pub tcp: TaskPollEvidence,
}

impl TaskPollSet {
    #[cfg(test)]
    pub(super) fn is_complete(self) -> bool {
        self.network.intervals != 0
            && self.radio.intervals != 0
            && self.udp_rx.intervals != 0
            && self.udp_tx.intervals != 0
            && self.tcp.intervals != 0
    }

    /// The poll residence of each task that reported, as typed
    /// measurements `task_poll.<task>.<counter>`, so run comparisons and
    /// performance reports see it beside throughput.
    pub fn measurements(self) -> Vec<oer_hil_run_bundle_format::run::Measurement> {
        use oer_hil_run_bundle_format::run::Measurement;
        use oer_hil_run_bundle_format::run::MeasurementUnit::*;
        [
            ("network", self.network),
            ("radio", self.radio),
            ("udp_rx", self.udp_rx),
            ("udp_tx", self.udp_tx),
            ("tcp", self.tcp),
        ]
        .into_iter()
        .filter(|(_, evidence)| evidence.intervals != 0)
        .flat_map(|(task, evidence)| {
            [
                ("polls", evidence.polls, Count),
                ("poll_us", evidence.poll_us, Microseconds),
                ("boot_max_us", evidence.poll_boot_max_us, Microseconds),
                ("over_100us", evidence.over_100us, Count),
                ("over_500us", evidence.over_500us, Count),
                ("over_1000us", evidence.over_1000us, Count),
                ("over_5000us", evidence.over_5000us, Count),
            ]
            .map(|(counter, value, unit)| {
                Measurement::observed(format!("task_poll.{task}.{counter}"), value, unit)
            })
        })
        .collect()
    }

    pub(super) fn merge_log_line(&mut self, line: &str) {
        if !(line.starts_with("ORTP ") || line.contains(" ORTP ")) {
            return;
        }
        let Some(sample) = (|| {
            Some(TaskPollEvidence {
                intervals: 1,
                polls: field(line, "polls")?,
                poll_us: field(line, "poll_us")?,
                poll_boot_max_us: field(line, "poll_boot_max_us")?,
                over_100us: field(line, "over_100us")?,
                over_500us: field(line, "over_500us")?,
                over_1000us: field(line, "over_1000us")?,
                over_5000us: field(line, "over_5000us")?,
            })
        })() else {
            return;
        };
        match text_field(line, "task") {
            Some("network") => self.network.merge(sample),
            Some("radio") => self.radio.merge(sample),
            Some("udp_rx") => self.udp_rx.merge(sample),
            Some("udp_tx") => self.udp_tx.merge(sample),
            Some("tcp") => self.tcp.merge(sample),
            Some(_) | None => {}
        }
    }
}

pub fn task_polls_from_log(log: &str) -> TaskPollSet {
    let mut task_polls = TaskPollSet::default();
    for line in log.lines() {
        task_polls.merge_log_line(line);
    }
    task_polls
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct MacIrqEvidence {
    pub spurious_entries: u64,
    pub rx_only_entries: u64,
    pub rx_mixed_entries: u64,
    pub tx_only_entries: u64,
    pub tx_mixed_entries: u64,
    pub other_only_entries: u64,
    pub extra_nonzero_snapshots: u64,
    pub saturated_entries: u64,
    pub auxiliary_entries: u64,
    pub unhandled_entries: u64,
}

impl MacIrqEvidence {
    pub(super) fn merge(&mut self, sample: Self) {
        self.spurious_entries = self
            .spurious_entries
            .saturating_add(sample.spurious_entries);
        self.rx_only_entries = self.rx_only_entries.saturating_add(sample.rx_only_entries);
        self.rx_mixed_entries = self
            .rx_mixed_entries
            .saturating_add(sample.rx_mixed_entries);
        self.tx_only_entries = self.tx_only_entries.saturating_add(sample.tx_only_entries);
        self.tx_mixed_entries = self
            .tx_mixed_entries
            .saturating_add(sample.tx_mixed_entries);
        self.other_only_entries = self
            .other_only_entries
            .saturating_add(sample.other_only_entries);
        self.extra_nonzero_snapshots = self
            .extra_nonzero_snapshots
            .saturating_add(sample.extra_nonzero_snapshots);
        self.saturated_entries = self
            .saturated_entries
            .saturating_add(sample.saturated_entries);
        self.auxiliary_entries = self
            .auxiliary_entries
            .saturating_add(sample.auxiliary_entries);
        self.unhandled_entries = self
            .unhandled_entries
            .saturating_add(sample.unhandled_entries);
    }

    pub fn classified_entries(self) -> u64 {
        self.spurious_entries
            .saturating_add(self.rx_only_entries)
            .saturating_add(self.rx_mixed_entries)
            .saturating_add(self.tx_only_entries)
            .saturating_add(self.tx_mixed_entries)
            .saturating_add(self.other_only_entries)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct RxPipelineEvidence {
    pub service_calls: u64,
    pub service_credit_samples: u64,
    pub frontier_zero_services: u64,
    pub frontier_one_services: u64,
    pub frontier_two_three_services: u64,
    pub frontier_four_seven_services: u64,
    pub frontier_eight_fifteen_services: u64,
    pub frontier_sixteen_thirty_one_services: u64,
    pub frontier_thirty_two_plus_services: u64,
    pub frontier_frames: u64,
    pub admitted_frames: u64,
    pub staged_bytes: u64,
    pub stage_empty_discards: u64,
    pub stage_too_long_discards: u64,
    pub backpressured_services: u64,
    pub bulk_capacity_blocked_services: u64,
    pub pool_credit_limited_services: u64,
    pub queue_credit_limited_services: u64,
    pub maximum_deferred_frames: u64,
    pub minimum_backpressured_pool_credits: u64,
    pub minimum_backpressured_queue_credits: u64,
    pub minimum_pool_credits: u64,
    pub minimum_queue_credits: u64,
    pub maximum_frontier: u64,
    pub maximum_admitted: u64,
    pub service_us: u64,
    pub service_max_us: u64,
    pub reload_transactions: u64,
    pub reload_us: u64,
    pub reload_max_us: u64,
    pub dma_buffer_full_increments: u64,
    pub dma_buffer_full_service_samples: u64,
    pub dma_buffer_full_between_services: u64,
    pub dma_buffer_full_during_services: u64,
    pub dma_buffer_full_between_service_samples: u64,
    pub dma_buffer_full_during_service_samples: u64,
    pub dma_buffer_full_last_service: u64,
    pub dma_buffer_full_last_phase: u64,
    pub dma_buffer_full_last_counter: u64,
    pub dma_buffer_full_last_frontier: u64,
    pub dma_buffer_full_last_admitted: u64,
    pub dma_buffer_full_last_pool_credits: u64,
    pub dma_buffer_full_last_queue_credits: u64,
    pub dma_buffer_full_last_service_us: u64,
    pub protocol_frames: u64,
    pub protocol_data_frames: u64,
    pub protocol_amsdu_mpdus: u64,
    pub protocol_amsdu_subframes: u64,
    pub protocol_units_le_1700: u64,
    pub protocol_units_1701_3400: u64,
    pub protocol_units_over_3400: u64,
    pub protocol_unit_max_bytes: u64,
    pub network_ready_waits: u64,
    pub network_ready_wait_us: u64,
    pub network_ready_wait_max_us: u64,
    pub dispatch_us: u64,
    pub dispatch_max_us: u64,
    pub network_publications: u64,
    pub network_published_bytes: u64,
    pub network_publish_us: u64,
    pub network_publish_max_us: u64,
    pub rx_irq_posts: u64,
    pub rx_irq_epochs: u64,
    pub mac_irq_entries: u64,
    pub rx_irq_coalesced_posts: u64,
    pub rx_irq_service_samples: u64,
    pub rx_irq_clock_skew_samples: u64,
    pub rx_irq_to_service_us: u64,
    pub rx_irq_to_service_max_us: u64,
}

impl RxPipelineEvidence {
    pub(super) fn merge(&mut self, sample: Self) {
        let had_credit_samples = self.service_credit_samples != 0;
        let sample_has_credit_samples = sample.service_credit_samples != 0;
        let had_backpressure = self.backpressured_services != 0;
        let sample_has_backpressure = sample.backpressured_services != 0;
        self.service_calls = self.service_calls.saturating_add(sample.service_calls);
        self.service_credit_samples = self
            .service_credit_samples
            .saturating_add(sample.service_credit_samples);
        self.frontier_zero_services = self
            .frontier_zero_services
            .saturating_add(sample.frontier_zero_services);
        self.frontier_one_services = self
            .frontier_one_services
            .saturating_add(sample.frontier_one_services);
        self.frontier_two_three_services = self
            .frontier_two_three_services
            .saturating_add(sample.frontier_two_three_services);
        self.frontier_four_seven_services = self
            .frontier_four_seven_services
            .saturating_add(sample.frontier_four_seven_services);
        self.frontier_eight_fifteen_services = self
            .frontier_eight_fifteen_services
            .saturating_add(sample.frontier_eight_fifteen_services);
        self.frontier_sixteen_thirty_one_services = self
            .frontier_sixteen_thirty_one_services
            .saturating_add(sample.frontier_sixteen_thirty_one_services);
        self.frontier_thirty_two_plus_services = self
            .frontier_thirty_two_plus_services
            .saturating_add(sample.frontier_thirty_two_plus_services);
        self.frontier_frames = self.frontier_frames.saturating_add(sample.frontier_frames);
        self.admitted_frames = self.admitted_frames.saturating_add(sample.admitted_frames);
        self.staged_bytes = self.staged_bytes.saturating_add(sample.staged_bytes);
        self.stage_empty_discards = self
            .stage_empty_discards
            .saturating_add(sample.stage_empty_discards);
        self.stage_too_long_discards = self
            .stage_too_long_discards
            .saturating_add(sample.stage_too_long_discards);
        self.backpressured_services = self
            .backpressured_services
            .saturating_add(sample.backpressured_services);
        self.bulk_capacity_blocked_services = self
            .bulk_capacity_blocked_services
            .saturating_add(sample.bulk_capacity_blocked_services);
        self.pool_credit_limited_services = self
            .pool_credit_limited_services
            .saturating_add(sample.pool_credit_limited_services);
        self.queue_credit_limited_services = self
            .queue_credit_limited_services
            .saturating_add(sample.queue_credit_limited_services);
        self.maximum_deferred_frames = self
            .maximum_deferred_frames
            .max(sample.maximum_deferred_frames);
        if sample_has_backpressure {
            if had_backpressure {
                self.minimum_backpressured_pool_credits = self
                    .minimum_backpressured_pool_credits
                    .min(sample.minimum_backpressured_pool_credits);
                self.minimum_backpressured_queue_credits = self
                    .minimum_backpressured_queue_credits
                    .min(sample.minimum_backpressured_queue_credits);
            } else {
                self.minimum_backpressured_pool_credits = sample.minimum_backpressured_pool_credits;
                self.minimum_backpressured_queue_credits =
                    sample.minimum_backpressured_queue_credits;
            }
        }
        if sample_has_credit_samples {
            if had_credit_samples {
                self.minimum_pool_credits =
                    self.minimum_pool_credits.min(sample.minimum_pool_credits);
                self.minimum_queue_credits =
                    self.minimum_queue_credits.min(sample.minimum_queue_credits);
            } else {
                self.minimum_pool_credits = sample.minimum_pool_credits;
                self.minimum_queue_credits = sample.minimum_queue_credits;
            }
        }
        self.maximum_frontier = self.maximum_frontier.max(sample.maximum_frontier);
        self.maximum_admitted = self.maximum_admitted.max(sample.maximum_admitted);
        self.service_us = self.service_us.saturating_add(sample.service_us);
        self.service_max_us = self.service_max_us.max(sample.service_max_us);
        self.reload_transactions = self
            .reload_transactions
            .saturating_add(sample.reload_transactions);
        self.reload_us = self.reload_us.saturating_add(sample.reload_us);
        self.reload_max_us = self.reload_max_us.max(sample.reload_max_us);
        let sample_has_buffer_full = sample.dma_buffer_full_increments != 0;
        self.dma_buffer_full_increments = self
            .dma_buffer_full_increments
            .saturating_add(sample.dma_buffer_full_increments);
        self.dma_buffer_full_service_samples = self
            .dma_buffer_full_service_samples
            .saturating_add(sample.dma_buffer_full_service_samples);
        self.dma_buffer_full_between_services = self
            .dma_buffer_full_between_services
            .saturating_add(sample.dma_buffer_full_between_services);
        self.dma_buffer_full_during_services = self
            .dma_buffer_full_during_services
            .saturating_add(sample.dma_buffer_full_during_services);
        self.dma_buffer_full_between_service_samples = self
            .dma_buffer_full_between_service_samples
            .saturating_add(sample.dma_buffer_full_between_service_samples);
        self.dma_buffer_full_during_service_samples = self
            .dma_buffer_full_during_service_samples
            .saturating_add(sample.dma_buffer_full_during_service_samples);
        if sample_has_buffer_full {
            self.dma_buffer_full_last_service = sample.dma_buffer_full_last_service;
            self.dma_buffer_full_last_phase = sample.dma_buffer_full_last_phase;
            self.dma_buffer_full_last_counter = sample.dma_buffer_full_last_counter;
            self.dma_buffer_full_last_frontier = sample.dma_buffer_full_last_frontier;
            self.dma_buffer_full_last_admitted = sample.dma_buffer_full_last_admitted;
            self.dma_buffer_full_last_pool_credits = sample.dma_buffer_full_last_pool_credits;
            self.dma_buffer_full_last_queue_credits = sample.dma_buffer_full_last_queue_credits;
            self.dma_buffer_full_last_service_us = sample.dma_buffer_full_last_service_us;
        }
        self.protocol_frames = self.protocol_frames.saturating_add(sample.protocol_frames);
        self.protocol_data_frames = self
            .protocol_data_frames
            .saturating_add(sample.protocol_data_frames);
        self.protocol_amsdu_mpdus = self
            .protocol_amsdu_mpdus
            .saturating_add(sample.protocol_amsdu_mpdus);
        self.protocol_amsdu_subframes = self
            .protocol_amsdu_subframes
            .saturating_add(sample.protocol_amsdu_subframes);
        self.protocol_units_le_1700 = self
            .protocol_units_le_1700
            .saturating_add(sample.protocol_units_le_1700);
        self.protocol_units_1701_3400 = self
            .protocol_units_1701_3400
            .saturating_add(sample.protocol_units_1701_3400);
        self.protocol_units_over_3400 = self
            .protocol_units_over_3400
            .saturating_add(sample.protocol_units_over_3400);
        self.protocol_unit_max_bytes = self
            .protocol_unit_max_bytes
            .max(sample.protocol_unit_max_bytes);
        self.network_ready_waits = self
            .network_ready_waits
            .saturating_add(sample.network_ready_waits);
        self.network_ready_wait_us = self
            .network_ready_wait_us
            .saturating_add(sample.network_ready_wait_us);
        self.network_ready_wait_max_us = self
            .network_ready_wait_max_us
            .max(sample.network_ready_wait_max_us);
        self.dispatch_us = self.dispatch_us.saturating_add(sample.dispatch_us);
        self.dispatch_max_us = self.dispatch_max_us.max(sample.dispatch_max_us);
        self.network_publications = self
            .network_publications
            .saturating_add(sample.network_publications);
        self.network_published_bytes = self
            .network_published_bytes
            .saturating_add(sample.network_published_bytes);
        self.network_publish_us = self
            .network_publish_us
            .saturating_add(sample.network_publish_us);
        self.network_publish_max_us = self
            .network_publish_max_us
            .max(sample.network_publish_max_us);
        self.rx_irq_posts = self.rx_irq_posts.saturating_add(sample.rx_irq_posts);
        self.rx_irq_epochs = self.rx_irq_epochs.saturating_add(sample.rx_irq_epochs);
        self.mac_irq_entries = self.mac_irq_entries.saturating_add(sample.mac_irq_entries);
        self.rx_irq_coalesced_posts = self
            .rx_irq_coalesced_posts
            .saturating_add(sample.rx_irq_coalesced_posts);
        self.rx_irq_service_samples = self
            .rx_irq_service_samples
            .saturating_add(sample.rx_irq_service_samples);
        self.rx_irq_clock_skew_samples = self
            .rx_irq_clock_skew_samples
            .saturating_add(sample.rx_irq_clock_skew_samples);
        self.rx_irq_to_service_us = self
            .rx_irq_to_service_us
            .saturating_add(sample.rx_irq_to_service_us);
        self.rx_irq_to_service_max_us = self
            .rx_irq_to_service_max_us
            .max(sample.rx_irq_to_service_max_us);
    }

    pub(super) fn frontier_histogram_total(self) -> u64 {
        self.frontier_zero_services
            .saturating_add(self.frontier_one_services)
            .saturating_add(self.frontier_two_three_services)
            .saturating_add(self.frontier_four_seven_services)
            .saturating_add(self.frontier_eight_fifteen_services)
            .saturating_add(self.frontier_sixteen_thirty_one_services)
            .saturating_add(self.frontier_thirty_two_plus_services)
    }
}

#[derive(Debug, Default, Eq, PartialEq, serde::Serialize)]
pub(super) struct DeviceReport {
    pub(super) rx: Vec<ThroughputSample>,
    pub(super) tx: Vec<TxSample>,
    pub(super) ampdu: Vec<AmpduSample>,
    pub(super) ampdu_histograms: Vec<AmpduHistogramSample>,
    pub(super) ampdu_timings: Vec<AmpduTimingSample>,
    pub(super) tx_irq_timings: Vec<TxIrqTimingSample>,
    pub(super) ampdu_block_acks: Vec<AmpduBlockAckSample>,
    pub(super) rx_mcs_histograms: Vec<([u64; 12], u64)>,
    pub(super) rx_ht40_long_gi_histograms: Vec<[u64; 8]>,
    pub(super) rx_ht40_short_gi_histograms: Vec<[u64; 8]>,
    pub(super) rx_ht20_histograms: Vec<([u64; 8], u64)>,
    pub(super) rx_s_mpdu: Vec<RxSmpduEvidence>,
    pub(super) rx_ampdu: Vec<RxAmpduEvidence>,
    pub(super) rx_service: Vec<RxPipelineEvidence>,
    pub(super) rx_dispatch: Vec<RxPipelineEvidence>,
    pub(super) rx_frontier: Vec<RxPipelineEvidence>,
    pub(super) mac_irq: Vec<MacIrqEvidence>,
    pub(super) task_polls: TaskPollSet,
    pub(super) rx_sequences: Vec<UdpSequenceEvidence>,
    pub(super) rx_order: Vec<RxOrderEvidence>,
    pub(super) rx_reorder: Vec<RxReorderEvidence>,
    pub(super) rx_formats: Vec<u8>,
    pub(super) dma_health: Vec<(u64, u64)>,
    pub(super) software_health: Vec<(u64, u64)>,
    pub(super) code_addresses: Vec<u64>,
    pub(super) failures: Vec<String>,
}
