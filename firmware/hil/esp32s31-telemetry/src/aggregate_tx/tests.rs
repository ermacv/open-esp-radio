use super::*;
use oer_ieee80211_mac::sequence::SequenceNumber;

#[test]
fn block_ack_readiness_is_current_state_while_transitions_are_interval_evidence() {
    let counters = AggregateTxCounters::new();
    let before = counters.snapshot();
    assert!(!before.block_ack_operational(0));

    counters.observe(AggregateTxObservation::BlockAckOperational {
        tid: 0,
        operational: true,
    });
    let operational = counters.snapshot();
    assert!(operational.block_ack_operational(0));
    assert!(!operational.block_ack_operational(8));
    let delta = operational.wrapping_delta_since(before);
    assert!(delta.block_ack_operational(0));
    assert_eq!(delta.block_ack_operational_transitions, 1);

    counters.set_block_ack_operational(0, true);
    assert_eq!(
        counters
            .snapshot()
            .wrapping_delta_since(operational)
            .block_ack_operational_transitions,
        0
    );

    counters.observe(AggregateTxObservation::BlockAckOperational {
        tid: 0,
        operational: false,
    });
    let stopped = counters.snapshot();
    assert!(!stopped.block_ack_operational(0));
    let delta = stopped.wrapping_delta_since(operational);
    assert!(!delta.block_ack_operational(0));
    assert_eq!(delta.block_ack_operational_transitions, 1);
}

