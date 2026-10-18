//! Lock-free ESP32-S31 HIL observations of the staged connected RX pipeline.
//!
//! These counters never participate in ownership or scheduling. Production
//! users that do not attach them pay only one predictable `Option` branch at
//! each instrumented phase; HIL attaches one shared instance in internal SRAM
//! and prints interval deltas only after the traffic sample has ended.

use core::sync::atomic::{AtomicU32, Ordering};

use oer_esp32s31_ieee80211_runtime::diagnostics::rx_pipeline::{
    RxNetworkPublicationOutcome, RxPipelineObservation, RxPipelineObserver,
    RxReorderAgreementObservation, RxReorderAgreementObserver, RxServiceObservation,
    RxStageDiscard,
};
use oer_ieee80211_mac::sequence::SequenceNumber;

/// Diagnostic observations spanning DMA staging, protocol dispatch and the
/// final `embassy-net` publication copy.
pub struct RxPipelineCounters {
    now_micros: fn() -> u64,
    service_calls: AtomicU32,
    frontier_zero_services: AtomicU32,
    frontier_one_services: AtomicU32,
    frontier_two_three_services: AtomicU32,
    frontier_four_seven_services: AtomicU32,
    frontier_eight_fifteen_services: AtomicU32,
    frontier_sixteen_thirty_one_services: AtomicU32,
    frontier_thirty_two_plus_services: AtomicU32,
    completion_frontier_frames: AtomicU32,
    completed_units: AtomicU32,
    completed_descriptors: AtomicU32,
    admitted_frames: AtomicU32,
    staged_units: AtomicU32,
    staged_bytes: AtomicU32,
    discarded_units: AtomicU32,
    recycled_descriptors: AtomicU32,
    overload_recycled_descriptors: AtomicU32,
    stage_empty_discards: AtomicU32,
    stage_too_long_discards: AtomicU32,
    stage_chained_discards: AtomicU32,
    stage_overload_bulk_discards: AtomicU32,
    overload_discarded_units: AtomicU32,
    critical_reserve_admissions: AtomicU32,
    bulk_capacity_blocked_services: AtomicU32,
    critical_admission_blocked_services: AtomicU32,
    backpressured_services: AtomicU32,
    pool_credit_limited_services: AtomicU32,
    queue_credit_limited_services: AtomicU32,
    maximum_deferred_frames: AtomicU32,
    minimum_backpressured_pool_credits: AtomicU32,
    minimum_backpressured_queue_credits: AtomicU32,
    minimum_pool_credits: AtomicU32,
    minimum_queue_credits: AtomicU32,
    maximum_frontier: AtomicU32,
    maximum_admitted: AtomicU32,
    service_micros: AtomicU32,
    service_lifetime_max_micros: AtomicU32,
    reload_transactions: AtomicU32,
    reload_micros: AtomicU32,
    reload_lifetime_max_micros: AtomicU32,
    dma_buffer_full_last_observation: AtomicU32,
    dma_buffer_full_increments: AtomicU32,
    dma_buffer_full_service_samples: AtomicU32,
    dma_buffer_full_between_services: AtomicU32,
    dma_buffer_full_during_services: AtomicU32,
    dma_buffer_full_between_service_samples: AtomicU32,
    dma_buffer_full_during_service_samples: AtomicU32,
    dma_buffer_full_last_service: AtomicU32,
    dma_buffer_full_last_phase: AtomicU32,
    dma_buffer_full_last_context: AtomicU32,
    dma_buffer_full_last_service_micros: AtomicU32,
    protocol_frames: AtomicU32,
    protocol_data_frames: AtomicU32,
    protocol_amsdu_mpdus: AtomicU32,
    protocol_amsdu_subframes: AtomicU32,
    protocol_units_le_1700: AtomicU32,
    protocol_units_1701_3400: AtomicU32,
    protocol_units_over_3400: AtomicU32,
    protocol_unit_lifetime_max_bytes: AtomicU32,
    reorder_starts: AtomicU32,
    reorder_stops: AtomicU32,
    reorder_last_start: AtomicU32,
    reorder_first_samples: AtomicU32,
    reorder_last_first: AtomicU32,
    reorder_last_first_distance: AtomicU32,
    reorder_ingress: AtomicU32,
    reorder_ingress_retries: AtomicU32,
    reorder_direct: AtomicU32,
    reorder_buffered: AtomicU32,
    reorder_released: AtomicU32,
    reorder_missing: AtomicU32,
    reorder_stale: AtomicU32,
    reorder_gap_expiries: AtomicU32,
    reorder_discarded: AtomicU32,
    reorder_current_occupied: AtomicU32,
    reorder_maximum_occupied: AtomicU32,
    network_ready_waits: AtomicU32,
    network_ready_wait_micros: AtomicU32,
    network_ready_wait_lifetime_max_micros: AtomicU32,
    protocol_frame_transactions: AtomicU32,
    protocol_frame_micros: AtomicU32,
    protocol_frame_lifetime_max_micros: AtomicU32,
    reorder_preflights: AtomicU32,
    reorder_preflight_micros: AtomicU32,
    reorder_preflight_lifetime_max_micros: AtomicU32,
    protocol_preflights: AtomicU32,
    protocol_preflight_micros: AtomicU32,
    protocol_preflight_lifetime_max_micros: AtomicU32,
    dispatch_micros: AtomicU32,
    dispatch_lifetime_max_micros: AtomicU32,
    network_publications: AtomicU32,
    network_enqueued: AtomicU32,
    network_dropped: AtomicU32,
    network_pool_exhausted: AtomicU32,
    network_published_bytes: AtomicU32,
    network_publish_micros: AtomicU32,
    network_publish_lifetime_max_micros: AtomicU32,
    rx_irq_epochs: AtomicU32,
    rx_irq_service_samples: AtomicU32,
    rx_irq_clock_skew_samples: AtomicU32,
    rx_irq_to_service_micros: AtomicU32,
    rx_irq_to_service_lifetime_max_micros: AtomicU32,
    pending_rx_irq_micros: AtomicU32,
}

impl RxPipelineCounters {
    const IRQ_TIME_VALID: u32 = 1 << 31;
    const IRQ_TIME_MASK: u32 = Self::IRQ_TIME_VALID - 1;
    const IRQ_SAMPLE_MASK: u32 = 63;

