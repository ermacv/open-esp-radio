//! Low-overhead phase timing for the Core0 RX DMA producer.
//!
//! The task-poll HIL image reads `mcycle` at synchronous ownership boundaries
//! and aggregates value-only counters in internal SRAM. The `telemetry_*`
//! totals measure the aggregate-update work itself so it can be separated
//! from the production-path intervals which surround those updates.

use core::sync::atomic::{AtomicU32, Ordering};

use super::core0_rx_performance::{CORE0_PERFORMANCE, Core0PerformanceSample};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Core0RxCycleSnapshot {
    pub services: u32,
    pub units: u32,
    pub total: u32,
    pub setup: u32,
    pub frontier: u32,
    pub admission: u32,
    /// Derived aggregate of `stage_take + stage_pool`, not an exclusive phase.
    pub stage_total: u32,
    pub stage_take: u32,
    pub stage_pool: u32,
    pub recycle: u32,
    pub reload: u32,
    pub publish: u32,
    pub tail: u32,
    pub runner_rx_calls: u32,
    pub runner_rx_total: u32,
    pub runner_rx_pre: u32,
    pub runner_rx_driver: u32,
    pub runner_rx_post: u32,
    pub mac_irq_entries: u32,
    pub mac_irq_cycles: u32,
    pub control_calls: u32,
    pub control_cycles: u32,
    pub control_idle: u32,
    pub control_more: u32,
    pub control_tx_pending: u32,
    pub radio_polls: u32,
    pub radio_poll_cycles: u32,
    pub radio_polls_with_rx: u32,
    pub poll_to_runner_cycles: u32,
    pub runner_to_poll_exit_cycles: u32,
    pub scheduler_rx_calls: u32,
    pub scheduler_software_rx_calls: u32,
    pub scheduler_irq_rx_calls: u32,
    pub scheduler_select_rx_calls: u32,
    pub scheduler_reentry_cycles: u32,
    pub scheduler_stop_cycles: u32,
    pub scheduler_housekeeping_cycles: u32,
    pub scheduler_discard_wakes_cycles: u32,
    pub scheduler_first_network_queue_cycles: u32,
    pub scheduler_control_ready_cycles: u32,
    pub scheduler_tx_checks_cycles: u32,
    pub scheduler_prepared_cycles: u32,
    pub scheduler_network_pending_cycles: u32,
    pub scheduler_idle_accounting_cycles: u32,
    pub scheduler_rx_checks_cycles: u32,
    pub protocol_polls: u32,
    pub protocol_poll_cycles: u32,
    pub protocol_polls_with_frame: u32,
    pub protocol_poll_to_first_frame_cycles: u32,
    pub protocol_last_frame_to_poll_exit_cycles: u32,
    pub protocol_dequeues: u32,
    pub protocol_poll_to_first_dequeue_cycles: u32,
    pub protocol_between_frame_to_dequeue_cycles: u32,
    pub protocol_dequeue_to_frame_cycles: u32,
    pub protocol_frame_calls: u32,
    pub protocol_frame_ordinary: u32,
    pub protocol_frame_scratch: u32,
    pub protocol_frame_total: u32,
    pub protocol_frame_preflight: u32,
    pub protocol_frame_wait: u32,
    pub protocol_frame_dispatch: u32,
    pub protocol_dispatch_pre_publish: u32,
    pub protocol_dispatch_capture: u32,
    pub protocol_dispatch_post_publish: u32,
    pub protocol_frame_publish_tail: u32,
    pub protocol_protected_view_calls: u32,
    pub protocol_protected_view_cycles: u32,
    pub ccmp_view_calls: u32,
    pub ccmp_view_total: u32,
    pub ccmp_view_admission: u32,
    pub ccmp_view_layout: u32,
    pub ccmp_view_projection: u32,
    pub ccmp_view_validate: u32,
    pub ccmp_view_telemetry: u32,
    pub data_calls: u32,
    pub data_completed: u32,
    pub data_total: u32,
    pub data_view: u32,
    pub data_fragment_guard: u32,
    pub data_decapsulate: u32,
    pub data_replay: u32,
    pub data_duplicate: u32,
    pub data_publish: u32,
    pub telemetry_dma_record: u32,
    pub telemetry_runner_record: u32,
    pub telemetry_scheduler_record: u32,
    pub telemetry_protocol_dequeue_record: u32,
    pub telemetry_protocol_entry_record: u32,
    pub telemetry_protocol_frame_record: u32,
    pub telemetry_data_record: u32,
}