#[test]
fn counters_preserve_distribution_and_timing_deltas() {
    let counters = AggregateTxCounters::new();
    let before = counters.snapshot();
    counters.observe(AggregateTxObservation::NetworkSingleMpdu {
        reason: NetworkSingleMpduReason::BlockAckUnavailable,
        ethernet_length: 42,
    });
    counters.observe(AggregateTxObservation::RateSelected {
        bandwidth_mhz: 40,
        nominal_kbps: 150_000,
    });
    counters.observe(AggregateTxObservation::Prepared {
        subframes: 2,
        stop: AggregateBuildStop::QueueEmpty,
    });
    counters.observe(AggregateTxObservation::Prepared {
        subframes: 32,
        stop: AggregateBuildStop::FrameLimit,
    });
    counters.observe(AggregateTxObservation::Published {
        at_micros: 80,
        program_micros: 3,
    });
    counters.observe(AggregateTxObservation::Published {
        at_micros: 95,
        program_micros: 5,
    });
    counters.observe(AggregateTxObservation::Completed {
        acknowledged: 31,
        individual_retry: true,
    });
    counters.observe(AggregateTxObservation::HardwareTimeout);
    counters.observe(AggregateTxObservation::ExchangeCompleted {
        micros: 41,
        publications: 1,
    });
    counters.observe(AggregateTxObservation::ExchangeCompleted {
        micros: 59,
        publications: 3,
    });
    counters.observe(AggregateTxObservation::ExchangeCompleted {
        micros: 71,
        publications: 4,
    });
    counters.observe(AggregateTxObservation::PreparationCompleted { micros: 7 });
    counters.observe(AggregateTxObservation::PreparationCompleted { micros: 11 });
    counters.record_tx_irq_epoch(|| 100);
    counters.observe(AggregateTxObservation::InterruptServiceStarted { at_micros: 109 });

    let delta = counters.snapshot().wrapping_delta_since(before);
    assert_eq!(delta.network_single_mpdu_started, 1);
    assert_eq!(delta.network_single_legacy_rate, 0);
    assert_eq!(delta.network_single_block_ack_unavailable, 1);
    assert_eq!(delta.network_single_ht_needs_pair, 0);
    assert_eq!(delta.network_single_fresh_aggregate_capacity, 0);
    assert_eq!(
        delta.network_single_fresh_capacity_lifetime_max_ethernet_length,
        0
    );
    assert_eq!(delta.rate_selections, 1);
    assert_eq!(delta.last_bandwidth_mhz, 40);
    assert_eq!(delta.last_nominal_rate_kbps, 150_000);
    assert_eq!(delta.aggregates_prepared, 2);
    assert_eq!(delta.aggregate_publications, 2);
    assert_eq!(delta.aggregates_completed, 1);
    assert_eq!(delta.subframes_acknowledged, 31);
    assert_eq!(delta.individual_retries, 1);
    assert_eq!(delta.hardware_timeouts, 1);
    assert_eq!(delta.collisions, 0);
    assert_eq!(delta.prepared_subframe_total(), 34);
    assert_eq!(delta.prepared_in_range(1, 1), 0);
    assert_eq!(delta.prepared_in_range(2, 3), 1);
    assert_eq!(delta.prepared_in_range(32, 32), 1);
    assert_eq!(delta.minimum_prepared_subframes(), Some(2));
    assert_eq!(delta.maximum_prepared_subframes(), Some(32));
    assert_eq!(delta.preparation_micros, 18);
    assert_eq!(delta.preparation_lifetime_max_micros, 11);
    assert_eq!(delta.publication_program_micros, 8);
    assert_eq!(delta.publication_program_lifetime_max_micros, 5);
    assert_eq!(delta.exchange_micros, 171);
    assert_eq!(delta.exchange_lifetime_max_micros, 71);
    assert_eq!(delta.single_publication_exchanges, 1);
    assert_eq!(delta.single_publication_exchange_micros, 41);
    assert_eq!(delta.single_publication_exchange_lifetime_max_micros, 41);
    assert_eq!(delta.retried_exchanges, 2);
    assert_eq!(delta.retried_exchange_publications, 7);
    assert_eq!(delta.retried_exchange_micros, 130);
    assert_eq!(delta.retried_exchange_lifetime_max_micros, 71);
    assert_eq!(delta.exchanges_by_publications[1], 1);
    assert_eq!(delta.exchange_micros_by_publications[1], 41);
    assert_eq!(delta.exchange_lifetime_max_micros_by_publications[1], 41);
    assert_eq!(delta.exchanges_by_publications[2], 0);
    assert_eq!(delta.exchanges_by_publications[3], 1);
    assert_eq!(delta.exchange_micros_by_publications[3], 59);
    assert_eq!(delta.exchange_lifetime_max_micros_by_publications[3], 59);
    assert_eq!(delta.exchanges_by_publications[4], 1);
    assert_eq!(delta.exchange_micros_by_publications[4], 71);
    assert_eq!(delta.exchange_lifetime_max_micros_by_publications[4], 71);
    assert_eq!(delta.tx_irq_epochs, 1);
    assert_eq!(delta.tx_irq_service_samples, 1);
    assert_eq!(delta.tx_irq_clock_skew_samples, 0);
    assert_eq!(delta.tx_irq_to_service_micros, 9);
    assert_eq!(delta.tx_irq_to_service_lifetime_max_micros, 9);
    assert_eq!(delta.tx_publication_to_irq_samples, 1);
    assert_eq!(delta.tx_publication_to_irq_micros, 5);
    assert_eq!(delta.tx_publication_to_irq_lifetime_max_micros, 5);
    assert_eq!(delta.stopped_at_frame_limit, 1);
    assert_eq!(delta.stopped_at_capacity_limit, 0);
    assert_eq!(delta.stopped_on_empty_queue, 1);
}

#[test]
fn tx_irq_clock_is_read_only_for_sampled_epochs() {
    use core::cell::Cell;

    let counters = AggregateTxCounters::new();
    let reads = Cell::new(0_u32);
    for _ in 0..64 {
        counters.record_tx_irq_epoch(|| {
            reads.set(reads.get() + 1);
            100
        });
    }

    assert_eq!(reads.get(), 1);
    assert_eq!(counters.snapshot().tx_irq_epochs, 64);
}

