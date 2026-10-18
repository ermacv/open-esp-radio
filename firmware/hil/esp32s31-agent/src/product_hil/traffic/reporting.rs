#![forbid(unsafe_code)]

use core::future::Future;

use embassy_futures::yield_now;
use embassy_time::Instant;
#[cfg(feature = "core0-rx-cycle-telemetry")]
use oer_esp32s31_ieee80211_system::{
    CORE0_AP_RX_CYCLES, CORE0_REORDER_CYCLES, CORE0_RX_CYCLES, CORE0_RX_SERVICE_HISTOGRAM,
    Core0ApRxCycleSnapshot, Core0ReorderSnapshot, Core0RxCycleSnapshot,
    Core0RxServiceHistogramSnapshot,
};
#[cfg(any(
    feature = "core0-rx-cycle-telemetry",
    feature = "core0-rx-coarse-telemetry"
))]
use oer_esp32s31_ieee80211_system::{
    CORE0_PERFORMANCE, Core0PerformanceSample, Core0PerformanceSnapshot,
};
#[cfg(feature = "core0-rx-coarse-telemetry")]
use oer_esp32s31_ieee80211_system::{TX_PERFORMANCE, TxPerformanceSnapshot};
use oer_hil_esp32s31_telemetry::{
    aggregate_tx::AggregateTxCounterSnapshot,
    mac_irq::MacIrqClassificationSnapshot,
    rx_pipeline::{RxPipelineCounterSnapshot, RxPipelineCounters},
    task_poll::{TaskPollCounters, TaskPollSet, TaskPollSetSnapshot, TaskPollSnapshot},
};
use oer_hil_protocol::{
    network::RadioEvidence, network::TxAggregateTimingEvidence, network::TxRadioEvidence,
};

use crate::console::runtime_log_reliably;

/// Entry classification over the measured workload, without its terminal drain.
#[cfg(feature = "mac-irq-telemetry")]
pub(in crate::product_hil) async fn log_mac_irq_interval(sample: MacIrqClassificationSnapshot) {
    runtime_log_reliably(format_args!(
        "hil-irq: spurious={} rx_only={} rx_mixed={}",
        sample.spurious_entries, sample.rx_only_entries, sample.rx_mixed_entries,
    ))
    .await;
    runtime_log_reliably(format_args!(
        "hil-irq: tx_only={} tx_mixed={} other_only={}",
        sample.tx_only_entries, sample.tx_mixed_entries, sample.other_only_entries,
    ))
    .await;
}

#[cfg(any(
    feature = "core0-rx-cycle-telemetry",
    feature = "core0-rx-coarse-telemetry"
))]
use super::cache_performance::L1CachePerformanceSnapshot;

pub(in crate::product_hil) fn aggregate_tx_evidence(
    aggregate: AggregateTxCounterSnapshot,
) -> (RadioEvidence, TxAggregateTimingEvidence) {
    let radio = RadioEvidence {
        rx: None,
        tx: Some(TxRadioEvidence {
            station_terminal: aggregate.station_terminal,
            bandwidth_mhz: u16::try_from(aggregate.last_bandwidth_mhz).unwrap_or(u16::MAX),
            aggregate_rate_kbps: aggregate.last_nominal_rate_kbps,
            aggregates_prepared: aggregate.aggregates_prepared,
            aggregate_publications: aggregate.aggregate_publications,
            publications_pending_start: aggregate.publications_pending_start,
            publications_pending_end: aggregate.publications_pending_end,
            aggregates_completed: aggregate.aggregates_completed,
            subframes_prepared: aggregate.prepared_subframe_total(),
            subframes_acknowledged: aggregate.subframes_acknowledged,
            individual_retries: aggregate.individual_retries,
            hardware_timeouts: aggregate.hardware_timeouts,
            collisions: aggregate.collisions,
            minimum_subframes: aggregate.minimum_prepared_subframes().unwrap_or(0),
            maximum_subframes: aggregate.maximum_prepared_subframes().unwrap_or(0),
            prepared_histogram: [
                aggregate.prepared_in_range(1, 1),
                aggregate.prepared_in_range(2, 3),
                aggregate.prepared_in_range(4, 7),
                aggregate.prepared_in_range(8, 15),
                aggregate.prepared_in_range(16, 23),
                aggregate.prepared_in_range(24, 30),
                aggregate.prepared_in_range(31, 31),
                aggregate.prepared_in_range(32, 32),
            ],
            stopped_at_frame_limit: aggregate.stopped_at_frame_limit,
            stopped_at_capacity_limit: aggregate.stopped_at_capacity_limit,
            stopped_on_empty_queue: aggregate.stopped_on_empty_queue,
            block_ack_samples: aggregate.block_ack_samples,
            block_ack_received: aggregate.block_ack_received,
            success_without_block_ack: aggregate.success_without_block_ack,
            nonzero_block_ack_control: aggregate.nonzero_block_ack_control,
            full_block_ack: aggregate.full_block_ack,
            partial_block_ack: aggregate.partial_block_ack,
            empty_block_ack: aggregate.empty_block_ack,
            block_ack_requests: aggregate.block_ack_requests,
            block_ack_requests_answered: aggregate.block_ack_requests_answered,
            tx_irq_epochs: aggregate.tx_irq_epochs,
            tx_irq_service_samples: aggregate.tx_irq_service_samples,
            tx_irq_clock_skew_samples: aggregate.tx_irq_clock_skew_samples,
            tx_publication_to_irq_samples: aggregate.tx_publication_to_irq_samples,
        }),
    };
    let timing = TxAggregateTimingEvidence {
        preparation_micros: aggregate.preparation_micros,
        preparation_max_micros: aggregate.preparation_lifetime_max_micros,
        publication_micros: aggregate.publication_program_micros,
        publication_max_micros: aggregate.publication_program_lifetime_max_micros,
        exchange_micros: aggregate.exchange_micros,
        exchange_max_micros: aggregate.exchange_lifetime_max_micros,
        first_exchanges: aggregate.single_publication_exchanges,
        first_exchange_micros: aggregate.single_publication_exchange_micros,
        first_exchange_max_micros: aggregate.single_publication_exchange_lifetime_max_micros,
        retried_exchanges: aggregate.retried_exchanges,
        retry_publications: aggregate.retried_exchange_publications,
        retry_exchange_micros: aggregate.retried_exchange_micros,
        retry_exchange_max_micros: aggregate.retried_exchange_lifetime_max_micros,
        tx_irq_epochs: aggregate.tx_irq_epochs,
        tx_irq_service_samples: aggregate.tx_irq_service_samples,
        tx_irq_clock_skew_samples: aggregate.tx_irq_clock_skew_samples,
        tx_irq_service_micros: aggregate.tx_irq_to_service_micros,
        tx_irq_service_max_micros: aggregate.tx_irq_to_service_lifetime_max_micros,
        tx_publication_to_irq_samples: aggregate.tx_publication_to_irq_samples,
        tx_publication_to_irq_micros: aggregate.tx_publication_to_irq_micros,
        tx_publication_to_irq_max_micros: aggregate.tx_publication_to_irq_lifetime_max_micros,
        standby_prepared: aggregate.standby_prepared,
        standby_published: aggregate.standby_published,
        standby_cancelled: aggregate.standby_cancelled,
        standby_pending_start: aggregate.standby_pending_start,
        standby_pending_end: aggregate.standby_pending_end,
    };
    (radio, timing)
}