impl Core0RxCycleSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            services: self.services.wrapping_sub(earlier.services),
            units: self.units.wrapping_sub(earlier.units),
            total: self.total.wrapping_sub(earlier.total),
            setup: self.setup.wrapping_sub(earlier.setup),
            frontier: self.frontier.wrapping_sub(earlier.frontier),
            admission: self.admission.wrapping_sub(earlier.admission),
            stage_total: self.stage_total.wrapping_sub(earlier.stage_total),
            stage_take: self.stage_take.wrapping_sub(earlier.stage_take),
            stage_pool: self.stage_pool.wrapping_sub(earlier.stage_pool),
            recycle: self.recycle.wrapping_sub(earlier.recycle),
            reload: self.reload.wrapping_sub(earlier.reload),
            publish: self.publish.wrapping_sub(earlier.publish),
            tail: self.tail.wrapping_sub(earlier.tail),
            runner_rx_calls: self.runner_rx_calls.wrapping_sub(earlier.runner_rx_calls),
            runner_rx_total: self.runner_rx_total.wrapping_sub(earlier.runner_rx_total),
            runner_rx_pre: self.runner_rx_pre.wrapping_sub(earlier.runner_rx_pre),
            runner_rx_driver: self.runner_rx_driver.wrapping_sub(earlier.runner_rx_driver),
            runner_rx_post: self.runner_rx_post.wrapping_sub(earlier.runner_rx_post),
            mac_irq_entries: self.mac_irq_entries.wrapping_sub(earlier.mac_irq_entries),
            mac_irq_cycles: self.mac_irq_cycles.wrapping_sub(earlier.mac_irq_cycles),
            control_calls: self.control_calls.wrapping_sub(earlier.control_calls),
            control_cycles: self.control_cycles.wrapping_sub(earlier.control_cycles),
            control_idle: self.control_idle.wrapping_sub(earlier.control_idle),
            control_more: self.control_more.wrapping_sub(earlier.control_more),
            control_tx_pending: self
                .control_tx_pending
                .wrapping_sub(earlier.control_tx_pending),
            radio_polls: self.radio_polls.wrapping_sub(earlier.radio_polls),
            radio_poll_cycles: self
                .radio_poll_cycles
                .wrapping_sub(earlier.radio_poll_cycles),
            radio_polls_with_rx: self
                .radio_polls_with_rx
                .wrapping_sub(earlier.radio_polls_with_rx),
            poll_to_runner_cycles: self
                .poll_to_runner_cycles
                .wrapping_sub(earlier.poll_to_runner_cycles),
            runner_to_poll_exit_cycles: self
                .runner_to_poll_exit_cycles
                .wrapping_sub(earlier.runner_to_poll_exit_cycles),
            scheduler_rx_calls: self
                .scheduler_rx_calls
                .wrapping_sub(earlier.scheduler_rx_calls),
            scheduler_software_rx_calls: self
                .scheduler_software_rx_calls
                .wrapping_sub(earlier.scheduler_software_rx_calls),
            scheduler_irq_rx_calls: self
                .scheduler_irq_rx_calls
                .wrapping_sub(earlier.scheduler_irq_rx_calls),
            scheduler_select_rx_calls: self
                .scheduler_select_rx_calls
                .wrapping_sub(earlier.scheduler_select_rx_calls),
            scheduler_reentry_cycles: self
                .scheduler_reentry_cycles
                .wrapping_sub(earlier.scheduler_reentry_cycles),
            scheduler_stop_cycles: self
                .scheduler_stop_cycles
                .wrapping_sub(earlier.scheduler_stop_cycles),
            scheduler_housekeeping_cycles: self
                .scheduler_housekeeping_cycles
                .wrapping_sub(earlier.scheduler_housekeeping_cycles),
            scheduler_discard_wakes_cycles: self
                .scheduler_discard_wakes_cycles
                .wrapping_sub(earlier.scheduler_discard_wakes_cycles),
            scheduler_first_network_queue_cycles: self
                .scheduler_first_network_queue_cycles
                .wrapping_sub(earlier.scheduler_first_network_queue_cycles),
            scheduler_control_ready_cycles: self
                .scheduler_control_ready_cycles
                .wrapping_sub(earlier.scheduler_control_ready_cycles),
            scheduler_tx_checks_cycles: self
                .scheduler_tx_checks_cycles
                .wrapping_sub(earlier.scheduler_tx_checks_cycles),
            scheduler_prepared_cycles: self
                .scheduler_prepared_cycles
                .wrapping_sub(earlier.scheduler_prepared_cycles),
            scheduler_network_pending_cycles: self
                .scheduler_network_pending_cycles
                .wrapping_sub(earlier.scheduler_network_pending_cycles),
            scheduler_idle_accounting_cycles: self
                .scheduler_idle_accounting_cycles
                .wrapping_sub(earlier.scheduler_idle_accounting_cycles),
            scheduler_rx_checks_cycles: self
                .scheduler_rx_checks_cycles
                .wrapping_sub(earlier.scheduler_rx_checks_cycles),
            protocol_polls: self.protocol_polls.wrapping_sub(earlier.protocol_polls),
            protocol_poll_cycles: self
                .protocol_poll_cycles
                .wrapping_sub(earlier.protocol_poll_cycles),
            protocol_polls_with_frame: self
                .protocol_polls_with_frame
                .wrapping_sub(earlier.protocol_polls_with_frame),
            protocol_poll_to_first_frame_cycles: self
                .protocol_poll_to_first_frame_cycles
                .wrapping_sub(earlier.protocol_poll_to_first_frame_cycles),
            protocol_last_frame_to_poll_exit_cycles: self
                .protocol_last_frame_to_poll_exit_cycles
                .wrapping_sub(earlier.protocol_last_frame_to_poll_exit_cycles),
            protocol_dequeues: self
                .protocol_dequeues
                .wrapping_sub(earlier.protocol_dequeues),
            protocol_poll_to_first_dequeue_cycles: self
                .protocol_poll_to_first_dequeue_cycles
                .wrapping_sub(earlier.protocol_poll_to_first_dequeue_cycles),
            protocol_between_frame_to_dequeue_cycles: self
                .protocol_between_frame_to_dequeue_cycles
                .wrapping_sub(earlier.protocol_between_frame_to_dequeue_cycles),
            protocol_dequeue_to_frame_cycles: self
                .protocol_dequeue_to_frame_cycles
                .wrapping_sub(earlier.protocol_dequeue_to_frame_cycles),
            protocol_frame_calls: self
                .protocol_frame_calls
                .wrapping_sub(earlier.protocol_frame_calls),
            protocol_frame_ordinary: self
                .protocol_frame_ordinary
                .wrapping_sub(earlier.protocol_frame_ordinary),
            protocol_frame_scratch: self
                .protocol_frame_scratch
                .wrapping_sub(earlier.protocol_frame_scratch),
            protocol_frame_total: self
                .protocol_frame_total
                .wrapping_sub(earlier.protocol_frame_total),
            protocol_frame_preflight: self
                .protocol_frame_preflight
                .wrapping_sub(earlier.protocol_frame_preflight),
            protocol_frame_wait: self
                .protocol_frame_wait
                .wrapping_sub(earlier.protocol_frame_wait),
            protocol_frame_dispatch: self
                .protocol_frame_dispatch
                .wrapping_sub(earlier.protocol_frame_dispatch),
            protocol_dispatch_pre_publish: self
                .protocol_dispatch_pre_publish
                .wrapping_sub(earlier.protocol_dispatch_pre_publish),
            protocol_dispatch_capture: self
                .protocol_dispatch_capture
                .wrapping_sub(earlier.protocol_dispatch_capture),
            protocol_dispatch_post_publish: self
                .protocol_dispatch_post_publish
                .wrapping_sub(earlier.protocol_dispatch_post_publish),
            protocol_frame_publish_tail: self
                .protocol_frame_publish_tail
                .wrapping_sub(earlier.protocol_frame_publish_tail),
            protocol_protected_view_calls: self
                .protocol_protected_view_calls
                .wrapping_sub(earlier.protocol_protected_view_calls),
            protocol_protected_view_cycles: self
                .protocol_protected_view_cycles
                .wrapping_sub(earlier.protocol_protected_view_cycles),
            ccmp_view_calls: self.ccmp_view_calls.wrapping_sub(earlier.ccmp_view_calls),
            ccmp_view_total: self.ccmp_view_total.wrapping_sub(earlier.ccmp_view_total),
            ccmp_view_admission: self
                .ccmp_view_admission
                .wrapping_sub(earlier.ccmp_view_admission),
            ccmp_view_layout: self.ccmp_view_layout.wrapping_sub(earlier.ccmp_view_layout),
            ccmp_view_projection: self
                .ccmp_view_projection
                .wrapping_sub(earlier.ccmp_view_projection),
            ccmp_view_validate: self
                .ccmp_view_validate
                .wrapping_sub(earlier.ccmp_view_validate),
            ccmp_view_telemetry: self
                .ccmp_view_telemetry
                .wrapping_sub(earlier.ccmp_view_telemetry),
            data_calls: self.data_calls.wrapping_sub(earlier.data_calls),
            data_completed: self.data_completed.wrapping_sub(earlier.data_completed),
            data_total: self.data_total.wrapping_sub(earlier.data_total),
            data_view: self.data_view.wrapping_sub(earlier.data_view),
            data_fragment_guard: self
                .data_fragment_guard
                .wrapping_sub(earlier.data_fragment_guard),
            data_decapsulate: self.data_decapsulate.wrapping_sub(earlier.data_decapsulate),
            data_replay: self.data_replay.wrapping_sub(earlier.data_replay),
            data_duplicate: self.data_duplicate.wrapping_sub(earlier.data_duplicate),
            data_publish: self.data_publish.wrapping_sub(earlier.data_publish),
            telemetry_dma_record: self
                .telemetry_dma_record
                .wrapping_sub(earlier.telemetry_dma_record),
            telemetry_runner_record: self
                .telemetry_runner_record
                .wrapping_sub(earlier.telemetry_runner_record),
            telemetry_scheduler_record: self
                .telemetry_scheduler_record
                .wrapping_sub(earlier.telemetry_scheduler_record),
            telemetry_protocol_dequeue_record: self
                .telemetry_protocol_dequeue_record
                .wrapping_sub(earlier.telemetry_protocol_dequeue_record),
            telemetry_protocol_entry_record: self
                .telemetry_protocol_entry_record
                .wrapping_sub(earlier.telemetry_protocol_entry_record),
            telemetry_protocol_frame_record: self
                .telemetry_protocol_frame_record
                .wrapping_sub(earlier.telemetry_protocol_frame_record),
            telemetry_data_record: self
                .telemetry_data_record
                .wrapping_sub(earlier.telemetry_data_record),
        }
    }
}

