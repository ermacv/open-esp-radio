//! Zero-sized stand-in for `core0_rx_cycles` without `task-poll-telemetry`.
//!
//! Every item mirrors the telemetry module API used by the datapath and does
//! nothing, so call sites need no feature gate.

/// Always zero without task-poll telemetry.
#[inline(always)]
pub(crate) const fn cycle_count() -> u32 {
    0
}

pub(crate) struct Core0RxCycleCounters;

pub(crate) static CORE0_RX_CYCLES: Core0RxCycleCounters = Core0RxCycleCounters;

impl Core0RxCycleCounters {
    #[inline(always)]
    pub fn begin_protocol_poll(&self, started: u32) {
        let _ = started;
    }

    #[inline(always)]
    pub fn end_protocol_poll(&self, ended: u32) {
        let _ = ended;
    }

    #[inline(always)]
    pub(crate) fn record_protocol_frame_dequeued(&self, dequeued: u32) {
        let _ = dequeued;
    }
}