pub(in crate::product_hil) async fn log_open_radio_ampdu_snapshot(
    aggregate: AggregateTxCounterSnapshot,
) {
    let secondary_active = aggregate.secondary_socket.count != 0
        || aggregate.secondary_socket.pending_polls != 0
        || aggregate.secondary_socket.errors != 0
        || aggregate.secondary_claim.count != 0;
    for (frontier, progress) in [
        ("socket", aggregate.secondary_socket),
        ("radio-claim", aggregate.secondary_claim),
    ] {
        if secondary_active {
            runtime_log_reliably(format_args!(
                "OTXFLOW flow=1 frontier={} count={} last={} gap_us={} after={} idle_us={} pending_polls={} wait_max_us={} pending_us={} errors={} first_us={:?}",
                frontier, progress.count, progress.last_sequence, progress.maximum_gap_micros,
                progress.sequence_after_maximum_gap, progress.idle_micros, progress.pending_polls,
                progress.maximum_wait_micros, progress.pending_micros,
                progress.errors, progress.first_admission_micros,
            )).await;
        }
    }
    let aggregate_min = aggregate.minimum_prepared_subframes().unwrap_or(0);
    let aggregate_max = aggregate.maximum_prepared_subframes().unwrap_or(0);
    runtime_log_reliably(format_args!(
        "OAMP width_mhz={} rate_kbps={} aggregates={} publications={} completed={} subframes={} \
         acknowledged={} single={} single_rate={} single_ba={} single_pair={} \
         single_capacity={} single_capacity_max_len={} individual_retry={} timeout={} collision={} \
         min={} max={} stop_frame={} stop_capacity={} stop_empty={}",
        aggregate.last_bandwidth_mhz,
        aggregate.last_nominal_rate_kbps,
        aggregate.aggregates_prepared,
        aggregate.aggregate_publications,
        aggregate.aggregates_completed,
        aggregate.prepared_subframe_total(),
        aggregate.subframes_acknowledged,
        aggregate.network_single_mpdu_started,
        aggregate.network_single_legacy_rate,
        aggregate.network_single_block_ack_unavailable,
        aggregate.network_single_ht_needs_pair,
        aggregate.network_single_fresh_aggregate_capacity,
        aggregate.network_single_fresh_capacity_lifetime_max_ethernet_length,
        aggregate.individual_retries,
        aggregate.hardware_timeouts,
        aggregate.collisions,
        aggregate_min,
        aggregate_max,
        aggregate.stopped_at_frame_limit,
        aggregate.stopped_at_capacity_limit,
        aggregate.stopped_on_empty_queue,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "OAMPH one={} two_three={} four_seven={} eight_fifteen={} \
         sixteen_twentythree={} twentyfour_thirty={} thirtyone={} full32={}",
        aggregate.prepared_in_range(1, 1),
        aggregate.prepared_in_range(2, 3),
        aggregate.prepared_in_range(4, 7),
        aggregate.prepared_in_range(8, 15),
        aggregate.prepared_in_range(16, 23),
        aggregate.prepared_in_range(24, 30),
        aggregate.prepared_in_range(31, 31),
        aggregate.prepared_in_range(32, 32),
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "OAMPW exchanges={} publications={} psdu_bytes={} mpdus={} nominal_data_us={} unestimated_publications={} saturated_exchanges={}",
        aggregate.work.exchanges,
        aggregate.work.publications,
        aggregate.work.psdu_bytes,
        aggregate.work.mpdus,
        aggregate.work.nominal_data_micros,
        aggregate.work.unestimated_publications,
        aggregate.work.saturated_exchanges,
    ))
    .await;
    yield_now().await;
    let ordinary = aggregate.station_ordinary;
    runtime_log_reliably(format_args!(
        "OTXR scope=station reported={} missing={} success={} cts_recovered={} cts_retries={} ack_retries={} collisions={} terminal_cts={} hardware_timeout={}",
        ordinary.reported, ordinary.missing, ordinary.success, ordinary.cts_recovered,
        ordinary.cts_retries, ordinary.ack_retries, ordinary.collisions,
        ordinary.terminal_cts, ordinary.hardware_timeout,
    )).await;
    for (kind, work) in [
        ("ampdu", aggregate.work),
        ("ordinary", aggregate.ordinary_work),
    ] {
        runtime_log_reliably(format_args!(
            "OTXW kind={} exchanges={} publications={} psdu_bytes={} mpdus={} ppdu_us={} unknown_ppdus={} saturated_exchanges={}",
            kind, work.exchanges, work.publications, work.psdu_bytes, work.mpdus,
            work.ppdu_micros, work.unestimated_ppdus, work.saturated_exchanges,
        )).await;
        runtime_log_reliably(format_args!(
            "OTXC kind={} aifs_slots={} backoff_slots={} unreported_publications={}",
            kind, work.aifs_slots, work.backoff_slots, work.unreported_contention,
        ))
        .await;
    }
    runtime_log_reliably(format_args!(
        "OAMPT preparation_us={} preparation_max_us={} publication_us={} \
         publication_max_us={} exchange_us={} exchange_max_us={} \
         first_exchanges={} first_exchange_us={} first_exchange_max_us={} \
         retried_exchanges={} retry_publications={} retry_exchange_us={} retry_exchange_max_us={}",
        aggregate.preparation_micros,
        aggregate.preparation_lifetime_max_micros,
        aggregate.publication_program_micros,
        aggregate.publication_program_lifetime_max_micros,
        aggregate.exchange_micros,
        aggregate.exchange_lifetime_max_micros,
        aggregate.single_publication_exchanges,
        aggregate.single_publication_exchange_micros,
        aggregate.single_publication_exchange_lifetime_max_micros,
        aggregate.retried_exchanges,
        aggregate.retried_exchange_publications,
        aggregate.retried_exchange_micros,
        aggregate.retried_exchange_lifetime_max_micros,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "OAMPTD completion_to_publication={}/{} completion_to_publication_max_us={} \
         completion_core_us={} completion_core_max_us={} release_us={} release_max_us={}",
        aggregate.completion_to_publication_samples,
        aggregate.completion_to_publication_micros,
        aggregate.completion_to_publication_lifetime_max_micros,
        aggregate.completion_core_micros,
        aggregate.completion_core_lifetime_max_micros,
        aggregate.backing_release_micros,
        aggregate.backing_release_lifetime_max_micros,
    ))
    .await;
    yield_now().await;
    let histogram = aggregate.completion_to_publication_histogram;
    runtime_log_reliably(format_args!(
        "OAMPTH le50={} le100={} le200={} le500={} le1000={} gt1000={} \
         unprepared={}/{} unprepared_max_us={}",
        histogram[0],
        histogram[1],
        histogram[2],
        histogram[3],
        histogram[4],
        histogram[5],
        aggregate.unprepared_publication_samples,
        aggregate.unprepared_publication_micros,
        aggregate.unprepared_publication_lifetime_max_micros,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "OAMPTR r2={}/{} r3={}/{} r4={}/{}",
        aggregate.exchanges_by_publications[2],
        aggregate.exchange_lifetime_max_micros_by_publications[2],
        aggregate.exchanges_by_publications[3],
        aggregate.exchange_lifetime_max_micros_by_publications[3],
        aggregate.exchanges_by_publications[4],
        aggregate.exchange_lifetime_max_micros_by_publications[4],
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "OAMPS completion_to_entry={}/{} completion_to_entry_max_us={} \
         entry_to_publication={}/{} entry_to_publication_max_us={}",
        aggregate.completion_to_prepared_entry_samples,
        aggregate.completion_to_prepared_entry_micros,
        aggregate.completion_to_prepared_entry_lifetime_max_micros,
        aggregate.prepared_entry_to_publication_samples,
        aggregate.prepared_entry_to_publication_micros,
        aggregate.prepared_entry_to_publication_lifetime_max_micros,
    ))
    .await;
    yield_now().await;
    let scheduler = aggregate.prepared_scheduler_timing;
    runtime_log_reliably(format_args!(
        "OAMPSP samples={} passes={} passes_max={} control_ready_passes={} \
         completion_to_return_us={} completion_to_return_max_us={} \
         return_to_first_pass_us={} return_to_first_pass_max_us={} \
         multi_pass={} additional_passes_us={} additional_passes_max_us={} \
         stop_poll_us={} stop_poll_max_us={} readiness_us={} readiness_max_us={}",
        scheduler.samples,
        scheduler.scheduler_passes,
        scheduler.scheduler_passes_lifetime_max,
        scheduler.control_ready_passes,
        scheduler.completion_to_active_service_return.micros,
        scheduler
            .completion_to_active_service_return
            .lifetime_max_micros,
        scheduler.active_service_return_to_first_pass.micros,
        scheduler
            .active_service_return_to_first_pass
            .lifetime_max_micros,
        scheduler.multi_pass_samples,
        scheduler.additional_passes.micros,
        scheduler.additional_passes.lifetime_max_micros,
        scheduler.stop_poll.micros,
        scheduler.stop_poll.lifetime_max_micros,
        scheduler.control_readiness.micros,
        scheduler.control_readiness.lifetime_max_micros,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "OAMPSR prepared_readiness_us={} prepared_readiness_max_us={} \
         batch_policy_us={} batch_policy_max_us={} batch_to_entry_us={} batch_to_entry_max_us={} \
         readiness_to_entry_us={} readiness_to_entry_max_us={}",
        scheduler.control_check_to_prepared_readiness.micros,
        scheduler
            .control_check_to_prepared_readiness
            .lifetime_max_micros,
        scheduler.prepared_readiness_to_batch.micros,
        scheduler.prepared_readiness_to_batch.lifetime_max_micros,
        scheduler.prepared_batch_to_entry.micros,
        scheduler.prepared_batch_to_entry.lifetime_max_micros,
        scheduler.control_check_to_prepared_entry.micros,
        scheduler
            .control_check_to_prepared_entry
            .lifetime_max_micros,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "OAMPB operational_tids={:#04x} operational_transitions={} samples={} received={} \
         success_without={} nonzero_control={} start_outside={} start_lag_max={} \
         full={} partial={} empty={}",
        aggregate.block_ack_operational_tids,
        aggregate.block_ack_operational_transitions,
        aggregate.block_ack_samples,
        aggregate.block_ack_received,
        aggregate.success_without_block_ack,
        aggregate.nonzero_block_ack_control,
        aggregate.block_ack_start_outside_window,
        aggregate.block_ack_start_lag_max,
        aggregate.full_block_ack,
        aggregate.partial_block_ack,
        aggregate.empty_block_ack,
    ))
    .await;
    yield_now().await;
    let positions = aggregate.partial_missing_by_position;
    let counts = aggregate.partial_missing_counts;
    runtime_log_reliably(format_args!(
        "OAMPBM missing1={} missing2={} missing3_4={} missing5_8={} missing9={} \
         positions={:?}",
        counts[0], counts[1], counts[2], counts[3], counts[4], positions,
    ))
    .await;
    yield_now().await;
    let full = aggregate.full_block_ack_snr;
    let partial = aggregate.partial_block_ack_snr;
    runtime_log_reliably(format_args!(
        "OAMPBS unavailable={} full_samples={} full_mean_ddb={} full={:?} \
         partial_samples={} partial_mean_ddb={} partial={:?}",
        aggregate.block_ack_snr_unavailable,
        full.samples,
        full.mean_decidb().unwrap_or(0),
        full.histogram,
        partial.samples,
        partial.mean_decidb().unwrap_or(0),
        partial.histogram,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "OAMPI tx_irq_epochs={} tx_irq_samples={} tx_irq_skew={} \
         tx_irq_service_us={} tx_irq_service_max_us={} tx_flight_samples={} \
         tx_flight_us={} tx_flight_max_us={}",
        aggregate.tx_irq_epochs,
        aggregate.tx_irq_service_samples,
        aggregate.tx_irq_clock_skew_samples,
        aggregate.tx_irq_to_service_micros,
        aggregate.tx_irq_to_service_lifetime_max_micros,
        aggregate.tx_publication_to_irq_samples,
        aggregate.tx_publication_to_irq_micros,
        aggregate.tx_publication_to_irq_lifetime_max_micros,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "OAMPP standby_prepared={} standby_published={} standby_cancelled={} pending_start={} pending_end={}",
        aggregate.standby_prepared, aggregate.standby_published, aggregate.standby_cancelled,
        aggregate.standby_pending_start, aggregate.standby_pending_end,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "OAMPA ap_udp_claimed={} backward={} first={} after={} maximum_distance={}",
        aggregate.ap_udp_claimed,
        aggregate.ap_udp_claim_backward,
        aggregate.ap_udp_claim_first_sequence,
        aggregate.ap_udp_claim_first_previous,
        aggregate.ap_udp_claim_maximum_distance,
    ))
    .await;
    yield_now().await;
}