pub struct Core0RxCycleCounters {
    services: AtomicU32,
    units: AtomicU32,
    total: AtomicU32,
    setup: AtomicU32,
    frontier: AtomicU32,
    admission: AtomicU32,
    stage_total: AtomicU32,
    stage_take: AtomicU32,
    stage_pool: AtomicU32,
    recycle: AtomicU32,
    reload: AtomicU32,
    publish: AtomicU32,
    tail: AtomicU32,
    runner_rx_calls: AtomicU32,
    runner_rx_total: AtomicU32,
    runner_rx_pre: AtomicU32,
    runner_rx_driver: AtomicU32,
    runner_rx_post: AtomicU32,
    mac_irq_entries: AtomicU32,
    mac_irq_cycles: AtomicU32,
    control_calls: AtomicU32,
    control_cycles: AtomicU32,
    control_idle: AtomicU32,
    control_more: AtomicU32,
    control_tx_pending: AtomicU32,
    radio_polls: AtomicU32,
    radio_poll_cycles: AtomicU32,
    radio_polls_with_rx: AtomicU32,
    poll_to_runner_cycles: AtomicU32,
    runner_to_poll_exit_cycles: AtomicU32,
    scheduler_rx_calls: AtomicU32,
    scheduler_software_rx_calls: AtomicU32,
    scheduler_irq_rx_calls: AtomicU32,
    scheduler_select_rx_calls: AtomicU32,
    scheduler_reentry_cycles: AtomicU32,
    scheduler_stop_cycles: AtomicU32,
    scheduler_housekeeping_cycles: AtomicU32,
    scheduler_discard_wakes_cycles: AtomicU32,
    scheduler_first_network_queue_cycles: AtomicU32,
    scheduler_control_ready_cycles: AtomicU32,
    scheduler_tx_checks_cycles: AtomicU32,
    scheduler_prepared_cycles: AtomicU32,
    scheduler_network_pending_cycles: AtomicU32,
    scheduler_idle_accounting_cycles: AtomicU32,
    scheduler_rx_checks_cycles: AtomicU32,
    protocol_polls: AtomicU32,
    protocol_poll_cycles: AtomicU32,
    protocol_polls_with_frame: AtomicU32,
    protocol_poll_to_first_frame_cycles: AtomicU32,
    protocol_last_frame_to_poll_exit_cycles: AtomicU32,
    protocol_dequeues: AtomicU32,
    protocol_poll_to_first_dequeue_cycles: AtomicU32,
    protocol_between_frame_to_dequeue_cycles: AtomicU32,
    protocol_dequeue_to_frame_cycles: AtomicU32,
    protocol_frame_calls: AtomicU32,
    protocol_frame_ordinary: AtomicU32,
    protocol_frame_scratch: AtomicU32,
    protocol_frame_total: AtomicU32,
    protocol_frame_preflight: AtomicU32,
    protocol_frame_wait: AtomicU32,
    protocol_frame_dispatch: AtomicU32,
    protocol_dispatch_pre_publish: AtomicU32,
    protocol_dispatch_capture: AtomicU32,
    protocol_dispatch_post_publish: AtomicU32,
    protocol_frame_publish_tail: AtomicU32,
    data_calls: AtomicU32,
    data_completed: AtomicU32,
    data_total: AtomicU32,
    data_view: AtomicU32,
    data_fragment_guard: AtomicU32,
    data_decapsulate: AtomicU32,
    data_replay: AtomicU32,
    data_duplicate: AtomicU32,
    data_publish: AtomicU32,
    telemetry_dma_record: AtomicU32,
    telemetry_runner_record: AtomicU32,
    telemetry_scheduler_record: AtomicU32,
    telemetry_protocol_dequeue_record: AtomicU32,
    telemetry_protocol_entry_record: AtomicU32,
    telemetry_protocol_frame_record: AtomicU32,
    telemetry_data_record: AtomicU32,
    active_poll_started: AtomicU32,
    active_poll_generation: AtomicU32,
    active_poll_runner_end: AtomicU32,
    active_poll_saw_runner: AtomicU32,
    active_protocol_poll_started: AtomicU32,
    active_protocol_poll_generation: AtomicU32,
    active_protocol_poll_last_frame_end: AtomicU32,
    active_protocol_poll_saw_frame: AtomicU32,
    active_protocol_poll_last_dequeue: AtomicU32,
    active_protocol_poll_saw_dequeue: AtomicU32,
}

