//! Lock-free ESP32-S31 HIL observations of connected aggregate TX.
//!
//! These counters do not participate in scheduling, retry policy or DMA
//! ownership. The production adapter publishes value-only events; this module
//! selects the histogram, atomics and interval snapshot used by qualification.

mod ordinary;
mod terminal;
pub use ordinary::StationOrdinarySnapshot;
mod work;
pub use work::AggregateTxWorkSnapshot;
use work::WorkCounters;

use core::sync::atomic::{AtomicU32, Ordering};

use oer_esp32s31_ieee80211_runtime::diagnostics::aggregate_tx::{
    AggregateBuildStop, AggregateTxObservation, AggregateTxObserver, NetworkSingleMpduReason,
    PreparedTxSchedulerPhase,
};

/// Number of histogram entries required for all currently legal A-MPDU sizes.
///
/// Index zero is deliberately unused; indexes `1..=32` are the number of
/// original MPDUs prepared for one aggregate exchange.
pub const AGGREGATE_TX_HISTOGRAM_BUCKETS: usize = 33;

/// Per-exchange timing buckets for one through three publications and the
/// terminal `4+` retry-policy bucket used by qualification.
pub const AGGREGATE_TX_PUBLICATION_BUCKETS: usize = 5;

/// Upper bounds of the completion-to-publication histogram buckets. The
/// last bucket counts every sample above the final bound.
pub const COMPLETION_TO_PUBLICATION_BOUNDS_MICROS: [u32; 5] = [50, 100, 200, 500, 1_000];
pub const COMPLETION_TO_PUBLICATION_BUCKETS: usize =
    COMPLETION_TO_PUBLICATION_BOUNDS_MICROS.len() + 1;

fn completion_to_publication_bucket(micros: u32) -> usize {
    COMPLETION_TO_PUBLICATION_BOUNDS_MICROS
        .iter()
        .position(|bound| micros <= *bound)
        .unwrap_or(COMPLETION_TO_PUBLICATION_BOUNDS_MICROS.len())
}

/// Original positions of one hardware BlockAck window.
pub const BLOCK_ACK_POSITIONS: usize = 32;
pub const PARTIAL_MISSING_BUCKETS: usize = 5;

fn partial_missing_bucket(missing: u8) -> usize {
    match missing {
        0 | 1 => 0,
        2 => 1,
        3..=4 => 2,
        5..=8 => 3,
        _ => 4,
    }
}

/// Lower bound of the second BlockAck SNR bucket. The first bucket counts
/// every sample below it and the last every sample at or above
/// `BLOCK_ACK_SNR_FLOOR_DB + (BLOCK_ACK_SNR_BUCKETS - 2) * BLOCK_ACK_SNR_STEP_DB`.
pub const BLOCK_ACK_SNR_FLOOR_DB: i8 = 20;
pub const BLOCK_ACK_SNR_STEP_DB: i8 = 4;
pub const BLOCK_ACK_SNR_BUCKETS: usize = 12;

fn block_ack_snr_bucket(snr_db: i8) -> usize {
    if snr_db < BLOCK_ACK_SNR_FLOOR_DB {
        return 0;
    }
    let step = (snr_db - BLOCK_ACK_SNR_FLOOR_DB) / BLOCK_ACK_SNR_STEP_DB;
    (step as usize + 1).min(BLOCK_ACK_SNR_BUCKETS - 1)
}

/// SNR of the responses that completed one class of aggregate publications.
struct BlockAckSnrCounters {
    samples: AtomicU32,
    /// Wrapping two's-complement sum of the signed samples.
    sum_db: AtomicU32,
    histogram: [AtomicU32; BLOCK_ACK_SNR_BUCKETS],
}

impl BlockAckSnrCounters {
    const fn new() -> Self {
        Self {
            samples: AtomicU32::new(0),
            sum_db: AtomicU32::new(0),
            histogram: [const { AtomicU32::new(0) }; BLOCK_ACK_SNR_BUCKETS],
        }
    }

    fn record(&self, snr_db: i8) {
        self.samples.fetch_add(1, Ordering::Relaxed);
        self.sum_db
            .fetch_add(i32::from(snr_db) as u32, Ordering::Relaxed);
        self.histogram[block_ack_snr_bucket(snr_db)].fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> BlockAckSnrSnapshot {
        BlockAckSnrSnapshot {
            samples: self.samples.load(Ordering::Relaxed),
            sum_db: self.sum_db.load(Ordering::Relaxed) as i32,
            histogram: core::array::from_fn(|bucket| {
                self.histogram[bucket].load(Ordering::Relaxed)
            }),
        }
    }
}

/// SNR distribution of one BlockAck class over an observation interval.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BlockAckSnrSnapshot {
    pub samples: u32,
    pub sum_db: i32,
    /// Samples below [`BLOCK_ACK_SNR_FLOOR_DB`], then one bucket per
    /// [`BLOCK_ACK_SNR_STEP_DB`], with the last bucket open-ended.
    pub histogram: [u32; BLOCK_ACK_SNR_BUCKETS],
}

impl BlockAckSnrSnapshot {
    /// Mean SNR in tenths of a decibel, when at least one sample exists.
    pub fn mean_decidb(&self) -> Option<i32> {
        (self.samples != 0).then(|| {
            let samples = i64::from(self.samples);
            (i64::from(self.sum_db) * 10 / samples) as i32
        })
    }

    fn delta(&self, earlier: &Self) -> Self {
        Self {
            samples: self.samples.wrapping_sub(earlier.samples),
            sum_db: self.sum_db.wrapping_sub(earlier.sum_db),
            histogram: core::array::from_fn(|bucket| {
                self.histogram[bucket].wrapping_sub(earlier.histogram[bucket])
            }),
        }
    }
}

struct PhaseTimingCounters {
    micros: AtomicU32,
    lifetime_max_micros: AtomicU32,
}

impl PhaseTimingCounters {
    const fn new() -> Self {
        Self {
            micros: AtomicU32::new(0),
            lifetime_max_micros: AtomicU32::new(0),
        }
    }

    fn record(&self, micros: u32) {
        self.micros.fetch_add(micros, Ordering::Relaxed);
        self.lifetime_max_micros
            .fetch_max(micros, Ordering::Relaxed);
    }