#[test]
fn terminal_completion_is_correlated_with_the_next_publication() {
    let counters = AggregateTxCounters::with_clock(|| 100);
    let before = counters.snapshot();

    counters.observe(AggregateTxObservation::Completed {
        acknowledged: 16,
        individual_retry: false,
    });
    counters.observe(AggregateTxObservation::PreparedSchedulerPhase {
        phase: PreparedTxSchedulerPhase::ActiveServiceReturned,
        at_micros: 110,
    });
    counters.observe(AggregateTxObservation::PreparedSchedulerPhase {
        phase: PreparedTxSchedulerPhase::SchedulerLoopResumed,
        at_micros: 125,
    });
    counters.observe(AggregateTxObservation::PreparedSchedulerPhase {
        phase: PreparedTxSchedulerPhase::StopPollCompleted,
        at_micros: 130,
    });
    counters.observe(AggregateTxObservation::PreparedSchedulerPhase {
        phase: PreparedTxSchedulerPhase::ControlReadinessChecked { ready: false },
        at_micros: 145,
    });
    counters.observe(AggregateTxObservation::PreparedSchedulerPhase {
        phase: PreparedTxSchedulerPhase::PreparedReadinessChecked,
        at_micros: 150,
    });
    counters.observe(AggregateTxObservation::PreparedSchedulerPhase {
        phase: PreparedTxSchedulerPhase::PreparedBatchChecked,
        at_micros: 155,
    });
    counters.observe(AggregateTxObservation::PreparedSchedulerPhase {
        phase: PreparedTxSchedulerPhase::PreparedEntry,
        at_micros: 160,
    });
    counters.observe(AggregateTxObservation::Published {
        at_micros: 175,
        program_micros: 4,
    });

    let delta = counters.snapshot().wrapping_delta_since(before);
    assert_eq!(delta.completion_to_publication_samples, 1);
    assert_eq!(delta.completion_to_publication_micros, 75);
    assert_eq!(delta.completion_to_publication_lifetime_max_micros, 75);
    assert_eq!(delta.completion_to_prepared_entry_samples, 1);
    assert_eq!(delta.completion_to_prepared_entry_micros, 60);
    assert_eq!(delta.prepared_entry_to_publication_samples, 1);
    assert_eq!(delta.prepared_entry_to_publication_micros, 15);
    assert_eq!(delta.prepared_scheduler_timing.samples, 1);
    assert_eq!(delta.prepared_scheduler_timing.scheduler_passes, 1);
    assert_eq!(delta.prepared_scheduler_timing.control_ready_passes, 0);
    assert_eq!(
        delta
            .prepared_scheduler_timing
            .completion_to_active_service_return
            .micros,
        10
    );
    assert_eq!(
        delta
            .prepared_scheduler_timing
            .active_service_return_to_first_pass
            .micros,
        15
    );
    assert_eq!(delta.prepared_scheduler_timing.multi_pass_samples, 0);
    assert_eq!(delta.prepared_scheduler_timing.additional_passes.micros, 0);
    assert_eq!(delta.prepared_scheduler_timing.stop_poll.micros, 5);
    assert_eq!(delta.prepared_scheduler_timing.control_readiness.micros, 15);
    assert_eq!(
        delta
            .prepared_scheduler_timing
            .control_check_to_prepared_entry
            .micros,
        15
    );
    assert_eq!(
        delta
            .prepared_scheduler_timing
            .control_check_to_prepared_readiness
            .micros,
        5
    );
    assert_eq!(
        delta
            .prepared_scheduler_timing
            .prepared_readiness_to_batch
            .micros,
        5
    );
    assert_eq!(
        delta
            .prepared_scheduler_timing
            .prepared_batch_to_entry
            .micros,
        5
    );
}

#[test]
fn interval_boundary_drops_stale_timing_and_ap_sequence_correlations() {
    fn ap_udp_frame(sequence: u32) -> [u8; 46] {
        let mut ethernet = [0_u8; 46];
        ethernet[14] = 0x45;
        ethernet[23] = 17;
        ethernet[34..36].copy_from_slice(&4_324_u16.to_be_bytes());
        ethernet[42..46].copy_from_slice(&sequence.to_be_bytes());
        ethernet
    }

    let counters = AggregateTxCounters::with_clock(|| 100);
    counters.observe(AggregateTxObservation::Completed {
        acknowledged: 32,
        individual_retry: false,
    });
    counters.observe_access_point_network_claim(&ap_udp_frame(7));

    counters.begin_interval();
    let before = counters.snapshot();
    counters.observe(AggregateTxObservation::Published {
        at_micros: 1_000_000,
        program_micros: 4,
    });
    counters.observe_access_point_network_claim(&ap_udp_frame(0));
    counters.observe_access_point_network_claim(&ap_udp_frame(1));

    let delta = counters.snapshot().wrapping_delta_since(before);
    assert_eq!(delta.completion_to_publication_samples, 0);
    assert_eq!(delta.completion_to_publication_micros, 0);
    assert_eq!(delta.ap_udp_claimed, 2);
    assert_eq!(delta.ap_udp_claim_backward, 0);
    assert_eq!(delta.ap_udp_claim_first_previous, u32::MAX);
    assert_eq!(delta.ap_udp_claim_first_sequence, u32::MAX);
    assert_eq!(delta.ap_udp_claim_maximum_distance, 0);
}