impl Core0RxCycleCounters {
    pub const fn new() -> Self {
        Self {
            services: AtomicU32::new(0),
            units: AtomicU32::new(0),
            total: AtomicU32::new(0),
            setup: AtomicU32::new(0),
            frontier: AtomicU32::new(0),
            admission: AtomicU32::new(0),
            stage_total: AtomicU32::new(0),
            stage_take: AtomicU32::new(0),
            stage_pool: AtomicU32::new(0),
            recycle: AtomicU32::new(0),
            reload: AtomicU32::new(0),
            publish: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            runner_rx_calls: AtomicU32::new(0),
            runner_rx_total: AtomicU32::new(0),
            runner_rx_pre: AtomicU32::new(0),
            runner_rx_driver: AtomicU32::new(0),
            runner_rx_post: AtomicU32::new(0),
            mac_irq_entries: AtomicU32::new(0),
            mac_irq_cycles: AtomicU32::new(0),
            control_calls: AtomicU32::new(0),
            control_cycles: AtomicU32::new(0),
            control_idle: AtomicU32::new(0),
            control_more: AtomicU32::new(0),
            control_tx_pending: AtomicU32::new(0),
            radio_polls: AtomicU32::new(0),
            radio_poll_cycles: AtomicU32::new(0),
            radio_polls_with_rx: AtomicU32::new(0),
            poll_to_runner_cycles: AtomicU32::new(0),
            runner_to_poll_exit_cycles: AtomicU32::new(0),
            scheduler_rx_calls: AtomicU32::new(0),
            scheduler_software_rx_calls: AtomicU32::new(0),
            scheduler_irq_rx_calls: AtomicU32::new(0),
            scheduler_select_rx_calls: AtomicU32::new(0),
            scheduler_reentry_cycles: AtomicU32::new(0),
            scheduler_stop_cycles: AtomicU32::new(0),
            scheduler_housekeeping_cycles: AtomicU32::new(0),
            scheduler_discard_wakes_cycles: AtomicU32::new(0),
            scheduler_first_network_queue_cycles: AtomicU32::new(0),
            scheduler_control_ready_cycles: AtomicU32::new(0),
            scheduler_tx_checks_cycles: AtomicU32::new(0),
            scheduler_prepared_cycles: AtomicU32::new(0),
            scheduler_network_pending_cycles: AtomicU32::new(0),
            scheduler_idle_accounting_cycles: AtomicU32::new(0),
            scheduler_rx_checks_cycles: AtomicU32::new(0),
            protocol_polls: AtomicU32::new(0),
            protocol_poll_cycles: AtomicU32::new(0),
            protocol_polls_with_frame: AtomicU32::new(0),
            protocol_poll_to_first_frame_cycles: AtomicU32::new(0),
            protocol_last_frame_to_poll_exit_cycles: AtomicU32::new(0),
            protocol_dequeues: AtomicU32::new(0),
            protocol_poll_to_first_dequeue_cycles: AtomicU32::new(0),
            protocol_between_frame_to_dequeue_cycles: AtomicU32::new(0),
            protocol_dequeue_to_frame_cycles: AtomicU32::new(0),
            protocol_frame_calls: AtomicU32::new(0),
            protocol_frame_ordinary: AtomicU32::new(0),
            protocol_frame_scratch: AtomicU32::new(0),
            protocol_frame_total: AtomicU32::new(0),
            protocol_frame_preflight: AtomicU32::new(0),
            protocol_frame_wait: AtomicU32::new(0),
            protocol_frame_dispatch: AtomicU32::new(0),
            protocol_dispatch_pre_publish: AtomicU32::new(0),
            protocol_dispatch_capture: AtomicU32::new(0),
            protocol_dispatch_post_publish: AtomicU32::new(0),
            protocol_frame_publish_tail: AtomicU32::new(0),
            data_calls: AtomicU32::new(0),
            data_completed: AtomicU32::new(0),
            data_total: AtomicU32::new(0),
            data_view: AtomicU32::new(0),
            data_fragment_guard: AtomicU32::new(0),
            data_decapsulate: AtomicU32::new(0),
            data_replay: AtomicU32::new(0),
            data_duplicate: AtomicU32::new(0),
            data_publish: AtomicU32::new(0),
            telemetry_dma_record: AtomicU32::new(0),
            telemetry_runner_record: AtomicU32::new(0),
            telemetry_scheduler_record: AtomicU32::new(0),
            telemetry_protocol_dequeue_record: AtomicU32::new(0),
            telemetry_protocol_entry_record: AtomicU32::new(0),
            telemetry_protocol_frame_record: AtomicU32::new(0),
            telemetry_data_record: AtomicU32::new(0),
            active_poll_started: AtomicU32::new(0),
            active_poll_generation: AtomicU32::new(0),
            active_poll_runner_end: AtomicU32::new(0),
            active_poll_saw_runner: AtomicU32::new(0),
            active_protocol_poll_started: AtomicU32::new(0),
            active_protocol_poll_generation: AtomicU32::new(0),
            active_protocol_poll_last_frame_end: AtomicU32::new(0),
            active_protocol_poll_saw_frame: AtomicU32::new(0),
            active_protocol_poll_last_dequeue: AtomicU32::new(0),
            active_protocol_poll_saw_dequeue: AtomicU32::new(0),
        }
    }

    fn record(&self, profile: Core0RxCycleSnapshot) {
        let telemetry_started = cycle_count();
        self.services.fetch_add(1, Ordering::Relaxed);
        self.units.fetch_add(profile.units, Ordering::Relaxed);
        self.total.fetch_add(profile.total, Ordering::Relaxed);
        self.setup.fetch_add(profile.setup, Ordering::Relaxed);
        self.frontier.fetch_add(profile.frontier, Ordering::Relaxed);
        self.admission
            .fetch_add(profile.admission, Ordering::Relaxed);
        self.stage_total
            .fetch_add(profile.stage_total, Ordering::Relaxed);
        self.stage_take
            .fetch_add(profile.stage_take, Ordering::Relaxed);
        self.stage_pool
            .fetch_add(profile.stage_pool, Ordering::Relaxed);
        self.recycle.fetch_add(profile.recycle, Ordering::Relaxed);
        self.reload.fetch_add(profile.reload, Ordering::Relaxed);
        self.publish.fetch_add(profile.publish, Ordering::Relaxed);
        self.tail.fetch_add(profile.tail, Ordering::Relaxed);
        self.telemetry_dma_record.fetch_add(
            cycle_count().wrapping_sub(telemetry_started),
            Ordering::Relaxed,
        );
    }

