// The recorded subset and its readers depend on the selected telemetry
// features; production builds read none of it.
#![cfg_attr(not(feature = "tx-phase-telemetry"), allow(dead_code))]

//! Paired Core0 cycle and retired-instruction accounting.
//!
//! `mcycle` alone measures executor residence, including cache and memory
//! stalls. Pairing it with `minstret` over the same coarse ownership intervals
//! exposes whether a busy Core0 is executing instructions or waiting. The
//! reads deliberately stay coarse: per-frame CSR sampling would perturb the
//! datapath which this diagnostic image is intended to measure.
//!
//! The module is always compiled. Each recording method returns immediately
//! unless its telemetry feature is enabled, and samples read zero without a
//! telemetry feature, so production builds perform no CSR reads and retain
//! no counters.

use core::sync::atomic::{AtomicU32, Ordering};

/// Whether TX-phase samples are recorded.
const TX_PHASES: bool = cfg!(feature = "tx-phase-telemetry");
/// Whether the coarse RX runner, DMA and interrupt counters are recorded.
const COARSE: bool = cfg!(feature = "core0-rx-coarse-telemetry");
/// Whether any Core0 cycle telemetry samples DMA service entry.
const SAMPLING: bool = cfg!(any(
    feature = "core0-rx-coarse-telemetry",
    feature = "task-poll-telemetry"
));
/// Call sites whose telemetry arguments require computation guard them with
/// this constant; production builds compile the branch out.
pub(crate) const TX_PHASE_TELEMETRY: bool = TX_PHASES;

/// The completion edge of a network transaction that already had a prepared
/// successor. Zero-sized without TX-phase telemetry.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Core0PreparedTxMark {
    #[cfg(feature = "tx-phase-telemetry")]
    completed: Option<Core0PerformanceSample>,
}

impl Core0PreparedTxMark {
    pub(crate) const fn new() -> Self {
        Self {
            #[cfg(feature = "tx-phase-telemetry")]
            completed: None,
        }
    }

    /// Mark a completion now, only when a successor was already prepared.
    #[inline(always)]
    pub(crate) fn mark(&mut self, prepared: bool) {
        #[cfg(feature = "tx-phase-telemetry")]
        {
            self.completed = prepared.then(Core0PerformanceSample::read);
        }
        #[cfg(not(feature = "tx-phase-telemetry"))]
        let _ = prepared;
    }

    /// Record the gap from the marked completion to the next publication.
    #[inline(always)]
    pub(crate) fn record_gap(&mut self) {
        #[cfg(feature = "tx-phase-telemetry")]
        if let Some(completed) = self.completed.take() {
            CORE0_PERFORMANCE.record_tx_prepared_gap(completed, Core0PerformanceSample::read());
        }
    }
}

/// Frames that took one RX protocol path in a turn. Zero-sized and always
/// zero without coarse Core0 telemetry.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Core0PathCount {
    #[cfg(feature = "core0-rx-coarse-telemetry")]
    value: usize,
}

impl Core0PathCount {
    #[inline(always)]
    pub fn increment(&mut self) {
        #[cfg(feature = "core0-rx-coarse-telemetry")]
        {
            self.value = self.value.saturating_add(1);
        }
    }

    #[inline(always)]
    pub const fn value(self) -> usize {
        #[cfg(feature = "core0-rx-coarse-telemetry")]
        {
            self.value
        }
        #[cfg(not(feature = "core0-rx-coarse-telemetry"))]
        {
            0
        }
    }
}

/// A saturating diagnostic count held by a datapath owner. Zero-sized and
/// always zero without TX-phase telemetry.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Core0Tally {
    #[cfg(feature = "tx-phase-telemetry")]
    value: usize,
}

impl Core0Tally {
    pub(crate) const fn new() -> Self {
        Self {
            #[cfg(feature = "tx-phase-telemetry")]
            value: 0,
        }
    }

    #[inline(always)]
    pub(crate) fn increment(&mut self) {
        #[cfg(feature = "tx-phase-telemetry")]
        {
            self.value = self.value.saturating_add(1);
        }
    }

    #[inline(always)]
    pub(crate) const fn value(self) -> usize {
        #[cfg(feature = "tx-phase-telemetry")]
        {
            self.value
        }
        #[cfg(not(feature = "tx-phase-telemetry"))]
        {
            0
        }
    }
}

/// One paired `mcycle`/`minstret` reading. Zero-sized without a sampling
/// telemetry feature, so samples held across an await cost no future state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Core0PerformanceSample {
    #[cfg(any(feature = "core0-rx-coarse-telemetry", feature = "task-poll-telemetry"))]
    cycles: u32,
    #[cfg(any(feature = "core0-rx-coarse-telemetry", feature = "task-poll-telemetry"))]
    instructions: u32,
}

impl Core0PerformanceSample {
    #[inline(always)]
    pub fn read() -> Self {
        Self::from_parts(cycle_count(), instruction_count())
    }

    #[inline(always)]
    const fn from_parts(cycles: u32, instructions: u32) -> Self {
        #[cfg(any(feature = "core0-rx-coarse-telemetry", feature = "task-poll-telemetry"))]
        {
            Self {
                cycles,
                instructions,
            }
        }
        #[cfg(not(any(feature = "core0-rx-coarse-telemetry", feature = "task-poll-telemetry")))]
        {
            let _ = (cycles, instructions);
            Self {}
        }
    }

    /// Retired cycles; zero without a sampling telemetry feature.
    #[inline(always)]
    pub const fn cycles(self) -> u32 {
        #[cfg(any(feature = "core0-rx-coarse-telemetry", feature = "task-poll-telemetry"))]
        {
            self.cycles
        }
        #[cfg(not(any(feature = "core0-rx-coarse-telemetry", feature = "task-poll-telemetry")))]
        {
            0
        }
    }

    /// Retired instructions; zero without a sampling telemetry feature.
    #[inline(always)]
    pub const fn instructions(self) -> u32 {
        #[cfg(any(feature = "core0-rx-coarse-telemetry", feature = "task-poll-telemetry"))]
        {
            self.instructions
        }
        #[cfg(not(any(feature = "core0-rx-coarse-telemetry", feature = "task-poll-telemetry")))]
        {
            0
        }
    }