/// Incoming progress matters for TX too: unresolved ARP can prevent any UDP egress.
/// Snapshots bound the workload; logging happens afterwards without a guessed drain.
#[cfg(feature = "mac-irq-telemetry")]
pub(in crate::product_hil) async fn log_tx_ingress(
    hardware: Option<crate::product_hil::ObservedRxStatistics>,
    pipeline: RxPipelineCounterSnapshot,
) {
    if let Some(hardware) = hardware {
        runtime_log_reliably(format_args!(
            "hil-tx-ingress: mpdu={} success={} fcs={} abort={} full={} tkip={} unmatched={}",
            hardware.mpdu_count,
            hardware.data_success,
            hardware.fcs_error,
            hardware.abort,
            hardware.buffer_full,
            hardware.tkip_error,
            hardware.last_unmatched_error,
        ))
        .await;
    }
    runtime_log_reliably(format_args!(
        "hil-tx-ingress: completed={} staged={} discarded={} protocol={} data={}",
        pipeline.completed_units,
        pipeline.staged_units,
        pipeline.discarded_units,
        pipeline.protocol_frames,
        pipeline.protocol_data_frames,
    ))
    .await;
    runtime_log_reliably(format_args!(
        "hil-tx-ingress: reorder_in={} direct={} buffered={} released={} stale={} discarded={} occupied={}",
        pipeline.reorder_ingress, pipeline.reorder_direct, pipeline.reorder_buffered,
        pipeline.reorder_released, pipeline.reorder_stale, pipeline.reorder_discarded,
        pipeline.reorder_current_occupied,
    )).await;
    runtime_log_reliably(format_args!(
        "hil-tx-ingress: publications={} enqueued={} dropped={} pool_exhausted={}",
        pipeline.network_publications,
        pipeline.network_enqueued,
        pipeline.network_dropped,
        pipeline.network_pool_exhausted,
    ))
    .await;
}