    pub fn snapshot(&self) -> Core0RxCycleSnapshot {
        let (protocol_protected_view_calls, protocol_protected_view_cycles) =
            oer_esp32s31_ieee80211::protected_data_rx::protected_data_view_cycle_snapshot();
        let ccmp_view = oer_esp32s31_ieee80211_mac::rx::ccmp_view_cycle_snapshot();
        Core0RxCycleSnapshot {
            services: self.services.load(Ordering::Relaxed),
            units: self.units.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
            setup: self.setup.load(Ordering::Relaxed),
            frontier: self.frontier.load(Ordering::Relaxed),
            admission: self.admission.load(Ordering::Relaxed),
            stage_total: self.stage_total.load(Ordering::Relaxed),
            stage_take: self.stage_take.load(Ordering::Relaxed),
            stage_pool: self.stage_pool.load(Ordering::Relaxed),
            recycle: self.recycle.load(Ordering::Relaxed),
            reload: self.reload.load(Ordering::Relaxed),
            publish: self.publish.load(Ordering::Relaxed),
            tail: self.tail.load(Ordering::Relaxed),
            runner_rx_calls: self.runner_rx_calls.load(Ordering::Relaxed),
            runner_rx_total: self.runner_rx_total.load(Ordering::Relaxed),
            runner_rx_pre: self.runner_rx_pre.load(Ordering::Relaxed),
            runner_rx_driver: self.runner_rx_driver.load(Ordering::Relaxed),
            runner_rx_post: self.runner_rx_post.load(Ordering::Relaxed),
            mac_irq_entries: self.mac_irq_entries.load(Ordering::Relaxed),
            mac_irq_cycles: self.mac_irq_cycles.load(Ordering::Relaxed),
            control_calls: self.control_calls.load(Ordering::Relaxed),
            control_cycles: self.control_cycles.load(Ordering::Relaxed),
            control_idle: self.control_idle.load(Ordering::Relaxed),
            control_more: self.control_more.load(Ordering::Relaxed),
            control_tx_pending: self.control_tx_pending.load(Ordering::Relaxed),
            radio_polls: self.radio_polls.load(Ordering::Relaxed),
            radio_poll_cycles: self.radio_poll_cycles.load(Ordering::Relaxed),
            radio_polls_with_rx: self.radio_polls_with_rx.load(Ordering::Relaxed),
            poll_to_runner_cycles: self.poll_to_runner_cycles.load(Ordering::Relaxed),
            runner_to_poll_exit_cycles: self.runner_to_poll_exit_cycles.load(Ordering::Relaxed),
            scheduler_rx_calls: self.scheduler_rx_calls.load(Ordering::Relaxed),
            scheduler_software_rx_calls: self.scheduler_software_rx_calls.load(Ordering::Relaxed),
            scheduler_irq_rx_calls: self.scheduler_irq_rx_calls.load(Ordering::Relaxed),
            scheduler_select_rx_calls: self.scheduler_select_rx_calls.load(Ordering::Relaxed),
            scheduler_reentry_cycles: self.scheduler_reentry_cycles.load(Ordering::Relaxed),
            scheduler_stop_cycles: self.scheduler_stop_cycles.load(Ordering::Relaxed),
            scheduler_housekeeping_cycles: self
                .scheduler_housekeeping_cycles
                .load(Ordering::Relaxed),
            scheduler_discard_wakes_cycles: self
                .scheduler_discard_wakes_cycles
                .load(Ordering::Relaxed),
            scheduler_first_network_queue_cycles: self
                .scheduler_first_network_queue_cycles
                .load(Ordering::Relaxed),
            scheduler_control_ready_cycles: self
                .scheduler_control_ready_cycles
                .load(Ordering::Relaxed),
            scheduler_tx_checks_cycles: self.scheduler_tx_checks_cycles.load(Ordering::Relaxed),
            scheduler_prepared_cycles: self.scheduler_prepared_cycles.load(Ordering::Relaxed),
            scheduler_network_pending_cycles: self
                .scheduler_network_pending_cycles
                .load(Ordering::Relaxed),
            scheduler_idle_accounting_cycles: self
                .scheduler_idle_accounting_cycles
                .load(Ordering::Relaxed),
            scheduler_rx_checks_cycles: self.scheduler_rx_checks_cycles.load(Ordering::Relaxed),
            protocol_polls: self.protocol_polls.load(Ordering::Relaxed),
            protocol_poll_cycles: self.protocol_poll_cycles.load(Ordering::Relaxed),
            protocol_polls_with_frame: self.protocol_polls_with_frame.load(Ordering::Relaxed),
            protocol_poll_to_first_frame_cycles: self
                .protocol_poll_to_first_frame_cycles
                .load(Ordering::Relaxed),
            protocol_last_frame_to_poll_exit_cycles: self
                .protocol_last_frame_to_poll_exit_cycles
                .load(Ordering::Relaxed),
            protocol_dequeues: self.protocol_dequeues.load(Ordering::Relaxed),
            protocol_poll_to_first_dequeue_cycles: self
                .protocol_poll_to_first_dequeue_cycles
                .load(Ordering::Relaxed),
            protocol_between_frame_to_dequeue_cycles: self
                .protocol_between_frame_to_dequeue_cycles
                .load(Ordering::Relaxed),
            protocol_dequeue_to_frame_cycles: self
                .protocol_dequeue_to_frame_cycles
                .load(Ordering::Relaxed),
            protocol_frame_calls: self.protocol_frame_calls.load(Ordering::Relaxed),
            protocol_frame_ordinary: self.protocol_frame_ordinary.load(Ordering::Relaxed),
            protocol_frame_scratch: self.protocol_frame_scratch.load(Ordering::Relaxed),
            protocol_frame_total: self.protocol_frame_total.load(Ordering::Relaxed),
            protocol_frame_preflight: self.protocol_frame_preflight.load(Ordering::Relaxed),
            protocol_frame_wait: self.protocol_frame_wait.load(Ordering::Relaxed),
            protocol_frame_dispatch: self.protocol_frame_dispatch.load(Ordering::Relaxed),
            protocol_dispatch_pre_publish: self
                .protocol_dispatch_pre_publish
                .load(Ordering::Relaxed),
            protocol_dispatch_capture: self.protocol_dispatch_capture.load(Ordering::Relaxed),
            protocol_dispatch_post_publish: self
                .protocol_dispatch_post_publish
                .load(Ordering::Relaxed),
            protocol_frame_publish_tail: self.protocol_frame_publish_tail.load(Ordering::Relaxed),
            protocol_protected_view_calls,
            protocol_protected_view_cycles,
            ccmp_view_calls: ccmp_view.calls,
            ccmp_view_total: ccmp_view.total,
            ccmp_view_admission: ccmp_view.admission,
            ccmp_view_layout: ccmp_view.layout,
            ccmp_view_projection: ccmp_view.projection,
            ccmp_view_validate: ccmp_view.validate,
            ccmp_view_telemetry: ccmp_view.telemetry,
            data_calls: self.data_calls.load(Ordering::Relaxed),
            data_completed: self.data_completed.load(Ordering::Relaxed),
            data_total: self.data_total.load(Ordering::Relaxed),
            data_view: self.data_view.load(Ordering::Relaxed),
            data_fragment_guard: self.data_fragment_guard.load(Ordering::Relaxed),
            data_decapsulate: self.data_decapsulate.load(Ordering::Relaxed),
            data_replay: self.data_replay.load(Ordering::Relaxed),
            data_duplicate: self.data_duplicate.load(Ordering::Relaxed),
            data_publish: self.data_publish.load(Ordering::Relaxed),
            telemetry_dma_record: self.telemetry_dma_record.load(Ordering::Relaxed),
            telemetry_runner_record: self.telemetry_runner_record.load(Ordering::Relaxed),
            telemetry_scheduler_record: self.telemetry_scheduler_record.load(Ordering::Relaxed),
            telemetry_protocol_dequeue_record: self
                .telemetry_protocol_dequeue_record
                .load(Ordering::Relaxed),
            telemetry_protocol_entry_record: self
                .telemetry_protocol_entry_record
                .load(Ordering::Relaxed),
            telemetry_protocol_frame_record: self
                .telemetry_protocol_frame_record
                .load(Ordering::Relaxed),
            telemetry_data_record: self.telemetry_data_record.load(Ordering::Relaxed),
        }
    }

    #[inline(always)]
    pub(crate) fn record_data_profile(
        &self,
        profile: oer_esp32s31_ieee80211_sta::connected_rx::ConnectedRxDataCycleProfile,
    ) {
        let telemetry_started = cycle_count();
        self.data_calls.fetch_add(profile.calls, Ordering::Relaxed);
        self.data_completed
            .fetch_add(profile.completed, Ordering::Relaxed);
        self.data_total.fetch_add(profile.total, Ordering::Relaxed);
        self.data_view.fetch_add(profile.view, Ordering::Relaxed);
        self.data_fragment_guard
            .fetch_add(profile.fragment_guard, Ordering::Relaxed);
        self.data_decapsulate
            .fetch_add(profile.decapsulate, Ordering::Relaxed);
        self.data_replay
            .fetch_add(profile.replay, Ordering::Relaxed);
        self.data_duplicate
            .fetch_add(profile.duplicate, Ordering::Relaxed);
        self.data_publish
            .fetch_add(profile.publish, Ordering::Relaxed);
        self.telemetry_data_record.fetch_add(
            cycle_count().wrapping_sub(telemetry_started),
            Ordering::Relaxed,
        );
    }