    #[inline(always)]
    fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self::from_parts(
            self.cycles().wrapping_sub(earlier.cycles()),
            self.instructions().wrapping_sub(earlier.instructions()),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Core0TxPhase {
    Start,
    Prepare,
    Publish,
    Service,
    Encode,
    Commit,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Core0PerformanceSnapshot {
    pub rx_interrupt_posts: u32,
    pub radio_polls: u32,
    pub radio_cycles: u32,
    pub radio_instructions: u32,
    pub poll_to_runner_cycles: u32,
    pub poll_to_runner_instructions: u32,
    pub runner_to_poll_exit_cycles: u32,
    pub runner_to_poll_exit_instructions: u32,
    pub runner_calls: u32,
    pub runner_cycles: u32,
    pub runner_instructions: u32,
    pub protocol_polls: u32,
    pub protocol_cycles: u32,
    pub protocol_instructions: u32,
    pub direct_protocol_frames: u32,
    pub asynchronous_protocol_frames: u32,
    pub dma_calls: u32,
    pub dma_empty_calls: u32,
    pub dma_single_unit_calls: u32,
    pub dma_two_unit_calls: u32,
    pub dma_three_to_seven_unit_calls: u32,
    pub dma_eight_plus_unit_calls: u32,
    pub dma_units: u32,
    pub dma_cycles: u32,
    pub dma_instructions: u32,
    pub protocol_frames: u32,
    pub protocol_frame_cycles: u32,
    pub protocol_frame_instructions: u32,
    pub tx_start_calls: u32,
    pub tx_start_cycles: u32,
    pub tx_start_instructions: u32,
    pub tx_prepare_calls: u32,
    pub tx_prepare_cycles: u32,
    pub tx_prepare_instructions: u32,
    pub tx_publish_calls: u32,
    pub tx_publish_cycles: u32,
    pub tx_publish_instructions: u32,
    pub tx_service_calls: u32,
    pub tx_service_cycles: u32,
    pub tx_service_instructions: u32,
    pub tx_encode_calls: u32,
    pub tx_encode_cycles: u32,
    pub tx_encode_instructions: u32,
    pub tx_commit_calls: u32,
    pub tx_commit_cycles: u32,
    pub tx_commit_instructions: u32,
    pub tx_prepared_gap_samples: u32,
    pub tx_prepared_gap_cycles: u32,
    pub tx_prepared_gap_instructions: u32,
    pub tx_prepared_gap_le_64us: u32,
    pub tx_prepared_gap_le_256us: u32,
    pub tx_prepared_gap_le_512us: u32,
    pub tx_prepared_gap_le_1024us: u32,
    pub tx_prepared_gap_gt_1024us: u32,
    pub tx_network_completions: u32,
    pub tx_completion_prepared: u32,
    pub tx_completion_prepared_full: u32,
    pub tx_completion_prepared_partial: u32,
    pub tx_completion_prepared_frames: u32,
    pub tx_completion_queued: u32,
    pub tx_completion_empty: u32,
    pub tx_initial_network_frames: u32,
    pub tx_ap_partial_frontiers: u32,
    pub tx_ap_partial_matching_retained: u32,
    pub tx_ap_partial_other_retained: u32,
    pub tx_ap_partial_network_ready: u32,
    pub tx_ap_partial_mismatch_claims: u32,
    pub tx_ap_partial_publications: u32,
    pub tx_ap_publication_admitted: u32,
    pub tx_ap_publication_pool_free: u32,
    pub tx_ap_publication_ready_same: u32,
    pub tx_ap_publication_ready_other: u32,
    pub tx_ap_publication_ingress_reserved: u32,
    pub tx_ap_publication_application_reserved: u32,
    pub tx_ap_publication_tokens_in_flight: u32,
    pub tx_ap_publication_radio_owned: u32,
    pub tx_ap_publication_unattributed_radio_owned: u32,
    pub rx_progress_drained: u32,
    pub rx_progress_probe_pending: u32,
    pub rx_progress_protocol_tx_blocked: u32,
    pub rx_progress_recycled_append_pending: u32,
    pub rx_progress_budget_exhausted: u32,
    pub rx_progress_stage_blocked: u32,
    pub rx_progress_network_blocked: u32,
    pub rx_progress_droppable: u32,
    pub dma_probe_recycled: u32,
    pub dma_probe_completed_frontier: u32,
    pub dma_probe_terminal_writeback: u32,
    pub dma_probe_republication: u32,
    pub adaptive_probe_delay_64: u32,
    pub adaptive_probe_delay_128: u32,
    pub adaptive_probe_delay_256: u32,
    pub adaptive_probe_delay_512: u32,
    pub adaptive_probe_delay_other: u32,
    pub adaptive_probe_empty_work: u32,
    pub adaptive_probe_work_units: u32,
    pub adaptive_probe_staged_bytes: u32,
    pub dma_entry_remaining_exhausted: u32,
    pub dma_entry_remaining_1_8: u32,
    pub dma_entry_remaining_9_16: u32,
    pub dma_entry_remaining_17_32: u32,
    pub dma_entry_remaining_33_48: u32,
    pub dma_entry_remaining_49_plus: u32,
    pub dma_entry_remaining_unknown: u32,
    pub dma_exhaustion_episodes: u32,
    pub dma_exhaustion_resolved_le_64us: u32,
    pub dma_exhaustion_resolved_le_256us: u32,
    pub dma_exhaustion_resolved_le_1024us: u32,
    pub dma_exhaustion_resolved_gt_1024us: u32,
}

impl Core0PerformanceSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            rx_interrupt_posts: self
                .rx_interrupt_posts
                .wrapping_sub(earlier.rx_interrupt_posts),
            radio_polls: self.radio_polls.wrapping_sub(earlier.radio_polls),
            radio_cycles: self.radio_cycles.wrapping_sub(earlier.radio_cycles),
            radio_instructions: self
                .radio_instructions
                .wrapping_sub(earlier.radio_instructions),
            poll_to_runner_cycles: self
                .poll_to_runner_cycles
                .wrapping_sub(earlier.poll_to_runner_cycles),
            poll_to_runner_instructions: self
                .poll_to_runner_instructions
                .wrapping_sub(earlier.poll_to_runner_instructions),
            runner_to_poll_exit_cycles: self
                .runner_to_poll_exit_cycles
                .wrapping_sub(earlier.runner_to_poll_exit_cycles),
            runner_to_poll_exit_instructions: self
                .runner_to_poll_exit_instructions
                .wrapping_sub(earlier.runner_to_poll_exit_instructions),
            runner_calls: self.runner_calls.wrapping_sub(earlier.runner_calls),
            runner_cycles: self.runner_cycles.wrapping_sub(earlier.runner_cycles),
            runner_instructions: self
                .runner_instructions
                .wrapping_sub(earlier.runner_instructions),
            protocol_polls: self.protocol_polls.wrapping_sub(earlier.protocol_polls),
            protocol_cycles: self.protocol_cycles.wrapping_sub(earlier.protocol_cycles),
            protocol_instructions: self
                .protocol_instructions
                .wrapping_sub(earlier.protocol_instructions),
            direct_protocol_frames: self
                .direct_protocol_frames
                .wrapping_sub(earlier.direct_protocol_frames),
            asynchronous_protocol_frames: self
                .asynchronous_protocol_frames
                .wrapping_sub(earlier.asynchronous_protocol_frames),
            dma_calls: self.dma_calls.wrapping_sub(earlier.dma_calls),
            dma_empty_calls: self.dma_empty_calls.wrapping_sub(earlier.dma_empty_calls),
            dma_single_unit_calls: self
                .dma_single_unit_calls
                .wrapping_sub(earlier.dma_single_unit_calls),
            dma_two_unit_calls: self
                .dma_two_unit_calls
                .wrapping_sub(earlier.dma_two_unit_calls),
            dma_three_to_seven_unit_calls: self
                .dma_three_to_seven_unit_calls
                .wrapping_sub(earlier.dma_three_to_seven_unit_calls),
            dma_eight_plus_unit_calls: self
                .dma_eight_plus_unit_calls
                .wrapping_sub(earlier.dma_eight_plus_unit_calls),
            dma_units: self.dma_units.wrapping_sub(earlier.dma_units),
            dma_cycles: self.dma_cycles.wrapping_sub(earlier.dma_cycles),
            dma_instructions: self.dma_instructions.wrapping_sub(earlier.dma_instructions),
            protocol_frames: self.protocol_frames.wrapping_sub(earlier.protocol_frames),
            protocol_frame_cycles: self
                .protocol_frame_cycles
                .wrapping_sub(earlier.protocol_frame_cycles),
            protocol_frame_instructions: self
                .protocol_frame_instructions
                .wrapping_sub(earlier.protocol_frame_instructions),
            tx_start_calls: self.tx_start_calls.wrapping_sub(earlier.tx_start_calls),
            tx_start_cycles: self.tx_start_cycles.wrapping_sub(earlier.tx_start_cycles),
            tx_start_instructions: self
                .tx_start_instructions
                .wrapping_sub(earlier.tx_start_instructions),
            tx_prepare_calls: self.tx_prepare_calls.wrapping_sub(earlier.tx_prepare_calls),
            tx_prepare_cycles: self
                .tx_prepare_cycles
                .wrapping_sub(earlier.tx_prepare_cycles),
            tx_prepare_instructions: self
                .tx_prepare_instructions
                .wrapping_sub(earlier.tx_prepare_instructions),
            tx_publish_calls: self.tx_publish_calls.wrapping_sub(earlier.tx_publish_calls),
            tx_publish_cycles: self
                .tx_publish_cycles
                .wrapping_sub(earlier.tx_publish_cycles),
            tx_publish_instructions: self
                .tx_publish_instructions
                .wrapping_sub(earlier.tx_publish_instructions),
            tx_service_calls: self.tx_service_calls.wrapping_sub(earlier.tx_service_calls),
            tx_service_cycles: self
                .tx_service_cycles
                .wrapping_sub(earlier.tx_service_cycles),
            tx_service_instructions: self
                .tx_service_instructions
                .wrapping_sub(earlier.tx_service_instructions),
            tx_encode_calls: self.tx_encode_calls.wrapping_sub(earlier.tx_encode_calls),
            tx_encode_cycles: self.tx_encode_cycles.wrapping_sub(earlier.tx_encode_cycles),
            tx_encode_instructions: self
                .tx_encode_instructions
                .wrapping_sub(earlier.tx_encode_instructions),
            tx_commit_calls: self.tx_commit_calls.wrapping_sub(earlier.tx_commit_calls),
            tx_commit_cycles: self.tx_commit_cycles.wrapping_sub(earlier.tx_commit_cycles),
            tx_commit_instructions: self
                .tx_commit_instructions
                .wrapping_sub(earlier.tx_commit_instructions),
            tx_prepared_gap_samples: self
                .tx_prepared_gap_samples
                .wrapping_sub(earlier.tx_prepared_gap_samples),
            tx_prepared_gap_cycles: self
                .tx_prepared_gap_cycles
                .wrapping_sub(earlier.tx_prepared_gap_cycles),
            tx_prepared_gap_instructions: self
                .tx_prepared_gap_instructions
                .wrapping_sub(earlier.tx_prepared_gap_instructions),
            tx_prepared_gap_le_64us: self
                .tx_prepared_gap_le_64us
                .wrapping_sub(earlier.tx_prepared_gap_le_64us),
            tx_prepared_gap_le_256us: self
                .tx_prepared_gap_le_256us
                .wrapping_sub(earlier.tx_prepared_gap_le_256us),
            tx_prepared_gap_le_512us: self
                .tx_prepared_gap_le_512us
                .wrapping_sub(earlier.tx_prepared_gap_le_512us),
            tx_prepared_gap_le_1024us: self
                .tx_prepared_gap_le_1024us
                .wrapping_sub(earlier.tx_prepared_gap_le_1024us),
            tx_prepared_gap_gt_1024us: self
                .tx_prepared_gap_gt_1024us
                .wrapping_sub(earlier.tx_prepared_gap_gt_1024us),
            tx_network_completions: self
                .tx_network_completions
                .wrapping_sub(earlier.tx_network_completions),
            tx_completion_prepared: self
                .tx_completion_prepared
                .wrapping_sub(earlier.tx_completion_prepared),
            tx_completion_prepared_full: self
                .tx_completion_prepared_full
                .wrapping_sub(earlier.tx_completion_prepared_full),
            tx_completion_prepared_partial: self
                .tx_completion_prepared_partial
                .wrapping_sub(earlier.tx_completion_prepared_partial),
            tx_completion_prepared_frames: self
                .tx_completion_prepared_frames
                .wrapping_sub(earlier.tx_completion_prepared_frames),
            tx_completion_queued: self
                .tx_completion_queued
                .wrapping_sub(earlier.tx_completion_queued),
            tx_completion_empty: self
                .tx_completion_empty
                .wrapping_sub(earlier.tx_completion_empty),
            tx_initial_network_frames: self
                .tx_initial_network_frames
                .wrapping_sub(earlier.tx_initial_network_frames),
            tx_ap_partial_frontiers: self
                .tx_ap_partial_frontiers
                .wrapping_sub(earlier.tx_ap_partial_frontiers),
            tx_ap_partial_matching_retained: self
                .tx_ap_partial_matching_retained
                .wrapping_sub(earlier.tx_ap_partial_matching_retained),
            tx_ap_partial_other_retained: self
                .tx_ap_partial_other_retained
                .wrapping_sub(earlier.tx_ap_partial_other_retained),
            tx_ap_partial_network_ready: self
                .tx_ap_partial_network_ready
                .wrapping_sub(earlier.tx_ap_partial_network_ready),
            tx_ap_partial_mismatch_claims: self
                .tx_ap_partial_mismatch_claims
                .wrapping_sub(earlier.tx_ap_partial_mismatch_claims),
            tx_ap_partial_publications: self
                .tx_ap_partial_publications
                .wrapping_sub(earlier.tx_ap_partial_publications),
            tx_ap_publication_admitted: self
                .tx_ap_publication_admitted
                .wrapping_sub(earlier.tx_ap_publication_admitted),
            tx_ap_publication_pool_free: self
                .tx_ap_publication_pool_free
                .wrapping_sub(earlier.tx_ap_publication_pool_free),
            tx_ap_publication_ready_same: self
                .tx_ap_publication_ready_same
                .wrapping_sub(earlier.tx_ap_publication_ready_same),
            tx_ap_publication_ready_other: self
                .tx_ap_publication_ready_other
                .wrapping_sub(earlier.tx_ap_publication_ready_other),
            tx_ap_publication_ingress_reserved: self
                .tx_ap_publication_ingress_reserved
                .wrapping_sub(earlier.tx_ap_publication_ingress_reserved),
            tx_ap_publication_application_reserved: self
                .tx_ap_publication_application_reserved
                .wrapping_sub(earlier.tx_ap_publication_application_reserved),
            tx_ap_publication_tokens_in_flight: self
                .tx_ap_publication_tokens_in_flight
                .wrapping_sub(earlier.tx_ap_publication_tokens_in_flight),
            tx_ap_publication_radio_owned: self
                .tx_ap_publication_radio_owned
                .wrapping_sub(earlier.tx_ap_publication_radio_owned),
            tx_ap_publication_unattributed_radio_owned: self
                .tx_ap_publication_unattributed_radio_owned
                .wrapping_sub(earlier.tx_ap_publication_unattributed_radio_owned),
            rx_progress_drained: self
                .rx_progress_drained
                .wrapping_sub(earlier.rx_progress_drained),
            rx_progress_probe_pending: self
                .rx_progress_probe_pending
                .wrapping_sub(earlier.rx_progress_probe_pending),
            rx_progress_protocol_tx_blocked: self
                .rx_progress_protocol_tx_blocked
                .wrapping_sub(earlier.rx_progress_protocol_tx_blocked),
            rx_progress_recycled_append_pending: self
                .rx_progress_recycled_append_pending
                .wrapping_sub(earlier.rx_progress_recycled_append_pending),
            rx_progress_budget_exhausted: self
                .rx_progress_budget_exhausted
                .wrapping_sub(earlier.rx_progress_budget_exhausted),
            rx_progress_stage_blocked: self
                .rx_progress_stage_blocked
                .wrapping_sub(earlier.rx_progress_stage_blocked),
            rx_progress_network_blocked: self
                .rx_progress_network_blocked
                .wrapping_sub(earlier.rx_progress_network_blocked),
            rx_progress_droppable: self
                .rx_progress_droppable
                .wrapping_sub(earlier.rx_progress_droppable),
            dma_probe_recycled: self
                .dma_probe_recycled
                .wrapping_sub(earlier.dma_probe_recycled),
            dma_probe_completed_frontier: self
                .dma_probe_completed_frontier
                .wrapping_sub(earlier.dma_probe_completed_frontier),
            dma_probe_terminal_writeback: self
                .dma_probe_terminal_writeback
                .wrapping_sub(earlier.dma_probe_terminal_writeback),
            dma_probe_republication: self
                .dma_probe_republication
                .wrapping_sub(earlier.dma_probe_republication),
            adaptive_probe_delay_64: self
                .adaptive_probe_delay_64
                .wrapping_sub(earlier.adaptive_probe_delay_64),
            adaptive_probe_delay_128: self
                .adaptive_probe_delay_128
                .wrapping_sub(earlier.adaptive_probe_delay_128),
            adaptive_probe_delay_256: self
                .adaptive_probe_delay_256
                .wrapping_sub(earlier.adaptive_probe_delay_256),
            adaptive_probe_delay_512: self
                .adaptive_probe_delay_512
                .wrapping_sub(earlier.adaptive_probe_delay_512),
            adaptive_probe_delay_other: self
                .adaptive_probe_delay_other
                .wrapping_sub(earlier.adaptive_probe_delay_other),
            adaptive_probe_empty_work: self
                .adaptive_probe_empty_work
                .wrapping_sub(earlier.adaptive_probe_empty_work),
            adaptive_probe_work_units: self
                .adaptive_probe_work_units
                .wrapping_sub(earlier.adaptive_probe_work_units),
            adaptive_probe_staged_bytes: self
                .adaptive_probe_staged_bytes
                .wrapping_sub(earlier.adaptive_probe_staged_bytes),
            dma_entry_remaining_exhausted: self
                .dma_entry_remaining_exhausted
                .wrapping_sub(earlier.dma_entry_remaining_exhausted),
            dma_entry_remaining_1_8: self
                .dma_entry_remaining_1_8
                .wrapping_sub(earlier.dma_entry_remaining_1_8),
            dma_entry_remaining_9_16: self
                .dma_entry_remaining_9_16
                .wrapping_sub(earlier.dma_entry_remaining_9_16),
            dma_entry_remaining_17_32: self
                .dma_entry_remaining_17_32
                .wrapping_sub(earlier.dma_entry_remaining_17_32),
            dma_entry_remaining_33_48: self
                .dma_entry_remaining_33_48
                .wrapping_sub(earlier.dma_entry_remaining_33_48),
            dma_entry_remaining_49_plus: self
                .dma_entry_remaining_49_plus
                .wrapping_sub(earlier.dma_entry_remaining_49_plus),
            dma_entry_remaining_unknown: self
                .dma_entry_remaining_unknown
                .wrapping_sub(earlier.dma_entry_remaining_unknown),
            dma_exhaustion_episodes: self
                .dma_exhaustion_episodes
                .wrapping_sub(earlier.dma_exhaustion_episodes),
            dma_exhaustion_resolved_le_64us: self
                .dma_exhaustion_resolved_le_64us
                .wrapping_sub(earlier.dma_exhaustion_resolved_le_64us),
            dma_exhaustion_resolved_le_256us: self
                .dma_exhaustion_resolved_le_256us
                .wrapping_sub(earlier.dma_exhaustion_resolved_le_256us),
            dma_exhaustion_resolved_le_1024us: self
                .dma_exhaustion_resolved_le_1024us
                .wrapping_sub(earlier.dma_exhaustion_resolved_le_1024us),
            dma_exhaustion_resolved_gt_1024us: self
                .dma_exhaustion_resolved_gt_1024us
                .wrapping_sub(earlier.dma_exhaustion_resolved_gt_1024us),
        }
    }
}