#[test]
fn scheduler_trace_recorder_preserves_detours_and_resets_at_terminal_service() {
    let trace = PreparedTxSchedulerTraceRecorder::new();
    trace.record(PreparedTxSchedulerPhase::ActiveServiceReturned, 10);
    trace.record(PreparedTxSchedulerPhase::SchedulerLoopResumed, 20);
    trace.record(
        PreparedTxSchedulerPhase::ControlReadinessChecked { ready: true },
        25,
    );
    trace.record(PreparedTxSchedulerPhase::ActiveServiceReturned, 30);
    trace.record(PreparedTxSchedulerPhase::SchedulerLoopResumed, 40);
    trace.record(PreparedTxSchedulerPhase::StopPollCompleted, 45);
    trace.record(
        PreparedTxSchedulerPhase::ControlReadinessChecked { ready: false },
        50,
    );
    trace.record(PreparedTxSchedulerPhase::PreparedReadinessChecked, 51);
    trace.record(PreparedTxSchedulerPhase::PreparedBatchChecked, 53);
    trace.record(PreparedTxSchedulerPhase::PreparedEntry, 55);

    assert_eq!(
        trace.take(),
        Some(PreparedTxSchedulerTrace {
            active_service_returned_micros: 30,
            first_scheduler_loop_resumed_micros: 40,
            scheduler_loop_resumed_micros: 40,
            stop_poll_completed_micros: 45,
            control_readiness_checked_micros: 50,
            prepared_readiness_checked_micros: 51,
            prepared_batch_checked_micros: 53,
            prepared_entry_micros: 55,
            scheduler_passes: 1,
            control_ready_passes: 0,
        })
    );
    assert_eq!(trace.take(), None);
}

#[test]
fn incomplete_scheduler_trace_cannot_become_a_timing_sample() {
    let trace = PreparedTxSchedulerTraceRecorder::new();
    trace.record(PreparedTxSchedulerPhase::ActiveServiceReturned, 10);
    trace.record(PreparedTxSchedulerPhase::PreparedEntry, 45);

    assert_eq!(trace.take(), None);
}
#[test]
fn ap_retention_losses_survive_traffic_windows_and_reset_only_with_the_ap_epoch() {
    use oer_esp32s31_ieee80211_runtime::diagnostics::aggregate_tx::NetworkTxRetentionDropReason;
    let counters = AggregateTxCounters::new();
    counters
        .observe_access_point_retention_drop(NetworkTxRetentionDropReason::UnicastPowerSaveFull);
    counters.begin_interval();
    counters.observe_access_point_retention_drop(NetworkTxRetentionDropReason::GroupPowerSaveFull);
    counters.observe_access_point_retention_drop(NetworkTxRetentionDropReason::ActiveQueueFull);
    counters.begin_interval();
    let evidence = counters.tx_retention.snapshot();
    assert_eq!(evidence.unicast_power_save_full, 1);
    assert_eq!(evidence.group_power_save_full, 1);
    assert_eq!(evidence.active_queue_full, 1);
    assert!(evidence.has_drops());
    counters.tx_retention.reset();
    assert!(!counters.tx_retention.snapshot().has_drops());
}

