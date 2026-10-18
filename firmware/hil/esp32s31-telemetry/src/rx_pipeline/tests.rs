use core::sync::atomic::{AtomicU64, Ordering};
use oer_ieee80211_mac::sequence::SequenceNumber;

use super::{RxCorrectnessObserver, RxPipelineCounters, RxServiceObservation};
use oer_esp32s31_ieee80211_runtime::diagnostics::rx_pipeline::{
    RxNetworkPublicationOutcome, RxPipelineObservation, RxPipelineObserver,
    RxReorderAgreementObservation, RxReorderAgreementObserver, RxStageDiscard,
};

static IRQ_CLOCK: AtomicU64 = AtomicU64::new(0);
static IRQ_SKEW_CLOCK: AtomicU64 = AtomicU64::new(0);

fn test_clock() -> u64 {
    0
}

fn irq_clock() -> u64 {
    IRQ_CLOCK.load(Ordering::Relaxed)
}

fn irq_skew_clock() -> u64 {
    IRQ_SKEW_CLOCK.load(Ordering::Relaxed)
}

#[test]
fn correctness_observer_keeps_required_facts_without_phase_profiling() {
    let counters = RxPipelineCounters::new(test_clock);
    let observer = RxCorrectnessObserver::new(&counters);

    observer.observe(RxReorderAgreementObservation::Started {
        tid: 3,
        starting_sequence: SequenceNumber::new(10).unwrap(),
        window: 16,
    });
    observer.observe(RxReorderAgreementObservation::First {
        tid: 3,
        start: SequenceNumber::new(10).unwrap(),
        sequence: SequenceNumber::new(11).unwrap(),
    });

    let snapshot = counters.snapshot();
    assert_eq!(snapshot.service_calls, 0);
    assert_eq!(snapshot.service_micros, 0);
    assert_eq!(snapshot.protocol_frames, 0);
    assert_eq!(snapshot.network_publications, 0);
    assert_eq!(snapshot.network_enqueued, 0);
    assert_eq!(snapshot.network_dropped, 0);
    assert_eq!(snapshot.dma_buffer_full_increments, 0);
    assert_eq!(snapshot.reorder_starts, 1);
    assert_eq!(snapshot.reorder_first_samples, 1);
    assert_eq!(snapshot.reorder_ingress, 0);
    assert_eq!(snapshot.reorder_buffered, 0);
    assert_eq!(snapshot.reorder_released, 0);
    assert_eq!(snapshot.reorder_missing, 0);
    assert_eq!(snapshot.reorder_current_occupied, 0);
    assert_eq!(snapshot.reorder_maximum_occupied, 0);
}

#[test]
fn interval_delta_retains_totals_limits_and_phase_times() {
    let counters = RxPipelineCounters::new(test_clock);
    let before = counters.snapshot();
    counters.record_service(RxServiceObservation {
        frontier: 4,
        pool_credits: 3,
        queue_credits: 5,
        admitted: 3,
        staged_bytes: 4_500,
        overload_discarded: 2,
        critical_reserve_admitted: 1,
        critical_admission_blocked: true,
        minimum_pool_credits: 1,
        minimum_queue_credits: 4,
        micros: 70,
        hardware_buffer_full_before: None,
        hardware_buffer_full_after: None,
        ..RxServiceObservation::default()
    });
    counters.record_stage_discard(RxStageDiscard::Empty);
    counters.record_stage_discard(RxStageDiscard::TooLong);
    counters.record_stage_discard(RxStageDiscard::Chained);
    counters.record_stage_discard(RxStageDiscard::OverloadBulk);
    counters.record_network_ready_wait(2);
    counters.record_network_publish(1_514, 13, RxNetworkPublicationOutcome::Enqueued);
    counters.record_network_publish(1_514, 17, RxNetworkPublicationOutcome::Dropped);
    counters.record_dispatch(true, true, 3, 2_750, 31);
    counters.observe(RxPipelineObservation::ReloadCompleted { micros: 4 });
    counters.observe(RxPipelineObservation::ReloadCompleted { micros: 7 });
    let delta = counters.snapshot().wrapping_delta_since(before);

    assert_eq!(delta.service_calls, 1);
    assert_eq!(delta.frontier_four_seven_services, 1);
    assert_eq!(delta.completion_frontier_frames, 4);
    assert_eq!(delta.admitted_frames, 3);
    assert_eq!(delta.staged_bytes, 4_500);
    assert_eq!(delta.reload_transactions, 2);
    assert_eq!(delta.reload_micros, 11);
    assert_eq!(delta.reload_lifetime_max_micros, 7);
    assert_eq!(delta.stage_empty_discards, 1);
    assert_eq!(delta.stage_too_long_discards, 1);
    assert_eq!(delta.stage_chained_discards, 1);
    assert_eq!(delta.stage_overload_bulk_discards, 1);
    assert_eq!(delta.overload_discarded_units, 2);
    assert_eq!(delta.critical_reserve_admissions, 1);
    assert_eq!(delta.critical_admission_blocked_services, 1);
    assert_eq!(delta.backpressured_services, 1);
    assert_eq!(delta.pool_credit_limited_services, 1);
    assert_eq!(delta.queue_credit_limited_services, 0);
    assert_eq!(delta.maximum_deferred_frames, 1);
    assert_eq!(delta.minimum_backpressured_pool_credits, 1);
    assert_eq!(delta.minimum_backpressured_queue_credits, 4);
    assert_eq!(delta.minimum_pool_credits, 1);
    assert_eq!(delta.minimum_queue_credits, 4);
    assert_eq!(delta.maximum_frontier, 4);
    assert_eq!(delta.maximum_admitted, 3);
    assert_eq!(delta.service_micros, 70);
    assert_eq!(delta.network_ready_wait_micros, 2);
    assert_eq!(delta.network_publications, 2);
    assert_eq!(delta.network_enqueued, 1);
    assert_eq!(delta.network_dropped, 1);
    assert_eq!(delta.network_published_bytes, 3_028);
    assert_eq!(delta.network_publish_micros, 30);
    assert_eq!(delta.protocol_data_frames, 1);
    assert_eq!(delta.protocol_amsdu_mpdus, 1);
    assert_eq!(delta.protocol_amsdu_subframes, 3);
    assert_eq!(delta.protocol_units_1701_3400, 1);
    assert_eq!(delta.protocol_unit_lifetime_max_bytes, 2_750);
    assert_eq!(delta.dispatch_micros, 31);
}