pub(in crate::product_hil) async fn log_open_radio_rx_pipeline_interval(
    earlier: RxPipelineCounterSnapshot,
    rx_irq_posts: u32,
    mac_irq_entries: u32,
    irq_classification: MacIrqClassificationSnapshot,
    irq_auxiliary_entries: u32,
    irq_unhandled_entries: u32,
    counters: &RxPipelineCounters,
) {
    let pipeline = counters.snapshot().wrapping_delta_since(earlier);
    runtime_log_reliably(format_args!(
        "ORXS calls={} frontier={} completed={} descriptors={} admitted={} staged={} bytes={} \
         discarded={} discard_empty={} discard_long={} discard_chained={} discard_overload={} \
         overload_discarded={} recycled={} overload_recycled={} \
         fmax={} amax={} service_us={} service_boot_max_us={}",
        pipeline.service_calls,
        pipeline.completion_frontier_frames,
        pipeline.completed_units,
        pipeline.completed_descriptors,
        pipeline.admitted_frames,
        pipeline.staged_units,
        pipeline.staged_bytes,
        pipeline.discarded_units,
        pipeline.stage_empty_discards,
        pipeline.stage_too_long_discards,
        pipeline.stage_chained_discards,
        pipeline.stage_overload_bulk_discards,
        pipeline.overload_discarded_units,
        pipeline.recycled_descriptors,
        pipeline.overload_recycled_descriptors,
        pipeline.maximum_frontier,
        pipeline.maximum_admitted,
        pipeline.service_micros,
        pipeline.service_lifetime_max_micros,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORXSC samples={} back={} bulk_blocked={} pool={} queue={} deferred_max={} \
         pool_min={} queue_min={} pool_floor={} queue_floor={}",
        pipeline.service_calls,
        pipeline.backpressured_services,
        pipeline.bulk_capacity_blocked_services,
        pipeline.pool_credit_limited_services,
        pipeline.queue_credit_limited_services,
        pipeline.maximum_deferred_frames,
        pipeline.minimum_backpressured_pool_credits,
        pipeline.minimum_backpressured_queue_credits,
        pipeline.minimum_pool_credits,
        pipeline.minimum_queue_credits,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORXL transactions={} reload_us={} reload_boot_max_us={}",
        pipeline.reload_transactions, pipeline.reload_micros, pipeline.reload_lifetime_max_micros,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORXB increments={} samples={} between={} during={} between_samples={} \
         during_samples={} last_service={} last_phase={} last_counter={} \
         last_frontier={} last_admitted={} last_pool={} last_queue={} last_service_us={}",
        pipeline.dma_buffer_full_increments,
        pipeline.dma_buffer_full_service_samples,
        pipeline.dma_buffer_full_between_services,
        pipeline.dma_buffer_full_during_services,
        pipeline.dma_buffer_full_between_service_samples,
        pipeline.dma_buffer_full_during_service_samples,
        pipeline.dma_buffer_full_last_service,
        pipeline.dma_buffer_full_last_phase,
        pipeline.dma_buffer_full_last_counter,
        pipeline.dma_buffer_full_last_frontier,
        pipeline.dma_buffer_full_last_admitted,
        pipeline.dma_buffer_full_last_pool_credits,
        pipeline.dma_buffer_full_last_queue_credits,
        pipeline.dma_buffer_full_last_service_micros,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORXD frames={} data={} amsdu={} amsdu_subframes={} unit_le1700={} \
         unit_1701_3400={} unit_over3400={} unit_boot_max_bytes={} \
         waits={} wait_us={} wait_boot_max_us={} dispatch_us={} \
         dispatch_boot_max_us={} publications={} enqueued={} dropped={} bytes={} \
         publish_us={} publish_boot_max_us={}",
        pipeline.protocol_frames,
        pipeline.protocol_data_frames,
        pipeline.protocol_amsdu_mpdus,
        pipeline.protocol_amsdu_subframes,
        pipeline.protocol_units_le_1700,
        pipeline.protocol_units_1701_3400,
        pipeline.protocol_units_over_3400,
        pipeline.protocol_unit_lifetime_max_bytes,
        pipeline.network_ready_waits,
        pipeline.network_ready_wait_micros,
        pipeline.network_ready_wait_lifetime_max_micros,
        pipeline.dispatch_micros,
        pipeline.dispatch_lifetime_max_micros,
        pipeline.network_publications,
        pipeline.network_enqueued,
        pipeline.network_dropped,
        pipeline.network_published_bytes,
        pipeline.network_publish_micros,
        pipeline.network_publish_lifetime_max_micros,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORXP frames={} frame_us={} frame_boot_max_us={} reorder={} reorder_us={} \
         reorder_boot_max_us={} preflight={} preflight_us={} preflight_boot_max_us={}",
        pipeline.protocol_frame_transactions,
        pipeline.protocol_frame_micros,
        pipeline.protocol_frame_lifetime_max_micros,
        pipeline.reorder_preflights,
        pipeline.reorder_preflight_micros,
        pipeline.reorder_preflight_lifetime_max_micros,
        pipeline.protocol_preflights,
        pipeline.protocol_preflight_micros,
        pipeline.protocol_preflight_lifetime_max_micros,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORXR starts={} stops={} start_tid={} start_seq={} window={} first_samples={} \
         first_tid={} first_start={} first_seq={} first_distance={} buffered={} released={} \
         missing={} stale={} expiries={} occupied={} occupied_max={}",
        pipeline.reorder_starts,
        pipeline.reorder_stops,
        pipeline.reorder_last_start >> 26 & 0x07,
        pipeline.reorder_last_start & 0x0fff,
        pipeline.reorder_last_start >> 16 & 0x03ff,
        pipeline.reorder_first_samples,
        pipeline.reorder_last_first >> 24 & 0x0f,
        pipeline.reorder_last_first >> 12 & 0x0fff,
        pipeline.reorder_last_first & 0x0fff,
        pipeline.reorder_last_first_distance,
        pipeline.reorder_buffered,
        pipeline.reorder_released,
        pipeline.reorder_missing,
        pipeline.reorder_stale,
        pipeline.reorder_gap_expiries,
        pipeline.reorder_current_occupied,
        pipeline.reorder_maximum_occupied,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORXF zero={} one={} two_three={} four_seven={} eight_fifteen={} \
         sixteen_thirty_one={} thirty_two_plus={} irq_posts={} irq_epochs={} \
         irq_entries={} irq_coalesced={} irq_samples={} irq_skew={} \
         irq_service_us={} irq_service_boot_max_us={}",
        pipeline.frontier_zero_services,
        pipeline.frontier_one_services,
        pipeline.frontier_two_three_services,
        pipeline.frontier_four_seven_services,
        pipeline.frontier_eight_fifteen_services,
        pipeline.frontier_sixteen_thirty_one_services,
        pipeline.frontier_thirty_two_plus_services,
        rx_irq_posts,
        pipeline.rx_irq_epochs,
        mac_irq_entries,
        rx_irq_posts.saturating_sub(pipeline.rx_irq_epochs),
        pipeline.rx_irq_service_samples,
        pipeline.rx_irq_clock_skew_samples,
        pipeline.rx_irq_to_service_micros,
        pipeline.rx_irq_to_service_lifetime_max_micros,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORXI spurious={} rx_only={} rx_mixed={} tx_only={} tx_mixed={} other_only={} \
         extra={} saturated={} aux_entries={} unhandled_entries={}",
        irq_classification.spurious_entries,
        irq_classification.rx_only_entries,
        irq_classification.rx_mixed_entries,
        irq_classification.tx_only_entries,
        irq_classification.tx_mixed_entries,
        irq_classification.other_only_entries,
        irq_classification.extra_nonzero_snapshots,
        irq_classification.saturated_entries,
        irq_auxiliary_entries,
        irq_unhandled_entries,
    ))
    .await;
    yield_now().await;
}