#[test]
fn submitted_work_is_separate_from_delivery_and_exchange_time() {
    let counters = AggregateTxCounters::new();
    let before = counters.snapshot();
    let mut work = oer_esp32s31_ieee80211_runtime::diagnostics::aggregate_tx::MacTxWork::default();
    work.record(3_000, 3, core::num::NonZeroU32::new(150_000));
    work.record(1_000, 1, None);
    counters.observe(AggregateTxObservation::WorkCompleted { work });
    let delta = counters.snapshot().wrapping_delta_since(before);
    assert_eq!(delta.work.exchanges, 1);
    assert_eq!(delta.work.publications, 2);
    assert_eq!(delta.work.psdu_bytes, 4_000);
    assert_eq!(delta.work.mpdus, 4);
    assert_eq!(delta.work.nominal_data_micros, 160);
    assert_eq!(delta.work.unestimated_publications, 1);
    assert_eq!(delta.work.saturated_exchanges, 0);
    assert_eq!(delta.exchange_micros, 0);
    assert_eq!(delta.subframes_acknowledged, 0);
    let next = counters.snapshot();
    assert_eq!(
        next.wrapping_delta_since(next).work,
        AggregateTxWorkSnapshot::default()
    );
}

#[test]
fn ordinary_receipts_do_not_recharge_aggregate_work() {
    let counters = AggregateTxCounters::new();
    let before = counters.snapshot();
    let mut work = oer_esp32s31_ieee80211_runtime::diagnostics::aggregate_tx::MacTxWork::new();
    work.record_publication(
        1000,
        1,
        core::num::NonZeroU32::new(54000),
        None,
        Some(oer_esp32s31_ieee80211_mac::tx::TxContention {
            aifsn: 3,
            backoff_slots: 7,
        }),
    );
    counters.observe(AggregateTxObservation::OrdinaryWorkCompleted { work });
    let after = counters.snapshot();
    let delta = after.wrapping_delta_since(before);
    assert_eq!(delta.ordinary_work.exchanges, 1);
    assert_eq!(delta.ordinary_work.publications, 1);
    assert_eq!(delta.ordinary_work.psdu_bytes, 1000);
    assert_eq!(delta.ordinary_work.aifs_slots, 3);
    assert_eq!(delta.ordinary_work.backoff_slots, 7);
    assert_eq!(delta.ordinary_work.unreported_contention, 0);
    assert_eq!(delta.work, Default::default());
    assert_eq!(
        after.wrapping_delta_since(after).ordinary_work,
        Default::default()
    );
}

#[test]
fn publication_balance_tracks_attempts_across_live_interval_boundaries() {
    let counters = AggregateTxCounters::new();
    counters.record_publication(1, 1);
    let start = counters.snapshot();
    counters.begin_interval();
    counters.observe(AggregateTxObservation::BlockAckProcessed {
        tx_status: 0,
        block_ack_received: true,
        control: 0,
        first_sequence: SequenceNumber::new(0).unwrap(),
        starting_sequence: SequenceNumber::new(0).unwrap(),
        subframes: 2,
        missing_original_indices: 0b10,
        block_ack_snr_db: Some(30),
    });
    counters.record_publication(2, 1); // Retry is a new publication.
    counters.record_hardware_timeout();
    counters.record_publication(3, 1); // Remains in flight at the end.
    let end = counters.snapshot();
    let interval = end.wrapping_delta_since(start);
    assert_eq!(interval.publications_pending_start, 1);
    assert_eq!(interval.publications_pending_end, 1);
    assert_eq!(interval.aggregate_publications, 2);
    assert_eq!(interval.block_ack_samples, 1);
    assert_eq!(interval.hardware_timeouts, 1);
    counters.record_collision();
    let next = counters.snapshot().wrapping_delta_since(end);
    assert_eq!(next.publications_pending_start, 1);
    assert_eq!(next.publications_pending_end, 0);
    assert_eq!(next.aggregate_publications, 0);
    assert_eq!(next.collisions, 1);
}

