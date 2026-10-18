//! RX runner and DMA service profiles selected by the enabled Core0 cycle
//! telemetry.
//!
//! Call sites use these types unconditionally. `task-poll-telemetry` selects
//! the phase-resolved profiles, `core0-rx-coarse-telemetry` the coarse ones,
//! and production builds get zero-sized profiles whose methods are empty.

use oer_esp32s31_ieee80211::rx::transaction::Phase;

#[cfg(feature = "task-poll-telemetry")]
pub(crate) use super::core0_rx_cycles::Core0RxRunnerCycleProfile as RxRunnerProfile;
#[cfg(all(
    feature = "core0-rx-coarse-telemetry",
    not(feature = "task-poll-telemetry")
))]
pub(crate) use super::core0_rx_performance::Core0PerformanceRunnerProfile as RxRunnerProfile;

/// Profile of one complete RX runner call; empty without cycle telemetry.
#[cfg(not(any(feature = "core0-rx-coarse-telemetry", feature = "task-poll-telemetry")))]
pub(crate) struct RxRunnerProfile;

#[cfg(not(any(feature = "core0-rx-coarse-telemetry", feature = "task-poll-telemetry")))]
impl RxRunnerProfile {
    #[inline(always)]
    pub(crate) fn begin() -> Self {
        Self
    }

    #[inline(always)]
    pub(crate) fn begin_driver(&mut self) {}

    #[inline(always)]
    pub(crate) fn end_driver(&mut self) {}

    #[inline(always)]
    pub(crate) fn finish_before_yield(self) {}
}

/// Profile of one DMA service transaction.
pub(crate) struct RxDmaProfile {
    #[cfg(feature = "task-poll-telemetry")]
    inner: super::core0_rx_cycles::Core0RxCycleProfile,
    #[cfg(all(
        feature = "core0-rx-coarse-telemetry",
        not(feature = "task-poll-telemetry")
    ))]
    inner: super::core0_rx_performance::Core0PerformanceDmaProfile,
}

impl RxDmaProfile {
    #[inline(always)]
    pub(crate) fn begin() -> Self {
        Self {
            #[cfg(feature = "task-poll-telemetry")]
            inner: super::core0_rx_cycles::Core0RxCycleProfile::begin(),
            #[cfg(all(
                feature = "core0-rx-coarse-telemetry",
                not(feature = "task-poll-telemetry")
            ))]
            inner: super::core0_rx_performance::Core0PerformanceDmaProfile::begin(),
        }
    }

    /// Attribute following cycles to `phase`; only the phase-resolved
    /// profile distinguishes phases.
    #[inline(always)]
    pub(crate) fn switch_to(&mut self, phase: Phase) {
        #[cfg(feature = "task-poll-telemetry")]
        {
            use super::core0_rx_cycles::Core0RxCyclePhase;
            self.inner.switch_to(match phase {
                Phase::Frontier => Core0RxCyclePhase::Frontier,
                Phase::Admission => Core0RxCyclePhase::Admission,
                Phase::StageTake => Core0RxCyclePhase::StageTake,
                Phase::StagePool => Core0RxCyclePhase::StagePool,
                Phase::Recycle => Core0RxCyclePhase::Recycle,
                Phase::Reload => Core0RxCyclePhase::Reload,
                Phase::Publish => Core0RxCyclePhase::Publish,
                Phase::Tail => Core0RxCyclePhase::Tail,
            });
        }
        #[cfg(not(feature = "task-poll-telemetry"))]
        let _ = phase;
    }

    #[inline(always)]
    pub(crate) fn finish(self, units: usize) {
        #[cfg(any(feature = "core0-rx-coarse-telemetry", feature = "task-poll-telemetry"))]
        self.inner.finish(units);
        #[cfg(not(any(feature = "core0-rx-coarse-telemetry", feature = "task-poll-telemetry")))]
        let _ = units;
    }
}