    pub const fn new(now_micros: fn() -> u64) -> Self {
        Self {
            now_micros,
            service_calls: AtomicU32::new(0),
            frontier_zero_services: AtomicU32::new(0),
            frontier_one_services: AtomicU32::new(0),
            frontier_two_three_services: AtomicU32::new(0),
            frontier_four_seven_services: AtomicU32::new(0),
            frontier_eight_fifteen_services: AtomicU32::new(0),
            frontier_sixteen_thirty_one_services: AtomicU32::new(0),
            frontier_thirty_two_plus_services: AtomicU32::new(0),
            completion_frontier_frames: AtomicU32::new(0),
            completed_units: AtomicU32::new(0),
            completed_descriptors: AtomicU32::new(0),
            admitted_frames: AtomicU32::new(0),
            staged_units: AtomicU32::new(0),
            staged_bytes: AtomicU32::new(0),
            discarded_units: AtomicU32::new(0),
            recycled_descriptors: AtomicU32::new(0),
            overload_recycled_descriptors: AtomicU32::new(0),
            stage_empty_discards: AtomicU32::new(0),
            stage_too_long_discards: AtomicU32::new(0),
            stage_chained_discards: AtomicU32::new(0),
            stage_overload_bulk_discards: AtomicU32::new(0),
            overload_discarded_units: AtomicU32::new(0),
            critical_reserve_admissions: AtomicU32::new(0),
            bulk_capacity_blocked_services: AtomicU32::new(0),
            critical_admission_blocked_services: AtomicU32::new(0),
            backpressured_services: AtomicU32::new(0),
            pool_credit_limited_services: AtomicU32::new(0),
            queue_credit_limited_services: AtomicU32::new(0),
            maximum_deferred_frames: AtomicU32::new(0),
            minimum_backpressured_pool_credits: AtomicU32::new(u32::MAX),
            minimum_backpressured_queue_credits: AtomicU32::new(u32::MAX),
            minimum_pool_credits: AtomicU32::new(u32::MAX),
            minimum_queue_credits: AtomicU32::new(u32::MAX),
            maximum_frontier: AtomicU32::new(0),
            maximum_admitted: AtomicU32::new(0),
            service_micros: AtomicU32::new(0),
            service_lifetime_max_micros: AtomicU32::new(0),
            reload_transactions: AtomicU32::new(0),
            reload_micros: AtomicU32::new(0),
            reload_lifetime_max_micros: AtomicU32::new(0),
            dma_buffer_full_last_observation: AtomicU32::new(0),
            dma_buffer_full_increments: AtomicU32::new(0),
            dma_buffer_full_service_samples: AtomicU32::new(0),
            dma_buffer_full_between_services: AtomicU32::new(0),
            dma_buffer_full_during_services: AtomicU32::new(0),
            dma_buffer_full_between_service_samples: AtomicU32::new(0),
            dma_buffer_full_during_service_samples: AtomicU32::new(0),
            dma_buffer_full_last_service: AtomicU32::new(0),
            dma_buffer_full_last_phase: AtomicU32::new(0),
            dma_buffer_full_last_context: AtomicU32::new(0),
            dma_buffer_full_last_service_micros: AtomicU32::new(0),
            protocol_frames: AtomicU32::new(0),
            protocol_data_frames: AtomicU32::new(0),
            protocol_amsdu_mpdus: AtomicU32::new(0),
            protocol_amsdu_subframes: AtomicU32::new(0),
            protocol_units_le_1700: AtomicU32::new(0),
            protocol_units_1701_3400: AtomicU32::new(0),
            protocol_units_over_3400: AtomicU32::new(0),
            protocol_unit_lifetime_max_bytes: AtomicU32::new(0),
            reorder_starts: AtomicU32::new(0),
            reorder_stops: AtomicU32::new(0),
            reorder_last_start: AtomicU32::new(0),
            reorder_first_samples: AtomicU32::new(0),
            reorder_last_first: AtomicU32::new(0),
            reorder_last_first_distance: AtomicU32::new(0),
            reorder_ingress: AtomicU32::new(0),
            reorder_ingress_retries: AtomicU32::new(0),
            reorder_direct: AtomicU32::new(0),
            reorder_buffered: AtomicU32::new(0),
            reorder_released: AtomicU32::new(0),
            reorder_missing: AtomicU32::new(0),
            reorder_stale: AtomicU32::new(0),
            reorder_gap_expiries: AtomicU32::new(0),
            reorder_discarded: AtomicU32::new(0),
            reorder_current_occupied: AtomicU32::new(0),
            reorder_maximum_occupied: AtomicU32::new(0),
            network_ready_waits: AtomicU32::new(0),
            network_ready_wait_micros: AtomicU32::new(0),
            network_ready_wait_lifetime_max_micros: AtomicU32::new(0),
            protocol_frame_transactions: AtomicU32::new(0),
            protocol_frame_micros: AtomicU32::new(0),
            protocol_frame_lifetime_max_micros: AtomicU32::new(0),
            reorder_preflights: AtomicU32::new(0),
            reorder_preflight_micros: AtomicU32::new(0),
            reorder_preflight_lifetime_max_micros: AtomicU32::new(0),
            protocol_preflights: AtomicU32::new(0),
            protocol_preflight_micros: AtomicU32::new(0),
            protocol_preflight_lifetime_max_micros: AtomicU32::new(0),
            dispatch_micros: AtomicU32::new(0),
            dispatch_lifetime_max_micros: AtomicU32::new(0),
            network_publications: AtomicU32::new(0),
            network_enqueued: AtomicU32::new(0),
            network_dropped: AtomicU32::new(0),
            network_pool_exhausted: AtomicU32::new(0),
            network_published_bytes: AtomicU32::new(0),
            network_publish_micros: AtomicU32::new(0),
            network_publish_lifetime_max_micros: AtomicU32::new(0),
            rx_irq_epochs: AtomicU32::new(0),
            rx_irq_service_samples: AtomicU32::new(0),
            rx_irq_clock_skew_samples: AtomicU32::new(0),
            rx_irq_to_service_micros: AtomicU32::new(0),
            rx_irq_to_service_lifetime_max_micros: AtomicU32::new(0),
            pending_rx_irq_micros: AtomicU32::new(0),
        }
    }

    fn clock_micros(&self) -> u64 {
        (self.now_micros)()
    }

    /// Sample one newly published RX wake epoch at the ISR-to-executor handoff.
    ///
    /// The ISR adapter calls this only before publishing a wake when no older
    /// RX wake is pending. Timestamp reads and cross-core CAS operations in the
    /// hard ISR are not free, so only one in 64 wake epochs is timed; the epoch
    /// count itself remains exact. The 31-bit microsecond image wraps every 35
    /// minutes, and modular subtraction remains exact for the bounded
    /// ISR-to-service interval.
    #[inline]
    pub fn record_rx_irq_epoch(&self) {
        let epoch = self.rx_irq_epochs.fetch_add(1, Ordering::Relaxed);
        if epoch & Self::IRQ_SAMPLE_MASK != 0 {
            return;
        }
        let timestamp = Self::IRQ_TIME_VALID | (self.clock_micros() as u32 & Self::IRQ_TIME_MASK);
        let _ = self.pending_rx_irq_micros.compare_exchange(
            0,
            timestamp,
            Ordering::Release,
            Ordering::Relaxed,
        );
    }