pub struct Core0PerformanceCounters {
    rx_interrupt_posts: AtomicU32,
    radio_polls: AtomicU32,
    radio_cycles: AtomicU32,
    radio_instructions: AtomicU32,
    poll_to_runner_cycles: AtomicU32,
    poll_to_runner_instructions: AtomicU32,
    runner_to_poll_exit_cycles: AtomicU32,
    runner_to_poll_exit_instructions: AtomicU32,
    runner_calls: AtomicU32,
    runner_cycles: AtomicU32,
    runner_instructions: AtomicU32,
    protocol_polls: AtomicU32,
    protocol_cycles: AtomicU32,
    protocol_instructions: AtomicU32,
    direct_protocol_frames: AtomicU32,
    asynchronous_protocol_frames: AtomicU32,
    dma_calls: AtomicU32,
    dma_empty_calls: AtomicU32,
    dma_single_unit_calls: AtomicU32,
    dma_two_unit_calls: AtomicU32,
    dma_three_to_seven_unit_calls: AtomicU32,
    dma_eight_plus_unit_calls: AtomicU32,
    dma_units: AtomicU32,
    dma_cycles: AtomicU32,
    dma_instructions: AtomicU32,
    protocol_frames: AtomicU32,
    protocol_frame_cycles: AtomicU32,
    protocol_frame_instructions: AtomicU32,
    tx_start_calls: AtomicU32,
    tx_start_cycles: AtomicU32,
    tx_start_instructions: AtomicU32,
    tx_prepare_calls: AtomicU32,
    tx_prepare_cycles: AtomicU32,
    tx_prepare_instructions: AtomicU32,
    tx_publish_calls: AtomicU32,
    tx_publish_cycles: AtomicU32,
    tx_publish_instructions: AtomicU32,
    tx_service_calls: AtomicU32,
    tx_service_cycles: AtomicU32,
    tx_service_instructions: AtomicU32,
    tx_encode_calls: AtomicU32,
    tx_encode_cycles: AtomicU32,
    tx_encode_instructions: AtomicU32,
    tx_commit_calls: AtomicU32,
    tx_commit_cycles: AtomicU32,
    tx_commit_instructions: AtomicU32,
    tx_prepared_gap_samples: AtomicU32,
    tx_prepared_gap_cycles: AtomicU32,
    tx_prepared_gap_instructions: AtomicU32,
    tx_prepared_gap_le_64us: AtomicU32,
    tx_prepared_gap_le_256us: AtomicU32,
    tx_prepared_gap_le_512us: AtomicU32,
    tx_prepared_gap_le_1024us: AtomicU32,
    tx_prepared_gap_gt_1024us: AtomicU32,
    tx_network_completions: AtomicU32,
    tx_completion_prepared: AtomicU32,
    tx_completion_prepared_full: AtomicU32,
    tx_completion_prepared_partial: AtomicU32,
    tx_completion_prepared_frames: AtomicU32,
    tx_completion_queued: AtomicU32,
    tx_completion_empty: AtomicU32,
    tx_initial_network_frames: AtomicU32,
    tx_ap_partial_frontiers: AtomicU32,
    tx_ap_partial_matching_retained: AtomicU32,
    tx_ap_partial_other_retained: AtomicU32,
    tx_ap_partial_network_ready: AtomicU32,
    tx_ap_partial_mismatch_claims: AtomicU32,
    tx_ap_partial_publications: AtomicU32,
    tx_ap_publication_admitted: AtomicU32,
    tx_ap_publication_pool_free: AtomicU32,
    tx_ap_publication_ready_same: AtomicU32,
    tx_ap_publication_ready_other: AtomicU32,
    tx_ap_publication_ingress_reserved: AtomicU32,
    tx_ap_publication_application_reserved: AtomicU32,
    tx_ap_publication_tokens_in_flight: AtomicU32,
    tx_ap_publication_radio_owned: AtomicU32,
    tx_ap_publication_unattributed_radio_owned: AtomicU32,
    active_radio_cycles: AtomicU32,
    active_radio_instructions: AtomicU32,
    active_radio_saw_runner: AtomicU32,
    active_runner_end_cycles: AtomicU32,
    active_runner_end_instructions: AtomicU32,
    active_protocol_cycles: AtomicU32,
    active_protocol_instructions: AtomicU32,
    rx_progress_drained: AtomicU32,
    rx_progress_probe_pending: AtomicU32,
    rx_progress_protocol_tx_blocked: AtomicU32,
    rx_progress_recycled_append_pending: AtomicU32,
    rx_progress_budget_exhausted: AtomicU32,
    rx_progress_stage_blocked: AtomicU32,
    rx_progress_network_blocked: AtomicU32,
    rx_progress_droppable: AtomicU32,
    dma_probe_recycled: AtomicU32,
    dma_probe_completed_frontier: AtomicU32,
    dma_probe_terminal_writeback: AtomicU32,
    dma_probe_republication: AtomicU32,
    adaptive_probe_delay_64: AtomicU32,
    adaptive_probe_delay_128: AtomicU32,
    adaptive_probe_delay_256: AtomicU32,
    adaptive_probe_delay_512: AtomicU32,
    adaptive_probe_delay_other: AtomicU32,
    adaptive_probe_empty_work: AtomicU32,
    adaptive_probe_work_units: AtomicU32,
    adaptive_probe_staged_bytes: AtomicU32,
    dma_entry_remaining_exhausted: AtomicU32,
    dma_entry_remaining_1_8: AtomicU32,
    dma_entry_remaining_9_16: AtomicU32,
    dma_entry_remaining_17_32: AtomicU32,
    dma_entry_remaining_33_48: AtomicU32,
    dma_entry_remaining_49_plus: AtomicU32,
    dma_entry_remaining_unknown: AtomicU32,
    dma_exhaustion_episodes: AtomicU32,
    dma_exhaustion_resolved_le_64us: AtomicU32,
    dma_exhaustion_resolved_le_256us: AtomicU32,
    dma_exhaustion_resolved_le_1024us: AtomicU32,
    dma_exhaustion_resolved_gt_1024us: AtomicU32,
    dma_exhaustion_active: AtomicU32,
    dma_exhaustion_started_cycles: AtomicU32,
}