#[test]
fn prepared_standby_owners_survive_interval_reset_until_publish_or_cancel() {
    let counters = AggregateTxCounters::new();
    counters.observe(AggregateTxObservation::StandbyPrepared);
    let start = counters.snapshot();
    counters.begin_interval();
    counters.observe(AggregateTxObservation::StandbyPublished);
    counters.observe(AggregateTxObservation::StandbyPrepared);
    let end = counters.snapshot();
    let delta = end.wrapping_delta_since(start);
    assert_eq!(delta.standby_pending_start, 1);
    assert_eq!(delta.standby_pending_end, 1);
    assert_eq!(delta.standby_prepared, 1);
    assert_eq!(delta.standby_published, 1);
    counters.observe(AggregateTxObservation::StandbyCancelled);
    let delta = counters.snapshot().wrapping_delta_since(end);
    assert_eq!(delta.standby_pending_start, 1);
    assert_eq!(delta.standby_pending_end, 0);
    assert_eq!(delta.standby_cancelled, 1);
}

#[test]
fn a_scheduler_detour_is_not_charged_to_return_latency() {
    let counters = AggregateTxCounters::with_clock(|| 100);
    let before = counters.snapshot();
    counters.observe(AggregateTxObservation::Completed {
        acknowledged: 32,
        individual_retry: false,
    });
    let phases = [
        (PreparedTxSchedulerPhase::ActiveServiceReturned, 110),
        // An RX detour: the first pass leaves the chain.
        (PreparedTxSchedulerPhase::SchedulerLoopResumed, 115),
        (PreparedTxSchedulerPhase::SchedulerLoopResumed, 715),
        (PreparedTxSchedulerPhase::StopPollCompleted, 716),
        (
            PreparedTxSchedulerPhase::ControlReadinessChecked { ready: false },
            718,
        ),
        (PreparedTxSchedulerPhase::PreparedReadinessChecked, 720),
        (PreparedTxSchedulerPhase::PreparedBatchChecked, 725),
        (PreparedTxSchedulerPhase::PreparedEntry, 730),
    ];
    for (phase, at_micros) in phases {
        counters.observe(AggregateTxObservation::PreparedSchedulerPhase { phase, at_micros });
    }
    counters.observe(AggregateTxObservation::Published {
        at_micros: 740,
        program_micros: 4,
    });
    let timing = counters
        .snapshot()
        .wrapping_delta_since(before)
        .prepared_scheduler_timing;
    assert_eq!(timing.samples, 1);
    assert_eq!(timing.scheduler_passes, 2);
    assert_eq!(timing.active_service_return_to_first_pass.micros, 5);
    assert_eq!(timing.multi_pass_samples, 1);
    assert_eq!(timing.additional_passes.micros, 600);
    assert_eq!(timing.stop_poll.micros, 1);
    let delta = counters.snapshot().wrapping_delta_since(before);
    assert_eq!(
        delta.completion_to_publication_histogram,
        [0, 0, 0, 0, 1, 0]
    );
    assert_eq!(delta.unprepared_publication_samples, 0);
}

#[test]
fn publication_latency_is_bucketed_and_unprepared_successors_are_counted() {
    let counters = AggregateTxCounters::with_clock(|| 100);
    let before = counters.snapshot();
    counters.observe(AggregateTxObservation::Completed {
        acknowledged: 32,
        individual_retry: false,
    });
    // No prepared entry: the scheduler claimed the successor itself.
    counters.observe(AggregateTxObservation::PreparedSchedulerPhase {
        phase: PreparedTxSchedulerPhase::ActiveServiceReturned,
        at_micros: 110,
    });
    counters.observe(AggregateTxObservation::Published {
        at_micros: 2_600,
        program_micros: 4,
    });
    let delta = counters.snapshot().wrapping_delta_since(before);
    assert_eq!(
        delta.completion_to_publication_histogram,
        [0, 0, 0, 0, 0, 1]
    );
    assert_eq!(delta.unprepared_publication_samples, 1);
    assert_eq!(delta.unprepared_publication_micros, 2_500);
    assert_eq!(delta.prepared_scheduler_timing.samples, 0);
}