    fn record_service_started(&self, started: u64) {
        let pending = if self.pending_rx_irq_micros.load(Ordering::Acquire) == 0 {
            0
        } else {
            self.pending_rx_irq_micros.swap(0, Ordering::AcqRel)
        };
        if pending != 0 {
            let started_modulo = started as u32 & Self::IRQ_TIME_MASK;
            let posted_modulo = pending & Self::IRQ_TIME_MASK;
            let elapsed = started_modulo.wrapping_sub(posted_modulo) & Self::IRQ_TIME_MASK;
            if elapsed <= Self::IRQ_TIME_MASK / 2 {
                self.rx_irq_service_samples.fetch_add(1, Ordering::Relaxed);
                Self::record_time(
                    &self.rx_irq_to_service_micros,
                    &self.rx_irq_to_service_lifetime_max_micros,
                    u64::from(elapsed),
                );
            } else {
                // The platform clock can differ by a few microseconds across
                // cores. A modular value in the upper half is therefore a
                // negative cross-core skew, not a multi-minute service delay.
                self.rx_irq_clock_skew_samples
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn snapshot(&self) -> RxPipelineCounterSnapshot {
        let buffer_full_observation = self
            .dma_buffer_full_last_observation
            .load(Ordering::Relaxed);
        let buffer_full_context = self.dma_buffer_full_last_context.load(Ordering::Relaxed);
        RxPipelineCounterSnapshot {
            service_calls: self.service_calls.load(Ordering::Relaxed),
            frontier_zero_services: self.frontier_zero_services.load(Ordering::Relaxed),
            frontier_one_services: self.frontier_one_services.load(Ordering::Relaxed),
            frontier_two_three_services: self.frontier_two_three_services.load(Ordering::Relaxed),
            frontier_four_seven_services: self.frontier_four_seven_services.load(Ordering::Relaxed),
            frontier_eight_fifteen_services: self
                .frontier_eight_fifteen_services
                .load(Ordering::Relaxed),
            frontier_sixteen_thirty_one_services: self
                .frontier_sixteen_thirty_one_services
                .load(Ordering::Relaxed),
            frontier_thirty_two_plus_services: self
                .frontier_thirty_two_plus_services
                .load(Ordering::Relaxed),
            completion_frontier_frames: self.completion_frontier_frames.load(Ordering::Relaxed),
            completed_units: self.completed_units.load(Ordering::Relaxed),
            completed_descriptors: self.completed_descriptors.load(Ordering::Relaxed),
            admitted_frames: self.admitted_frames.load(Ordering::Relaxed),
            staged_units: self.staged_units.load(Ordering::Relaxed),
            staged_bytes: self.staged_bytes.load(Ordering::Relaxed),
            discarded_units: self.discarded_units.load(Ordering::Relaxed),
            recycled_descriptors: self.recycled_descriptors.load(Ordering::Relaxed),
            overload_recycled_descriptors: self
                .overload_recycled_descriptors
                .load(Ordering::Relaxed),
            stage_empty_discards: self.stage_empty_discards.load(Ordering::Relaxed),
            stage_too_long_discards: self.stage_too_long_discards.load(Ordering::Relaxed),
            stage_chained_discards: self.stage_chained_discards.load(Ordering::Relaxed),
            stage_overload_bulk_discards: self.stage_overload_bulk_discards.load(Ordering::Relaxed),
            overload_discarded_units: self.overload_discarded_units.load(Ordering::Relaxed),
            critical_reserve_admissions: self.critical_reserve_admissions.load(Ordering::Relaxed),
            bulk_capacity_blocked_services: self
                .bulk_capacity_blocked_services
                .load(Ordering::Relaxed),
            critical_admission_blocked_services: self
                .critical_admission_blocked_services
                .load(Ordering::Relaxed),
            backpressured_services: self.backpressured_services.load(Ordering::Relaxed),
            pool_credit_limited_services: self.pool_credit_limited_services.load(Ordering::Relaxed),
            queue_credit_limited_services: self
                .queue_credit_limited_services
                .load(Ordering::Relaxed),
            maximum_deferred_frames: self.maximum_deferred_frames.load(Ordering::Relaxed),
            minimum_backpressured_pool_credits: self
                .minimum_backpressured_pool_credits
                .load(Ordering::Relaxed),
            minimum_backpressured_queue_credits: self
                .minimum_backpressured_queue_credits
                .load(Ordering::Relaxed),
            minimum_pool_credits: self.minimum_pool_credits.load(Ordering::Relaxed),
            minimum_queue_credits: self.minimum_queue_credits.load(Ordering::Relaxed),
            maximum_frontier: self.maximum_frontier.load(Ordering::Relaxed),
            maximum_admitted: self.maximum_admitted.load(Ordering::Relaxed),
            service_micros: self.service_micros.load(Ordering::Relaxed),
            service_lifetime_max_micros: self.service_lifetime_max_micros.load(Ordering::Relaxed),
            reload_transactions: self.reload_transactions.load(Ordering::Relaxed),
            reload_micros: self.reload_micros.load(Ordering::Relaxed),
            reload_lifetime_max_micros: self.reload_lifetime_max_micros.load(Ordering::Relaxed),
            dma_buffer_full_increments: self.dma_buffer_full_increments.load(Ordering::Relaxed),
            dma_buffer_full_service_samples: self
                .dma_buffer_full_service_samples
                .load(Ordering::Relaxed),
            dma_buffer_full_between_services: self
                .dma_buffer_full_between_services
                .load(Ordering::Relaxed),
            dma_buffer_full_during_services: self
                .dma_buffer_full_during_services
                .load(Ordering::Relaxed),
            dma_buffer_full_between_service_samples: self
                .dma_buffer_full_between_service_samples
                .load(Ordering::Relaxed),
            dma_buffer_full_during_service_samples: self
                .dma_buffer_full_during_service_samples
                .load(Ordering::Relaxed),
            dma_buffer_full_last_service: self.dma_buffer_full_last_service.load(Ordering::Relaxed),
            dma_buffer_full_last_phase: self.dma_buffer_full_last_phase.load(Ordering::Relaxed),
            dma_buffer_full_last_counter: buffer_full_observation & u32::from(u16::MAX),
            dma_buffer_full_last_frontier: buffer_full_context & 0xff,
            dma_buffer_full_last_admitted: buffer_full_context >> 8 & 0xff,
            dma_buffer_full_last_pool_credits: buffer_full_context >> 16 & 0xff,
            dma_buffer_full_last_queue_credits: buffer_full_context >> 24,
            dma_buffer_full_last_service_micros: self
                .dma_buffer_full_last_service_micros
                .load(Ordering::Relaxed),
            protocol_frames: self.protocol_frames.load(Ordering::Relaxed),
            protocol_data_frames: self.protocol_data_frames.load(Ordering::Relaxed),
            protocol_amsdu_mpdus: self.protocol_amsdu_mpdus.load(Ordering::Relaxed),
            protocol_amsdu_subframes: self.protocol_amsdu_subframes.load(Ordering::Relaxed),
            protocol_units_le_1700: self.protocol_units_le_1700.load(Ordering::Relaxed),
            protocol_units_1701_3400: self.protocol_units_1701_3400.load(Ordering::Relaxed),
            protocol_units_over_3400: self.protocol_units_over_3400.load(Ordering::Relaxed),
            protocol_unit_lifetime_max_bytes: self
                .protocol_unit_lifetime_max_bytes
                .load(Ordering::Relaxed),
            reorder_starts: self.reorder_starts.load(Ordering::Relaxed),
            reorder_stops: self.reorder_stops.load(Ordering::Relaxed),
            reorder_last_start: self.reorder_last_start.load(Ordering::Relaxed),
            reorder_first_samples: self.reorder_first_samples.load(Ordering::Relaxed),
            reorder_last_first: self.reorder_last_first.load(Ordering::Relaxed),
            reorder_last_first_distance: self.reorder_last_first_distance.load(Ordering::Relaxed),
            reorder_ingress: self.reorder_ingress.load(Ordering::Relaxed),
            reorder_ingress_retries: self.reorder_ingress_retries.load(Ordering::Relaxed),
            reorder_direct: self.reorder_direct.load(Ordering::Relaxed),
            reorder_buffered: self.reorder_buffered.load(Ordering::Relaxed),
            reorder_released: self.reorder_released.load(Ordering::Relaxed),
            reorder_missing: self.reorder_missing.load(Ordering::Relaxed),
            reorder_stale: self.reorder_stale.load(Ordering::Relaxed),
            reorder_gap_expiries: self.reorder_gap_expiries.load(Ordering::Relaxed),
            reorder_discarded: self.reorder_discarded.load(Ordering::Relaxed),
            reorder_current_occupied: self.reorder_current_occupied.load(Ordering::Relaxed),
            reorder_maximum_occupied: self.reorder_maximum_occupied.load(Ordering::Relaxed),
            network_ready_waits: self.network_ready_waits.load(Ordering::Relaxed),
            network_ready_wait_micros: self.network_ready_wait_micros.load(Ordering::Relaxed),
            network_ready_wait_lifetime_max_micros: self
                .network_ready_wait_lifetime_max_micros
                .load(Ordering::Relaxed),
            protocol_frame_transactions: self.protocol_frame_transactions.load(Ordering::Relaxed),
            protocol_frame_micros: self.protocol_frame_micros.load(Ordering::Relaxed),
            protocol_frame_lifetime_max_micros: self
                .protocol_frame_lifetime_max_micros
                .load(Ordering::Relaxed),
            reorder_preflights: self.reorder_preflights.load(Ordering::Relaxed),
            reorder_preflight_micros: self.reorder_preflight_micros.load(Ordering::Relaxed),
            reorder_preflight_lifetime_max_micros: self
                .reorder_preflight_lifetime_max_micros
                .load(Ordering::Relaxed),
            protocol_preflights: self.protocol_preflights.load(Ordering::Relaxed),
            protocol_preflight_micros: self.protocol_preflight_micros.load(Ordering::Relaxed),
            protocol_preflight_lifetime_max_micros: self
                .protocol_preflight_lifetime_max_micros
                .load(Ordering::Relaxed),
            dispatch_micros: self.dispatch_micros.load(Ordering::Relaxed),
            dispatch_lifetime_max_micros: self.dispatch_lifetime_max_micros.load(Ordering::Relaxed),
            network_publications: self.network_publications.load(Ordering::Relaxed),
            network_enqueued: self.network_enqueued.load(Ordering::Relaxed),
            network_dropped: self.network_dropped.load(Ordering::Relaxed),
            network_pool_exhausted: self.network_pool_exhausted.load(Ordering::Relaxed),
            network_published_bytes: self.network_published_bytes.load(Ordering::Relaxed),
            network_publish_micros: self.network_publish_micros.load(Ordering::Relaxed),
            network_publish_lifetime_max_micros: self
                .network_publish_lifetime_max_micros
                .load(Ordering::Relaxed),
            rx_irq_epochs: self.rx_irq_epochs.load(Ordering::Relaxed),
            rx_irq_service_samples: self.rx_irq_service_samples.load(Ordering::Relaxed),
            rx_irq_clock_skew_samples: self.rx_irq_clock_skew_samples.load(Ordering::Relaxed),
            rx_irq_to_service_micros: self.rx_irq_to_service_micros.load(Ordering::Relaxed),
            rx_irq_to_service_lifetime_max_micros: self
                .rx_irq_to_service_lifetime_max_micros
                .load(Ordering::Relaxed),
        }
    }

    pub(crate) fn record_service(&self, observation: RxServiceObservation) {
        let RxServiceObservation {
            frontier,
            pool_credits: _,
            queue_credits: _,
            completed_units,
            completed_descriptors,
            admitted,
            staged_units,
            staged_bytes,
            discarded_units,
            recycled_descriptors,
            overload_discarded,
            overload_recycled_descriptors,
            critical_reserve_admitted,
            stage_capacity_blocked,
            critical_admission_blocked,
            minimum_pool_credits,
            minimum_queue_credits,
            micros,
            hardware_buffer_full_before: _,
            hardware_buffer_full_after: _,
        } = observation;
        let service = self.service_calls.fetch_add(1, Ordering::Relaxed) + 1;
        match frontier {
            0 => &self.frontier_zero_services,
            1 => &self.frontier_one_services,
            2..=3 => &self.frontier_two_three_services,
            4..=7 => &self.frontier_four_seven_services,
            8..=15 => &self.frontier_eight_fifteen_services,
            16..=31 => &self.frontier_sixteen_thirty_one_services,
            _ => &self.frontier_thirty_two_plus_services,
        }
        .fetch_add(1, Ordering::Relaxed);
        Self::add_usize(&self.completion_frontier_frames, frontier);
        Self::add_usize(&self.completed_units, completed_units);
        Self::add_usize(&self.completed_descriptors, completed_descriptors);
        Self::add_usize(&self.admitted_frames, admitted);
        Self::add_usize(&self.staged_units, staged_units);
        Self::add_usize(&self.staged_bytes, staged_bytes);
        Self::add_usize(&self.discarded_units, discarded_units);
        Self::add_usize(&self.recycled_descriptors, recycled_descriptors);
        Self::add_usize(
            &self.overload_recycled_descriptors,
            overload_recycled_descriptors,
        );
        Self::add_usize(&self.overload_discarded_units, overload_discarded);
        Self::add_usize(&self.critical_reserve_admissions, critical_reserve_admitted);
        if stage_capacity_blocked {
            self.bulk_capacity_blocked_services
                .fetch_add(1, Ordering::Relaxed);
        }
        if critical_admission_blocked {
            self.critical_admission_blocked_services
                .fetch_add(1, Ordering::Relaxed);
        }
        self.maximum_frontier.fetch_max(
            u32::try_from(frontier).unwrap_or(u32::MAX),
            Ordering::Relaxed,
        );
        self.maximum_admitted.fetch_max(
            u32::try_from(admitted).unwrap_or(u32::MAX),
            Ordering::Relaxed,
        );
        if admitted < frontier {
            self.maximum_deferred_frames.fetch_max(
                u32::try_from(frontier - admitted).unwrap_or(u32::MAX),
                Ordering::Relaxed,
            );
        }
        // A bounded service budget leaves a later DMA frontier pending, but it
        // is not upper-layer backpressure: those units have not been examined
        // or discarded. Attribute backpressure only when this transaction
        // actually exhausted admission credits or could not preserve a
        // critical unit, and use the in-transaction low-water marks rather
        // than the entry credits that preceded staging.
        self.minimum_pool_credits.fetch_min(
            u32::try_from(minimum_pool_credits).unwrap_or(u32::MAX),
            Ordering::Relaxed,
        );
        self.minimum_queue_credits.fetch_min(
            u32::try_from(minimum_queue_credits).unwrap_or(u32::MAX),
            Ordering::Relaxed,
        );
        if stage_capacity_blocked || overload_discarded != 0 || critical_admission_blocked {
            self.backpressured_services.fetch_add(1, Ordering::Relaxed);
            self.minimum_backpressured_pool_credits.fetch_min(
                u32::try_from(minimum_pool_credits).unwrap_or(u32::MAX),
                Ordering::Relaxed,
            );
            self.minimum_backpressured_queue_credits.fetch_min(
                u32::try_from(minimum_queue_credits).unwrap_or(u32::MAX),
                Ordering::Relaxed,
            );
            if minimum_pool_credits <= minimum_queue_credits {
                self.pool_credit_limited_services
                    .fetch_add(1, Ordering::Relaxed);
            }
            if minimum_queue_credits <= minimum_pool_credits {
                self.queue_credit_limited_services
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        Self::record_time(
            &self.service_micros,
            &self.service_lifetime_max_micros,
            micros,
        );
        self.record_dma_buffer_full(service, observation);
    }

    fn record_dma_buffer_full(&self, service: u32, observation: RxServiceObservation) {
        const VALID: u32 = 1 << 16;
        const BETWEEN_SERVICES: u32 = 1;
        const DURING_SERVICE: u32 = 2;

        let (Some(before), Some(after)) = (
            observation.hardware_buffer_full_before,
            observation.hardware_buffer_full_after,
        ) else {
            return;
        };
        let encoded = VALID | u32::from(after);
        let previous = self
            .dma_buffer_full_last_observation
            .swap(encoded, Ordering::Relaxed);
        let between = if previous & VALID == 0 {
            0
        } else {
            before.wrapping_sub(previous as u16)
        };
        let during = after.wrapping_sub(before);
        let increments = between.wrapping_add(during);
        if increments == 0 {
            return;
        }

        self.dma_buffer_full_increments
            .fetch_add(u32::from(increments), Ordering::Relaxed);
        self.dma_buffer_full_service_samples
            .fetch_add(1, Ordering::Relaxed);
        if between != 0 {
            self.dma_buffer_full_between_services
                .fetch_add(u32::from(between), Ordering::Relaxed);
            self.dma_buffer_full_between_service_samples
                .fetch_add(1, Ordering::Relaxed);
        }
        if during != 0 {
            self.dma_buffer_full_during_services
                .fetch_add(u32::from(during), Ordering::Relaxed);
            self.dma_buffer_full_during_service_samples
                .fetch_add(1, Ordering::Relaxed);
        }
        self.dma_buffer_full_last_service
            .store(service, Ordering::Relaxed);
        self.dma_buffer_full_last_phase.store(
            (if between != 0 { BETWEEN_SERVICES } else { 0 })
                | (if during != 0 { DURING_SERVICE } else { 0 }),
            Ordering::Relaxed,
        );
        let byte = |value: usize| u32::try_from(value.min(0xff)).unwrap_or(0xff);
        self.dma_buffer_full_last_context.store(
            byte(observation.frontier)
                | byte(observation.admitted) << 8
                | byte(observation.pool_credits) << 16
                | byte(observation.queue_credits) << 24,
            Ordering::Relaxed,
        );
        self.dma_buffer_full_last_service_micros.store(
            u32::try_from(observation.micros).unwrap_or(u32::MAX),
            Ordering::Relaxed,
        );
    }

    pub(crate) fn record_network_ready_wait(&self, micros: u64) {
        self.network_ready_waits.fetch_add(1, Ordering::Relaxed);
        Self::record_time(
            &self.network_ready_wait_micros,
            &self.network_ready_wait_lifetime_max_micros,
            micros,
        );
    }

    fn record_protocol_frame(&self, micros: u64) {
        self.protocol_frame_transactions
            .fetch_add(1, Ordering::Relaxed);
        Self::record_time(
            &self.protocol_frame_micros,
            &self.protocol_frame_lifetime_max_micros,
            micros,
        );
    }

    fn record_reorder_preflight(&self, micros: u64) {
        self.reorder_preflights.fetch_add(1, Ordering::Relaxed);
        Self::record_time(
            &self.reorder_preflight_micros,
            &self.reorder_preflight_lifetime_max_micros,
            micros,
        );
    }

    fn record_protocol_preflight(&self, micros: u64) {
        self.protocol_preflights.fetch_add(1, Ordering::Relaxed);
        Self::record_time(
            &self.protocol_preflight_micros,
            &self.protocol_preflight_lifetime_max_micros,
            micros,
        );
    }

    fn record_stage_discard(&self, error: RxStageDiscard) {
        match error {
            RxStageDiscard::Empty => &self.stage_empty_discards,
            RxStageDiscard::TooLong => &self.stage_too_long_discards,
            RxStageDiscard::Chained => &self.stage_chained_discards,
            RxStageDiscard::OverloadBulk => &self.stage_overload_bulk_discards,
        }
        .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_dispatch(
        &self,
        data: bool,
        amsdu: bool,
        amsdu_subframes: u8,
        unit_bytes: usize,
        micros: u64,
    ) {
        self.protocol_frames.fetch_add(1, Ordering::Relaxed);
        if data {
            self.protocol_data_frames.fetch_add(1, Ordering::Relaxed);
        }
        if amsdu {
            self.protocol_amsdu_mpdus.fetch_add(1, Ordering::Relaxed);
            self.protocol_amsdu_subframes
                .fetch_add(u32::from(amsdu_subframes), Ordering::Relaxed);
        }
        match unit_bytes {
            0..=1700 => &self.protocol_units_le_1700,
            1701..=3400 => &self.protocol_units_1701_3400,
            _ => &self.protocol_units_over_3400,
        }
        .fetch_add(1, Ordering::Relaxed);
        self.protocol_unit_lifetime_max_bytes.fetch_max(
            u32::try_from(unit_bytes).unwrap_or(u32::MAX),
            Ordering::Relaxed,
        );
        Self::record_time(
            &self.dispatch_micros,
            &self.dispatch_lifetime_max_micros,
            micros,
        );
    }

    pub(crate) fn record_reorder_start(
        &self,
        tid: u8,
        starting_sequence: SequenceNumber,
        window: u16,
    ) {
        self.reorder_starts.fetch_add(1, Ordering::Relaxed);
        self.reorder_last_start.store(
            u32::from(starting_sequence.get()) | (u32::from(window) << 16) | (u32::from(tid) << 26),
            Ordering::Relaxed,
        );
    }

    pub(crate) fn record_reorder_stop(&self) {
        self.reorder_stops.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_reorder_first(
        &self,
        tid: u8,
        start: SequenceNumber,
        sequence: SequenceNumber,
    ) {
        self.reorder_first_samples.fetch_add(1, Ordering::Relaxed);
        self.reorder_last_first.store(
            u32::from(sequence.get()) | (u32::from(start.get()) << 12) | (u32::from(tid) << 24),
            Ordering::Relaxed,
        );
        self.reorder_last_first_distance.store(
            u32::from(start.forward_distance(sequence)),
            Ordering::Relaxed,
        );
    }

    pub(crate) fn record_reorder_ingress(&self, active: bool, retry: bool) {
        self.reorder_ingress.fetch_add(1, Ordering::Relaxed);
        if retry {
            self.reorder_ingress_retries.fetch_add(1, Ordering::Relaxed);
        }
        if !active {
            self.reorder_direct.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_reorder_release(
        &self,
        buffered: bool,
        released: u8,
        missing: u16,
        stale: bool,
    ) {
        if buffered {
            self.reorder_buffered.fetch_add(1, Ordering::Relaxed);
        }
        self.reorder_released
            .fetch_add(u32::from(released), Ordering::Relaxed);
        self.reorder_missing
            .fetch_add(u32::from(missing), Ordering::Relaxed);
        if stale {
            self.reorder_stale.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_reorder_gap_expiry(&self) {
        self.reorder_gap_expiries.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_reorder_discard(&self) {
        self.reorder_discarded.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_reorder_occupied(&self, occupied: u32) {
        self.reorder_current_occupied
            .store(occupied, Ordering::Relaxed);
        self.reorder_maximum_occupied
            .fetch_max(occupied, Ordering::Relaxed);
    }

    pub(crate) fn record_network_publish(
        &self,
        bytes: usize,
        micros: u64,
        outcome: RxNetworkPublicationOutcome,
    ) {
        self.network_publications.fetch_add(1, Ordering::Relaxed);
        match outcome {
            RxNetworkPublicationOutcome::Enqueued => {
                self.network_enqueued.fetch_add(1, Ordering::Relaxed);
            }
            RxNetworkPublicationOutcome::PoolExhausted => {
                self.network_dropped.fetch_add(1, Ordering::Relaxed);
                self.network_pool_exhausted.fetch_add(1, Ordering::Relaxed);
            }
            RxNetworkPublicationOutcome::Dropped => {
                self.network_dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
        Self::add_usize(&self.network_published_bytes, bytes);
        Self::record_time(
            &self.network_publish_micros,
            &self.network_publish_lifetime_max_micros,
            micros,
        );
    }

    fn add_usize(counter: &AtomicU32, value: usize) {
        counter.fetch_add(u32::try_from(value).unwrap_or(u32::MAX), Ordering::Relaxed);
    }

    fn record_time(total: &AtomicU32, maximum: &AtomicU32, micros: u64) {
        let micros = u32::try_from(micros).unwrap_or(u32::MAX);
        total.fetch_add(micros, Ordering::Relaxed);
        maximum.fetch_max(micros, Ordering::Relaxed);
    }
}

impl RxPipelineObserver for RxPipelineCounters {
    fn now_micros(&self) -> u64 {
        self.clock_micros()
    }

    fn observe(&self, observation: RxPipelineObservation) {
        match observation {
            RxPipelineObservation::ServiceStarted { at_micros } => {
                self.record_service_started(at_micros)
            }
            RxPipelineObservation::ServiceCompleted(observation) => {
                self.record_service(observation)
            }
            RxPipelineObservation::ReloadCompleted { micros } => {
                self.reload_transactions.fetch_add(1, Ordering::Relaxed);
                Self::record_time(
                    &self.reload_micros,
                    &self.reload_lifetime_max_micros,
                    micros,
                );
            }
            RxPipelineObservation::StageDiscarded(discard) => self.record_stage_discard(discard),
            RxPipelineObservation::NetworkReadyWait { micros } => {
                self.record_network_ready_wait(micros)
            }
            RxPipelineObservation::ProtocolFrameCompleted { micros } => {
                self.record_protocol_frame(micros)
            }
            RxPipelineObservation::ReorderPrepared { micros } => {
                self.record_reorder_preflight(micros)
            }
            RxPipelineObservation::ProtocolPreflightCompleted { micros } => {
                self.record_protocol_preflight(micros)
            }
            RxPipelineObservation::ProtocolDispatched {
                data,
                amsdu,
                amsdu_subframes,
                unit_bytes,
                micros,
            } => self.record_dispatch(data, amsdu, amsdu_subframes, unit_bytes, micros),
            RxPipelineObservation::ReorderStarted {
                tid,
                starting_sequence,
                window,
            } => self.record_reorder_start(tid, starting_sequence, window),
            RxPipelineObservation::ReorderStopped => self.record_reorder_stop(),
            RxPipelineObservation::ReorderFirst {
                tid,
                start,
                sequence,
            } => self.record_reorder_first(tid, start, sequence),
            RxPipelineObservation::ReorderIngress { active, retry } => {
                self.record_reorder_ingress(active, retry)
            }
            RxPipelineObservation::ReorderReleased {
                buffered,
                released,
                missing,
                stale,
            } => self.record_reorder_release(buffered, released, missing, stale),
            RxPipelineObservation::ReorderGapExpired => self.record_reorder_gap_expiry(),
            RxPipelineObservation::ReorderDiscarded => self.record_reorder_discard(),
            RxPipelineObservation::ReorderOccupied { occupied } => {
                self.record_reorder_occupied(occupied)
            }
            RxPipelineObservation::NetworkPublication {
                bytes,
                micros,
                outcome,
            } => self.record_network_publish(bytes, micros, outcome),
        }
    }
}

/// Minimal correctness observer for facts which must remain available in the
/// ordinary correctness image.
///
/// It receives only agreement lifecycle edges, so attaching it introduces no
/// platform timer reads or per-frame pipeline callbacks. Hardware health and
/// network admission remain independently sampled by the product snapshot;
/// full reorder distributions belong to a dedicated diagnostic image.
pub struct RxCorrectnessObserver<'a> {
    counters: &'a RxPipelineCounters,
}

impl<'a> RxCorrectnessObserver<'a> {
    pub const fn new(counters: &'a RxPipelineCounters) -> Self {
        Self { counters }
    }
}

impl RxReorderAgreementObserver for RxCorrectnessObserver<'_> {
    fn observe(&self, observation: RxReorderAgreementObservation) {
        match observation {
            RxReorderAgreementObservation::Started {
                tid,
                starting_sequence,
                window,
            } => self
                .counters
                .record_reorder_start(tid, starting_sequence, window),
            RxReorderAgreementObservation::Stopped => self.counters.record_reorder_stop(),
            RxReorderAgreementObservation::First {
                tid,
                start,
                sequence,
            } => self.counters.record_reorder_first(tid, start, sequence),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxPipelineCounterSnapshot {
    pub service_calls: u32,
    pub frontier_zero_services: u32,
    pub frontier_one_services: u32,
    pub frontier_two_three_services: u32,
    pub frontier_four_seven_services: u32,
    pub frontier_eight_fifteen_services: u32,
    pub frontier_sixteen_thirty_one_services: u32,
    pub frontier_thirty_two_plus_services: u32,
    pub completion_frontier_frames: u32,
    pub completed_units: u32,
    pub completed_descriptors: u32,
    pub admitted_frames: u32,
    pub staged_units: u32,
    pub staged_bytes: u32,
    pub discarded_units: u32,
    pub recycled_descriptors: u32,
    pub overload_recycled_descriptors: u32,
    pub stage_empty_discards: u32,
    pub stage_too_long_discards: u32,
    pub stage_chained_discards: u32,
    /// Bulk data units deliberately discarded and exactly recycled under overload.
    pub stage_overload_bulk_discards: u32,
    pub overload_discarded_units: u32,
    pub critical_reserve_admissions: u32,
    pub bulk_capacity_blocked_services: u32,
    pub critical_admission_blocked_services: u32,
    pub backpressured_services: u32,
    pub pool_credit_limited_services: u32,
    pub queue_credit_limited_services: u32,
    /// Largest completed frontier suffix deferred by one service since boot.
    pub maximum_deferred_frames: u32,
    /// Smallest staging-pool credit observation at a backpressured service.
    pub minimum_backpressured_pool_credits: u32,
    /// Smallest staging-queue credit observation at a backpressured service.
    pub minimum_backpressured_queue_credits: u32,
    /// Smallest staging-pool credit observation since boot.
    pub minimum_pool_credits: u32,
    /// Smallest staging-queue credit observation since boot.
    pub minimum_queue_credits: u32,
    /// Maximum observed since boot, not an interval delta.
    pub maximum_frontier: u32,
    /// Maximum observed since boot, not an interval delta.
    pub maximum_admitted: u32,
    pub service_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub service_lifetime_max_micros: u32,
    pub reload_transactions: u32,
    pub reload_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub reload_lifetime_max_micros: u32,
    /// Hardware `BUFFER_FULL` increments observed at RX service boundaries.
    pub dma_buffer_full_increments: u32,
    /// Service boundaries at which one or more new increments were observed.
    pub dma_buffer_full_service_samples: u32,
    /// Increments after the preceding service exited and before this one began.
    pub dma_buffer_full_between_services: u32,
    /// Increments between this service's entry and exit samples.
    pub dma_buffer_full_during_services: u32,
    pub dma_buffer_full_between_service_samples: u32,
    pub dma_buffer_full_during_service_samples: u32,
    /// Boot-lifetime service ordinal carrying the most recent observation.
    pub dma_buffer_full_last_service: u32,
    /// Bit 0: between services; bit 1: during the recorded service.
    pub dma_buffer_full_last_phase: u32,
    pub dma_buffer_full_last_counter: u32,
    pub dma_buffer_full_last_frontier: u32,
    pub dma_buffer_full_last_admitted: u32,
    pub dma_buffer_full_last_pool_credits: u32,
    pub dma_buffer_full_last_queue_credits: u32,
    pub dma_buffer_full_last_service_micros: u32,
    pub protocol_frames: u32,
    pub protocol_data_frames: u32,
    pub protocol_amsdu_mpdus: u32,
    pub protocol_amsdu_subframes: u32,
    pub protocol_units_le_1700: u32,
    pub protocol_units_1701_3400: u32,
    pub protocol_units_over_3400: u32,
    /// Maximum raw staged unit size observed since boot, not an interval delta.
    pub protocol_unit_lifetime_max_bytes: u32,
    pub reorder_starts: u32,
    pub reorder_stops: u32,
    /// Last packed `tid[28:26] | window[21:16] | starting_sequence[11:0]`.
    pub reorder_last_start: u32,
    pub reorder_first_samples: u32,
    /// Last packed `tid[27:24] | start[23:12] | first_sequence[11:0]`.
    pub reorder_last_first: u32,
    pub reorder_last_first_distance: u32,
    pub reorder_ingress: u32,
    pub reorder_ingress_retries: u32,
    pub reorder_direct: u32,
    pub reorder_buffered: u32,
    pub reorder_released: u32,
    pub reorder_missing: u32,
    pub reorder_stale: u32,
    pub reorder_gap_expiries: u32,
    pub reorder_discarded: u32,
    pub reorder_current_occupied: u32,
    pub reorder_maximum_occupied: u32,
    pub network_ready_waits: u32,
    pub network_ready_wait_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub network_ready_wait_lifetime_max_micros: u32,
    pub protocol_frame_transactions: u32,
    pub protocol_frame_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub protocol_frame_lifetime_max_micros: u32,
    pub reorder_preflights: u32,
    pub reorder_preflight_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub reorder_preflight_lifetime_max_micros: u32,
    pub protocol_preflights: u32,
    pub protocol_preflight_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub protocol_preflight_lifetime_max_micros: u32,
    pub dispatch_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub dispatch_lifetime_max_micros: u32,
    pub network_publications: u32,
    pub network_enqueued: u32,
    pub network_dropped: u32,
    /// Subset of drops caused by shared network packet pool exhaustion.
    pub network_pool_exhausted: u32,
    pub network_published_bytes: u32,
    pub network_publish_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub network_publish_lifetime_max_micros: u32,
    pub rx_irq_epochs: u32,
    pub rx_irq_service_samples: u32,
    pub rx_irq_clock_skew_samples: u32,
    pub rx_irq_to_service_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub rx_irq_to_service_lifetime_max_micros: u32,
}

impl RxPipelineCounterSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            service_calls: self.service_calls.wrapping_sub(earlier.service_calls),
            frontier_zero_services: self
                .frontier_zero_services
                .wrapping_sub(earlier.frontier_zero_services),
            frontier_one_services: self
                .frontier_one_services
                .wrapping_sub(earlier.frontier_one_services),
            frontier_two_three_services: self
                .frontier_two_three_services
                .wrapping_sub(earlier.frontier_two_three_services),
            frontier_four_seven_services: self
                .frontier_four_seven_services
                .wrapping_sub(earlier.frontier_four_seven_services),
            frontier_eight_fifteen_services: self
                .frontier_eight_fifteen_services
                .wrapping_sub(earlier.frontier_eight_fifteen_services),
            frontier_sixteen_thirty_one_services: self
                .frontier_sixteen_thirty_one_services
                .wrapping_sub(earlier.frontier_sixteen_thirty_one_services),
            frontier_thirty_two_plus_services: self
                .frontier_thirty_two_plus_services
                .wrapping_sub(earlier.frontier_thirty_two_plus_services),
            completion_frontier_frames: self
                .completion_frontier_frames
                .wrapping_sub(earlier.completion_frontier_frames),
            completed_units: self.completed_units.wrapping_sub(earlier.completed_units),
            completed_descriptors: self
                .completed_descriptors
                .wrapping_sub(earlier.completed_descriptors),
            admitted_frames: self.admitted_frames.wrapping_sub(earlier.admitted_frames),
            staged_units: self.staged_units.wrapping_sub(earlier.staged_units),
            staged_bytes: self.staged_bytes.wrapping_sub(earlier.staged_bytes),
            discarded_units: self.discarded_units.wrapping_sub(earlier.discarded_units),
            recycled_descriptors: self
                .recycled_descriptors
                .wrapping_sub(earlier.recycled_descriptors),
            overload_recycled_descriptors: self
                .overload_recycled_descriptors
                .wrapping_sub(earlier.overload_recycled_descriptors),
            stage_empty_discards: self
                .stage_empty_discards
                .wrapping_sub(earlier.stage_empty_discards),
            stage_too_long_discards: self
                .stage_too_long_discards
                .wrapping_sub(earlier.stage_too_long_discards),
            stage_chained_discards: self
                .stage_chained_discards
                .wrapping_sub(earlier.stage_chained_discards),
            stage_overload_bulk_discards: self
                .stage_overload_bulk_discards
                .wrapping_sub(earlier.stage_overload_bulk_discards),
            overload_discarded_units: self
                .overload_discarded_units
                .wrapping_sub(earlier.overload_discarded_units),
            critical_reserve_admissions: self
                .critical_reserve_admissions
                .wrapping_sub(earlier.critical_reserve_admissions),
            bulk_capacity_blocked_services: self
                .bulk_capacity_blocked_services
                .wrapping_sub(earlier.bulk_capacity_blocked_services),
            critical_admission_blocked_services: self
                .critical_admission_blocked_services
                .wrapping_sub(earlier.critical_admission_blocked_services),
            backpressured_services: self
                .backpressured_services
                .wrapping_sub(earlier.backpressured_services),
            pool_credit_limited_services: self
                .pool_credit_limited_services
                .wrapping_sub(earlier.pool_credit_limited_services),
            queue_credit_limited_services: self
                .queue_credit_limited_services
                .wrapping_sub(earlier.queue_credit_limited_services),
            maximum_deferred_frames: self.maximum_deferred_frames,
            minimum_backpressured_pool_credits: self.minimum_backpressured_pool_credits,
            minimum_backpressured_queue_credits: self.minimum_backpressured_queue_credits,
            minimum_pool_credits: self.minimum_pool_credits,
            minimum_queue_credits: self.minimum_queue_credits,
            maximum_frontier: self.maximum_frontier,
            maximum_admitted: self.maximum_admitted,
            service_micros: self.service_micros.wrapping_sub(earlier.service_micros),
            service_lifetime_max_micros: self.service_lifetime_max_micros,
            reload_transactions: self
                .reload_transactions
                .wrapping_sub(earlier.reload_transactions),
            reload_micros: self.reload_micros.wrapping_sub(earlier.reload_micros),
            reload_lifetime_max_micros: self.reload_lifetime_max_micros,
            dma_buffer_full_increments: self
                .dma_buffer_full_increments
                .wrapping_sub(earlier.dma_buffer_full_increments),
            dma_buffer_full_service_samples: self
                .dma_buffer_full_service_samples
                .wrapping_sub(earlier.dma_buffer_full_service_samples),
            dma_buffer_full_between_services: self
                .dma_buffer_full_between_services
                .wrapping_sub(earlier.dma_buffer_full_between_services),
            dma_buffer_full_during_services: self
                .dma_buffer_full_during_services
                .wrapping_sub(earlier.dma_buffer_full_during_services),
            dma_buffer_full_between_service_samples: self
                .dma_buffer_full_between_service_samples
                .wrapping_sub(earlier.dma_buffer_full_between_service_samples),
            dma_buffer_full_during_service_samples: self
                .dma_buffer_full_during_service_samples
                .wrapping_sub(earlier.dma_buffer_full_during_service_samples),
            dma_buffer_full_last_service: self.dma_buffer_full_last_service,
            dma_buffer_full_last_phase: self.dma_buffer_full_last_phase,
            dma_buffer_full_last_counter: self.dma_buffer_full_last_counter,
            dma_buffer_full_last_frontier: self.dma_buffer_full_last_frontier,
            dma_buffer_full_last_admitted: self.dma_buffer_full_last_admitted,
            dma_buffer_full_last_pool_credits: self.dma_buffer_full_last_pool_credits,
            dma_buffer_full_last_queue_credits: self.dma_buffer_full_last_queue_credits,
            dma_buffer_full_last_service_micros: self.dma_buffer_full_last_service_micros,
            protocol_frames: self.protocol_frames.wrapping_sub(earlier.protocol_frames),
            protocol_data_frames: self
                .protocol_data_frames
                .wrapping_sub(earlier.protocol_data_frames),
            protocol_amsdu_mpdus: self
                .protocol_amsdu_mpdus
                .wrapping_sub(earlier.protocol_amsdu_mpdus),
            protocol_amsdu_subframes: self
                .protocol_amsdu_subframes
                .wrapping_sub(earlier.protocol_amsdu_subframes),
            protocol_units_le_1700: self
                .protocol_units_le_1700
                .wrapping_sub(earlier.protocol_units_le_1700),
            protocol_units_1701_3400: self
                .protocol_units_1701_3400
                .wrapping_sub(earlier.protocol_units_1701_3400),
            protocol_units_over_3400: self
                .protocol_units_over_3400
                .wrapping_sub(earlier.protocol_units_over_3400),
            protocol_unit_lifetime_max_bytes: self.protocol_unit_lifetime_max_bytes,
            reorder_starts: self.reorder_starts.wrapping_sub(earlier.reorder_starts),
            reorder_stops: self.reorder_stops.wrapping_sub(earlier.reorder_stops),
            reorder_last_start: self.reorder_last_start,
            reorder_first_samples: self
                .reorder_first_samples
                .wrapping_sub(earlier.reorder_first_samples),
            reorder_last_first: self.reorder_last_first,
            reorder_last_first_distance: self.reorder_last_first_distance,
            reorder_ingress: self.reorder_ingress.wrapping_sub(earlier.reorder_ingress),
            reorder_ingress_retries: self
                .reorder_ingress_retries
                .wrapping_sub(earlier.reorder_ingress_retries),
            reorder_direct: self.reorder_direct.wrapping_sub(earlier.reorder_direct),
            reorder_buffered: self.reorder_buffered.wrapping_sub(earlier.reorder_buffered),
            reorder_released: self.reorder_released.wrapping_sub(earlier.reorder_released),
            reorder_missing: self.reorder_missing.wrapping_sub(earlier.reorder_missing),
            reorder_stale: self.reorder_stale.wrapping_sub(earlier.reorder_stale),
            reorder_gap_expiries: self
                .reorder_gap_expiries
                .wrapping_sub(earlier.reorder_gap_expiries),
            reorder_discarded: self
                .reorder_discarded
                .wrapping_sub(earlier.reorder_discarded),
            reorder_current_occupied: self.reorder_current_occupied,
            reorder_maximum_occupied: self.reorder_maximum_occupied,
            network_ready_waits: self
                .network_ready_waits
                .wrapping_sub(earlier.network_ready_waits),
            network_ready_wait_micros: self
                .network_ready_wait_micros
                .wrapping_sub(earlier.network_ready_wait_micros),
            network_ready_wait_lifetime_max_micros: self.network_ready_wait_lifetime_max_micros,
            protocol_frame_transactions: self
                .protocol_frame_transactions
                .wrapping_sub(earlier.protocol_frame_transactions),
            protocol_frame_micros: self
                .protocol_frame_micros
                .wrapping_sub(earlier.protocol_frame_micros),
            protocol_frame_lifetime_max_micros: self.protocol_frame_lifetime_max_micros,
            reorder_preflights: self
                .reorder_preflights
                .wrapping_sub(earlier.reorder_preflights),
            reorder_preflight_micros: self
                .reorder_preflight_micros
                .wrapping_sub(earlier.reorder_preflight_micros),
            reorder_preflight_lifetime_max_micros: self.reorder_preflight_lifetime_max_micros,
            protocol_preflights: self
                .protocol_preflights
                .wrapping_sub(earlier.protocol_preflights),
            protocol_preflight_micros: self
                .protocol_preflight_micros
                .wrapping_sub(earlier.protocol_preflight_micros),
            protocol_preflight_lifetime_max_micros: self.protocol_preflight_lifetime_max_micros,
            dispatch_micros: self.dispatch_micros.wrapping_sub(earlier.dispatch_micros),
            dispatch_lifetime_max_micros: self.dispatch_lifetime_max_micros,
            network_publications: self
                .network_publications
                .wrapping_sub(earlier.network_publications),
            network_enqueued: self.network_enqueued.wrapping_sub(earlier.network_enqueued),
            network_dropped: self.network_dropped.wrapping_sub(earlier.network_dropped),
            network_pool_exhausted: self
                .network_pool_exhausted
                .wrapping_sub(earlier.network_pool_exhausted),
            network_published_bytes: self
                .network_published_bytes
                .wrapping_sub(earlier.network_published_bytes),
            network_publish_micros: self
                .network_publish_micros
                .wrapping_sub(earlier.network_publish_micros),
            network_publish_lifetime_max_micros: self.network_publish_lifetime_max_micros,
            rx_irq_epochs: self.rx_irq_epochs.wrapping_sub(earlier.rx_irq_epochs),
            rx_irq_service_samples: self
                .rx_irq_service_samples
                .wrapping_sub(earlier.rx_irq_service_samples),
            rx_irq_clock_skew_samples: self
                .rx_irq_clock_skew_samples
                .wrapping_sub(earlier.rx_irq_clock_skew_samples),
            rx_irq_to_service_micros: self
                .rx_irq_to_service_micros
                .wrapping_sub(earlier.rx_irq_to_service_micros),
            rx_irq_to_service_lifetime_max_micros: self.rx_irq_to_service_lifetime_max_micros,
        }
    }
}

#[cfg(test)]
mod tests;