impl Core0PerformanceCounters {
    pub const fn new() -> Self {
        Self {
            rx_interrupt_posts: AtomicU32::new(0),
            radio_polls: AtomicU32::new(0),
            radio_cycles: AtomicU32::new(0),
            radio_instructions: AtomicU32::new(0),
            poll_to_runner_cycles: AtomicU32::new(0),
            poll_to_runner_instructions: AtomicU32::new(0),
            runner_to_poll_exit_cycles: AtomicU32::new(0),
            runner_to_poll_exit_instructions: AtomicU32::new(0),
            runner_calls: AtomicU32::new(0),
            runner_cycles: AtomicU32::new(0),
            runner_instructions: AtomicU32::new(0),
            protocol_polls: AtomicU32::new(0),
            protocol_cycles: AtomicU32::new(0),
            protocol_instructions: AtomicU32::new(0),
            direct_protocol_frames: AtomicU32::new(0),
            asynchronous_protocol_frames: AtomicU32::new(0),
            dma_calls: AtomicU32::new(0),
            dma_empty_calls: AtomicU32::new(0),
            dma_single_unit_calls: AtomicU32::new(0),
            dma_two_unit_calls: AtomicU32::new(0),
            dma_three_to_seven_unit_calls: AtomicU32::new(0),
            dma_eight_plus_unit_calls: AtomicU32::new(0),
            dma_units: AtomicU32::new(0),
            dma_cycles: AtomicU32::new(0),
            dma_instructions: AtomicU32::new(0),
            protocol_frames: AtomicU32::new(0),
            protocol_frame_cycles: AtomicU32::new(0),
            protocol_frame_instructions: AtomicU32::new(0),
            tx_start_calls: AtomicU32::new(0),
            tx_start_cycles: AtomicU32::new(0),
            tx_start_instructions: AtomicU32::new(0),
            tx_prepare_calls: AtomicU32::new(0),
            tx_prepare_cycles: AtomicU32::new(0),
            tx_prepare_instructions: AtomicU32::new(0),
            tx_publish_calls: AtomicU32::new(0),
            tx_publish_cycles: AtomicU32::new(0),
            tx_publish_instructions: AtomicU32::new(0),
            tx_service_calls: AtomicU32::new(0),
            tx_service_cycles: AtomicU32::new(0),
            tx_service_instructions: AtomicU32::new(0),
            tx_encode_calls: AtomicU32::new(0),
            tx_encode_cycles: AtomicU32::new(0),
            tx_encode_instructions: AtomicU32::new(0),
            tx_commit_calls: AtomicU32::new(0),
            tx_commit_cycles: AtomicU32::new(0),
            tx_commit_instructions: AtomicU32::new(0),
            tx_prepared_gap_samples: AtomicU32::new(0),
            tx_prepared_gap_cycles: AtomicU32::new(0),
            tx_prepared_gap_instructions: AtomicU32::new(0),
            tx_prepared_gap_le_64us: AtomicU32::new(0),
            tx_prepared_gap_le_256us: AtomicU32::new(0),
            tx_prepared_gap_le_512us: AtomicU32::new(0),
            tx_prepared_gap_le_1024us: AtomicU32::new(0),
            tx_prepared_gap_gt_1024us: AtomicU32::new(0),
            tx_network_completions: AtomicU32::new(0),
            tx_completion_prepared: AtomicU32::new(0),
            tx_completion_prepared_full: AtomicU32::new(0),
            tx_completion_prepared_partial: AtomicU32::new(0),
            tx_completion_prepared_frames: AtomicU32::new(0),
            tx_completion_queued: AtomicU32::new(0),
            tx_completion_empty: AtomicU32::new(0),
            tx_initial_network_frames: AtomicU32::new(0),
            tx_ap_partial_frontiers: AtomicU32::new(0),
            tx_ap_partial_matching_retained: AtomicU32::new(0),
            tx_ap_partial_other_retained: AtomicU32::new(0),
            tx_ap_partial_network_ready: AtomicU32::new(0),
            tx_ap_partial_mismatch_claims: AtomicU32::new(0),
            tx_ap_partial_publications: AtomicU32::new(0),
            tx_ap_publication_admitted: AtomicU32::new(0),
            tx_ap_publication_pool_free: AtomicU32::new(0),
            tx_ap_publication_ready_same: AtomicU32::new(0),
            tx_ap_publication_ready_other: AtomicU32::new(0),
            tx_ap_publication_ingress_reserved: AtomicU32::new(0),
            tx_ap_publication_application_reserved: AtomicU32::new(0),
            tx_ap_publication_tokens_in_flight: AtomicU32::new(0),
            tx_ap_publication_radio_owned: AtomicU32::new(0),
            tx_ap_publication_unattributed_radio_owned: AtomicU32::new(0),
            active_radio_cycles: AtomicU32::new(0),
            active_radio_instructions: AtomicU32::new(0),
            active_radio_saw_runner: AtomicU32::new(0),
            active_runner_end_cycles: AtomicU32::new(0),
            active_runner_end_instructions: AtomicU32::new(0),
            active_protocol_cycles: AtomicU32::new(0),
            active_protocol_instructions: AtomicU32::new(0),
            rx_progress_drained: AtomicU32::new(0),
            rx_progress_probe_pending: AtomicU32::new(0),
            rx_progress_protocol_tx_blocked: AtomicU32::new(0),
            rx_progress_recycled_append_pending: AtomicU32::new(0),
            rx_progress_budget_exhausted: AtomicU32::new(0),
            rx_progress_stage_blocked: AtomicU32::new(0),
            rx_progress_network_blocked: AtomicU32::new(0),
            rx_progress_droppable: AtomicU32::new(0),
            dma_probe_recycled: AtomicU32::new(0),
            dma_probe_completed_frontier: AtomicU32::new(0),
            dma_probe_terminal_writeback: AtomicU32::new(0),
            dma_probe_republication: AtomicU32::new(0),
            adaptive_probe_delay_64: AtomicU32::new(0),
            adaptive_probe_delay_128: AtomicU32::new(0),
            adaptive_probe_delay_256: AtomicU32::new(0),
            adaptive_probe_delay_512: AtomicU32::new(0),
            adaptive_probe_delay_other: AtomicU32::new(0),
            adaptive_probe_empty_work: AtomicU32::new(0),
            adaptive_probe_work_units: AtomicU32::new(0),
            adaptive_probe_staged_bytes: AtomicU32::new(0),
            dma_entry_remaining_exhausted: AtomicU32::new(0),
            dma_entry_remaining_1_8: AtomicU32::new(0),
            dma_entry_remaining_9_16: AtomicU32::new(0),
            dma_entry_remaining_17_32: AtomicU32::new(0),
            dma_entry_remaining_33_48: AtomicU32::new(0),
            dma_entry_remaining_49_plus: AtomicU32::new(0),
            dma_entry_remaining_unknown: AtomicU32::new(0),
            dma_exhaustion_episodes: AtomicU32::new(0),
            dma_exhaustion_resolved_le_64us: AtomicU32::new(0),
            dma_exhaustion_resolved_le_256us: AtomicU32::new(0),
            dma_exhaustion_resolved_le_1024us: AtomicU32::new(0),
            dma_exhaustion_resolved_gt_1024us: AtomicU32::new(0),
            dma_exhaustion_active: AtomicU32::new(0),
            dma_exhaustion_started_cycles: AtomicU32::new(0),
        }
    }