#[test]
fn partial_block_acks_are_attributed_to_original_positions() {
    let counters = AggregateTxCounters::new();
    let before = counters.snapshot();
    let block_ack =
        |subframes, missing_original_indices| AggregateTxObservation::BlockAckProcessed {
            tx_status: 0,
            block_ack_received: true,
            control: 0,
            first_sequence: SequenceNumber::new(0).unwrap(),
            starting_sequence: SequenceNumber::new(0).unwrap(),
            subframes,
            missing_original_indices,
            block_ack_snr_db: Some(30),
        };
    // First publication loses positions 3 and 31; its compacted retry then
    // loses original position 31 again.
    counters.observe(block_ack(32, 1 << 3 | 1 << 31));
    counters.observe(block_ack(2, 1 << 31));
    // Neither a full nor an empty BlockAck is attributed.
    counters.observe(block_ack(32, 0));
    counters.observe(block_ack(2, 0b11));
    let delta = counters.snapshot().wrapping_delta_since(before);
    assert_eq!(delta.partial_block_ack, 2);
    assert_eq!(delta.full_block_ack, 1);
    assert_eq!(delta.empty_block_ack, 1);
    assert_eq!(delta.partial_missing_by_position[3], 1);
    assert_eq!(delta.partial_missing_by_position[31], 2);
    assert_eq!(delta.partial_missing_by_position.iter().sum::<u32>(), 3);
    assert_eq!(delta.partial_missing_counts, [1, 1, 0, 0, 0]);
}

#[test]
fn block_ack_snr_is_classified_by_acknowledgement_outcome() {
    let counters = AggregateTxCounters::new();
    let before = counters.snapshot();
    let block_ack = |block_ack_received, missing_original_indices, block_ack_snr_db| {
        AggregateTxObservation::BlockAckProcessed {
            tx_status: 0,
            block_ack_received,
            control: 0,
            first_sequence: SequenceNumber::new(0).unwrap(),
            starting_sequence: SequenceNumber::new(0).unwrap(),
            subframes: 32,
            missing_original_indices,
            block_ack_snr_db,
        }
    };
    counters.observe(block_ack(true, 0, Some(41)));
    counters.observe(block_ack(true, 0, Some(44)));
    counters.observe(block_ack(true, 0, Some(12)));
    counters.observe(block_ack(true, 1 << 5, Some(21)));
    // A success without a received BlockAck carries no response SNR.
    counters.observe(block_ack(false, 1 << 5, Some(50)));
    counters.observe(block_ack(true, 0, None));
    // An empty BlockAck has no acknowledged subframe to attribute.
    counters.observe(block_ack(true, u32::MAX, Some(35)));
    let delta = counters.snapshot().wrapping_delta_since(before);
    assert_eq!(delta.full_block_ack_snr.samples, 3);
    assert_eq!(delta.full_block_ack_snr.mean_decidb(), Some(323));
    assert_eq!(delta.full_block_ack_snr.histogram[0], 1);
    assert_eq!(delta.full_block_ack_snr.histogram[6], 1);
    assert_eq!(delta.full_block_ack_snr.histogram[7], 1);
    assert_eq!(delta.partial_block_ack_snr.samples, 1);
    assert_eq!(delta.partial_block_ack_snr.mean_decidb(), Some(210));
    assert_eq!(delta.partial_block_ack_snr.histogram[1], 1);
    assert_eq!(delta.block_ack_snr_unavailable, 2);
}

#[test]
fn block_ack_snr_extremes_fall_into_open_buckets() {
    let counters = AggregateTxCounters::new();
    let before = counters.snapshot();
    for snr_db in [i8::MIN, i8::MAX] {
        counters.observe(AggregateTxObservation::BlockAckProcessed {
            tx_status: 0,
            block_ack_received: true,
            control: 0,
            first_sequence: SequenceNumber::new(0).unwrap(),
            starting_sequence: SequenceNumber::new(0).unwrap(),
            subframes: 2,
            missing_original_indices: 0,
            block_ack_snr_db: Some(snr_db),
        });
    }
    let snr = counters
        .snapshot()
        .wrapping_delta_since(before)
        .full_block_ack_snr;
    assert_eq!(snr.histogram[0], 1);
    assert_eq!(snr.histogram[BLOCK_ACK_SNR_BUCKETS - 1], 1);
    assert_eq!(snr.mean_decidb(), Some(-5));
}