pub(in crate::product_hil) async fn log_open_radio_task_poll_interval(
    earlier: TaskPollSetSnapshot,
    enabled: bool,
    counters: &TaskPollSet,
) {
    if !enabled {
        return;
    }
    log_open_radio_task_poll_snapshot(counters.snapshot().wrapping_delta_since(earlier)).await;
}

pub(in crate::product_hil) async fn log_open_radio_task_poll_snapshot(
    interval: TaskPollSetSnapshot,
) {
    log_open_radio_task_poll("network", interval.network).await;
    log_open_radio_task_poll("radio", interval.radio).await;
    log_open_radio_task_poll("udp_rx", interval.udp_rx).await;
    log_open_radio_task_poll("udp_tx", interval.udp_tx).await;
    log_open_radio_task_poll("tcp", interval.tcp).await;
}

#[cfg(feature = "core0-rx-cycle-telemetry")]
pub(in crate::product_hil) async fn log_open_radio_core0_rx_cycles(
    earlier: Core0RxCycleSnapshot,
    ap_earlier: Core0ApRxCycleSnapshot,
    performance_earlier: Core0PerformanceSnapshot,
    reorder_earlier: Core0ReorderSnapshot,
    cache: L1CachePerformanceSnapshot,
) {
    let cycles = CORE0_RX_CYCLES.snapshot().wrapping_delta_since(earlier);
    let ap = CORE0_AP_RX_CYCLES
        .snapshot()
        .wrapping_delta_since(ap_earlier);
    let performance = CORE0_PERFORMANCE
        .snapshot()
        .wrapping_delta_since(performance_earlier);
    let reorder = CORE0_REORDER_CYCLES
        .snapshot()
        .wrapping_delta_since(reorder_earlier);
    runtime_log_reliably(format_args!(
        "ORC0A calls={} total={} view={} dispatch={} dispatch_leaf={} reorder_key={} leaf_peer={} leaf_publish_check={} leaf_body={} leaf_admission={} leaf_observe={} publication={} activity_tail={} telemetry={}",
        ap.calls,
        ap.total,
        ap.view,
        ap.dispatch,
        ap.dispatch_leaf,
        ap.reorder_key,
        ap.leaf_peer,
        ap.leaf_publish_check,
        ap.leaf_body,
        ap.leaf_admission,
        ap.leaf_observe,
        ap.publication,
        ap.activity_tail,
        ap.telemetry,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0AP in_place_eligible={} in_place_published={} deferred_published={} reorder_buffered={} turn_calls={} turn_frames={} initial_batch={} initial_reorder={} mailbox_blocked={} tx_blocked={} batch_pending={} reorder_pending={} drained={} budget={}",
        ap.in_place_eligible,
        ap.in_place_published,
        ap.deferred_published,
        ap.reorder_buffered,
        ap.turn_calls,
        ap.turn_frames,
        ap.turn_initial_batch,
        ap.turn_initial_reorder,
        ap.turn_mailbox_blocked,
        ap.turn_tx_blocked,
        ap.turn_batch_pending,
        ap.turn_reorder_pending,
        ap.turn_drained,
        ap.turn_budget_exhausted,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0 services={} units={} total={} setup={} frontier={} admission={} \
         stage_total={} stage_take={} stage_pool={} recycle={} reload={} publish={} tail={}",
        cycles.services,
        cycles.units,
        cycles.total,
        cycles.setup,
        cycles.frontier,
        cycles.admission,
        cycles.stage_total,
        cycles.stage_take,
        cycles.stage_pool,
        cycles.recycle,
        cycles.reload,
        cycles.publish,
        cycles.tail,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0R runner_calls={} runner_total={} runner_pre={} runner_driver={} runner_post={} \
         mac_irq_entries={} mac_irq_cycles={} control_calls={} control_cycles={} \
         control_idle={} control_more={} control_tx={}",
        cycles.runner_rx_calls,
        cycles.runner_rx_total,
        cycles.runner_rx_pre,
        cycles.runner_rx_driver,
        cycles.runner_rx_post,
        cycles.mac_irq_entries,
        cycles.mac_irq_cycles,
        cycles.control_calls,
        cycles.control_cycles,
        cycles.control_idle,
        cycles.control_more,
        cycles.control_tx_pending,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0P polls={} poll_cycles={} polls_with_rx={} poll_to_runner={} runner_to_exit={}",
        cycles.radio_polls,
        cycles.radio_poll_cycles,
        cycles.radio_polls_with_rx,
        cycles.poll_to_runner_cycles,
        cycles.runner_to_poll_exit_cycles,
    ))
    .await;
    yield_now().await;
    // Accepted-list exhaustion distinguishes a physical descriptor-list stop
    // from the similarly named MAC RX statistics counter. Keep this line
    // ahead of the optional fine-grained phase dump so diagnostic UART loss
    // cannot remove the primary BUFFER_FULL correlation.
    runtime_log_reliably(format_args!(
        "ORC0R exhausted={} remaining1_8={} remaining9_16={} remaining17_32={} remaining33_48={} remaining49_plus={} unknown={} episodes={} resolved_le64us={} resolved_le256us={} resolved_le1024us={} resolved_gt1024us={}",
        performance.dma_entry_remaining_exhausted,
        performance.dma_entry_remaining_1_8,
        performance.dma_entry_remaining_9_16,
        performance.dma_entry_remaining_17_32,
        performance.dma_entry_remaining_33_48,
        performance.dma_entry_remaining_49_plus,
        performance.dma_entry_remaining_unknown,
        performance.dma_exhaustion_episodes,
        performance.dma_exhaustion_resolved_le_64us,
        performance.dma_exhaustion_resolved_le_256us,
        performance.dma_exhaustion_resolved_le_1024us,
        performance.dma_exhaustion_resolved_gt_1024us,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0S calls={} software={} irq={} select={} reentry={} stop={} housekeeping={} tx_checks={} rx_checks={}",
        cycles.scheduler_rx_calls,
        cycles.scheduler_software_rx_calls,
        cycles.scheduler_irq_rx_calls,
        cycles.scheduler_select_rx_calls,
        cycles.scheduler_reentry_cycles,
        cycles.scheduler_stop_cycles,
        cycles.scheduler_housekeeping_cycles,
        cycles.scheduler_tx_checks_cycles,
        cycles.scheduler_rx_checks_cycles,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0H discard={} queue={} control_ready={} prepared={} pending={} idle={}",
        cycles.scheduler_discard_wakes_cycles,
        cycles.scheduler_first_network_queue_cycles,
        cycles.scheduler_control_ready_cycles,
        cycles.scheduler_prepared_cycles,
        cycles.scheduler_network_pending_cycles,
        cycles.scheduler_idle_accounting_cycles,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0T polls={} poll_cycles={} polls_with_frame={} poll_to_frame={} frame_to_exit={}",
        cycles.protocol_polls,
        cycles.protocol_poll_cycles,
        cycles.protocol_polls_with_frame,
        cycles.protocol_poll_to_first_frame_cycles,
        cycles.protocol_last_frame_to_poll_exit_cycles,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0Q dequeues={} poll_to_dequeue={} between_to_dequeue={} dequeue_to_frame={}",
        cycles.protocol_dequeues,
        cycles.protocol_poll_to_first_dequeue_cycles,
        cycles.protocol_between_frame_to_dequeue_cycles,
        cycles.protocol_dequeue_to_frame_cycles,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0D calls={} ordinary={} scratch={} total={} preflight={} wait={} dispatch={} publish_tail={}",
        cycles.protocol_frame_calls,
        cycles.protocol_frame_ordinary,
        cycles.protocol_frame_scratch,
        cycles.protocol_frame_total,
        cycles.protocol_frame_preflight,
        cycles.protocol_frame_wait,
        cycles.protocol_frame_dispatch,
        cycles.protocol_frame_publish_tail,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0E dispatch_pre={} capture={} dispatch_post={} protected_calls={} protected_cycles={}",
        cycles.protocol_dispatch_pre_publish,
        cycles.protocol_dispatch_capture,
        cycles.protocol_dispatch_post_publish,
        cycles.protocol_protected_view_calls,
        cycles.protocol_protected_view_cycles,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0F calls={} completed={} total={} view={} fragment_guard={} decapsulate={} replay={} duplicate={} publish={}",
        cycles.data_calls,
        cycles.data_completed,
        cycles.data_total,
        cycles.data_view,
        cycles.data_fragment_guard,
        cycles.data_decapsulate,
        cycles.data_replay,
        cycles.data_duplicate,
        cycles.data_publish,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0M calls={} total={} admission={} layout={} projection={} validate={} telemetry={}",
        cycles.ccmp_view_calls,
        cycles.ccmp_view_total,
        cycles.ccmp_view_admission,
        cycles.ccmp_view_layout,
        cycles.ccmp_view_projection,
        cycles.ccmp_view_validate,
        cycles.ccmp_view_telemetry,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0I dma={} runner={} scheduler={} protocol_dequeue={} protocol_entry={} protocol_frame={} data={}",
        cycles.telemetry_dma_record,
        cycles.telemetry_runner_record,
        cycles.telemetry_scheduler_record,
        cycles.telemetry_protocol_dequeue_record,
        cycles.telemetry_protocol_entry_record,
        cycles.telemetry_protocol_frame_record,
        cycles.telemetry_data_record,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0J radio_polls={} radio_cycles={} radio_instret={} poll_to_runner_instret={} runner_to_exit_instret={} runner_calls={} runner_cycles={} runner_instret={} protocol_polls={} protocol_cycles={} protocol_instret={}",
        performance.radio_polls,
        performance.radio_cycles,
        performance.radio_instructions,
        performance.poll_to_runner_instructions,
        performance.runner_to_poll_exit_instructions,
        performance.runner_calls,
        performance.runner_cycles,
        performance.runner_instructions,
        performance.protocol_polls,
        performance.protocol_cycles,
        performance.protocol_instructions,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0K dma_calls={} dma_units={} dma_cycles={} dma_instret={} protocol_frames={} protocol_frame_cycles={} protocol_frame_instret={}",
        performance.dma_calls,
        performance.dma_units,
        performance.dma_cycles,
        performance.dma_instructions,
        performance.protocol_frames,
        performance.protocol_frame_cycles,
        performance.protocol_frame_instructions,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0G calls={} no_key={} inactive={} immediate={} slow={} total={} key={} bank={} ingress_observer={} first={} ingest={} deadline={} release_observer={} occupied_observer={} prepared_observer={} tail={} telemetry={}",
        reorder.calls,
        reorder.no_key,
        reorder.inactive,
        reorder.immediate,
        reorder.slow,
        reorder.total,
        reorder.key,
        reorder.bank,
        reorder.ingress_observer,
        reorder.first,
        reorder.ingest,
        reorder.deadline,
        reorder.release_observer,
        reorder.occupied_observer,
        reorder.prepared_observer,
        reorder.tail,
        reorder.telemetry_record,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0L calls={} accepted={} preflight_rejected={} key_rejected={} bank_rejected={} reorder_rejected={} duplicate_or_ignored={} total={} preflight={} key={} bank={} ingest={} deadline={} dispatch={} tail={} telemetry={}",
        reorder.direct_calls,
        reorder.direct_accepted,
        reorder.direct_preflight_rejected,
        reorder.direct_key_rejected,
        reorder.direct_bank_rejected,
        reorder.direct_reorder_rejected,
        reorder.direct_duplicate_or_ignored,
        reorder.direct_total,
        reorder.direct_preflight,
        reorder.direct_key,
        reorder.direct_bank,
        reorder.direct_ingest,
        reorder.direct_deadline,
        reorder.direct_dispatch,
        reorder.direct_tail,
        reorder.direct_telemetry_record,
    ))
    .await;
    yield_now().await;
    log_open_radio_l1_cache_interval(cache).await;
}

