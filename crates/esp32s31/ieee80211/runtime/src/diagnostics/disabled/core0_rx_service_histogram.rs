//! Zero-sized stand-in for `core0_rx_service_histogram` without `task-poll-telemetry`.
//!
//! Every item mirrors the telemetry module API used by the datapath and does
//! nothing, so call sites need no feature gate.

pub(crate) struct Core0RxServiceHistogram;

pub(crate) static CORE0_RX_SERVICE_HISTOGRAM: Core0RxServiceHistogram = Core0RxServiceHistogram;

impl Core0RxServiceHistogram {
    #[inline(always)]
    pub(crate) fn record_spsc_pop(&self, cycles: u32, empty: bool) {
        let _ = (cycles, empty);
    }

    #[inline(always)]
    pub(crate) fn record_spsc_push(&self, cycles: u32, full: bool) {
        let _ = (cycles, full);
    }
}