#[test]
fn preserved_bulk_head_is_reported_as_admission_backpressure() {
    let counters = RxPipelineCounters::new(test_clock);
    let before = counters.snapshot();
    counters.record_service(RxServiceObservation {
        frontier: 4,
        pool_credits: 1,
        queue_credits: 8,
        admitted: 0,
        stage_capacity_blocked: true,
        minimum_pool_credits: 1,
        minimum_queue_credits: 8,
        ..RxServiceObservation::default()
    });
    let delta = counters.snapshot().wrapping_delta_since(before);

    assert_eq!(delta.bulk_capacity_blocked_services, 1);
    assert_eq!(delta.backpressured_services, 1);
    assert_eq!(delta.pool_credit_limited_services, 1);
    assert_eq!(delta.queue_credit_limited_services, 0);
    assert_eq!(delta.minimum_backpressured_pool_credits, 1);
    assert_eq!(delta.minimum_backpressured_queue_credits, 8);
    assert_eq!(delta.minimum_pool_credits, 1);
    assert_eq!(delta.minimum_queue_credits, 8);
}

#[test]
fn budget_deferred_frontier_is_not_reported_as_admission_backpressure() {
    let counters = RxPipelineCounters::new(test_clock);
    let before = counters.snapshot();
    counters.record_service(RxServiceObservation {
        frontier: 45,
        pool_credits: 32,
        queue_credits: 32,
        completed_units: 32,
        admitted: 32,
        minimum_pool_credits: 8,
        minimum_queue_credits: 8,
        ..RxServiceObservation::default()
    });
    let delta = counters.snapshot().wrapping_delta_since(before);

    assert_eq!(delta.maximum_deferred_frames, 13);
    assert_eq!(delta.backpressured_services, 0);
    assert_eq!(delta.pool_credit_limited_services, 0);
    assert_eq!(delta.queue_credit_limited_services, 0);
}

#[test]
fn sampled_irq_latency_wraps_and_frontiers_cover_every_service() {
    IRQ_CLOCK.store((1_u64 << 31) - 3, Ordering::Relaxed);
    let counters = RxPipelineCounters::new(irq_clock);
    let before = counters.snapshot();
    counters.record_rx_irq_epoch();
    IRQ_CLOCK.store((1_u64 << 31) - 1, Ordering::Relaxed);
    counters.record_rx_irq_epoch();
    IRQ_CLOCK.store((1_u64 << 31) + 4, Ordering::Relaxed);
    let started = counters.begin_service();
    counters.record_service(RxServiceObservation {
        frontier: 32,
        pool_credits: 32,
        queue_credits: 32,
        admitted: 32,
        staged_bytes: 48_000,
        overload_discarded: 0,
        critical_reserve_admitted: 0,
        critical_admission_blocked: false,
        micros: 80,
        hardware_buffer_full_before: None,
        hardware_buffer_full_after: None,
        ..RxServiceObservation::default()
    });
    counters.begin_service();
    counters.record_service(RxServiceObservation {
        frontier: 0,
        pool_credits: 32,
        queue_credits: 32,
        admitted: 0,
        staged_bytes: 0,
        overload_discarded: 0,
        critical_reserve_admitted: 0,
        critical_admission_blocked: false,
        micros: 1,
        hardware_buffer_full_before: None,
        hardware_buffer_full_after: None,
        ..RxServiceObservation::default()
    });
    let delta = counters.snapshot().wrapping_delta_since(before);

    assert_eq!(started, (1_u64 << 31) + 4);
    assert_eq!(delta.rx_irq_epochs, 2);
    assert_eq!(delta.rx_irq_service_samples, 1);
    assert_eq!(delta.rx_irq_to_service_micros, 7);
    assert_eq!(delta.rx_irq_to_service_lifetime_max_micros, 7);
    assert_eq!(delta.frontier_zero_services, 1);
    assert_eq!(delta.frontier_thirty_two_plus_services, 1);
    assert_eq!(delta.service_calls, 2);
}