#[cfg(any(
    feature = "core0-rx-cycle-telemetry",
    feature = "core0-rx-coarse-telemetry"
))]
pub(in crate::product_hil) async fn log_open_radio_l1_cache_interval(
    cache: L1CachePerformanceSnapshot,
) {
    runtime_log_reliably(format_args!(
        "OCACHEI trace={} bus0_enabled={} bus1_enabled={} bus0_hit={} bus0_miss={} bus0_conflict={} bus0_next={} bus1_hit={} bus1_miss={} bus1_conflict={} bus1_next={}",
        u8::from(cache.trace_enabled),
        u8::from(cache.counter_enable.ibus0),
        u8::from(cache.counter_enable.ibus1),
        cache.ibus0.hit,
        cache.ibus0.miss,
        cache.ibus0.conflict,
        cache.ibus0.next_level_read,
        cache.ibus1.hit,
        cache.ibus1.miss,
        cache.ibus1.conflict,
        cache.ibus1.next_level_read,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "OCACHED bus0_enabled={} bus1_enabled={} bus0_hit={} bus0_miss={} bus0_conflict={} bus0_next_read={} bus0_next_write={} bus1_hit={} bus1_miss={} bus1_conflict={} bus1_next_read={} bus1_next_write={}",
        u8::from(cache.counter_enable.dbus0),
        u8::from(cache.counter_enable.dbus1),
        cache.dbus0.hit,
        cache.dbus0.miss,
        cache.dbus0.conflict,
        cache.dbus0.next_level_read,
        cache.dbus0.next_level_write,
        cache.dbus1.hit,
        cache.dbus1.miss,
        cache.dbus1.conflict,
        cache.dbus1.next_level_read,
        cache.dbus1.next_level_write,
    ))
    .await;
    yield_now().await;
}

