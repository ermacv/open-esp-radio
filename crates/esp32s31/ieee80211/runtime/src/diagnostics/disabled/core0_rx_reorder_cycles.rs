//! Zero-sized stand-in for `core0_rx_reorder_cycles` without `task-poll-telemetry`.
//!
//! Every item mirrors the telemetry module API used by the datapath and does
//! nothing, so call sites need no feature gate.

pub(crate) use super::core0_paths::{Core0DirectPath, Core0ReorderPath};

pub(crate) struct Core0ReorderCounters;

pub(crate) static CORE0_REORDER_CYCLES: Core0ReorderCounters = Core0ReorderCounters;

impl Core0ReorderCounters {
    #[inline(always)]
    pub(crate) fn bank_completed(&self) {}

    #[inline(always)]
    pub(crate) fn begin(&self) {}

    #[inline(always)]
    pub(crate) fn deadline_completed(&self) {}

    #[inline(always)]
    pub(crate) fn finish(&self, path: Core0ReorderPath) {
        let _ = path;
    }

    #[inline(always)]
    pub(crate) fn first_completed(&self) {}

    #[inline(always)]
    pub(crate) fn ingest_completed(&self) {}

    #[inline(always)]
    pub(crate) fn ingress_observer_completed(&self) {}

    #[inline(always)]
    pub(crate) fn key_completed(&self) {}

    #[inline(always)]
    pub(crate) fn occupied_observer_completed(&self) {}

    #[inline(always)]
    pub(crate) fn prepared_observer_completed(&self) {}

    #[inline(always)]
    pub(crate) fn release_observer_completed(&self) {}
}

pub(crate) struct Core0DirectCycleProfile;

impl Core0DirectCycleProfile {
    #[inline(always)]
    pub(crate) fn bank_completed(&mut self) {}

    #[inline(always)]
    pub(crate) fn deadline_completed(&mut self) {}

    #[inline(always)]
    pub(crate) fn dispatch_completed(&mut self) {}

    #[inline(always)]
    pub(crate) fn finish(self, path: Core0DirectPath) {
        let _ = path;
    }

    #[inline(always)]
    pub(crate) fn ingest_completed(&mut self) {}

    #[inline(always)]
    pub(crate) fn key_completed(&mut self) {}

    #[inline(always)]
    pub(crate) fn preflight_completed(&mut self) {}

    #[inline(always)]
    pub(crate) fn begin() -> Self {
        Self
    }
}