    #[inline(always)]
    pub(crate) fn record_rx_interrupt_post(&self) {
        if !COARSE {
            return;
        }
        self.rx_interrupt_posts.fetch_add(1, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn begin_radio_poll(&self, started: Core0PerformanceSample) {
        if !SAMPLING {
            return;
        }
        self.active_radio_cycles
            .store(started.cycles(), Ordering::Relaxed);
        self.active_radio_instructions
            .store(started.instructions(), Ordering::Relaxed);
        self.active_radio_saw_runner.store(0, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn record_radio_poll(
        &self,
        started: Core0PerformanceSample,
        ended: Core0PerformanceSample,
    ) {
        if !SAMPLING {
            return;
        }
        let delta = ended.wrapping_delta_since(started);
        self.radio_polls.fetch_add(1, Ordering::Relaxed);
        self.radio_cycles
            .fetch_add(delta.cycles(), Ordering::Relaxed);
        self.radio_instructions
            .fetch_add(delta.instructions(), Ordering::Relaxed);
        if self.active_radio_saw_runner.load(Ordering::Relaxed) != 0 {
            self.runner_to_poll_exit_cycles.fetch_add(
                ended
                    .cycles()
                    .wrapping_sub(self.active_runner_end_cycles.load(Ordering::Relaxed)),
                Ordering::Relaxed,
            );
            self.runner_to_poll_exit_instructions.fetch_add(
                ended
                    .instructions()
                    .wrapping_sub(self.active_runner_end_instructions.load(Ordering::Relaxed)),
                Ordering::Relaxed,
            );
        }
    }

    #[inline(always)]
    pub(crate) fn record_runner(
        &self,
        started: Core0PerformanceSample,
        ended: Core0PerformanceSample,
    ) {
        if !SAMPLING {
            return;
        }
        let delta = ended.wrapping_delta_since(started);
        self.runner_calls.fetch_add(1, Ordering::Relaxed);
        self.runner_cycles
            .fetch_add(delta.cycles(), Ordering::Relaxed);
        self.runner_instructions
            .fetch_add(delta.instructions(), Ordering::Relaxed);
        self.poll_to_runner_cycles.fetch_add(
            started
                .cycles()
                .wrapping_sub(self.active_radio_cycles.load(Ordering::Relaxed)),
            Ordering::Relaxed,
        );
        self.poll_to_runner_instructions.fetch_add(
            started
                .instructions()
                .wrapping_sub(self.active_radio_instructions.load(Ordering::Relaxed)),
            Ordering::Relaxed,
        );
        self.active_runner_end_instructions
            .store(ended.instructions(), Ordering::Relaxed);
        self.active_runner_end_cycles
            .store(ended.cycles(), Ordering::Relaxed);
        self.active_radio_saw_runner.store(1, Ordering::Relaxed);
    }

    #[inline(always)]
    pub(crate) fn begin_protocol_poll(&self, started: Core0PerformanceSample) {
        if !SAMPLING {
            return;
        }
        self.active_protocol_cycles
            .store(started.cycles(), Ordering::Relaxed);
        self.active_protocol_instructions
            .store(started.instructions(), Ordering::Relaxed);
    }

    #[inline(always)]
    pub(crate) fn end_protocol_poll(&self, ended: Core0PerformanceSample) {
        if !SAMPLING {
            return;
        }
        let started = Core0PerformanceSample::from_parts(
            self.active_protocol_cycles.load(Ordering::Relaxed),
            self.active_protocol_instructions.load(Ordering::Relaxed),
        );
        let delta = ended.wrapping_delta_since(started);
        self.protocol_polls.fetch_add(1, Ordering::Relaxed);
        self.protocol_cycles
            .fetch_add(delta.cycles(), Ordering::Relaxed);
        self.protocol_instructions
            .fetch_add(delta.instructions(), Ordering::Relaxed);
    }

    #[inline(always)]
    pub(crate) fn record_protocol_paths(&self, direct: usize, asynchronous: usize) {
        if !COARSE {
            return;
        }
        self.direct_protocol_frames
            .fetch_add(u32::try_from(direct).unwrap_or(u32::MAX), Ordering::Relaxed);
        self.asynchronous_protocol_frames.fetch_add(
            u32::try_from(asynchronous).unwrap_or(u32::MAX),
            Ordering::Relaxed,
        );
    }

    #[inline(always)]
    pub(crate) fn record_dma(
        &self,
        units: usize,
        started: Core0PerformanceSample,
        ended: Core0PerformanceSample,
    ) {
        if !SAMPLING {
            return;
        }
        let delta = ended.wrapping_delta_since(started);
        self.dma_calls.fetch_add(1, Ordering::Relaxed);
        if units == 0 {
            self.dma_empty_calls.fetch_add(1, Ordering::Relaxed);
        } else if units == 1 {
            self.dma_single_unit_calls.fetch_add(1, Ordering::Relaxed);
        } else if units == 2 {
            self.dma_two_unit_calls.fetch_add(1, Ordering::Relaxed);
        } else if units < 8 {
            self.dma_three_to_seven_unit_calls
                .fetch_add(1, Ordering::Relaxed);
        } else {
            self.dma_eight_plus_unit_calls
                .fetch_add(1, Ordering::Relaxed);
        }
        self.dma_units
            .fetch_add(u32::try_from(units).unwrap_or(u32::MAX), Ordering::Relaxed);
        self.dma_cycles.fetch_add(delta.cycles(), Ordering::Relaxed);
        self.dma_instructions
            .fetch_add(delta.instructions(), Ordering::Relaxed);
    }

    #[inline(always)]
    pub(crate) fn record_rx_progress(
        &self,
        progress: oer_esp32s31_ieee80211::datapath::DatapathRxProgress,
    ) {
        if !COARSE {
            return;
        }
        use oer_esp32s31_ieee80211::datapath::DatapathRxProgress;

        let counter = match progress {
            DatapathRxProgress::Drained => &self.rx_progress_drained,
            DatapathRxProgress::ProbePending => &self.rx_progress_probe_pending,
            DatapathRxProgress::ProtocolBlockedByTx => &self.rx_progress_protocol_tx_blocked,
            DatapathRxProgress::RecycledAppendPending => &self.rx_progress_recycled_append_pending,
            DatapathRxProgress::BudgetExhausted => &self.rx_progress_budget_exhausted,
            DatapathRxProgress::StageCapacityBlocked => &self.rx_progress_stage_blocked,
            DatapathRxProgress::NetworkBackpressured => &self.rx_progress_network_blocked,
            DatapathRxProgress::UpperLayerBlockedButDroppable => &self.rx_progress_droppable,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    #[inline(always)]
    pub(crate) fn record_dma_probe_reasons(
        &self,
        recycled: bool,
        completed_frontier: bool,
        terminal_writeback: bool,
        republication: bool,
    ) {
        if !COARSE {
            return;
        }
        for (present, counter) in [
            (recycled, &self.dma_probe_recycled),
            (completed_frontier, &self.dma_probe_completed_frontier),
            (terminal_writeback, &self.dma_probe_terminal_writeback),
            (republication, &self.dma_probe_republication),
        ] {
            if present {
                counter.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    #[inline(always)]
    pub(crate) fn record_adaptive_probe_selection(
        &self,
        delay_micros: u64,
        work: oer_esp32s31_ieee80211::datapath::DatapathRxWorkCounters,
    ) {
        if !COARSE {
            return;
        }
        let delay_counter = match delay_micros {
            64 => &self.adaptive_probe_delay_64,
            128 => &self.adaptive_probe_delay_128,
            256 => &self.adaptive_probe_delay_256,
            512 => &self.adaptive_probe_delay_512,
            _ => &self.adaptive_probe_delay_other,
        };
        delay_counter.fetch_add(1, Ordering::Relaxed);
        if work.completed_units == 0 {
            self.adaptive_probe_empty_work
                .fetch_add(1, Ordering::Relaxed);
        }
        self.adaptive_probe_work_units.fetch_add(
            u32::try_from(work.completed_units).unwrap_or(u32::MAX),
            Ordering::Relaxed,
        );
        self.adaptive_probe_staged_bytes.fetch_add(
            u32::try_from(work.staged_bytes).unwrap_or(u32::MAX),
            Ordering::Relaxed,
        );
    }

    /// Record the instantaneous accepted-list credits seen when a DMA service
    /// begins. This is deliberately bucketed: it is diagnostic pressure
    /// evidence and never participates in the descriptor ownership protocol.
    #[inline(always)]
    pub(crate) fn record_dma_entry_remaining(&self, remaining: Option<usize>) {
        if !SAMPLING {
            return;
        }
        match remaining {
            Some(0) if self.dma_exhaustion_active.load(Ordering::Relaxed) == 0 => {
                self.dma_exhaustion_started_cycles
                    .store(cycle_count(), Ordering::Relaxed);
                self.dma_exhaustion_active.store(1, Ordering::Relaxed);
                self.dma_exhaustion_episodes.fetch_add(1, Ordering::Relaxed);
            }
            Some(1..) if self.dma_exhaustion_active.swap(0, Ordering::Relaxed) != 0 => {
                // The performance image fixes Core0 at 320 MHz. This interval
                // begins at the first service entry which observes NEXT=0, so
                // it is a lower bound on the finite-list stop rather than an
                // ownership or hardware-error signal.
                let cycles = cycle_count()
                    .wrapping_sub(self.dma_exhaustion_started_cycles.load(Ordering::Relaxed));
                let resolved = match cycles {
                    0..=20_480 => &self.dma_exhaustion_resolved_le_64us,
                    20_481..=81_920 => &self.dma_exhaustion_resolved_le_256us,
                    81_921..=327_680 => &self.dma_exhaustion_resolved_le_1024us,
                    _ => &self.dma_exhaustion_resolved_gt_1024us,
                };
                resolved.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
        let counter = match remaining {
            Some(0) => &self.dma_entry_remaining_exhausted,
            Some(1..=8) => &self.dma_entry_remaining_1_8,
            Some(9..=16) => &self.dma_entry_remaining_9_16,
            Some(17..=32) => &self.dma_entry_remaining_17_32,
            Some(33..=48) => &self.dma_entry_remaining_33_48,
            Some(49..) => &self.dma_entry_remaining_49_plus,
            None => &self.dma_entry_remaining_unknown,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    #[inline(always)]
    #[cfg(feature = "task-poll-telemetry")]
    pub(crate) fn record_protocol_frame(
        &self,
        started: Core0PerformanceSample,
        ended: Core0PerformanceSample,
    ) {
        let delta = ended.wrapping_delta_since(started);
        self.protocol_frames.fetch_add(1, Ordering::Relaxed);
        self.protocol_frame_cycles
            .fetch_add(delta.cycles(), Ordering::Relaxed);
        self.protocol_frame_instructions
            .fetch_add(delta.instructions(), Ordering::Relaxed);
    }

    #[inline(always)]
    pub(crate) fn record_tx_phase(
        &self,
        phase: Core0TxPhase,
        started: Core0PerformanceSample,
        ended: Core0PerformanceSample,
    ) {
        if !TX_PHASES {
            return;
        }
        let delta = ended.wrapping_delta_since(started);
        let (calls, cycles, instructions) = match phase {
            Core0TxPhase::Start => (
                &self.tx_start_calls,
                &self.tx_start_cycles,
                &self.tx_start_instructions,
            ),
            Core0TxPhase::Prepare => (
                &self.tx_prepare_calls,
                &self.tx_prepare_cycles,
                &self.tx_prepare_instructions,
            ),
            Core0TxPhase::Publish => (
                &self.tx_publish_calls,
                &self.tx_publish_cycles,
                &self.tx_publish_instructions,
            ),
            Core0TxPhase::Service => (
                &self.tx_service_calls,
                &self.tx_service_cycles,
                &self.tx_service_instructions,
            ),
            Core0TxPhase::Encode => (
                &self.tx_encode_calls,
                &self.tx_encode_cycles,
                &self.tx_encode_instructions,
            ),
            Core0TxPhase::Commit => (
                &self.tx_commit_calls,
                &self.tx_commit_cycles,
                &self.tx_commit_instructions,
            ),
        };
        calls.fetch_add(1, Ordering::Relaxed);
        cycles.fetch_add(delta.cycles(), Ordering::Relaxed);
        instructions.fetch_add(delta.instructions(), Ordering::Relaxed);
    }

    /// Record only a saturated, already-prepared successor edge.
    ///
    /// The caller captures `completed` only when a standby network aggregate
    /// already exists. This deliberately excludes the terminal completion of
    /// a workload, so time between HIL sessions cannot contaminate the air-gap
    /// measurement.
    #[inline(always)]
    pub(crate) fn record_tx_prepared_gap(
        &self,
        completed: Core0PerformanceSample,
        next_publication_entry: Core0PerformanceSample,
    ) {
        if !TX_PHASES {
            return;
        }
        let delta = next_publication_entry.wrapping_delta_since(completed);
        self.tx_prepared_gap_samples.fetch_add(1, Ordering::Relaxed);
        self.tx_prepared_gap_cycles
            .fetch_add(delta.cycles(), Ordering::Relaxed);
        self.tx_prepared_gap_instructions
            .fetch_add(delta.instructions(), Ordering::Relaxed);
        let bucket = match delta.cycles() {
            0..=20_480 => &self.tx_prepared_gap_le_64us,
            20_481..=81_920 => &self.tx_prepared_gap_le_256us,
            81_921..=163_840 => &self.tx_prepared_gap_le_512us,
            163_841..=327_680 => &self.tx_prepared_gap_le_1024us,
            _ => &self.tx_prepared_gap_gt_1024us,
        };
        bucket.fetch_add(1, Ordering::Relaxed);
    }

    /// Classify whether Core0 already has the next network transaction when
    /// the current one completes. The three outcome counters are mutually
    /// exclusive and therefore sum to `tx_network_completions`.
    #[inline(always)]
    pub(crate) fn record_tx_network_completion(
        &self,
        prepared_frames: usize,
        preferred_frames: usize,
        queued: bool,
    ) {
        if !TX_PHASES {
            return;
        }
        self.tx_network_completions.fetch_add(1, Ordering::Relaxed);
        let prepared = prepared_frames != 0;
        let outcome = if prepared {
            self.tx_completion_prepared_frames.fetch_add(
                u32::try_from(prepared_frames).unwrap_or(u32::MAX),
                Ordering::Relaxed,
            );
            if prepared_frames >= preferred_frames.max(1) {
                self.tx_completion_prepared_full
                    .fetch_add(1, Ordering::Relaxed);
            } else {
                self.tx_completion_prepared_partial
                    .fetch_add(1, Ordering::Relaxed);
            }
            &self.tx_completion_prepared
        } else if queued {
            &self.tx_completion_queued
        } else {
            &self.tx_completion_empty
        };
        outcome.fetch_add(1, Ordering::Relaxed);
    }

    /// Retain the size of the first network transaction which establishes a
    /// new scheduler burst. A fresh role epoch can otherwise enter a stable
    /// phase offset without leaving any evidence in successor-only counters.
    #[inline(always)]
    pub(crate) fn record_tx_initial_network_frames(&self, frames: usize) {
        if !TX_PHASES {
            return;
        }
        self.tx_initial_network_frames
            .fetch_add(u32::try_from(frames).unwrap_or(u32::MAX), Ordering::Relaxed);
    }

    /// Describe only an AP standby which is still below its negotiated frame
    /// limit after the role has drained every immediately visible matching
    /// lease. These counters diagnose queue geometry; they never participate
    /// in admission or wake decisions.
    #[inline(always)]
    pub(crate) fn record_ap_partial_frontier(
        &self,
        matching_retained: usize,
        other_retained: usize,
        network_ready: usize,
        mismatch_claims: usize,
    ) {
        if !TX_PHASES {
            return;
        }
        self.tx_ap_partial_frontiers.fetch_add(1, Ordering::Relaxed);
        self.tx_ap_partial_matching_retained.fetch_add(
            u32::try_from(matching_retained).unwrap_or(u32::MAX),
            Ordering::Relaxed,
        );
        self.tx_ap_partial_other_retained.fetch_add(
            u32::try_from(other_retained).unwrap_or(u32::MAX),
            Ordering::Relaxed,
        );
        self.tx_ap_partial_network_ready.fetch_add(
            u32::try_from(network_ready).unwrap_or(u32::MAX),
            Ordering::Relaxed,
        );
        self.tx_ap_partial_mismatch_claims.fetch_add(
            u32::try_from(mismatch_claims).unwrap_or(u32::MAX),
            Ordering::Relaxed,
        );
    }

    /// Record the complete pool geometry whenever AP observes a partial
    /// standby after draining every immediately visible matching frame.
    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    pub(crate) fn record_ap_partial_publication(
        &self,
        admitted: usize,
        pool_free: usize,
        ready_same: usize,
        ready_other: usize,
        ingress_reserved: usize,
        application_reserved: usize,
        tokens_in_flight: usize,
        radio_owned: usize,
        unattributed_radio_owned: usize,
    ) {
        if !TX_PHASES {
            return;
        }
        self.tx_ap_partial_publications
            .fetch_add(1, Ordering::Relaxed);
        for (counter, value) in [
            (&self.tx_ap_publication_admitted, admitted),
            (&self.tx_ap_publication_pool_free, pool_free),
            (&self.tx_ap_publication_ready_same, ready_same),
            (&self.tx_ap_publication_ready_other, ready_other),
            (&self.tx_ap_publication_ingress_reserved, ingress_reserved),
            (
                &self.tx_ap_publication_application_reserved,
                application_reserved,
            ),
            (&self.tx_ap_publication_tokens_in_flight, tokens_in_flight),
            (&self.tx_ap_publication_radio_owned, radio_owned),
            (
                &self.tx_ap_publication_unattributed_radio_owned,
                unattributed_radio_owned,
            ),
        ] {
            counter.fetch_add(u32::try_from(value).unwrap_or(u32::MAX), Ordering::Relaxed);
        }
    }

    pub fn snapshot(&self) -> Core0PerformanceSnapshot {
        Core0PerformanceSnapshot {
            rx_interrupt_posts: self.rx_interrupt_posts.load(Ordering::Relaxed),
            radio_polls: self.radio_polls.load(Ordering::Relaxed),
            radio_cycles: self.radio_cycles.load(Ordering::Relaxed),
            radio_instructions: self.radio_instructions.load(Ordering::Relaxed),
            poll_to_runner_cycles: self.poll_to_runner_cycles.load(Ordering::Relaxed),
            poll_to_runner_instructions: self.poll_to_runner_instructions.load(Ordering::Relaxed),
            runner_to_poll_exit_cycles: self.runner_to_poll_exit_cycles.load(Ordering::Relaxed),
            runner_to_poll_exit_instructions: self
                .runner_to_poll_exit_instructions
                .load(Ordering::Relaxed),
            runner_calls: self.runner_calls.load(Ordering::Relaxed),
            runner_cycles: self.runner_cycles.load(Ordering::Relaxed),
            runner_instructions: self.runner_instructions.load(Ordering::Relaxed),
            protocol_polls: self.protocol_polls.load(Ordering::Relaxed),
            protocol_cycles: self.protocol_cycles.load(Ordering::Relaxed),
            protocol_instructions: self.protocol_instructions.load(Ordering::Relaxed),
            direct_protocol_frames: self.direct_protocol_frames.load(Ordering::Relaxed),
            asynchronous_protocol_frames: self.asynchronous_protocol_frames.load(Ordering::Relaxed),
            dma_calls: self.dma_calls.load(Ordering::Relaxed),
            dma_empty_calls: self.dma_empty_calls.load(Ordering::Relaxed),
            dma_single_unit_calls: self.dma_single_unit_calls.load(Ordering::Relaxed),
            dma_two_unit_calls: self.dma_two_unit_calls.load(Ordering::Relaxed),
            dma_three_to_seven_unit_calls: self
                .dma_three_to_seven_unit_calls
                .load(Ordering::Relaxed),
            dma_eight_plus_unit_calls: self.dma_eight_plus_unit_calls.load(Ordering::Relaxed),
            dma_units: self.dma_units.load(Ordering::Relaxed),
            dma_cycles: self.dma_cycles.load(Ordering::Relaxed),
            dma_instructions: self.dma_instructions.load(Ordering::Relaxed),
            protocol_frames: self.protocol_frames.load(Ordering::Relaxed),
            protocol_frame_cycles: self.protocol_frame_cycles.load(Ordering::Relaxed),
            protocol_frame_instructions: self.protocol_frame_instructions.load(Ordering::Relaxed),
            tx_start_calls: self.tx_start_calls.load(Ordering::Relaxed),
            tx_start_cycles: self.tx_start_cycles.load(Ordering::Relaxed),
            tx_start_instructions: self.tx_start_instructions.load(Ordering::Relaxed),
            tx_prepare_calls: self.tx_prepare_calls.load(Ordering::Relaxed),
            tx_prepare_cycles: self.tx_prepare_cycles.load(Ordering::Relaxed),
            tx_prepare_instructions: self.tx_prepare_instructions.load(Ordering::Relaxed),
            tx_publish_calls: self.tx_publish_calls.load(Ordering::Relaxed),
            tx_publish_cycles: self.tx_publish_cycles.load(Ordering::Relaxed),
            tx_publish_instructions: self.tx_publish_instructions.load(Ordering::Relaxed),
            tx_service_calls: self.tx_service_calls.load(Ordering::Relaxed),
            tx_service_cycles: self.tx_service_cycles.load(Ordering::Relaxed),
            tx_service_instructions: self.tx_service_instructions.load(Ordering::Relaxed),
            tx_encode_calls: self.tx_encode_calls.load(Ordering::Relaxed),
            tx_encode_cycles: self.tx_encode_cycles.load(Ordering::Relaxed),
            tx_encode_instructions: self.tx_encode_instructions.load(Ordering::Relaxed),
            tx_commit_calls: self.tx_commit_calls.load(Ordering::Relaxed),
            tx_commit_cycles: self.tx_commit_cycles.load(Ordering::Relaxed),
            tx_commit_instructions: self.tx_commit_instructions.load(Ordering::Relaxed),
            tx_prepared_gap_samples: self.tx_prepared_gap_samples.load(Ordering::Relaxed),
            tx_prepared_gap_cycles: self.tx_prepared_gap_cycles.load(Ordering::Relaxed),
            tx_prepared_gap_instructions: self.tx_prepared_gap_instructions.load(Ordering::Relaxed),
            tx_prepared_gap_le_64us: self.tx_prepared_gap_le_64us.load(Ordering::Relaxed),
            tx_prepared_gap_le_256us: self.tx_prepared_gap_le_256us.load(Ordering::Relaxed),
            tx_prepared_gap_le_512us: self.tx_prepared_gap_le_512us.load(Ordering::Relaxed),
            tx_prepared_gap_le_1024us: self.tx_prepared_gap_le_1024us.load(Ordering::Relaxed),
            tx_prepared_gap_gt_1024us: self.tx_prepared_gap_gt_1024us.load(Ordering::Relaxed),
            tx_network_completions: self.tx_network_completions.load(Ordering::Relaxed),
            tx_completion_prepared: self.tx_completion_prepared.load(Ordering::Relaxed),
            tx_completion_prepared_full: self.tx_completion_prepared_full.load(Ordering::Relaxed),
            tx_completion_prepared_partial: self
                .tx_completion_prepared_partial
                .load(Ordering::Relaxed),
            tx_completion_prepared_frames: self
                .tx_completion_prepared_frames
                .load(Ordering::Relaxed),
            tx_completion_queued: self.tx_completion_queued.load(Ordering::Relaxed),
            tx_completion_empty: self.tx_completion_empty.load(Ordering::Relaxed),
            tx_initial_network_frames: self.tx_initial_network_frames.load(Ordering::Relaxed),
            tx_ap_partial_frontiers: self.tx_ap_partial_frontiers.load(Ordering::Relaxed),
            tx_ap_partial_matching_retained: self
                .tx_ap_partial_matching_retained
                .load(Ordering::Relaxed),
            tx_ap_partial_other_retained: self.tx_ap_partial_other_retained.load(Ordering::Relaxed),
            tx_ap_partial_network_ready: self.tx_ap_partial_network_ready.load(Ordering::Relaxed),
            tx_ap_partial_mismatch_claims: self
                .tx_ap_partial_mismatch_claims
                .load(Ordering::Relaxed),
            tx_ap_partial_publications: self.tx_ap_partial_publications.load(Ordering::Relaxed),
            tx_ap_publication_admitted: self.tx_ap_publication_admitted.load(Ordering::Relaxed),
            tx_ap_publication_pool_free: self.tx_ap_publication_pool_free.load(Ordering::Relaxed),
            tx_ap_publication_ready_same: self.tx_ap_publication_ready_same.load(Ordering::Relaxed),
            tx_ap_publication_ready_other: self
                .tx_ap_publication_ready_other
                .load(Ordering::Relaxed),
            tx_ap_publication_ingress_reserved: self
                .tx_ap_publication_ingress_reserved
                .load(Ordering::Relaxed),
            tx_ap_publication_application_reserved: self
                .tx_ap_publication_application_reserved
                .load(Ordering::Relaxed),
            tx_ap_publication_tokens_in_flight: self
                .tx_ap_publication_tokens_in_flight
                .load(Ordering::Relaxed),
            tx_ap_publication_radio_owned: self
                .tx_ap_publication_radio_owned
                .load(Ordering::Relaxed),
            tx_ap_publication_unattributed_radio_owned: self
                .tx_ap_publication_unattributed_radio_owned
                .load(Ordering::Relaxed),
            rx_progress_drained: self.rx_progress_drained.load(Ordering::Relaxed),
            rx_progress_probe_pending: self.rx_progress_probe_pending.load(Ordering::Relaxed),
            rx_progress_protocol_tx_blocked: self
                .rx_progress_protocol_tx_blocked
                .load(Ordering::Relaxed),
            rx_progress_recycled_append_pending: self
                .rx_progress_recycled_append_pending
                .load(Ordering::Relaxed),
            rx_progress_budget_exhausted: self.rx_progress_budget_exhausted.load(Ordering::Relaxed),
            rx_progress_stage_blocked: self.rx_progress_stage_blocked.load(Ordering::Relaxed),
            rx_progress_network_blocked: self.rx_progress_network_blocked.load(Ordering::Relaxed),
            rx_progress_droppable: self.rx_progress_droppable.load(Ordering::Relaxed),
            dma_probe_recycled: self.dma_probe_recycled.load(Ordering::Relaxed),
            dma_probe_completed_frontier: self.dma_probe_completed_frontier.load(Ordering::Relaxed),
            dma_probe_terminal_writeback: self.dma_probe_terminal_writeback.load(Ordering::Relaxed),
            dma_probe_republication: self.dma_probe_republication.load(Ordering::Relaxed),
            adaptive_probe_delay_64: self.adaptive_probe_delay_64.load(Ordering::Relaxed),
            adaptive_probe_delay_128: self.adaptive_probe_delay_128.load(Ordering::Relaxed),
            adaptive_probe_delay_256: self.adaptive_probe_delay_256.load(Ordering::Relaxed),
            adaptive_probe_delay_512: self.adaptive_probe_delay_512.load(Ordering::Relaxed),
            adaptive_probe_delay_other: self.adaptive_probe_delay_other.load(Ordering::Relaxed),
            adaptive_probe_empty_work: self.adaptive_probe_empty_work.load(Ordering::Relaxed),
            adaptive_probe_work_units: self.adaptive_probe_work_units.load(Ordering::Relaxed),
            adaptive_probe_staged_bytes: self.adaptive_probe_staged_bytes.load(Ordering::Relaxed),
            dma_entry_remaining_exhausted: self
                .dma_entry_remaining_exhausted
                .load(Ordering::Relaxed),
            dma_entry_remaining_1_8: self.dma_entry_remaining_1_8.load(Ordering::Relaxed),
            dma_entry_remaining_9_16: self.dma_entry_remaining_9_16.load(Ordering::Relaxed),
            dma_entry_remaining_17_32: self.dma_entry_remaining_17_32.load(Ordering::Relaxed),
            dma_entry_remaining_33_48: self.dma_entry_remaining_33_48.load(Ordering::Relaxed),
            dma_entry_remaining_49_plus: self.dma_entry_remaining_49_plus.load(Ordering::Relaxed),
            dma_entry_remaining_unknown: self.dma_entry_remaining_unknown.load(Ordering::Relaxed),
            dma_exhaustion_episodes: self.dma_exhaustion_episodes.load(Ordering::Relaxed),
            dma_exhaustion_resolved_le_64us: self
                .dma_exhaustion_resolved_le_64us
                .load(Ordering::Relaxed),
            dma_exhaustion_resolved_le_256us: self
                .dma_exhaustion_resolved_le_256us
                .load(Ordering::Relaxed),
            dma_exhaustion_resolved_le_1024us: self
                .dma_exhaustion_resolved_le_1024us
                .load(Ordering::Relaxed),
            dma_exhaustion_resolved_gt_1024us: self
                .dma_exhaustion_resolved_gt_1024us
                .load(Ordering::Relaxed),
        }
    }
}

impl Default for Core0PerformanceCounters {
    fn default() -> Self {
        Self::new()
    }
}

pub static CORE0_PERFORMANCE: Core0PerformanceCounters = Core0PerformanceCounters::new();

/// Low-overhead measurement of one complete RX runner call.
///
/// Unlike the deep phase profiler this owner performs no intermediate reads.
/// The terminal update happens before the mandatory cooperative yield, so
/// sleeping executor time is not charged to the runner.
#[cfg(all(
    feature = "core0-rx-coarse-telemetry",
    not(feature = "task-poll-telemetry")
))]
pub(crate) struct Core0PerformanceRunnerProfile {
    started: Core0PerformanceSample,
}

#[cfg(all(
    feature = "core0-rx-coarse-telemetry",
    not(feature = "task-poll-telemetry")
))]
impl Core0PerformanceRunnerProfile {
    #[inline(always)]
    pub(crate) fn begin() -> Self {
        Self {
            started: Core0PerformanceSample::read(),
        }
    }

    #[inline(always)]
    pub(crate) fn begin_driver(&mut self) {}

    #[inline(always)]
    pub(crate) fn end_driver(&mut self) {}

    #[inline(always)]
    pub(crate) fn finish_before_yield(self) {
        CORE0_PERFORMANCE.record_runner(self.started, Core0PerformanceSample::read());
    }
}

/// Low-overhead measurement of one complete DMA service transaction.
#[cfg(all(
    feature = "core0-rx-coarse-telemetry",
    not(feature = "task-poll-telemetry")
))]
pub(crate) struct Core0PerformanceDmaProfile {
    started: Core0PerformanceSample,
}

#[cfg(all(
    feature = "core0-rx-coarse-telemetry",
    not(feature = "task-poll-telemetry")
))]
impl Core0PerformanceDmaProfile {
    #[inline(always)]
    pub(crate) fn begin() -> Self {
        Self {
            started: Core0PerformanceSample::read(),
        }
    }

    #[inline(always)]
    pub(crate) fn finish(self, units: usize) {
        CORE0_PERFORMANCE.record_dma(units, self.started, Core0PerformanceSample::read());
    }
}

#[cfg(all(
    target_arch = "riscv32",
    any(feature = "core0-rx-coarse-telemetry", feature = "task-poll-telemetry")
))]
#[inline(always)]
fn cycle_count() -> u32 {
    riscv::register::mcycle::read() as u32
}

#[cfg(not(all(
    target_arch = "riscv32",
    any(feature = "core0-rx-coarse-telemetry", feature = "task-poll-telemetry")
)))]
#[inline(always)]
fn cycle_count() -> u32 {
    0
}

#[cfg(all(
    target_arch = "riscv32",
    any(feature = "core0-rx-coarse-telemetry", feature = "task-poll-telemetry")
))]
#[inline(always)]
fn instruction_count() -> u32 {
    riscv::register::minstret::read() as u32
}

#[cfg(not(all(
    target_arch = "riscv32",
    any(feature = "core0-rx-coarse-telemetry", feature = "task-poll-telemetry")
)))]
#[inline(always)]
fn instruction_count() -> u32 {
    0
}

#[cfg(test)]
mod tests;