/// Emit only batch-level Core0 cost evidence.
///
/// This is intentionally distinct from `log_open_radio_core0_rx_cycles`: the
/// coarse image never links its per-frame phase counters or driver observers.
#[cfg(feature = "core0-rx-coarse-telemetry")]
pub(in crate::product_hil) async fn log_open_radio_core0_rx_coarse(
    earlier: Core0PerformanceSnapshot,
) {
    let performance = CORE0_PERFORMANCE.snapshot().wrapping_delta_since(earlier);
    runtime_log_reliably(format_args!(
        "ORC0C rx_irq_posts={} radio_polls={} radio_cycles={} radio_instret={} poll_to_runner_cycles={} poll_to_runner_instret={} runner_to_exit_cycles={} runner_to_exit_instret={} runner_calls={} runner_cycles={} runner_instret={}",
        performance.rx_interrupt_posts,
        performance.radio_polls,
        performance.radio_cycles,
        performance.radio_instructions,
        performance.poll_to_runner_cycles,
        performance.poll_to_runner_instructions,
        performance.runner_to_poll_exit_cycles,
        performance.runner_to_poll_exit_instructions,
        performance.runner_calls,
        performance.runner_cycles,
        performance.runner_instructions,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0B protocol_polls={} protocol_cycles={} protocol_instret={} direct_frames={} async_frames={} dma_calls={} dma_empty={} dma_units={} dma_cycles={} dma_instret={}",
        performance.protocol_polls,
        performance.protocol_cycles,
        performance.protocol_instructions,
        performance.direct_protocol_frames,
        performance.asynchronous_protocol_frames,
        performance.dma_calls,
        performance.dma_empty_calls,
        performance.dma_units,
        performance.dma_cycles,
        performance.dma_instructions,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0TX start_calls={} start_cycles={} start_instret={} prepare_calls={} prepare_cycles={} prepare_instret={} publish_calls={} publish_cycles={} publish_instret={} service_calls={} service_cycles={} service_instret={}",
        performance.tx_start_calls,
        performance.tx_start_cycles,
        performance.tx_start_instructions,
        performance.tx_prepare_calls,
        performance.tx_prepare_cycles,
        performance.tx_prepare_instructions,
        performance.tx_publish_calls,
        performance.tx_publish_cycles,
        performance.tx_publish_instructions,
        performance.tx_service_calls,
        performance.tx_service_cycles,
        performance.tx_service_instructions,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0TXN encode_calls={} encode_cycles={} encode_instret={} commit_calls={} commit_cycles={} commit_instret={}",
        performance.tx_encode_calls,
        performance.tx_encode_cycles,
        performance.tx_encode_instructions,
        performance.tx_commit_calls,
        performance.tx_commit_cycles,
        performance.tx_commit_instructions,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0TXG samples={} cycles={} instret={} le64={} le256={} le512={} le1024={} gt1024={} completions={} prepared={} full={} partial={} frames={} queued={} empty={}",
        performance.tx_prepared_gap_samples,
        performance.tx_prepared_gap_cycles,
        performance.tx_prepared_gap_instructions,
        performance.tx_prepared_gap_le_64us,
        performance.tx_prepared_gap_le_256us,
        performance.tx_prepared_gap_le_512us,
        performance.tx_prepared_gap_le_1024us,
        performance.tx_prepared_gap_gt_1024us,
        performance.tx_network_completions,
        performance.tx_completion_prepared,
        performance.tx_completion_prepared_full,
        performance.tx_completion_prepared_partial,
        performance.tx_completion_prepared_frames,
        performance.tx_completion_queued,
        performance.tx_completion_empty,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0TXF initial_frames={} partial_samples={} matching_retained={} other_retained={} network_ready={} mismatch_claims={}",
        performance.tx_initial_network_frames,
        performance.tx_ap_partial_frontiers,
        performance.tx_ap_partial_matching_retained,
        performance.tx_ap_partial_other_retained,
        performance.tx_ap_partial_network_ready,
        performance.tx_ap_partial_mismatch_claims,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0TXO ownership_samples={} admitted={} pool_free={} ready_same={} ready_other={} ingress_reserved={} application_reserved={} tokens_in_flight={} radio_owned={} unattributed_radio_owned={}",
        performance.tx_ap_partial_publications,
        performance.tx_ap_publication_admitted,
        performance.tx_ap_publication_pool_free,
        performance.tx_ap_publication_ready_same,
        performance.tx_ap_publication_ready_other,
        performance.tx_ap_publication_ingress_reserved,
        performance.tx_ap_publication_application_reserved,
        performance.tx_ap_publication_tokens_in_flight,
        performance.tx_ap_publication_radio_owned,
        performance.tx_ap_publication_unattributed_radio_owned,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0P drained={} probe={} protocol_tx_blocked={} recycled_append={} budget={} stage_blocked={} network_blocked={} droppable={}",
        performance.rx_progress_drained,
        performance.rx_progress_probe_pending,
        performance.rx_progress_protocol_tx_blocked,
        performance.rx_progress_recycled_append_pending,
        performance.rx_progress_budget_exhausted,
        performance.rx_progress_stage_blocked,
        performance.rx_progress_network_blocked,
        performance.rx_progress_droppable,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0Q empty={} one={} two={} three_seven={} eight_plus={} probe_recycled={} probe_frontier={} probe_terminal={} probe_republication={}",
        performance.dma_empty_calls,
        performance.dma_single_unit_calls,
        performance.dma_two_unit_calls,
        performance.dma_three_to_seven_unit_calls,
        performance.dma_eight_plus_unit_calls,
        performance.dma_probe_recycled,
        performance.dma_probe_completed_frontier,
        performance.dma_probe_terminal_writeback,
        performance.dma_probe_republication,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0A delay64={} delay128={} delay256={} delay512={} delay_other={} empty_work={} work_units={} staged_bytes={}",
        performance.adaptive_probe_delay_64,
        performance.adaptive_probe_delay_128,
        performance.adaptive_probe_delay_256,
        performance.adaptive_probe_delay_512,
        performance.adaptive_probe_delay_other,
        performance.adaptive_probe_empty_work,
        performance.adaptive_probe_work_units,
        performance.adaptive_probe_staged_bytes,
    ))
    .await;
    yield_now().await;
}

