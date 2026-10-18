//! Zero-sized stand-in for `core0_ap_rx_cycles` without `task-poll-telemetry`.
//!
//! Every item mirrors the telemetry module API used by the datapath and does
//! nothing, so call sites need no feature gate.

pub(crate) use super::core0_paths::Core0ApRxTurnExit;

pub(crate) struct Core0ApRxCycleCounters;

pub(crate) static CORE0_AP_RX_CYCLES: Core0ApRxCycleCounters = Core0ApRxCycleCounters;

impl Core0ApRxCycleCounters {
    #[inline(always)]
    pub(crate) fn record_turn(&self, frames: usize, exit: Core0ApRxTurnExit) {
        let _ = (frames, exit);
    }
}
