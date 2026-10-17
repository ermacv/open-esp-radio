//! Statically dispatched observation hooks without hardware or lease access.

use super::{Discard, ServiceObservation};
use oer_esp32s31_ieee80211_mac::rx::RxRingError;

#[derive(Clone, Copy)]
pub enum Phase {
    Frontier,
    Admission,
    StageTake,
    StagePool,
    Recycle,
    Reload,
    Publish,
    Tail,
}

/// Adapter-local timing and diagnostics for one synchronous transaction.
///
/// Disabled hooks must not sample clocks. The associated flags guard optional
/// hardware observations before their arguments are evaluated.
#[allow(unused_variables)]
pub trait Hooks {
    const LOG_ERRORS: bool = false;
    const SAMPLE_ENTRY_REMAINING: bool = false;
    fn observing(&self) -> bool {
        false
    }
    fn entry_remaining(&mut self, remaining: Option<usize>) {}
    fn phase(&mut self, phase: Phase) {}
    fn stage_discarded(&self, discard: Discard) {}
    fn now_micros(&self) -> Option<u64> {
        None
    }
    fn reload_completed(&self, started: Option<u64>) {}
    fn elapsed_service_micros(&self) -> u64 {
        0
    }
    fn service_completed(&self, observation: ServiceObservation) {}
    /// Preserved diagnostic selector for the post-recycle continuation. Unlike
    /// observation methods, this explicit hook selects the existing probe policy.
    fn recycled_probe_pending(&self, recycled_descriptors: usize) -> bool {
        recycled_descriptors != 0
    }
    fn probe_reasons(&self, recycled: bool, frontier: bool, writeback: bool, republication: bool) {}
    fn finish(self, units: usize)
    where
        Self: Sized,
    {
    }
    fn log_busy(
        &self,
        stage: &'static str,
        error: RxRingError,
        head: usize,
        descriptors: usize,
        detached: usize,
        released: usize,
    ) {
    }
    #[allow(clippy::too_many_arguments)]
    fn log_recycle_refused(
        &self,
        descriptors: usize,
        recycle_start: usize,
        accepted_tail: usize,
        observed: u32,
        last: u32,
        next: u32,
        software_reload_pending: bool,
        hardware_reload_pending: bool,
    ) {
    }
}
impl Hooks for () {}