/// Emit Core0 general-memory to internal-SRAM promotion costs for TX.
#[cfg(feature = "core0-rx-coarse-telemetry")]
pub(in crate::product_hil) async fn log_open_radio_tx_promotion(earlier: TxPerformanceSnapshot) {
    let performance = TX_PERFORMANCE.snapshot().wrapping_delta_since(earlier);
    let (ba_peers, ba_min, ba_max) = crate::product_hil::access_point_tx_block_ack_geometry();
    runtime_log_reliably(format_args!(
        "ONTXQ returns={} ba_peers={} ba_min={} ba_max={}",
        performance.radio_returns, ba_peers, ba_min, ba_max,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ONTXP attempts={} successes={} no_credit={} bytes={} total_cycles={} total_instret={} credit_cycles={} credit_instret={} destination_claim_cycles={} destination_claim_instret={} copy_cycles={} copy_instret={}",
        performance.promotion_attempts,
        performance.promotion_successes,
        performance.promotion_no_credit,
        performance.promotion_bytes,
        performance.promotion_cycles,
        performance.promotion_instructions,
        performance.promotion_credit_cycles,
        performance.promotion_credit_instructions,
        performance.promotion_destination_claim_cycles,
        performance.promotion_destination_claim_instructions,
        performance.promotion_copy_cycles,
        performance.promotion_copy_instructions,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ONTXPO publication_cycles={} publication_instret={} source_release_cycles={} source_release_instret={} radio_claim_cycles={} radio_claim_instret={} unattributed_cycles={} unattributed_instret={}",
        performance.promotion_publication_cycles,
        performance.promotion_publication_instructions,
        performance.promotion_source_release_cycles,
        performance.promotion_source_release_instructions,
        performance.promotion_radio_claim_cycles,
        performance.promotion_radio_claim_instructions,
        performance.promotion_unattributed_cycles(),
        performance.promotion_unattributed_instructions(),
    ))
    .await;
    yield_now().await;
}

#[cfg(feature = "core0-rx-cycle-telemetry")]
pub(in crate::product_hil) async fn log_open_radio_core0_rx_service_histogram(
    earlier: &Core0RxServiceHistogramSnapshot,
) {
    let services = CORE0_RX_SERVICE_HISTOGRAM
        .snapshot()
        .wrapping_delta_since(*earlier);
    runtime_log_reliably(format_args!(
        "ORC0X spsc_push_calls={} spsc_push_full={} spsc_push_cycles={} \
         spsc_pop_calls={} spsc_pop_empty={} spsc_pop_cycles={} \
         service_record_cycles={} spsc_record_cycles={}",
        services.spsc_push_calls,
        services.spsc_push_full,
        services.spsc_push_cycles,
        services.spsc_pop_calls,
        services.spsc_pop_empty,
        services.spsc_pop_cycles,
        services.service_record_cycles,
        services.spsc_record_cycles,
    ))
    .await;
    yield_now().await;
    for (units, bin) in services.bins.into_iter().enumerate() {
        if bin.services == 0 {
            continue;
        }
        runtime_log_reliably(format_args!(
            "ORC0B units={} services={} total={} setup={} frontier={} admission={} \
             stage_take={} stage_pool={} publish={} tail={}",
            units,
            bin.services,
            bin.total,
            bin.setup,
            bin.frontier,
            bin.admission,
            bin.stage_take,
            bin.stage_pool,
            bin.publish,
            bin.tail,
        ))
        .await;
        yield_now().await;
    }
}

async fn log_open_radio_task_poll(task: &str, poll: TaskPollSnapshot) {
    runtime_log_reliably(format_args!(
        "ORTP task={task} polls={} poll_us={} poll_boot_max_us={} \
         over_100us={} over_500us={} over_1000us={} over_5000us={}",
        poll.polls,
        poll.poll_micros,
        poll.lifetime_max_micros,
        poll.over_100_micros,
        poll.over_500_micros,
        poll.over_1_000_micros,
        poll.over_5_000_micros,
    ))
    .await;
    yield_now().await;
}

/// Observe continuous executor residence without changing the wrapped
/// future's wake or pending semantics. Wall time includes interrupt
/// preemption, which is intentional: a long task poll that blocks sibling
/// Embassy work is harmful regardless of whether its body or an ISR consumed
/// the interval.
#[allow(
    large_assignments,
    reason = "the generic wrapper pins the already owner-rich future inside the final static Embassy task arena; the linked-image stack gate and runtime watermark audits remain authoritative"
)]
pub(in crate::product_hil) async fn observe_open_radio_task_polls<F: Future>(
    future: F,
    counters: &'static TaskPollCounters,
    enabled: bool,
) -> F::Output {
    if !enabled {
        return future.await;
    }
    let mut future = core::pin::pin!(future);
    core::future::poll_fn(|context| {
        let started = Instant::now();
        let result = future.as_mut().poll(context);
        counters.record(started.elapsed().as_micros());
        result
    })
    .await
}

#[cfg(any(
    feature = "core0-rx-cycle-telemetry",
    feature = "core0-rx-coarse-telemetry"
))]
#[allow(
    large_assignments,
    reason = "the diagnostic wrapper pins the existing radio future directly inside its reviewed static task arena"
)]
pub(in crate::product_hil) async fn observe_open_radio_core0_task_polls<F: Future>(
    future: F,
    counters: &'static TaskPollCounters,
) -> F::Output {
    let mut future = core::pin::pin!(future);
    core::future::poll_fn(|context| {
        let started = Instant::now();
        let performance_started = Core0PerformanceSample::read();
        CORE0_PERFORMANCE.begin_radio_poll(performance_started);
        #[cfg(feature = "core0-rx-cycle-telemetry")]
        CORE0_RX_CYCLES.begin_radio_poll(performance_started.cycles());
        let result = future.as_mut().poll(context);
        let performance_ended = Core0PerformanceSample::read();
        #[cfg(feature = "core0-rx-cycle-telemetry")]
        CORE0_RX_CYCLES.end_radio_poll(performance_ended.cycles());
        CORE0_PERFORMANCE.record_radio_poll(performance_started, performance_ended);
        counters.record(started.elapsed().as_micros());
        result
    })
    .await
}

#[cfg(feature = "tx-wait-probe")]
pub(super) async fn log_tx_wait_trace(trace: &oer_hil_esp32s31_telemetry::tx_wait::Trace) {
    for (index, sample) in trace.records().enumerate() {
        let at = (u64::from(sample.at_high) << 32) | u64::from(sample.at_low);
        runtime_log_reliably(format_args!(
            "hil-tx-wait: sample={} at={} age_us={} late_us={} seq={} q={} available={} deadline_wake={}",
            index,
            at,
            sample.elapsed_us,
            sample.lateness_us,
            sample.sequence,
            sample.queue,
            sample.available,
            sample.deadline_wake,
        ))
        .await;
        runtime_log_reliably(format_args!(
            "hil-tx-wait: sample={} enabled={} valid={} done={} timeout={} collision={} aifsn={} backoff={} priority={} cca={}/{} mac_active={}",
            index, sample.enabled, sample.valid, sample.done, sample.timeout, sample.collision,
            sample.aifsn, sample.backoff, sample.priority, sample.cca_force, sample.cca_aux_force,
            sample.mac_active_state,
        )).await;
        runtime_log_reliably(format_args!(
            "hil-tx-wait: sample={} rx_hang={} tx_hang={} rx_tx_hang={} rx_tx_panic={}",
            index, sample.rx_hang, sample.tx_hang, sample.rx_tx_hang, sample.rx_tx_panic,
        ))
        .await;
        runtime_log_reliably(format_args!(
            "hil-tx-wait: sample={} rx_mpdu={} rx_signal={} rx_end={} rx_fcs={} rx_abort={}",
            index,
            sample.rx_mpdu_count,
            sample.rx_signal_count,
            sample.rx_end_count,
            sample.rx_fcs_error_count,
            sample.rx_abort_count,
        ))
        .await;
    }
    runtime_log_reliably(format_args!(
        "hil-tx-wait: scope=boot retention=8-per-age-band retained={} dropped={}",
        trace.records().count(),
        trace.dropped()
    ))
    .await;
}