    #[inline(always)]
    pub fn record_mac_irq(&self, cycles: u32) {
        self.mac_irq_entries.fetch_add(1, Ordering::Relaxed);
        self.mac_irq_cycles.fetch_add(cycles, Ordering::Relaxed);
    }

    #[inline(always)]
    pub(crate) fn record_control(&self, cycles: u32, outcome: Core0ControlOutcome) {
        self.control_calls.fetch_add(1, Ordering::Relaxed);
        self.control_cycles.fetch_add(cycles, Ordering::Relaxed);
        match outcome {
            Core0ControlOutcome::Idle => self.control_idle.fetch_add(1, Ordering::Relaxed),
            Core0ControlOutcome::More => self.control_more.fetch_add(1, Ordering::Relaxed),
            Core0ControlOutcome::TxPending => {
                self.control_tx_pending.fetch_add(1, Ordering::Relaxed)
            }
            Core0ControlOutcome::Exit => return,
        };
    }

    #[inline(always)]
    pub fn begin_radio_poll(&self, started: u32) {
        self.active_poll_started.store(started, Ordering::Relaxed);
        self.active_poll_generation.fetch_add(1, Ordering::Relaxed);
        self.active_poll_saw_runner.store(0, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn end_radio_poll(&self, ended: u32) {
        let started = self.active_poll_started.load(Ordering::Relaxed);
        self.radio_polls.fetch_add(1, Ordering::Relaxed);
        self.radio_poll_cycles
            .fetch_add(ended.wrapping_sub(started), Ordering::Relaxed);
        if self.active_poll_saw_runner.load(Ordering::Relaxed) != 0 {
            let runner_end = self.active_poll_runner_end.load(Ordering::Relaxed);
            self.radio_polls_with_rx.fetch_add(1, Ordering::Relaxed);
            self.runner_to_poll_exit_cycles
                .fetch_add(ended.wrapping_sub(runner_end), Ordering::Relaxed);
        }
    }

    #[inline(always)]
    fn record_runner_entry(&self, entered: u32) {
        let poll_started = self.active_poll_started.load(Ordering::Relaxed);
        self.poll_to_runner_cycles
            .fetch_add(entered.wrapping_sub(poll_started), Ordering::Relaxed);
        self.active_poll_saw_runner.store(1, Ordering::Relaxed);
    }

    #[inline(always)]
    fn record_runner_end(&self, ended: u32) {
        self.active_poll_runner_end.store(ended, Ordering::Relaxed);
    }

    #[inline(always)]
    fn record_runner_rx(&self, pre: u32, driver: u32, post: u32) {
        let telemetry_started = cycle_count();
        self.runner_rx_calls.fetch_add(1, Ordering::Relaxed);
        self.runner_rx_pre.fetch_add(pre, Ordering::Relaxed);
        self.runner_rx_driver.fetch_add(driver, Ordering::Relaxed);
        self.runner_rx_post.fetch_add(post, Ordering::Relaxed);
        self.runner_rx_total.fetch_add(
            pre.wrapping_add(driver).wrapping_add(post),
            Ordering::Relaxed,
        );
        self.telemetry_runner_record.fetch_add(
            cycle_count().wrapping_sub(telemetry_started),
            Ordering::Relaxed,
        );
    }

    #[inline(always)]
    pub fn begin_protocol_poll(&self, started: u32) {
        self.active_protocol_poll_started
            .store(started, Ordering::Relaxed);
        self.active_protocol_poll_generation
            .fetch_add(1, Ordering::Relaxed);
        self.active_protocol_poll_saw_frame
            .store(0, Ordering::Relaxed);
        self.active_protocol_poll_saw_dequeue
            .store(0, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn end_protocol_poll(&self, ended: u32) {
        let started = self.active_protocol_poll_started.load(Ordering::Relaxed);
        self.protocol_polls.fetch_add(1, Ordering::Relaxed);
        self.protocol_poll_cycles
            .fetch_add(ended.wrapping_sub(started), Ordering::Relaxed);
        if self.active_protocol_poll_saw_frame.load(Ordering::Relaxed) != 0 {
            let last_frame_end = self
                .active_protocol_poll_last_frame_end
                .load(Ordering::Relaxed);
            self.protocol_polls_with_frame
                .fetch_add(1, Ordering::Relaxed);
            self.protocol_last_frame_to_poll_exit_cycles
                .fetch_add(ended.wrapping_sub(last_frame_end), Ordering::Relaxed);
        }
    }

    #[inline(always)]
    pub(crate) fn record_protocol_frame_dequeued(&self, dequeued: u32) {
        let telemetry_started = cycle_count();
        self.protocol_dequeues.fetch_add(1, Ordering::Relaxed);
        if self
            .active_protocol_poll_saw_dequeue
            .swap(1, Ordering::Relaxed)
            == 0
        {
            let poll_started = self.active_protocol_poll_started.load(Ordering::Relaxed);
            self.protocol_poll_to_first_dequeue_cycles
                .fetch_add(dequeued.wrapping_sub(poll_started), Ordering::Relaxed);
        } else {
            let last_frame_end = self
                .active_protocol_poll_last_frame_end
                .load(Ordering::Relaxed);
            self.protocol_between_frame_to_dequeue_cycles
                .fetch_add(dequeued.wrapping_sub(last_frame_end), Ordering::Relaxed);
        }
        self.active_protocol_poll_last_dequeue
            .store(dequeued, Ordering::Relaxed);
        self.telemetry_protocol_dequeue_record.fetch_add(
            cycle_count().wrapping_sub(telemetry_started),
            Ordering::Relaxed,
        );
    }

    #[inline(always)]
    fn record_protocol_frame_entry(&self, entered: u32) -> u32 {
        let telemetry_started = cycle_count();
        let generation = self.active_protocol_poll_generation.load(Ordering::Relaxed);
        if self
            .active_protocol_poll_saw_frame
            .swap(1, Ordering::Relaxed)
            == 0
        {
            let poll_started = self.active_protocol_poll_started.load(Ordering::Relaxed);
            self.protocol_poll_to_first_frame_cycles
                .fetch_add(entered.wrapping_sub(poll_started), Ordering::Relaxed);
        }
        let dequeued = self
            .active_protocol_poll_last_dequeue
            .load(Ordering::Relaxed);
        self.protocol_dequeue_to_frame_cycles
            .fetch_add(entered.wrapping_sub(dequeued), Ordering::Relaxed);
        self.telemetry_protocol_entry_record.fetch_add(
            cycle_count().wrapping_sub(telemetry_started),
            Ordering::Relaxed,
        );
        generation
    }

    #[inline(always)]
    fn record_protocol_frame(&self, profile: Core0ProtocolCycleProfile, ended: u32) {
        if self.active_protocol_poll_generation.load(Ordering::Relaxed) != profile.poll_generation {
            return;
        }
        let telemetry_started = cycle_count();
        self.active_protocol_poll_last_frame_end
            .store(ended, Ordering::Relaxed);
        self.protocol_frame_calls.fetch_add(1, Ordering::Relaxed);
        match profile.path {
            Core0ProtocolPath::Ordinary => {
                self.protocol_frame_ordinary.fetch_add(1, Ordering::Relaxed);
            }
            Core0ProtocolPath::Scratch => {
                self.protocol_frame_scratch.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.protocol_frame_total
            .fetch_add(ended.wrapping_sub(profile.started), Ordering::Relaxed);
        self.protocol_frame_preflight
            .fetch_add(profile.preflight, Ordering::Relaxed);
        self.protocol_frame_wait
            .fetch_add(profile.wait, Ordering::Relaxed);
        self.protocol_frame_dispatch
            .fetch_add(profile.dispatch, Ordering::Relaxed);
        self.protocol_dispatch_pre_publish
            .fetch_add(profile.dispatch_pre_publish, Ordering::Relaxed);
        self.protocol_dispatch_capture
            .fetch_add(profile.dispatch_capture, Ordering::Relaxed);
        self.protocol_dispatch_post_publish
            .fetch_add(profile.dispatch_post_publish, Ordering::Relaxed);
        self.protocol_frame_publish_tail
            .fetch_add(profile.publish_tail, Ordering::Relaxed);
        self.telemetry_protocol_frame_record.fetch_add(
            cycle_count().wrapping_sub(telemetry_started),
            Ordering::Relaxed,
        );
    }

    #[inline(always)]
    fn record_scheduler_rx(&self, profile: Core0RxSchedulerCycleProfile) {
        if self.active_poll_generation.load(Ordering::Relaxed) != profile.poll_generation {
            return;
        }
        let telemetry_started = cycle_count();
        self.scheduler_rx_calls.fetch_add(1, Ordering::Relaxed);
        match profile.path {
            Core0RxSchedulerPath::Software => {
                self.scheduler_software_rx_calls
                    .fetch_add(1, Ordering::Relaxed);
            }
            Core0RxSchedulerPath::Irq => {
                self.scheduler_irq_rx_calls.fetch_add(1, Ordering::Relaxed);
            }
            Core0RxSchedulerPath::Select => {
                self.scheduler_select_rx_calls
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        self.scheduler_reentry_cycles
            .fetch_add(profile.reentry, Ordering::Relaxed);
        self.scheduler_stop_cycles
            .fetch_add(profile.stop, Ordering::Relaxed);
        self.scheduler_housekeeping_cycles
            .fetch_add(profile.housekeeping, Ordering::Relaxed);
        self.scheduler_discard_wakes_cycles
            .fetch_add(profile.discard_wakes, Ordering::Relaxed);
        self.scheduler_first_network_queue_cycles
            .fetch_add(profile.first_network_queue, Ordering::Relaxed);
        self.scheduler_control_ready_cycles
            .fetch_add(profile.control_ready, Ordering::Relaxed);
        self.scheduler_tx_checks_cycles
            .fetch_add(profile.tx_checks, Ordering::Relaxed);
        self.scheduler_prepared_cycles
            .fetch_add(profile.prepared, Ordering::Relaxed);
        self.scheduler_network_pending_cycles
            .fetch_add(profile.network_pending, Ordering::Relaxed);
        self.scheduler_idle_accounting_cycles
            .fetch_add(profile.idle_accounting, Ordering::Relaxed);
        self.scheduler_rx_checks_cycles
            .fetch_add(profile.rx_checks, Ordering::Relaxed);
        self.telemetry_scheduler_record.fetch_add(
            cycle_count().wrapping_sub(telemetry_started),
            Ordering::Relaxed,
        );
    }
}

impl Default for Core0RxCycleCounters {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Core0ControlOutcome {
    Idle,
    More,
    TxPending,
    Exit,
}

#[derive(Clone, Copy)]
pub(crate) enum Core0RxSchedulerPath {
    Software,
    Irq,
    Select,
}

#[derive(Clone, Copy)]
pub(crate) enum Core0ProtocolPath {
    Ordinary,
    Scratch,
}

pub static CORE0_RX_CYCLES: Core0RxCycleCounters = Core0RxCycleCounters::new();

#[derive(Clone, Copy)]
pub(crate) enum Core0RxCyclePhase {
    Setup,
    Frontier,
    Admission,
    StageTake,
    StagePool,
    Recycle,
    Reload,
    Publish,
    Tail,
}

pub(crate) struct Core0ProtocolCycleProfile {
    performance_started: Core0PerformanceSample,
    poll_generation: u32,
    started: u32,
    last: u32,
    preflight: u32,
    wait: u32,
    dispatch: u32,
    dispatch_pre_publish: u32,
    dispatch_capture: u32,
    dispatch_post_publish: u32,
    publish_tail: u32,
    path: Core0ProtocolPath,
}

impl Core0ProtocolCycleProfile {
    #[inline(always)]
    pub(crate) fn begin() -> Self {
        let performance_started = Core0PerformanceSample::read();
        let now = performance_started.cycles();
        Self {
            performance_started,
            poll_generation: CORE0_RX_CYCLES.record_protocol_frame_entry(now),
            started: now,
            last: now,
            preflight: 0,
            wait: 0,
            dispatch: 0,
            dispatch_pre_publish: 0,
            dispatch_capture: 0,
            dispatch_post_publish: 0,
            publish_tail: 0,
            path: Core0ProtocolPath::Ordinary,
        }
    }

    #[inline(always)]
    pub(crate) fn preflight_completed(&mut self) {
        let now = cycle_count();
        self.preflight = now.wrapping_sub(self.last);
        self.last = now;
    }

    #[inline(always)]
    pub(crate) fn wait_completed(&mut self) {
        let now = cycle_count();
        self.wait = now.wrapping_sub(self.last);
        self.last = now;
    }

    #[inline(always)]
    pub(crate) fn dispatch_completed(&mut self) {
        let now = cycle_count();
        self.dispatch = now.wrapping_sub(self.last);
        self.last = now;
    }

    #[inline(always)]
    pub(crate) fn dispatch_split_completed(&mut self, callback_started: u32, callback_ended: u32) {
        let now = cycle_count();
        self.dispatch_pre_publish = callback_started.wrapping_sub(self.last);
        self.dispatch_capture = callback_ended.wrapping_sub(callback_started);
        self.dispatch_post_publish = now.wrapping_sub(callback_ended);
        self.dispatch = now.wrapping_sub(self.last);
        self.last = now;
    }

    #[inline(always)]
    pub(crate) fn finish(mut self, path: Core0ProtocolPath) {
        let performance_ended = Core0PerformanceSample::read();
        let now = performance_ended.cycles();
        self.publish_tail = now.wrapping_sub(self.last);
        self.path = path;
        let performance_started = self.performance_started;
        CORE0_RX_CYCLES.record_protocol_frame(self, now);
        CORE0_PERFORMANCE.record_protocol_frame(performance_started, performance_ended);
    }
}

pub(crate) struct Core0RxSchedulerCycleProfile {
    poll_generation: u32,
    last: u32,
    reentry: u32,
    stop: u32,
    housekeeping: u32,
    discard_wakes: u32,
    first_network_queue: u32,
    control_ready: u32,
    tx_checks: u32,
    prepared: u32,
    network_pending: u32,
    idle_accounting: u32,
    rx_checks: u32,
    path: Core0RxSchedulerPath,
}

impl Core0RxSchedulerCycleProfile {
    #[inline(always)]
    pub(crate) fn begin() -> Self {
        let now = cycle_count();
        Self {
            poll_generation: CORE0_RX_CYCLES
                .active_poll_generation
                .load(Ordering::Relaxed),
            last: now,
            reentry: now.wrapping_sub(CORE0_RX_CYCLES.active_poll_started.load(Ordering::Relaxed)),
            stop: 0,
            housekeeping: 0,
            discard_wakes: 0,
            first_network_queue: 0,
            control_ready: 0,
            tx_checks: 0,
            prepared: 0,
            network_pending: 0,
            idle_accounting: 0,
            rx_checks: 0,
            path: Core0RxSchedulerPath::Software,
        }
    }

    #[inline(always)]
    pub(crate) fn stop_completed(&mut self) {
        let now = cycle_count();
        self.stop = now.wrapping_sub(self.last);
        self.last = now;
    }

    #[inline(always)]
    pub(crate) fn discard_wakes_completed(&mut self) {
        let now = cycle_count();
        self.discard_wakes = now.wrapping_sub(self.last);
        self.last = now;
    }

    #[inline(always)]
    pub(crate) fn first_network_queue_completed(&mut self) {
        let now = cycle_count();
        self.first_network_queue = now.wrapping_sub(self.last);
        self.last = now;
    }

    #[inline(always)]
    pub(crate) fn control_ready_completed(&mut self) {
        let now = cycle_count();
        self.control_ready = now.wrapping_sub(self.last);
        self.housekeeping = self
            .discard_wakes
            .wrapping_add(self.first_network_queue)
            .wrapping_add(self.control_ready);
        self.last = now;
    }

    #[inline(always)]
    pub(crate) fn prepared_completed(&mut self) {
        let now = cycle_count();
        self.prepared = now.wrapping_sub(self.last);
        self.last = now;
    }

    #[inline(always)]
    pub(crate) fn network_pending_completed(&mut self) {
        let now = cycle_count();
        self.network_pending = now.wrapping_sub(self.last);
        self.last = now;
    }

    #[inline(always)]
    pub(crate) fn tx_checks_completed(&mut self) {
        let now = cycle_count();
        self.idle_accounting = now.wrapping_sub(self.last);
        self.tx_checks = self
            .prepared
            .wrapping_add(self.network_pending)
            .wrapping_add(self.idle_accounting);
        self.last = now;
    }

    #[inline(always)]
    pub(crate) fn finish(mut self, path: Core0RxSchedulerPath) {
        self.rx_checks = cycle_count().wrapping_sub(self.last);
        self.path = path;
        CORE0_RX_CYCLES.record_scheduler_rx(self);
    }
}

pub(crate) struct Core0RxCycleProfile {
    performance_started: Core0PerformanceSample,
    started: u32,
    last: u32,
    phase: Core0RxCyclePhase,
    sample: Core0RxCycleSnapshot,
}

pub(crate) struct Core0RxRunnerCycleProfile {
    performance_started: Core0PerformanceSample,
    started: u32,
    last: u32,
    pre: u32,
    driver: u32,
}

impl Core0RxRunnerCycleProfile {
    #[inline(always)]
    pub(crate) fn begin() -> Self {
        let performance_started = Core0PerformanceSample::read();
        let now = performance_started.cycles();
        CORE0_RX_CYCLES.record_runner_entry(now);
        Self {
            performance_started,
            started: now,
            last: now,
            pre: 0,
            driver: 0,
        }
    }

    #[inline(always)]
    pub(crate) fn begin_driver(&mut self) {
        let now = cycle_count();
        self.pre = now.wrapping_sub(self.last);
        self.last = now;
    }

    #[inline(always)]
    pub(crate) fn end_driver(&mut self) {
        let now = cycle_count();
        self.driver = now.wrapping_sub(self.last);
        self.last = now;
    }

    /// Record immediately before the cooperative yield. Time asleep after
    /// yielding is deliberately excluded from Core0 executor residence.
    #[inline(always)]
    pub(crate) fn finish_before_yield(self) {
        let performance_ended = Core0PerformanceSample::read();
        let now = performance_ended.cycles();
        let post = now.wrapping_sub(self.last);
        debug_assert_eq!(
            now.wrapping_sub(self.started),
            self.pre.wrapping_add(self.driver).wrapping_add(post)
        );
        CORE0_RX_CYCLES.record_runner_rx(self.pre, self.driver, post);
        CORE0_RX_CYCLES.record_runner_end(now);
        CORE0_PERFORMANCE.record_runner(self.performance_started, performance_ended);
    }
}

impl Core0RxCycleProfile {
    #[inline(always)]
    pub(crate) fn begin() -> Self {
        let performance_started = Core0PerformanceSample::read();
        let now = performance_started.cycles();
        Self {
            performance_started,
            started: now,
            last: now,
            phase: Core0RxCyclePhase::Setup,
            sample: Core0RxCycleSnapshot::default(),
        }
    }

    #[inline(always)]
    pub(crate) fn switch_to(&mut self, phase: Core0RxCyclePhase) {
        let now = cycle_count();
        self.add_current(now.wrapping_sub(self.last));
        self.last = now;
        self.phase = phase;
    }

    #[inline(always)]
    pub(crate) fn finish(mut self, units: usize) {
        let performance_ended = Core0PerformanceSample::read();
        let now = performance_ended.cycles();
        self.add_current(now.wrapping_sub(self.last));
        self.sample.units = u32::try_from(units).unwrap_or(u32::MAX);
        self.sample.total = now.wrapping_sub(self.started);
        CORE0_PERFORMANCE.record_dma(units, self.performance_started, performance_ended);
        crate::diagnostics::core0_rx_service_histogram::CORE0_RX_SERVICE_HISTOGRAM
            .record_service(units, &self.sample);
        if units == 0 {
            return;
        }
        CORE0_RX_CYCLES.record(self.sample);
    }

    #[inline(always)]
    fn add_current(&mut self, elapsed: u32) {
        let destination = match self.phase {
            Core0RxCyclePhase::Setup => &mut self.sample.setup,
            Core0RxCyclePhase::Frontier => &mut self.sample.frontier,
            Core0RxCyclePhase::Admission => &mut self.sample.admission,
            Core0RxCyclePhase::StageTake => {
                self.sample.stage_total = self.sample.stage_total.wrapping_add(elapsed);
                &mut self.sample.stage_take
            }
            Core0RxCyclePhase::StagePool => {
                self.sample.stage_total = self.sample.stage_total.wrapping_add(elapsed);
                &mut self.sample.stage_pool
            }
            Core0RxCyclePhase::Recycle => &mut self.sample.recycle,
            Core0RxCyclePhase::Reload => &mut self.sample.reload,
            Core0RxCyclePhase::Publish => &mut self.sample.publish,
            Core0RxCyclePhase::Tail => &mut self.sample.tail,
        };
        *destination = destination.wrapping_add(elapsed);
    }
}

#[cfg(target_arch = "riscv32")]
#[inline(always)]
pub fn cycle_count() -> u32 {
    riscv::register::mcycle::read() as u32
}

#[cfg(not(target_arch = "riscv32"))]
#[inline(always)]
pub fn cycle_count() -> u32 {
    0
}

#[cfg(test)]
mod tests;