#[test]
fn negative_cross_core_clock_skew_is_not_reported_as_latency() {
    IRQ_SKEW_CLOCK.store(100, Ordering::Relaxed);
    let counters = RxPipelineCounters::new(irq_skew_clock);
    counters.record_rx_irq_epoch();
    IRQ_SKEW_CLOCK.store(96, Ordering::Relaxed);
    counters.begin_service();
    let snapshot = counters.snapshot();

    assert_eq!(snapshot.rx_irq_service_samples, 0);
    assert_eq!(snapshot.rx_irq_clock_skew_samples, 1);
    assert_eq!(snapshot.rx_irq_to_service_micros, 0);
}

#[test]
fn buffer_full_wrap_is_classified_between_service_transactions() {
    let counters = RxPipelineCounters::new(test_clock);
    counters.record_service(RxServiceObservation {
        frontier: 1,
        pool_credits: 64,
        queue_credits: 64,
        admitted: 1,
        staged_bytes: 1_600,
        overload_discarded: 0,
        critical_reserve_admitted: 0,
        critical_admission_blocked: false,
        micros: 20,
        hardware_buffer_full_before: Some(0xfffe),
        hardware_buffer_full_after: Some(0xfffe),
        ..RxServiceObservation::default()
    });
    let before = counters.snapshot();

    counters.record_service(RxServiceObservation {
        frontier: 7,
        pool_credits: 60,
        queue_credits: 61,
        admitted: 7,
        staged_bytes: 11_200,
        overload_discarded: 0,
        critical_reserve_admitted: 0,
        critical_admission_blocked: false,
        micros: 73,
        hardware_buffer_full_before: Some(1),
        hardware_buffer_full_after: Some(1),
        ..RxServiceObservation::default()
    });
    let delta = counters.snapshot().wrapping_delta_since(before);

    assert_eq!(delta.dma_buffer_full_increments, 3);
    assert_eq!(delta.dma_buffer_full_service_samples, 1);
    assert_eq!(delta.dma_buffer_full_between_services, 3);
    assert_eq!(delta.dma_buffer_full_during_services, 0);
    assert_eq!(delta.dma_buffer_full_between_service_samples, 1);
    assert_eq!(delta.dma_buffer_full_during_service_samples, 0);
    assert_eq!(delta.dma_buffer_full_last_service, 2);
    assert_eq!(delta.dma_buffer_full_last_phase, 1);
    assert_eq!(delta.dma_buffer_full_last_counter, 1);
    assert_eq!(delta.dma_buffer_full_last_frontier, 7);
    assert_eq!(delta.dma_buffer_full_last_admitted, 7);
    assert_eq!(delta.dma_buffer_full_last_pool_credits, 60);
    assert_eq!(delta.dma_buffer_full_last_queue_credits, 61);
    assert_eq!(delta.dma_buffer_full_last_service_micros, 73);
}

#[test]
fn buffer_full_is_classified_inside_the_observed_service_transaction() {
    let counters = RxPipelineCounters::new(test_clock);
    let before = counters.snapshot();

    counters.record_service(RxServiceObservation {
        frontier: 32,
        pool_credits: 32,
        queue_credits: 32,
        admitted: 32,
        staged_bytes: 48_000,
        overload_discarded: 0,
        critical_reserve_admitted: 0,
        critical_admission_blocked: false,
        micros: 810,
        hardware_buffer_full_before: Some(7),
        hardware_buffer_full_after: Some(9),
        ..RxServiceObservation::default()
    });
    let delta = counters.snapshot().wrapping_delta_since(before);

    assert_eq!(delta.dma_buffer_full_increments, 2);
    assert_eq!(delta.dma_buffer_full_between_services, 0);
    assert_eq!(delta.dma_buffer_full_during_services, 2);
    assert_eq!(delta.dma_buffer_full_between_service_samples, 0);
    assert_eq!(delta.dma_buffer_full_during_service_samples, 1);
    assert_eq!(delta.dma_buffer_full_last_phase, 2);
    assert_eq!(delta.dma_buffer_full_last_counter, 9);
}

#[test]
fn pool_exhaustion_is_a_distinct_subset_of_publication_drops() {
    let counters = RxPipelineCounters::new(|| 0);
    counters.record_network_publish(42, 0, RxNetworkPublicationOutcome::PoolExhausted);
    let before = counters.snapshot();
    counters.record_network_publish(42, 0, RxNetworkPublicationOutcome::Dropped);
    counters.record_network_publish(42, 0, RxNetworkPublicationOutcome::PoolExhausted);
    counters.record_network_publish(42, 0, RxNetworkPublicationOutcome::Enqueued);
    let delta = counters.snapshot().wrapping_delta_since(before);
    assert_eq!(delta.network_publications, 3);
    assert_eq!(delta.network_dropped, 2);
    assert_eq!(delta.network_pool_exhausted, 1);
    assert_eq!(delta.network_enqueued, 1);
}