    fn snapshot(&self) -> PhaseTimingSnapshot {
        PhaseTimingSnapshot {
            micros: self.micros.load(Ordering::Relaxed),
            lifetime_max_micros: self.lifetime_max_micros.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PreparedTxSchedulerTrace {
    active_service_returned_micros: u64,
    /// The first scheduler pass after the active service returned.
    first_scheduler_loop_resumed_micros: u64,
    /// The pass that reached the prepared entry.
    scheduler_loop_resumed_micros: u64,
    stop_poll_completed_micros: u64,
    control_readiness_checked_micros: u64,
    prepared_readiness_checked_micros: u64,
    prepared_batch_checked_micros: u64,
    prepared_entry_micros: u64,
    scheduler_passes: u8,
    control_ready_passes: u8,
}

/// Observer-owned assembly state for one prepared-publication trace.
///
/// The HIL observer already lives in internal SRAM. Keeping this state here
/// leaves the safe production adapter value-only and avoids growing its
/// affine TX owner or async frame.
struct PreparedTxSchedulerTraceRecorder {
    active_service_returned_micros: AtomicU32,
    first_scheduler_loop_resumed_micros: AtomicU32,
    scheduler_loop_resumed_micros: AtomicU32,
    stop_poll_completed_micros: AtomicU32,
    control_readiness_checked_micros: AtomicU32,
    prepared_readiness_checked_micros: AtomicU32,
    prepared_batch_checked_micros: AtomicU32,
    prepared_entry_micros: AtomicU32,
    scheduler_passes: AtomicU32,
    control_ready_passes: AtomicU32,
}

impl PreparedTxSchedulerTraceRecorder {
    const VALID: u32 = 1 << 31;
    const TIME_MASK: u32 = Self::VALID - 1;

    const fn new() -> Self {
        Self {
            active_service_returned_micros: AtomicU32::new(0),
            first_scheduler_loop_resumed_micros: AtomicU32::new(0),
            scheduler_loop_resumed_micros: AtomicU32::new(0),
            stop_poll_completed_micros: AtomicU32::new(0),
            control_readiness_checked_micros: AtomicU32::new(0),
            prepared_readiness_checked_micros: AtomicU32::new(0),
            prepared_batch_checked_micros: AtomicU32::new(0),
            prepared_entry_micros: AtomicU32::new(0),
            scheduler_passes: AtomicU32::new(0),
            control_ready_passes: AtomicU32::new(0),
        }
    }

    fn reset(&self) {
        self.active_service_returned_micros
            .store(0, Ordering::Relaxed);
        self.first_scheduler_loop_resumed_micros
            .store(0, Ordering::Relaxed);
        self.scheduler_loop_resumed_micros
            .store(0, Ordering::Relaxed);
        self.stop_poll_completed_micros.store(0, Ordering::Relaxed);
        self.control_readiness_checked_micros
            .store(0, Ordering::Relaxed);
        self.prepared_readiness_checked_micros
            .store(0, Ordering::Relaxed);
        self.prepared_batch_checked_micros
            .store(0, Ordering::Relaxed);
        self.prepared_entry_micros.store(0, Ordering::Relaxed);
        self.scheduler_passes.store(0, Ordering::Relaxed);
        self.control_ready_passes.store(0, Ordering::Relaxed);
    }

    fn record(&self, phase: PreparedTxSchedulerPhase, at_micros: u64) {
        let timestamp = Self::VALID | at_micros as u32 & Self::TIME_MASK;
        match phase {
            PreparedTxSchedulerPhase::ActiveServiceReturned => {
                self.reset();
                self.active_service_returned_micros
                    .store(timestamp, Ordering::Relaxed);
            }
            PreparedTxSchedulerPhase::SchedulerLoopResumed => {
                // Only the observer thread writes this recorder, so the
                // first pass is simply the first write after the reset.
                if self
                    .first_scheduler_loop_resumed_micros
                    .load(Ordering::Relaxed)
                    == 0
                {
                    self.first_scheduler_loop_resumed_micros
                        .store(timestamp, Ordering::Relaxed);
                }
                self.scheduler_loop_resumed_micros
                    .store(timestamp, Ordering::Relaxed);
                self.scheduler_passes.fetch_add(1, Ordering::Relaxed);
            }
            PreparedTxSchedulerPhase::StopPollCompleted => {
                self.stop_poll_completed_micros
                    .store(timestamp, Ordering::Relaxed);
            }
            PreparedTxSchedulerPhase::ControlReadinessChecked { ready } => {
                self.control_readiness_checked_micros
                    .store(timestamp, Ordering::Relaxed);
                self.control_ready_passes
                    .fetch_add(u32::from(ready), Ordering::Relaxed);
            }
            PreparedTxSchedulerPhase::PreparedReadinessChecked => {
                self.prepared_readiness_checked_micros
                    .store(timestamp, Ordering::Relaxed);
            }
            PreparedTxSchedulerPhase::PreparedBatchChecked => {
                self.prepared_batch_checked_micros
                    .store(timestamp, Ordering::Relaxed);
            }
            PreparedTxSchedulerPhase::PreparedEntry => {
                self.prepared_entry_micros
                    .store(timestamp, Ordering::Relaxed);
            }
        }
    }

    fn take(&self) -> Option<PreparedTxSchedulerTrace> {
        let take_time = |value: &AtomicU32| {
            let timestamp = value.swap(0, Ordering::Relaxed);
            (timestamp & Self::VALID != 0).then_some(u64::from(timestamp & Self::TIME_MASK))
        };
        let active_service_returned_micros = take_time(&self.active_service_returned_micros);
        let first_scheduler_loop_resumed_micros =
            take_time(&self.first_scheduler_loop_resumed_micros);
        let scheduler_loop_resumed_micros = take_time(&self.scheduler_loop_resumed_micros);
        let stop_poll_completed_micros = take_time(&self.stop_poll_completed_micros);
        let control_readiness_checked_micros = take_time(&self.control_readiness_checked_micros);
        let prepared_readiness_checked_micros = take_time(&self.prepared_readiness_checked_micros);
        let prepared_batch_checked_micros = take_time(&self.prepared_batch_checked_micros);
        let prepared_entry_micros = take_time(&self.prepared_entry_micros);
        let scheduler_passes = self.scheduler_passes.swap(0, Ordering::Relaxed);
        let control_ready_passes = self.control_ready_passes.swap(0, Ordering::Relaxed);
        Some(PreparedTxSchedulerTrace {
            active_service_returned_micros: active_service_returned_micros?,
            first_scheduler_loop_resumed_micros: first_scheduler_loop_resumed_micros?,
            scheduler_loop_resumed_micros: scheduler_loop_resumed_micros?,
            stop_poll_completed_micros: stop_poll_completed_micros?,
            control_readiness_checked_micros: control_readiness_checked_micros?,
            prepared_readiness_checked_micros: prepared_readiness_checked_micros?,
            prepared_batch_checked_micros: prepared_batch_checked_micros?,
            prepared_entry_micros: prepared_entry_micros?,
            scheduler_passes: u8::try_from(scheduler_passes).unwrap_or(u8::MAX),
            control_ready_passes: u8::try_from(control_ready_passes).unwrap_or(u8::MAX),
        })
    }
}

struct PreparedTxSchedulerTimingCounters {
    samples: AtomicU32,
    scheduler_passes: AtomicU32,
    scheduler_passes_lifetime_max: AtomicU32,
    control_ready_passes: AtomicU32,
    multi_pass_samples: AtomicU32,
    completion_to_active_service_return: PhaseTimingCounters,
    active_service_return_to_first_pass: PhaseTimingCounters,
    /// First pass to the entry pass, recorded only for multi-pass samples so
    /// rare RX/control detours do not masquerade as return latency.
    additional_passes: PhaseTimingCounters,
    stop_poll: PhaseTimingCounters,
    control_readiness: PhaseTimingCounters,
    control_check_to_prepared_readiness: PhaseTimingCounters,
    prepared_readiness_to_batch: PhaseTimingCounters,
    prepared_batch_to_entry: PhaseTimingCounters,
    control_check_to_prepared_entry: PhaseTimingCounters,
}

impl PreparedTxSchedulerTimingCounters {
    const fn new() -> Self {
        Self {
            samples: AtomicU32::new(0),
            scheduler_passes: AtomicU32::new(0),
            scheduler_passes_lifetime_max: AtomicU32::new(0),
            control_ready_passes: AtomicU32::new(0),
            multi_pass_samples: AtomicU32::new(0),
            completion_to_active_service_return: PhaseTimingCounters::new(),
            active_service_return_to_first_pass: PhaseTimingCounters::new(),
            additional_passes: PhaseTimingCounters::new(),
            stop_poll: PhaseTimingCounters::new(),
            control_readiness: PhaseTimingCounters::new(),
            control_check_to_prepared_readiness: PhaseTimingCounters::new(),
            prepared_readiness_to_batch: PhaseTimingCounters::new(),
            prepared_batch_to_entry: PhaseTimingCounters::new(),
            control_check_to_prepared_entry: PhaseTimingCounters::new(),
        }
    }

    fn elapsed(start_micros: u32, end_micros: u64) -> Option<u32> {
        let end = end_micros as u32 & AggregateTxCounters::IRQ_TIME_MASK;
        let elapsed = end.wrapping_sub(start_micros) & AggregateTxCounters::IRQ_TIME_MASK;
        (elapsed <= AggregateTxCounters::IRQ_TIME_MASK / 2).then_some(elapsed)
    }

    fn record(&self, completion_micros: u32, trace: PreparedTxSchedulerTrace) {
        let active_return =
            trace.active_service_returned_micros as u32 & AggregateTxCounters::IRQ_TIME_MASK;
        let first_scheduler_loop =
            trace.first_scheduler_loop_resumed_micros as u32 & AggregateTxCounters::IRQ_TIME_MASK;
        let scheduler_loop =
            trace.scheduler_loop_resumed_micros as u32 & AggregateTxCounters::IRQ_TIME_MASK;
        let stop_poll =
            trace.stop_poll_completed_micros as u32 & AggregateTxCounters::IRQ_TIME_MASK;
        let control_check =
            trace.control_readiness_checked_micros as u32 & AggregateTxCounters::IRQ_TIME_MASK;
        let prepared_readiness =
            trace.prepared_readiness_checked_micros as u32 & AggregateTxCounters::IRQ_TIME_MASK;
        let prepared_batch =
            trace.prepared_batch_checked_micros as u32 & AggregateTxCounters::IRQ_TIME_MASK;

        let Some(completion_to_active_return) =
            Self::elapsed(completion_micros, trace.active_service_returned_micros)
        else {
            return;
        };
        let Some(active_return_to_first_pass) =
            Self::elapsed(active_return, trace.first_scheduler_loop_resumed_micros)
        else {
            return;
        };
        let Some(additional_passes) =
            Self::elapsed(first_scheduler_loop, trace.scheduler_loop_resumed_micros)
        else {
            return;
        };
        let Some(stop_poll_micros) =
            Self::elapsed(scheduler_loop, trace.stop_poll_completed_micros)
        else {
            return;
        };
        let Some(control_readiness_micros) =
            Self::elapsed(stop_poll, trace.control_readiness_checked_micros)
        else {
            return;
        };
        let Some(control_to_prepared_readiness) =
            Self::elapsed(control_check, trace.prepared_readiness_checked_micros)
        else {
            return;
        };
        let Some(prepared_readiness_to_batch) =
            Self::elapsed(prepared_readiness, trace.prepared_batch_checked_micros)
        else {
            return;
        };
        let Some(prepared_batch_to_entry) =
            Self::elapsed(prepared_batch, trace.prepared_entry_micros)
        else {
            return;
        };
        let Some(control_to_entry) = Self::elapsed(control_check, trace.prepared_entry_micros)
        else {
            return;
        };

        self.samples.fetch_add(1, Ordering::Relaxed);
        self.scheduler_passes
            .fetch_add(u32::from(trace.scheduler_passes), Ordering::Relaxed);
        self.scheduler_passes_lifetime_max
            .fetch_max(u32::from(trace.scheduler_passes), Ordering::Relaxed);
        self.control_ready_passes
            .fetch_add(u32::from(trace.control_ready_passes), Ordering::Relaxed);
        self.completion_to_active_service_return
            .record(completion_to_active_return);
        self.active_service_return_to_first_pass
            .record(active_return_to_first_pass);
        if trace.scheduler_passes > 1 {
            self.multi_pass_samples.fetch_add(1, Ordering::Relaxed);
            self.additional_passes.record(additional_passes);
        }
        self.stop_poll.record(stop_poll_micros);
        self.control_readiness.record(control_readiness_micros);
        self.control_check_to_prepared_readiness
            .record(control_to_prepared_readiness);
        self.prepared_readiness_to_batch
            .record(prepared_readiness_to_batch);
        self.prepared_batch_to_entry.record(prepared_batch_to_entry);
        self.control_check_to_prepared_entry
            .record(control_to_entry);
    }

    fn snapshot(&self) -> PreparedTxSchedulerTimingSnapshot {
        PreparedTxSchedulerTimingSnapshot {
            samples: self.samples.load(Ordering::Relaxed),
            scheduler_passes: self.scheduler_passes.load(Ordering::Relaxed),
            scheduler_passes_lifetime_max: self
                .scheduler_passes_lifetime_max
                .load(Ordering::Relaxed),
            control_ready_passes: self.control_ready_passes.load(Ordering::Relaxed),
            multi_pass_samples: self.multi_pass_samples.load(Ordering::Relaxed),
            completion_to_active_service_return: self
                .completion_to_active_service_return
                .snapshot(),
            active_service_return_to_first_pass: self
                .active_service_return_to_first_pass
                .snapshot(),
            additional_passes: self.additional_passes.snapshot(),
            stop_poll: self.stop_poll.snapshot(),
            control_readiness: self.control_readiness.snapshot(),
            control_check_to_prepared_readiness: self
                .control_check_to_prepared_readiness
                .snapshot(),
            prepared_readiness_to_batch: self.prepared_readiness_to_batch.snapshot(),
            prepared_batch_to_entry: self.prepared_batch_to_entry.snapshot(),
            control_check_to_prepared_entry: self.control_check_to_prepared_entry.snapshot(),
        }
    }
}

/// Lock-free HIL observations of the production connected TX owner.
///
/// The counters are diagnostic only and never participate in scheduling.
/// Relaxed atomics keep a HIL observer from adding synchronization to the
/// radio path it is measuring.
pub struct AggregateTxCounters {
    terminal: terminal::Counters,
    pub secondary_socket: crate::tx_progress::Counters,
    secondary_claim: crate::tx_progress::Counters,
    pub tx_retention: crate::tx_retention::TxRetentionCounters,
    #[cfg(feature = "tx-wait-probe")]
    pub wait_trace: crate::tx_wait::Trace,
    now_micros: fn() -> u64,
    ap_udp_claim_highest: AtomicU32,
    ap_udp_claimed: AtomicU32,
    ap_udp_claim_backward: AtomicU32,
    ap_udp_claim_first_previous: AtomicU32,
    ap_udp_claim_first_sequence: AtomicU32,
    ap_udp_claim_maximum_distance: AtomicU32,
    block_ack_operational_tids: AtomicU32,
    block_ack_operational_transitions: AtomicU32,
    network_single_mpdu_started: AtomicU32,
    network_single_legacy_rate: AtomicU32,
    network_single_block_ack_unavailable: AtomicU32,
    network_single_ht_needs_pair: AtomicU32,
    network_single_fresh_aggregate_capacity: AtomicU32,
    network_single_fresh_capacity_lifetime_max_ethernet_length: AtomicU32,
    rate_selections: AtomicU32,
    last_bandwidth_mhz: AtomicU32,
    last_nominal_rate_kbps: AtomicU32,
    aggregates_prepared: AtomicU32,
    standby_prepared: AtomicU32,
    standby_published: AtomicU32,
    standby_cancelled: AtomicU32,
    standby_pending: AtomicU32,
    aggregate_publications: AtomicU32,
    publications_pending: AtomicU32,
    aggregates_completed: AtomicU32,
    subframes_acknowledged: AtomicU32,
    individual_retries: AtomicU32,
    hardware_timeouts: AtomicU32,
    collisions: AtomicU32,
    preparation_micros: AtomicU32,
    preparation_lifetime_max_micros: AtomicU32,
    publication_program_micros: AtomicU32,
    publication_program_lifetime_max_micros: AtomicU32,
    last_publication_micros: AtomicU32,
    last_completion_micros: AtomicU32,
    completion_to_publication_samples: AtomicU32,
    completion_to_publication_micros: AtomicU32,
    completion_to_publication_lifetime_max_micros: AtomicU32,
    completion_to_publication_histogram: [AtomicU32; COMPLETION_TO_PUBLICATION_BUCKETS],
    unprepared_publication_samples: AtomicU32,
    unprepared_publication_micros: AtomicU32,
    unprepared_publication_lifetime_max_micros: AtomicU32,
    completion_to_prepared_entry_samples: AtomicU32,
    completion_to_prepared_entry_micros: AtomicU32,
    completion_to_prepared_entry_lifetime_max_micros: AtomicU32,
    prepared_entry_to_publication_samples: AtomicU32,
    prepared_entry_to_publication_micros: AtomicU32,
    prepared_entry_to_publication_lifetime_max_micros: AtomicU32,
    prepared_scheduler_trace: PreparedTxSchedulerTraceRecorder,
    prepared_scheduler_timing: PreparedTxSchedulerTimingCounters,
    completion_core_micros: AtomicU32,
    completion_core_lifetime_max_micros: AtomicU32,
    backing_release_micros: AtomicU32,
    backing_release_lifetime_max_micros: AtomicU32,
    work: WorkCounters,
    ordinary_work: WorkCounters,
    station_ordinary: ordinary::Counters,
    exchange_micros: AtomicU32,
    exchange_lifetime_max_micros: AtomicU32,
    single_publication_exchanges: AtomicU32,
    single_publication_exchange_micros: AtomicU32,
    single_publication_exchange_lifetime_max_micros: AtomicU32,
    retried_exchanges: AtomicU32,
    retried_exchange_publications: AtomicU32,
    retried_exchange_micros: AtomicU32,
    retried_exchange_lifetime_max_micros: AtomicU32,
    exchanges_by_publications: [AtomicU32; AGGREGATE_TX_PUBLICATION_BUCKETS],
    exchange_micros_by_publications: [AtomicU32; AGGREGATE_TX_PUBLICATION_BUCKETS],
    exchange_lifetime_max_micros_by_publications: [AtomicU32; AGGREGATE_TX_PUBLICATION_BUCKETS],
    block_ack_samples: AtomicU32,
    block_ack_received: AtomicU32,
    success_without_block_ack: AtomicU32,
    nonzero_block_ack_control: AtomicU32,
    block_ack_start_outside_window: AtomicU32,
    block_ack_start_lag_max: AtomicU32,
    full_block_ack: AtomicU32,
    partial_block_ack: AtomicU32,
    partial_missing_by_position: [AtomicU32; BLOCK_ACK_POSITIONS],
    partial_missing_counts: [AtomicU32; PARTIAL_MISSING_BUCKETS],
    empty_block_ack: AtomicU32,
    block_ack_requests: AtomicU32,
    block_ack_requests_answered: AtomicU32,
    full_block_ack_snr: BlockAckSnrCounters,
    partial_block_ack_snr: BlockAckSnrCounters,
    block_ack_snr_unavailable: AtomicU32,
    tx_irq_epochs: AtomicU32,
    tx_irq_service_samples: AtomicU32,
    tx_irq_clock_skew_samples: AtomicU32,
    tx_irq_to_service_micros: AtomicU32,
    tx_irq_to_service_lifetime_max_micros: AtomicU32,
    tx_publication_to_irq_samples: AtomicU32,
    tx_publication_to_irq_micros: AtomicU32,
    tx_publication_to_irq_lifetime_max_micros: AtomicU32,
    pending_tx_irq_micros: AtomicU32,
    stopped_at_frame_limit: AtomicU32,
    stopped_at_capacity_limit: AtomicU32,
    stopped_on_empty_queue: AtomicU32,
    prepared_subframes: [AtomicU32; AGGREGATE_TX_HISTOGRAM_BUCKETS],
}

impl AggregateTxCounters {
    const IRQ_SAMPLE_MASK: u32 = 0x3f;
    const IRQ_TIME_VALID: u32 = 1 << 31;
    const IRQ_TIME_MASK: u32 = Self::IRQ_TIME_VALID - 1;

    pub const fn new() -> Self {
        Self::with_clock(|| 0)
    }

    pub const fn with_clock(now_micros: fn() -> u64) -> Self {
        Self {
            terminal: terminal::Counters::new(),
            tx_retention: crate::tx_retention::TxRetentionCounters::new(),
            #[cfg(feature = "tx-wait-probe")]
            wait_trace: crate::tx_wait::Trace::new(),
            now_micros,
            secondary_socket: crate::tx_progress::Counters::new(),
            secondary_claim: crate::tx_progress::Counters::new(),
            ap_udp_claim_highest: AtomicU32::new(u32::MAX),
            ap_udp_claimed: AtomicU32::new(0),
            ap_udp_claim_backward: AtomicU32::new(0),
            ap_udp_claim_first_previous: AtomicU32::new(u32::MAX),
            ap_udp_claim_first_sequence: AtomicU32::new(u32::MAX),
            ap_udp_claim_maximum_distance: AtomicU32::new(0),
            block_ack_operational_tids: AtomicU32::new(0),
            block_ack_operational_transitions: AtomicU32::new(0),
            network_single_mpdu_started: AtomicU32::new(0),
            network_single_legacy_rate: AtomicU32::new(0),
            network_single_block_ack_unavailable: AtomicU32::new(0),
            network_single_ht_needs_pair: AtomicU32::new(0),
            network_single_fresh_aggregate_capacity: AtomicU32::new(0),
            network_single_fresh_capacity_lifetime_max_ethernet_length: AtomicU32::new(0),
            rate_selections: AtomicU32::new(0),
            last_bandwidth_mhz: AtomicU32::new(0),
            last_nominal_rate_kbps: AtomicU32::new(0),
            aggregates_prepared: AtomicU32::new(0),
            standby_prepared: AtomicU32::new(0),
            standby_published: AtomicU32::new(0),
            standby_cancelled: AtomicU32::new(0),
            standby_pending: AtomicU32::new(0),
            aggregate_publications: AtomicU32::new(0),
            publications_pending: AtomicU32::new(0),
            aggregates_completed: AtomicU32::new(0),
            subframes_acknowledged: AtomicU32::new(0),
            individual_retries: AtomicU32::new(0),
            hardware_timeouts: AtomicU32::new(0),
            collisions: AtomicU32::new(0),
            preparation_micros: AtomicU32::new(0),
            preparation_lifetime_max_micros: AtomicU32::new(0),
            publication_program_micros: AtomicU32::new(0),
            publication_program_lifetime_max_micros: AtomicU32::new(0),
            last_publication_micros: AtomicU32::new(0),
            last_completion_micros: AtomicU32::new(0),
            completion_to_publication_samples: AtomicU32::new(0),
            completion_to_publication_micros: AtomicU32::new(0),
            completion_to_publication_lifetime_max_micros: AtomicU32::new(0),
            completion_to_publication_histogram: [const { AtomicU32::new(0) };
                COMPLETION_TO_PUBLICATION_BUCKETS],
            unprepared_publication_samples: AtomicU32::new(0),
            unprepared_publication_micros: AtomicU32::new(0),
            unprepared_publication_lifetime_max_micros: AtomicU32::new(0),
            completion_to_prepared_entry_samples: AtomicU32::new(0),
            completion_to_prepared_entry_micros: AtomicU32::new(0),
            completion_to_prepared_entry_lifetime_max_micros: AtomicU32::new(0),
            prepared_entry_to_publication_samples: AtomicU32::new(0),
            prepared_entry_to_publication_micros: AtomicU32::new(0),
            prepared_entry_to_publication_lifetime_max_micros: AtomicU32::new(0),
            prepared_scheduler_trace: PreparedTxSchedulerTraceRecorder::new(),
            prepared_scheduler_timing: PreparedTxSchedulerTimingCounters::new(),
            completion_core_micros: AtomicU32::new(0),
            completion_core_lifetime_max_micros: AtomicU32::new(0),
            backing_release_micros: AtomicU32::new(0),
            backing_release_lifetime_max_micros: AtomicU32::new(0),
            work: WorkCounters::new(),
            ordinary_work: WorkCounters::new(),
            station_ordinary: ordinary::Counters::new(),
            exchange_micros: AtomicU32::new(0),
            exchange_lifetime_max_micros: AtomicU32::new(0),
            single_publication_exchanges: AtomicU32::new(0),
            single_publication_exchange_micros: AtomicU32::new(0),
            single_publication_exchange_lifetime_max_micros: AtomicU32::new(0),
            retried_exchanges: AtomicU32::new(0),
            retried_exchange_publications: AtomicU32::new(0),
            retried_exchange_micros: AtomicU32::new(0),
            retried_exchange_lifetime_max_micros: AtomicU32::new(0),
            exchanges_by_publications: [const { AtomicU32::new(0) };
                AGGREGATE_TX_PUBLICATION_BUCKETS],
            exchange_micros_by_publications: [const { AtomicU32::new(0) };
                AGGREGATE_TX_PUBLICATION_BUCKETS],
            exchange_lifetime_max_micros_by_publications: [const { AtomicU32::new(0) };
                AGGREGATE_TX_PUBLICATION_BUCKETS],
            block_ack_samples: AtomicU32::new(0),
            block_ack_received: AtomicU32::new(0),
            success_without_block_ack: AtomicU32::new(0),
            nonzero_block_ack_control: AtomicU32::new(0),
            block_ack_start_outside_window: AtomicU32::new(0),
            block_ack_start_lag_max: AtomicU32::new(0),
            full_block_ack: AtomicU32::new(0),
            partial_block_ack: AtomicU32::new(0),
            partial_missing_by_position: [const { AtomicU32::new(0) }; BLOCK_ACK_POSITIONS],
            partial_missing_counts: [const { AtomicU32::new(0) }; PARTIAL_MISSING_BUCKETS],
            empty_block_ack: AtomicU32::new(0),
            block_ack_requests: AtomicU32::new(0),
            block_ack_requests_answered: AtomicU32::new(0),
            full_block_ack_snr: BlockAckSnrCounters::new(),
            partial_block_ack_snr: BlockAckSnrCounters::new(),
            block_ack_snr_unavailable: AtomicU32::new(0),
            tx_irq_epochs: AtomicU32::new(0),
            tx_irq_service_samples: AtomicU32::new(0),
            tx_irq_clock_skew_samples: AtomicU32::new(0),
            tx_irq_to_service_micros: AtomicU32::new(0),
            tx_irq_to_service_lifetime_max_micros: AtomicU32::new(0),
            tx_publication_to_irq_samples: AtomicU32::new(0),
            tx_publication_to_irq_micros: AtomicU32::new(0),
            tx_publication_to_irq_lifetime_max_micros: AtomicU32::new(0),
            pending_tx_irq_micros: AtomicU32::new(0),
            stopped_at_frame_limit: AtomicU32::new(0),
            stopped_at_capacity_limit: AtomicU32::new(0),
            stopped_on_empty_queue: AtomicU32::new(0),
            prepared_subframes: [const { AtomicU32::new(0) }; AGGREGATE_TX_HISTOGRAM_BUCKETS],
        }
    }

    /// Start one HIL measurement interval at a quiescent workload boundary.
    ///
    /// Cumulative counters survive between sessions so snapshots can form
    /// deltas. Correlation endpoints and AP payload sequence state must not:
    /// carrying either across a role restart would manufacture one very long
    /// completion gap and classify the restarted sequence zero as backward.
    /// Call on the radio executor before taking the next interval's first
    /// snapshot. Outstanding publications survive the interval boundary.
    pub fn begin_interval(&self) {
        let now = (self.now_micros)() as u32;
        self.secondary_socket.reset(now);
        self.secondary_claim.reset(now);
        self.last_publication_micros.store(0, Ordering::Relaxed);
        self.last_completion_micros.store(0, Ordering::Relaxed);
        self.pending_tx_irq_micros.store(0, Ordering::Relaxed);
        self.prepared_scheduler_trace.reset();
        self.ap_udp_claim_highest.store(u32::MAX, Ordering::Relaxed);
        self.ap_udp_claim_backward.store(0, Ordering::Relaxed);
        self.ap_udp_claim_first_previous
            .store(u32::MAX, Ordering::Relaxed);
        self.ap_udp_claim_first_sequence
            .store(u32::MAX, Ordering::Relaxed);
        self.ap_udp_claim_maximum_distance
            .store(0, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> AggregateTxCounterSnapshot {
        let now = (self.now_micros)() as u32;
        AggregateTxCounterSnapshot {
            station_terminal: self.terminal.snapshot(),
            secondary_socket: self.secondary_socket.snapshot(now),
            secondary_claim: self.secondary_claim.snapshot(now),
            ap_udp_claimed: self.ap_udp_claimed.load(Ordering::Relaxed),
            ap_udp_claim_backward: self.ap_udp_claim_backward.load(Ordering::Relaxed),
            ap_udp_claim_first_previous: self.ap_udp_claim_first_previous.load(Ordering::Relaxed),
            ap_udp_claim_first_sequence: self.ap_udp_claim_first_sequence.load(Ordering::Relaxed),
            ap_udp_claim_maximum_distance: self
                .ap_udp_claim_maximum_distance
                .load(Ordering::Relaxed),
            block_ack_operational_tids: self.block_ack_operational_tids.load(Ordering::Acquire),
            block_ack_operational_transitions: self
                .block_ack_operational_transitions
                .load(Ordering::Relaxed),
            network_single_mpdu_started: self.network_single_mpdu_started.load(Ordering::Relaxed),
            network_single_legacy_rate: self.network_single_legacy_rate.load(Ordering::Relaxed),
            network_single_block_ack_unavailable: self
                .network_single_block_ack_unavailable
                .load(Ordering::Relaxed),
            network_single_ht_needs_pair: self.network_single_ht_needs_pair.load(Ordering::Relaxed),
            network_single_fresh_aggregate_capacity: self
                .network_single_fresh_aggregate_capacity
                .load(Ordering::Relaxed),
            network_single_fresh_capacity_lifetime_max_ethernet_length: self
                .network_single_fresh_capacity_lifetime_max_ethernet_length
                .load(Ordering::Relaxed),
            rate_selections: self.rate_selections.load(Ordering::Relaxed),
            last_bandwidth_mhz: self.last_bandwidth_mhz.load(Ordering::Relaxed),
            last_nominal_rate_kbps: self.last_nominal_rate_kbps.load(Ordering::Relaxed),
            aggregates_prepared: self.aggregates_prepared.load(Ordering::Relaxed),
            standby_prepared: self.standby_prepared.load(Ordering::Relaxed),
            standby_published: self.standby_published.load(Ordering::Relaxed),
            standby_cancelled: self.standby_cancelled.load(Ordering::Relaxed),
            standby_pending_start: 0,
            standby_pending_end: self.standby_pending.load(Ordering::Relaxed),
            aggregate_publications: self.aggregate_publications.load(Ordering::Relaxed),
            publications_pending_start: 0,
            publications_pending_end: self.publications_pending.load(Ordering::Relaxed),
            aggregates_completed: self.aggregates_completed.load(Ordering::Relaxed),
            subframes_acknowledged: self.subframes_acknowledged.load(Ordering::Relaxed),
            individual_retries: self.individual_retries.load(Ordering::Relaxed),
            hardware_timeouts: self.hardware_timeouts.load(Ordering::Relaxed),
            collisions: self.collisions.load(Ordering::Relaxed),
            preparation_micros: self.preparation_micros.load(Ordering::Relaxed),
            preparation_lifetime_max_micros: self
                .preparation_lifetime_max_micros
                .load(Ordering::Relaxed),
            publication_program_micros: self.publication_program_micros.load(Ordering::Relaxed),
            publication_program_lifetime_max_micros: self
                .publication_program_lifetime_max_micros
                .load(Ordering::Relaxed),
            completion_to_publication_samples: self
                .completion_to_publication_samples
                .load(Ordering::Relaxed),
            completion_to_publication_micros: self
                .completion_to_publication_micros
                .load(Ordering::Relaxed),
            completion_to_publication_lifetime_max_micros: self
                .completion_to_publication_lifetime_max_micros
                .load(Ordering::Relaxed),
            completion_to_publication_histogram: core::array::from_fn(|bucket| {
                self.completion_to_publication_histogram[bucket].load(Ordering::Relaxed)
            }),
            unprepared_publication_samples: self
                .unprepared_publication_samples
                .load(Ordering::Relaxed),
            unprepared_publication_micros: self
                .unprepared_publication_micros
                .load(Ordering::Relaxed),
            unprepared_publication_lifetime_max_micros: self
                .unprepared_publication_lifetime_max_micros
                .load(Ordering::Relaxed),
            completion_to_prepared_entry_samples: self
                .completion_to_prepared_entry_samples
                .load(Ordering::Relaxed),
            completion_to_prepared_entry_micros: self
                .completion_to_prepared_entry_micros
                .load(Ordering::Relaxed),
            completion_to_prepared_entry_lifetime_max_micros: self
                .completion_to_prepared_entry_lifetime_max_micros
                .load(Ordering::Relaxed),
            prepared_entry_to_publication_samples: self
                .prepared_entry_to_publication_samples
                .load(Ordering::Relaxed),
            prepared_entry_to_publication_micros: self
                .prepared_entry_to_publication_micros
                .load(Ordering::Relaxed),
            prepared_entry_to_publication_lifetime_max_micros: self
                .prepared_entry_to_publication_lifetime_max_micros
                .load(Ordering::Relaxed),
            prepared_scheduler_timing: self.prepared_scheduler_timing.snapshot(),
            completion_core_micros: self.completion_core_micros.load(Ordering::Relaxed),
            completion_core_lifetime_max_micros: self
                .completion_core_lifetime_max_micros
                .load(Ordering::Relaxed),
            backing_release_micros: self.backing_release_micros.load(Ordering::Relaxed),
            backing_release_lifetime_max_micros: self
                .backing_release_lifetime_max_micros
                .load(Ordering::Relaxed),
            work: self.work.snapshot(),
            ordinary_work: self.ordinary_work.snapshot(),
            station_ordinary: self.station_ordinary.snapshot(),
            exchange_micros: self.exchange_micros.load(Ordering::Relaxed),
            exchange_lifetime_max_micros: self.exchange_lifetime_max_micros.load(Ordering::Relaxed),
            single_publication_exchanges: self.single_publication_exchanges.load(Ordering::Relaxed),
            single_publication_exchange_micros: self
                .single_publication_exchange_micros
                .load(Ordering::Relaxed),
            single_publication_exchange_lifetime_max_micros: self
                .single_publication_exchange_lifetime_max_micros
                .load(Ordering::Relaxed),
            retried_exchanges: self.retried_exchanges.load(Ordering::Relaxed),
            retried_exchange_publications: self
                .retried_exchange_publications
                .load(Ordering::Relaxed),
            retried_exchange_micros: self.retried_exchange_micros.load(Ordering::Relaxed),
            retried_exchange_lifetime_max_micros: self
                .retried_exchange_lifetime_max_micros
                .load(Ordering::Relaxed),
            exchanges_by_publications: core::array::from_fn(|index| {
                self.exchanges_by_publications[index].load(Ordering::Relaxed)
            }),
            exchange_micros_by_publications: core::array::from_fn(|index| {
                self.exchange_micros_by_publications[index].load(Ordering::Relaxed)
            }),
            exchange_lifetime_max_micros_by_publications: core::array::from_fn(|index| {
                self.exchange_lifetime_max_micros_by_publications[index].load(Ordering::Relaxed)
            }),
            block_ack_samples: self.block_ack_samples.load(Ordering::Relaxed),
            block_ack_received: self.block_ack_received.load(Ordering::Relaxed),
            success_without_block_ack: self.success_without_block_ack.load(Ordering::Relaxed),
            nonzero_block_ack_control: self.nonzero_block_ack_control.load(Ordering::Relaxed),
            block_ack_start_outside_window: self
                .block_ack_start_outside_window
                .load(Ordering::Relaxed),
            block_ack_start_lag_max: self.block_ack_start_lag_max.load(Ordering::Relaxed),
            full_block_ack: self.full_block_ack.load(Ordering::Relaxed),
            partial_block_ack: self.partial_block_ack.load(Ordering::Relaxed),
            partial_missing_by_position: core::array::from_fn(|position| {
                self.partial_missing_by_position[position].load(Ordering::Relaxed)
            }),
            partial_missing_counts: core::array::from_fn(|bucket| {
                self.partial_missing_counts[bucket].load(Ordering::Relaxed)
            }),
            empty_block_ack: self.empty_block_ack.load(Ordering::Relaxed),
            block_ack_requests: self.block_ack_requests.load(Ordering::Relaxed),
            block_ack_requests_answered: self.block_ack_requests_answered.load(Ordering::Relaxed),
            full_block_ack_snr: self.full_block_ack_snr.snapshot(),
            partial_block_ack_snr: self.partial_block_ack_snr.snapshot(),
            block_ack_snr_unavailable: self.block_ack_snr_unavailable.load(Ordering::Relaxed),
            tx_irq_epochs: self.tx_irq_epochs.load(Ordering::Relaxed),
            tx_irq_service_samples: self.tx_irq_service_samples.load(Ordering::Relaxed),
            tx_irq_clock_skew_samples: self.tx_irq_clock_skew_samples.load(Ordering::Relaxed),
            tx_irq_to_service_micros: self.tx_irq_to_service_micros.load(Ordering::Relaxed),
            tx_irq_to_service_lifetime_max_micros: self
                .tx_irq_to_service_lifetime_max_micros
                .load(Ordering::Relaxed),
            tx_publication_to_irq_samples: self
                .tx_publication_to_irq_samples
                .load(Ordering::Relaxed),
            tx_publication_to_irq_micros: self.tx_publication_to_irq_micros.load(Ordering::Relaxed),
            tx_publication_to_irq_lifetime_max_micros: self
                .tx_publication_to_irq_lifetime_max_micros
                .load(Ordering::Relaxed),
            stopped_at_frame_limit: self.stopped_at_frame_limit.load(Ordering::Relaxed),
            stopped_at_capacity_limit: self.stopped_at_capacity_limit.load(Ordering::Relaxed),
            stopped_on_empty_queue: self.stopped_on_empty_queue.load(Ordering::Relaxed),
            prepared_subframes: core::array::from_fn(|index| {
                self.prepared_subframes[index].load(Ordering::Relaxed)
            }),
        }
    }

    /// Publish the current operational state of one TX BlockAck TID.
    ///
    /// This is idempotent because role observers may report a complete state
    /// snapshot on every bounded service turn. Transition evidence counts
    /// actual edges, not repeated observations of the same state.
    pub fn set_block_ack_operational(&self, tid: u8, operational: bool) {
        let Some(mask) = 1_u32.checked_shl(u32::from(tid)) else {
            return;
        };
        let previous = if operational {
            self.block_ack_operational_tids
                .fetch_or(mask, Ordering::AcqRel)
        } else {
            self.block_ack_operational_tids
                .fetch_and(!mask, Ordering::AcqRel)
        };
        if (previous & mask != 0) != operational {
            self.block_ack_operational_transitions
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_network_single_mpdu(
        &self,
        reason: NetworkSingleMpduReason,
        ethernet_length: usize,
    ) {
        self.network_single_mpdu_started
            .fetch_add(1, Ordering::Relaxed);
        let reason = match reason {
            NetworkSingleMpduReason::LegacyRate => &self.network_single_legacy_rate,
            NetworkSingleMpduReason::BlockAckUnavailable => {
                &self.network_single_block_ack_unavailable
            }
            NetworkSingleMpduReason::HtNeedsPair => &self.network_single_ht_needs_pair,
            NetworkSingleMpduReason::FreshAggregateCapacity => {
                self.network_single_fresh_capacity_lifetime_max_ethernet_length
                    .fetch_max(
                        u32::try_from(ethernet_length).unwrap_or(u32::MAX),
                        Ordering::Relaxed,
                    );
                &self.network_single_fresh_aggregate_capacity
            }
        };
        reason.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_prepared(&self, subframes: u8, stop: AggregateBuildStop) {
        let index = usize::from(subframes);
        debug_assert!((1..AGGREGATE_TX_HISTOGRAM_BUCKETS).contains(&index));
        self.aggregates_prepared.fetch_add(1, Ordering::Relaxed);
        if let Some(bucket) = self.prepared_subframes.get(index) {
            bucket.fetch_add(1, Ordering::Relaxed);
        }
        match stop {
            AggregateBuildStop::FrameLimit => {
                self.stopped_at_frame_limit.fetch_add(1, Ordering::Relaxed);
            }
            AggregateBuildStop::CapacityLimit => {
                self.stopped_at_capacity_limit
                    .fetch_add(1, Ordering::Relaxed);
            }
            AggregateBuildStop::QueueEmpty => {
                self.stopped_on_empty_queue.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub(crate) fn record_preparation_time(&self, micros: u64) {
        Self::record_time(
            &self.preparation_micros,
            &self.preparation_lifetime_max_micros,
            micros,
        );
    }

    pub(crate) fn record_publication(&self, at_micros: u64, program_micros: u64) {
        let prepared_scheduler = self.prepared_scheduler_trace.take();
        let at_modulo = at_micros as u32 & Self::IRQ_TIME_MASK;
        let completion = self.last_completion_micros.swap(0, Ordering::AcqRel);
        if completion != 0 {
            let completed_modulo = completion & Self::IRQ_TIME_MASK;
            let elapsed = at_modulo.wrapping_sub(completed_modulo) & Self::IRQ_TIME_MASK;
            if elapsed <= Self::IRQ_TIME_MASK / 2 {
                self.completion_to_publication_samples
                    .fetch_add(1, Ordering::Relaxed);
                Self::record_time(
                    &self.completion_to_publication_micros,
                    &self.completion_to_publication_lifetime_max_micros,
                    u64::from(elapsed),
                );
                self.completion_to_publication_histogram[completion_to_publication_bucket(elapsed)]
                    .fetch_add(1, Ordering::Relaxed);
                if prepared_scheduler.is_none() {
                    // The successor did not enter through the prepared path:
                    // the scheduler collected, claimed or waited for it.
                    self.unprepared_publication_samples
                        .fetch_add(1, Ordering::Relaxed);
                    Self::record_time(
                        &self.unprepared_publication_micros,
                        &self.unprepared_publication_lifetime_max_micros,
                        u64::from(elapsed),
                    );
                }
            }
            if let Some(trace) = prepared_scheduler {
                let entry_micros = trace.prepared_entry_micros;
                let entry_modulo = entry_micros as u32 & Self::IRQ_TIME_MASK;
                let completion_to_entry =
                    entry_modulo.wrapping_sub(completed_modulo) & Self::IRQ_TIME_MASK;
                let entry_to_publication =
                    at_modulo.wrapping_sub(entry_modulo) & Self::IRQ_TIME_MASK;
                if completion_to_entry <= Self::IRQ_TIME_MASK / 2
                    && entry_to_publication <= Self::IRQ_TIME_MASK / 2
                {
                    self.completion_to_prepared_entry_samples
                        .fetch_add(1, Ordering::Relaxed);
                    Self::record_time(
                        &self.completion_to_prepared_entry_micros,
                        &self.completion_to_prepared_entry_lifetime_max_micros,
                        u64::from(completion_to_entry),
                    );
                    self.prepared_entry_to_publication_samples
                        .fetch_add(1, Ordering::Relaxed);
                    Self::record_time(
                        &self.prepared_entry_to_publication_micros,
                        &self.prepared_entry_to_publication_lifetime_max_micros,
                        u64::from(entry_to_publication),
                    );
                    self.prepared_scheduler_timing
                        .record(completed_modulo, trace);
                }
            }
        }
        self.last_publication_micros
            .store(Self::IRQ_TIME_VALID | at_modulo, Ordering::Release);
        self.aggregate_publications.fetch_add(1, Ordering::Relaxed);
        self.publications_pending.fetch_add(1, Ordering::Relaxed);
        Self::record_time(
            &self.publication_program_micros,
            &self.publication_program_lifetime_max_micros,
            program_micros,
        );
    }

    pub(crate) fn record_complete(&self, acknowledged: u8, individual_retry: bool) {
        self.aggregates_completed.fetch_add(1, Ordering::Relaxed);
        self.subframes_acknowledged
            .fetch_add(u32::from(acknowledged), Ordering::Relaxed);
        if individual_retry {
            self.individual_retries.fetch_add(1, Ordering::Relaxed);
        }
        // Correlate the next publication with the end of this diagnostic
        // observer, not with its first bookkeeping operation. Otherwise the
        // observer's own PSRAM/atomic cost is charged to the production
        // completion-to-publication scheduler boundary that it is measuring.
        // Release pairs the completed counters with the boundary timestamp.
        let now_modulo = (self.now_micros)() as u32 & Self::IRQ_TIME_MASK;
        self.last_completion_micros
            .store(Self::IRQ_TIME_VALID | now_modulo, Ordering::Release);
    }

    pub(crate) fn record_hardware_timeout(&self) {
        self.publications_pending.fetch_sub(1, Ordering::Relaxed);
        self.hardware_timeouts.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_collision(&self) {
        self.publications_pending.fetch_sub(1, Ordering::Relaxed);
        self.collisions.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_exchange_time(&self, micros: u64, publications: u8) {
        Self::record_time(
            &self.exchange_micros,
            &self.exchange_lifetime_max_micros,
            micros,
        );
        let publication_bucket =
            usize::from(publications).clamp(1, AGGREGATE_TX_PUBLICATION_BUCKETS - 1);
        self.exchanges_by_publications[publication_bucket].fetch_add(1, Ordering::Relaxed);
        Self::record_time(
            &self.exchange_micros_by_publications[publication_bucket],
            &self.exchange_lifetime_max_micros_by_publications[publication_bucket],
            micros,
        );
        if publications <= 1 {
            self.single_publication_exchanges
                .fetch_add(1, Ordering::Relaxed);
            Self::record_time(
                &self.single_publication_exchange_micros,
                &self.single_publication_exchange_lifetime_max_micros,
                micros,
            );
        } else {
            self.retried_exchanges.fetch_add(1, Ordering::Relaxed);
            self.retried_exchange_publications
                .fetch_add(u32::from(publications), Ordering::Relaxed);
            Self::record_time(
                &self.retried_exchange_micros,
                &self.retried_exchange_lifetime_max_micros,
                micros,
            );
        }
    }

    /// Sample one newly published TX wake epoch at the ISR-to-executor handoff.
    ///
    /// The caller supplies the clock lazily so the hard ISR reads it only for
    /// one in 64 wake epochs. Epoch counting remains exact, and an already
    /// pending sample is never replaced by a newer coalesced interrupt.
    #[inline]
    pub fn record_tx_irq_epoch(&self, now_micros: impl FnOnce() -> u64) {
        // Consume this correlation on every TX edge, including unsampled
        // epochs, so an ordinary completion cannot inherit an older A-MPDU
        // publication timestamp.
        let publication = self.last_publication_micros.swap(0, Ordering::AcqRel);
        let epoch = self.tx_irq_epochs.fetch_add(1, Ordering::Relaxed);
        if epoch & Self::IRQ_SAMPLE_MASK != 0 {
            return;
        }
        let now_modulo = now_micros() as u32 & Self::IRQ_TIME_MASK;
        let timestamp = Self::IRQ_TIME_VALID | now_modulo;
        if publication != 0 {
            let posted_modulo = publication & Self::IRQ_TIME_MASK;
            let elapsed = now_modulo.wrapping_sub(posted_modulo) & Self::IRQ_TIME_MASK;
            if elapsed <= Self::IRQ_TIME_MASK / 2 {
                self.tx_publication_to_irq_samples
                    .fetch_add(1, Ordering::Relaxed);
                Self::record_time(
                    &self.tx_publication_to_irq_micros,
                    &self.tx_publication_to_irq_lifetime_max_micros,
                    u64::from(elapsed),
                );
            }
        }
        let _ = self.pending_tx_irq_micros.compare_exchange(
            0,
            timestamp,
            Ordering::Release,
            Ordering::Relaxed,
        );
    }

    fn record_tx_service_started(&self, started: u64) {
        let pending = if self.pending_tx_irq_micros.load(Ordering::Acquire) == 0 {
            0
        } else {
            self.pending_tx_irq_micros.swap(0, Ordering::AcqRel)
        };
        if pending == 0 {
            return;
        }
        let started_modulo = started as u32 & Self::IRQ_TIME_MASK;
        let posted_modulo = pending & Self::IRQ_TIME_MASK;
        let elapsed = started_modulo.wrapping_sub(posted_modulo) & Self::IRQ_TIME_MASK;
        if elapsed <= Self::IRQ_TIME_MASK / 2 {
            self.tx_irq_service_samples.fetch_add(1, Ordering::Relaxed);
            Self::record_time(
                &self.tx_irq_to_service_micros,
                &self.tx_irq_to_service_lifetime_max_micros,
                u64::from(elapsed),
            );
        } else {
            // The clock can differ slightly across the ISR and radio cores.
            self.tx_irq_clock_skew_samples
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_time(total: &AtomicU32, maximum: &AtomicU32, micros: u64) {
        let micros = u32::try_from(micros).unwrap_or(u32::MAX);
        total.fetch_add(micros, Ordering::Relaxed);
        maximum.fetch_max(micros, Ordering::Relaxed);
    }
}

impl AggregateTxObserver for AggregateTxCounters {
    fn observe_station_ordinary(
        &self,
        outcome: Option<
            oer_esp32s31_ieee80211_runtime::diagnostics::aggregate_tx::OrdinaryTxOutcome,
        >,
    ) {
        self.station_ordinary.record(outcome);
    }

    fn observe_station_terminal(
        &self,
        status: oer_ieee80211_softmac::MacAmpduTxStatus<oer_esp32s31_ieee80211_mac::tx::TxPhyRate>,
    ) {
        self.terminal.record(status);
    }

    #[cfg(feature = "tx-wait-probe")]
    fn observe_wait_probe(
        &self,
        sample: oer_esp32s31_ieee80211_runtime::diagnostics::aggregate_tx::TxWaitSample,
    ) {
        self.wait_trace.record(sample);
    }
    fn now_micros(&self) -> u64 {
        (self.now_micros)()
    }

    fn observe(&self, observation: AggregateTxObservation) {
        match observation {
            AggregateTxObservation::WorkCompleted { work } => self.work.record(work),
            AggregateTxObservation::OrdinaryWorkCompleted { work } => {
                self.ordinary_work.record(work)
            }
            AggregateTxObservation::BlockAckOperational { tid, operational } => {
                self.set_block_ack_operational(tid, operational);
            }
            AggregateTxObservation::InterruptServiceStarted { at_micros } => {
                self.record_tx_service_started(at_micros);
            }
            AggregateTxObservation::NetworkSingleMpdu {
                reason,
                ethernet_length,
            } => self.record_network_single_mpdu(reason, ethernet_length),
            AggregateTxObservation::RateSelected {
                bandwidth_mhz,
                nominal_kbps,
            } => {
                self.last_bandwidth_mhz
                    .store(u32::from(bandwidth_mhz), Ordering::Relaxed);
                self.last_nominal_rate_kbps
                    .store(nominal_kbps, Ordering::Relaxed);
                self.rate_selections.fetch_add(1, Ordering::Relaxed);
            }
            AggregateTxObservation::Prepared { subframes, stop } => {
                self.record_prepared(subframes, stop);
            }
            AggregateTxObservation::PreparationCompleted { micros } => {
                self.record_preparation_time(micros);
            }
            AggregateTxObservation::StandbyPrepared => {
                self.standby_pending.fetch_add(1, Ordering::Relaxed);
                self.standby_prepared.fetch_add(1, Ordering::Relaxed);
            }
            AggregateTxObservation::StandbyPublished => {
                self.standby_pending.fetch_sub(1, Ordering::Relaxed);
                self.standby_published.fetch_add(1, Ordering::Relaxed);
            }
            AggregateTxObservation::StandbyCancelled => {
                self.standby_pending.fetch_sub(1, Ordering::Relaxed);
                self.standby_cancelled.fetch_add(1, Ordering::Relaxed);
            }
            AggregateTxObservation::PreparedSchedulerPhase { phase, at_micros } => {
                self.prepared_scheduler_trace.record(phase, at_micros);
            }
            AggregateTxObservation::Published {
                at_micros,
                program_micros,
            } => {
                self.record_publication(at_micros, program_micros);
            }
            AggregateTxObservation::BlockAckRequestCompleted { answered } => {
                self.block_ack_requests.fetch_add(1, Ordering::Relaxed);
                if answered {
                    self.block_ack_requests_answered
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
            AggregateTxObservation::BlockAckProcessed {
                tx_status,
                block_ack_received,
                control,
                first_sequence,
                starting_sequence,
                subframes,
                missing_original_indices,
                block_ack_snr_db,
            } => {
                let missing = missing_original_indices.count_ones() as u8;
                self.publications_pending.fetch_sub(1, Ordering::Relaxed);
                self.block_ack_samples.fetch_add(1, Ordering::Relaxed);
                if block_ack_received {
                    self.block_ack_received.fetch_add(1, Ordering::Relaxed);
                } else if tx_status == 0 {
                    self.success_without_block_ack
                        .fetch_add(1, Ordering::Relaxed);
                }
                if control != 0 {
                    self.nonzero_block_ack_control
                        .fetch_add(1, Ordering::Relaxed);
                }
                if block_ack_received {
                    let lag = starting_sequence.forward_distance(first_sequence);
                    let advance = first_sequence.forward_distance(starting_sequence);
                    if lag < 64 {
                        self.block_ack_start_lag_max
                            .fetch_max(u32::from(lag), Ordering::Relaxed);
                    } else if advance <= 64 {
                        // Some peers publish the first not-yet-acknowledged
                        // sequence as SSN. That advances the bitmap beyond
                        // already completed MPDUs and is not an invalid BA.
                        self.block_ack_start_lag_max
                            .fetch_max(u32::from(advance), Ordering::Relaxed);
                    } else {
                        self.block_ack_start_outside_window
                            .fetch_add(1, Ordering::Relaxed);
                    }
                }
                let snr_class = if missing == 0 {
                    Some(&self.full_block_ack_snr)
                } else if missing == subframes {
                    None
                } else {
                    Some(&self.partial_block_ack_snr)
                };
                match (snr_class, block_ack_snr_db) {
                    (Some(class), Some(snr_db)) if block_ack_received => class.record(snr_db),
                    (Some(_), _) => {
                        self.block_ack_snr_unavailable
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    (None, _) => {}
                }
                if missing == 0 {
                    self.full_block_ack.fetch_add(1, Ordering::Relaxed);
                } else if missing == subframes {
                    self.empty_block_ack.fetch_add(1, Ordering::Relaxed);
                } else {
                    self.partial_block_ack.fetch_add(1, Ordering::Relaxed);
                    self.partial_missing_counts[partial_missing_bucket(missing)]
                        .fetch_add(1, Ordering::Relaxed);
                    let mut positions = missing_original_indices;
                    while positions != 0 {
                        let position = positions.trailing_zeros() as usize;
                        self.partial_missing_by_position[position].fetch_add(1, Ordering::Relaxed);
                        positions &= positions - 1;
                    }
                }
            }
            AggregateTxObservation::CompletionCoreCompleted { micros } => {
                Self::record_time(
                    &self.completion_core_micros,
                    &self.completion_core_lifetime_max_micros,
                    micros,
                );
            }
            AggregateTxObservation::BackingReleaseCompleted { micros } => {
                Self::record_time(
                    &self.backing_release_micros,
                    &self.backing_release_lifetime_max_micros,
                    micros,
                );
            }
            AggregateTxObservation::Completed {
                acknowledged,
                individual_retry,
            } => self.record_complete(acknowledged, individual_retry),
            AggregateTxObservation::HardwareTimeout => self.record_hardware_timeout(),
            AggregateTxObservation::Collision => self.record_collision(),
            AggregateTxObservation::ExchangeCompleted {
                micros,
                publications,
            } => {
                self.record_exchange_time(micros, publications);
            }
        }
    }

    fn observe_access_point_retention_drop(
        &self,
        reason: oer_esp32s31_ieee80211_runtime::diagnostics::aggregate_tx::NetworkTxRetentionDropReason,
    ) {
        self.tx_retention.observe(reason);
    }

    fn observe_access_point_network_claim(&self, ethernet: &[u8]) {
        if let Some(sequence) = qualification_udp_sequence_for_port(ethernet, 4_325) {
            self.secondary_claim
                .admitted(sequence, (self.now_micros)() as u32);
        }
        let Some(sequence) = qualification_udp_sequence(ethernet) else {
            return;
        };
        self.ap_udp_claimed.fetch_add(1, Ordering::Relaxed);
        let previous = self.ap_udp_claim_highest.load(Ordering::Relaxed);
        if previous == u32::MAX || sequence > previous {
            self.ap_udp_claim_highest.store(sequence, Ordering::Relaxed);
            return;
        }
        self.ap_udp_claim_backward.fetch_add(1, Ordering::Relaxed);
        if self.ap_udp_claim_first_sequence.load(Ordering::Relaxed) == u32::MAX {
            self.ap_udp_claim_first_previous
                .store(previous, Ordering::Relaxed);
            self.ap_udp_claim_first_sequence
                .store(sequence, Ordering::Relaxed);
        }
        self.ap_udp_claim_maximum_distance
            .fetch_max(previous - sequence, Ordering::Relaxed);
    }
}

fn qualification_udp_sequence(ethernet: &[u8]) -> Option<u32> {
    qualification_udp_sequence_for_port(ethernet, 4_324)
}

fn qualification_udp_sequence_for_port(ethernet: &[u8], port: u16) -> Option<u32> {
    let ip = 14_usize;
    let version_ihl = *ethernet.get(ip)?;
    if version_ihl >> 4 != 4 || version_ihl & 0x0f < 5 || *ethernet.get(ip + 9)? != 17 {
        return None;
    }
    let udp = ip + usize::from(version_ihl & 0x0f) * 4;
    if u16::from_be_bytes(ethernet.get(udp..udp + 2)?.try_into().ok()?) != port {
        return None;
    }
    Some(u32::from_be_bytes(
        ethernet.get(udp + 8..udp + 12)?.try_into().ok()?,
    ))
}

impl Default for AggregateTxCounters {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhaseTimingSnapshot {
    pub micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub lifetime_max_micros: u32,
}

impl PhaseTimingSnapshot {
    fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            micros: self.micros.wrapping_sub(earlier.micros),
            lifetime_max_micros: self.lifetime_max_micros,
        }
    }
}

/// Diagnostic decomposition of completion-to-prepared-entry scheduler time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreparedTxSchedulerTimingSnapshot {
    pub samples: u32,
    pub scheduler_passes: u32,
    /// Maximum passes observed since boot, not an interval delta.
    pub scheduler_passes_lifetime_max: u32,
    pub control_ready_passes: u32,
    /// Samples whose prepared entry needed more than one scheduler pass.
    pub multi_pass_samples: u32,
    pub completion_to_active_service_return: PhaseTimingSnapshot,
    /// Active service return to the first scheduler pass.
    pub active_service_return_to_first_pass: PhaseTimingSnapshot,
    /// First pass to the entry pass, over multi-pass samples only.
    pub additional_passes: PhaseTimingSnapshot,
    pub stop_poll: PhaseTimingSnapshot,
    pub control_readiness: PhaseTimingSnapshot,
    pub control_check_to_prepared_readiness: PhaseTimingSnapshot,
    pub prepared_readiness_to_batch: PhaseTimingSnapshot,
    pub prepared_batch_to_entry: PhaseTimingSnapshot,
    pub control_check_to_prepared_entry: PhaseTimingSnapshot,
}

impl PreparedTxSchedulerTimingSnapshot {
    fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            samples: self.samples.wrapping_sub(earlier.samples),
            scheduler_passes: self.scheduler_passes.wrapping_sub(earlier.scheduler_passes),
            scheduler_passes_lifetime_max: self.scheduler_passes_lifetime_max,
            control_ready_passes: self
                .control_ready_passes
                .wrapping_sub(earlier.control_ready_passes),
            multi_pass_samples: self
                .multi_pass_samples
                .wrapping_sub(earlier.multi_pass_samples),
            completion_to_active_service_return: self
                .completion_to_active_service_return
                .wrapping_delta_since(earlier.completion_to_active_service_return),
            active_service_return_to_first_pass: self
                .active_service_return_to_first_pass
                .wrapping_delta_since(earlier.active_service_return_to_first_pass),
            additional_passes: self
                .additional_passes
                .wrapping_delta_since(earlier.additional_passes),
            stop_poll: self.stop_poll.wrapping_delta_since(earlier.stop_poll),
            control_readiness: self
                .control_readiness
                .wrapping_delta_since(earlier.control_readiness),
            control_check_to_prepared_readiness: self
                .control_check_to_prepared_readiness
                .wrapping_delta_since(earlier.control_check_to_prepared_readiness),
            prepared_readiness_to_batch: self
                .prepared_readiness_to_batch
                .wrapping_delta_since(earlier.prepared_readiness_to_batch),
            prepared_batch_to_entry: self
                .prepared_batch_to_entry
                .wrapping_delta_since(earlier.prepared_batch_to_entry),
            control_check_to_prepared_entry: self
                .control_check_to_prepared_entry
                .wrapping_delta_since(earlier.control_check_to_prepared_entry),
        }
    }
}

/// One coherent-enough diagnostic observation of [`AggregateTxCounters`].
///
/// The fields may straddle a live TX update, so interval qualification must
/// tolerate one aggregate crossing a sample boundary. Individual counters
/// remain exact monotonic observations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AggregateTxCounterSnapshot {
    pub station_terminal: oer_hil_protocol::network::StationTxTerminalEvidence,
    pub secondary_socket: crate::tx_progress::Snapshot,
    pub secondary_claim: crate::tx_progress::Snapshot,
    pub ap_udp_claimed: u32,
    pub ap_udp_claim_backward: u32,
    pub ap_udp_claim_first_previous: u32,
    pub ap_udp_claim_first_sequence: u32,
    pub ap_udp_claim_maximum_distance: u32,
    /// Current operational TX BlockAck TID bitmap, not an interval delta.
    pub block_ack_operational_tids: u32,
    pub block_ack_operational_transitions: u32,
    pub network_single_mpdu_started: u32,
    pub network_single_legacy_rate: u32,
    pub network_single_block_ack_unavailable: u32,
    pub network_single_ht_needs_pair: u32,
    pub network_single_fresh_aggregate_capacity: u32,
    /// Maximum rejected Ethernet length since boot, not an interval delta.
    pub network_single_fresh_capacity_lifetime_max_ethernet_length: u32,
    pub rate_selections: u32,
    /// Most recent actual aggregate vector, not an interval delta.
    pub last_bandwidth_mhz: u32,
    /// Most recent actual aggregate vector, not an interval delta.
    pub last_nominal_rate_kbps: u32,
    pub aggregates_prepared: u32,
    pub standby_prepared: u32,
    pub standby_published: u32,
    pub standby_cancelled: u32,
    pub standby_pending_start: u32,
    pub standby_pending_end: u32,
    pub aggregate_publications: u32,
    /// Outstanding publications at the interval boundaries, not deltas.
    pub publications_pending_start: u32,
    pub publications_pending_end: u32,
    pub aggregates_completed: u32,
    pub subframes_acknowledged: u32,
    pub individual_retries: u32,
    pub hardware_timeouts: u32,
    pub collisions: u32,
    pub preparation_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub preparation_lifetime_max_micros: u32,
    pub publication_program_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub publication_program_lifetime_max_micros: u32,
    pub completion_to_publication_samples: u32,
    pub completion_to_publication_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub completion_to_publication_lifetime_max_micros: u32,
    /// Samples at or below each [`COMPLETION_TO_PUBLICATION_BOUNDS_MICROS`]
    /// bound, then above the last bound.
    pub completion_to_publication_histogram: [u32; COMPLETION_TO_PUBLICATION_BUCKETS],
    /// Publications whose successor was not entered through the prepared
    /// path, with their completion-to-publication time.
    pub unprepared_publication_samples: u32,
    pub unprepared_publication_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub unprepared_publication_lifetime_max_micros: u32,
    pub completion_to_prepared_entry_samples: u32,
    pub completion_to_prepared_entry_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub completion_to_prepared_entry_lifetime_max_micros: u32,
    pub prepared_entry_to_publication_samples: u32,
    pub prepared_entry_to_publication_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub prepared_entry_to_publication_lifetime_max_micros: u32,
    pub prepared_scheduler_timing: PreparedTxSchedulerTimingSnapshot,
    pub completion_core_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub completion_core_lifetime_max_micros: u32,
    pub backing_release_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub backing_release_lifetime_max_micros: u32,
    pub work: AggregateTxWorkSnapshot,
    pub ordinary_work: AggregateTxWorkSnapshot,
    pub station_ordinary: StationOrdinarySnapshot,
    pub exchange_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub exchange_lifetime_max_micros: u32,
    pub single_publication_exchanges: u32,
    pub single_publication_exchange_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub single_publication_exchange_lifetime_max_micros: u32,
    pub retried_exchanges: u32,
    pub retried_exchange_publications: u32,
    pub retried_exchange_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub retried_exchange_lifetime_max_micros: u32,
    /// Index is the publication count; the last bucket represents `4+`.
    pub exchanges_by_publications: [u32; AGGREGATE_TX_PUBLICATION_BUCKETS],
    pub exchange_micros_by_publications: [u32; AGGREGATE_TX_PUBLICATION_BUCKETS],
    /// Lifetime maxima by publication-count bucket, not interval deltas.
    pub exchange_lifetime_max_micros_by_publications: [u32; AGGREGATE_TX_PUBLICATION_BUCKETS],
    pub block_ack_samples: u32,
    pub block_ack_received: u32,
    pub success_without_block_ack: u32,
    pub nonzero_block_ack_control: u32,
    pub block_ack_start_outside_window: u32,
    /// Maximum backward distance since boot, not an interval delta.
    pub block_ack_start_lag_max: u32,
    pub full_block_ack: u32,
    pub partial_block_ack: u32,
    /// Partial BlockAcks missing each original aggregate position.
    pub partial_missing_by_position: [u32; BLOCK_ACK_POSITIONS],
    /// Partial BlockAcks by missing count: 1, 2, 3-4, 5-8, 9 or more.
    pub partial_missing_counts: [u32; PARTIAL_MISSING_BUCKETS],
    pub empty_block_ack: u32,
    /// BlockAckReqs sent for aggregates whose protection exchange failed at
    /// every attempt, and those a BlockAck answered.
    pub block_ack_requests: u32,
    pub block_ack_requests_answered: u32,
    /// Response SNR of BlockAcks that acknowledged every original subframe.
    pub full_block_ack_snr: BlockAckSnrSnapshot,
    /// Response SNR of BlockAcks that left some original subframes missing.
    pub partial_block_ack_snr: BlockAckSnrSnapshot,
    /// Completions whose status carried no response SNR.
    pub block_ack_snr_unavailable: u32,
    pub tx_irq_epochs: u32,
    pub tx_irq_service_samples: u32,
    pub tx_irq_clock_skew_samples: u32,
    pub tx_irq_to_service_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub tx_irq_to_service_lifetime_max_micros: u32,
    pub tx_publication_to_irq_samples: u32,
    pub tx_publication_to_irq_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub tx_publication_to_irq_lifetime_max_micros: u32,
    pub stopped_at_frame_limit: u32,
    pub stopped_at_capacity_limit: u32,
    pub stopped_on_empty_queue: u32,
    pub prepared_subframes: [u32; AGGREGATE_TX_HISTOGRAM_BUCKETS],
}

impl AggregateTxCounterSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            station_terminal: terminal::delta(self.station_terminal, earlier.station_terminal),
            ap_udp_claimed: self.ap_udp_claimed.wrapping_sub(earlier.ap_udp_claimed),
            ap_udp_claim_backward: self
                .ap_udp_claim_backward
                .wrapping_sub(earlier.ap_udp_claim_backward),
            ap_udp_claim_first_previous: self.ap_udp_claim_first_previous,
            ap_udp_claim_first_sequence: self.ap_udp_claim_first_sequence,
            ap_udp_claim_maximum_distance: self.ap_udp_claim_maximum_distance,
            block_ack_operational_tids: self.block_ack_operational_tids,
            block_ack_operational_transitions: self
                .block_ack_operational_transitions
                .wrapping_sub(earlier.block_ack_operational_transitions),
            network_single_mpdu_started: self
                .network_single_mpdu_started
                .wrapping_sub(earlier.network_single_mpdu_started),
            network_single_legacy_rate: self
                .network_single_legacy_rate
                .wrapping_sub(earlier.network_single_legacy_rate),
            network_single_block_ack_unavailable: self
                .network_single_block_ack_unavailable
                .wrapping_sub(earlier.network_single_block_ack_unavailable),
            network_single_ht_needs_pair: self
                .network_single_ht_needs_pair
                .wrapping_sub(earlier.network_single_ht_needs_pair),
            network_single_fresh_aggregate_capacity: self
                .network_single_fresh_aggregate_capacity
                .wrapping_sub(earlier.network_single_fresh_aggregate_capacity),
            network_single_fresh_capacity_lifetime_max_ethernet_length: self
                .network_single_fresh_capacity_lifetime_max_ethernet_length,
            rate_selections: self.rate_selections.wrapping_sub(earlier.rate_selections),
            last_bandwidth_mhz: self.last_bandwidth_mhz,
            last_nominal_rate_kbps: self.last_nominal_rate_kbps,
            aggregates_prepared: self
                .aggregates_prepared
                .wrapping_sub(earlier.aggregates_prepared),
            standby_prepared: self.standby_prepared.wrapping_sub(earlier.standby_prepared),
            standby_published: self
                .standby_published
                .wrapping_sub(earlier.standby_published),
            standby_cancelled: self
                .standby_cancelled
                .wrapping_sub(earlier.standby_cancelled),
            aggregate_publications: self
                .aggregate_publications
                .wrapping_sub(earlier.aggregate_publications),
            secondary_socket: self.secondary_socket,
            secondary_claim: self.secondary_claim,
            standby_pending_start: earlier.standby_pending_end,
            standby_pending_end: self.standby_pending_end,
            publications_pending_start: earlier.publications_pending_end,
            publications_pending_end: self.publications_pending_end,
            aggregates_completed: self
                .aggregates_completed
                .wrapping_sub(earlier.aggregates_completed),
            subframes_acknowledged: self
                .subframes_acknowledged
                .wrapping_sub(earlier.subframes_acknowledged),
            individual_retries: self
                .individual_retries
                .wrapping_sub(earlier.individual_retries),
            hardware_timeouts: self
                .hardware_timeouts
                .wrapping_sub(earlier.hardware_timeouts),
            collisions: self.collisions.wrapping_sub(earlier.collisions),
            preparation_micros: self
                .preparation_micros
                .wrapping_sub(earlier.preparation_micros),
            preparation_lifetime_max_micros: self.preparation_lifetime_max_micros,
            publication_program_micros: self
                .publication_program_micros
                .wrapping_sub(earlier.publication_program_micros),
            publication_program_lifetime_max_micros: self.publication_program_lifetime_max_micros,
            completion_to_publication_samples: self
                .completion_to_publication_samples
                .wrapping_sub(earlier.completion_to_publication_samples),
            completion_to_publication_micros: self
                .completion_to_publication_micros
                .wrapping_sub(earlier.completion_to_publication_micros),
            completion_to_publication_lifetime_max_micros: self
                .completion_to_publication_lifetime_max_micros,
            completion_to_publication_histogram: core::array::from_fn(|bucket| {
                self.completion_to_publication_histogram[bucket]
                    .wrapping_sub(earlier.completion_to_publication_histogram[bucket])
            }),
            unprepared_publication_samples: self
                .unprepared_publication_samples
                .wrapping_sub(earlier.unprepared_publication_samples),
            unprepared_publication_micros: self
                .unprepared_publication_micros
                .wrapping_sub(earlier.unprepared_publication_micros),
            unprepared_publication_lifetime_max_micros: self
                .unprepared_publication_lifetime_max_micros,
            completion_to_prepared_entry_samples: self
                .completion_to_prepared_entry_samples
                .wrapping_sub(earlier.completion_to_prepared_entry_samples),
            completion_to_prepared_entry_micros: self
                .completion_to_prepared_entry_micros
                .wrapping_sub(earlier.completion_to_prepared_entry_micros),
            completion_to_prepared_entry_lifetime_max_micros: self
                .completion_to_prepared_entry_lifetime_max_micros,
            prepared_entry_to_publication_samples: self
                .prepared_entry_to_publication_samples
                .wrapping_sub(earlier.prepared_entry_to_publication_samples),
            prepared_entry_to_publication_micros: self
                .prepared_entry_to_publication_micros
                .wrapping_sub(earlier.prepared_entry_to_publication_micros),
            prepared_entry_to_publication_lifetime_max_micros: self
                .prepared_entry_to_publication_lifetime_max_micros,
            prepared_scheduler_timing: self
                .prepared_scheduler_timing
                .wrapping_delta_since(earlier.prepared_scheduler_timing),
            completion_core_micros: self
                .completion_core_micros
                .wrapping_sub(earlier.completion_core_micros),
            completion_core_lifetime_max_micros: self.completion_core_lifetime_max_micros,
            backing_release_micros: self
                .backing_release_micros
                .wrapping_sub(earlier.backing_release_micros),
            backing_release_lifetime_max_micros: self.backing_release_lifetime_max_micros,
            work: self.work.delta_since(earlier.work),
            ordinary_work: self.ordinary_work.delta_since(earlier.ordinary_work),
            station_ordinary: self.station_ordinary.delta_since(earlier.station_ordinary),
            exchange_micros: self.exchange_micros.wrapping_sub(earlier.exchange_micros),
            exchange_lifetime_max_micros: self.exchange_lifetime_max_micros,
            single_publication_exchanges: self
                .single_publication_exchanges
                .wrapping_sub(earlier.single_publication_exchanges),
            single_publication_exchange_micros: self
                .single_publication_exchange_micros
                .wrapping_sub(earlier.single_publication_exchange_micros),
            single_publication_exchange_lifetime_max_micros: self
                .single_publication_exchange_lifetime_max_micros,
            retried_exchanges: self
                .retried_exchanges
                .wrapping_sub(earlier.retried_exchanges),
            retried_exchange_publications: self
                .retried_exchange_publications
                .wrapping_sub(earlier.retried_exchange_publications),
            retried_exchange_micros: self
                .retried_exchange_micros
                .wrapping_sub(earlier.retried_exchange_micros),
            retried_exchange_lifetime_max_micros: self.retried_exchange_lifetime_max_micros,
            exchanges_by_publications: core::array::from_fn(|index| {
                self.exchanges_by_publications[index]
                    .wrapping_sub(earlier.exchanges_by_publications[index])
            }),
            exchange_micros_by_publications: core::array::from_fn(|index| {
                self.exchange_micros_by_publications[index]
                    .wrapping_sub(earlier.exchange_micros_by_publications[index])
            }),
            exchange_lifetime_max_micros_by_publications: self
                .exchange_lifetime_max_micros_by_publications,
            block_ack_samples: self
                .block_ack_samples
                .wrapping_sub(earlier.block_ack_samples),
            block_ack_received: self
                .block_ack_received
                .wrapping_sub(earlier.block_ack_received),
            success_without_block_ack: self
                .success_without_block_ack
                .wrapping_sub(earlier.success_without_block_ack),
            nonzero_block_ack_control: self
                .nonzero_block_ack_control
                .wrapping_sub(earlier.nonzero_block_ack_control),
            block_ack_start_outside_window: self
                .block_ack_start_outside_window
                .wrapping_sub(earlier.block_ack_start_outside_window),
            block_ack_start_lag_max: self.block_ack_start_lag_max,
            full_block_ack: self.full_block_ack.wrapping_sub(earlier.full_block_ack),
            partial_block_ack: self
                .partial_block_ack
                .wrapping_sub(earlier.partial_block_ack),
            partial_missing_by_position: core::array::from_fn(|position| {
                self.partial_missing_by_position[position]
                    .wrapping_sub(earlier.partial_missing_by_position[position])
            }),
            partial_missing_counts: core::array::from_fn(|bucket| {
                self.partial_missing_counts[bucket]
                    .wrapping_sub(earlier.partial_missing_counts[bucket])
            }),
            empty_block_ack: self.empty_block_ack.wrapping_sub(earlier.empty_block_ack),
            block_ack_requests: self
                .block_ack_requests
                .wrapping_sub(earlier.block_ack_requests),
            block_ack_requests_answered: self
                .block_ack_requests_answered
                .wrapping_sub(earlier.block_ack_requests_answered),
            full_block_ack_snr: self.full_block_ack_snr.delta(&earlier.full_block_ack_snr),
            partial_block_ack_snr: self
                .partial_block_ack_snr
                .delta(&earlier.partial_block_ack_snr),
            block_ack_snr_unavailable: self
                .block_ack_snr_unavailable
                .wrapping_sub(earlier.block_ack_snr_unavailable),
            tx_irq_epochs: self.tx_irq_epochs.wrapping_sub(earlier.tx_irq_epochs),
            tx_irq_service_samples: self
                .tx_irq_service_samples
                .wrapping_sub(earlier.tx_irq_service_samples),
            tx_irq_clock_skew_samples: self
                .tx_irq_clock_skew_samples
                .wrapping_sub(earlier.tx_irq_clock_skew_samples),
            tx_irq_to_service_micros: self
                .tx_irq_to_service_micros
                .wrapping_sub(earlier.tx_irq_to_service_micros),
            tx_irq_to_service_lifetime_max_micros: self.tx_irq_to_service_lifetime_max_micros,
            tx_publication_to_irq_samples: self
                .tx_publication_to_irq_samples
                .wrapping_sub(earlier.tx_publication_to_irq_samples),
            tx_publication_to_irq_micros: self
                .tx_publication_to_irq_micros
                .wrapping_sub(earlier.tx_publication_to_irq_micros),
            tx_publication_to_irq_lifetime_max_micros: self
                .tx_publication_to_irq_lifetime_max_micros,
            stopped_at_frame_limit: self
                .stopped_at_frame_limit
                .wrapping_sub(earlier.stopped_at_frame_limit),
            stopped_at_capacity_limit: self
                .stopped_at_capacity_limit
                .wrapping_sub(earlier.stopped_at_capacity_limit),
            stopped_on_empty_queue: self
                .stopped_on_empty_queue
                .wrapping_sub(earlier.stopped_on_empty_queue),
            prepared_subframes: core::array::from_fn(|index| {
                self.prepared_subframes[index].wrapping_sub(earlier.prepared_subframes[index])
            }),
        }
    }

    pub fn prepared_subframe_total(&self) -> u32 {
        self.prepared_subframes
            .iter()
            .enumerate()
            .map(|(subframes, count)| (subframes as u32).saturating_mul(*count))
            .fold(0, u32::saturating_add)
    }

    pub const fn block_ack_operational(&self, tid: u8) -> bool {
        let mask = match 1_u32.checked_shl(tid as u32) {
            Some(mask) => mask,
            None => 0,
        };
        self.block_ack_operational_tids & mask != 0
    }

    pub fn prepared_in_range(&self, minimum: usize, maximum: usize) -> u32 {
        let start = minimum.clamp(1, AGGREGATE_TX_HISTOGRAM_BUCKETS);
        let end = maximum
            .saturating_add(1)
            .min(AGGREGATE_TX_HISTOGRAM_BUCKETS);
        self.prepared_subframes[start..end]
            .iter()
            .copied()
            .fold(0, u32::saturating_add)
    }

    pub fn minimum_prepared_subframes(&self) -> Option<u8> {
        self.prepared_subframes
            .iter()
            .enumerate()
            .skip(1)
            .find_map(|(subframes, count)| (*count != 0).then_some(subframes as u8))
    }

    pub fn maximum_prepared_subframes(&self) -> Option<u8> {
        self.prepared_subframes
            .iter()
            .enumerate()
            .skip(1)
            .rev()
            .find_map(|(subframes, count)| (*count != 0).then_some(subframes as u8))
    }
}

#[cfg(test)]
mod tests;
