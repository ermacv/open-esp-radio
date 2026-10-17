//! Event-driven ESP32-S31 receive-signal power measurement.
//!
//! Primary reference: complete rev0 ROM `phy_get_rx_sig_pwr` at
//! `0x2f82_9ea2`, size `0x76`, in `esp32s31_rev0_rom.elf`.
//!
//! ROM enables both PHY clocks, synchronously disables a previous IQ
//! estimator session, starts a new session through the blocking
//! `phy_iq_est_enable`, and returns the squared magnitude of two derived
//! signed accumulator values. This module preserves that order but exposes
//! both one-microsecond intervals and readiness as external completions.

/// Required pinned `libphy.a` vendor-ABI no-op leaf; the body is one `ret`.
#[inline]
#[cfg(feature = "validation-probes")]
pub const fn noise_check_loop() {}

use crate::calibration::estimator::{
    PhyDcIqDelayPhase, PhyDcIqEnablePhase, PhyDcIqReadinessSnapshot,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhySignalPowerRequest {
    /// Parent-owned identity; ROM receives only `shift`.
    pub measurement: u16,
    pub shift: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhySignalPowerAccumulatorSnapshot {
    /// PAC `SIGNAL_POWER_SUM_I`.
    pub sum_i: i32,
    /// PAC `SIGNAL_POWER_DIFFERENCE_I`.
    pub difference_i: i32,
    /// PAC `SIGNAL_POWER_DIFFERENCE_Q`.
    pub difference_q: i32,
    /// PAC `SIGNAL_POWER_SUM_Q`.
    pub sum_q: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhySignalPowerOutcome {
    pub request: PhySignalPowerRequest,
    pub value: i64,
    /// Rust-owned replacement for `phy_param_rom + 0x1ac`.
    pub readiness_activity_edges: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhySignalPowerFailure {
    ReadinessTimedOut {
        request: PhySignalPowerRequest,
        readiness_activity_edges: u16,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhySignalPowerClock {
    Tx,
    Rx,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhySignalPowerAction {
    ConfigureClock {
        request: PhySignalPowerRequest,
        clock: PhySignalPowerClock,
        enabled: bool,
    },
    SetEstimatorEnable {
        request: PhySignalPowerRequest,
        phase: PhyDcIqEnablePhase,
        enabled: bool,
    },
    DelayMicros {
        request: PhySignalPowerRequest,
        phase: PhyDcIqDelayPhase,
        micros: u32,
    },
    ConfigureEstimator {
        request: PhySignalPowerRequest,
        control: u16,
    },
    AwaitReadinessEdge {
        request: PhySignalPowerRequest,
        readiness_activity_edges: u16,
        readiness_samples: u16,
    },
    ReadAccumulators(PhySignalPowerRequest),
    Complete(PhySignalPowerOutcome),
    Failed(PhySignalPowerFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhySignalPowerCompletion {
    ClockConfigured {
        request: PhySignalPowerRequest,
        clock: PhySignalPowerClock,
        enabled: bool,
    },
    EstimatorEnableSet {
        request: PhySignalPowerRequest,
        phase: PhyDcIqEnablePhase,
        enabled: bool,
    },
    DelayElapsed {
        request: PhySignalPowerRequest,
        phase: PhyDcIqDelayPhase,
        micros: u32,
    },
    EstimatorConfigured {
        request: PhySignalPowerRequest,
        control: u16,
    },
    ReadinessObserved {
        request: PhySignalPowerRequest,
        snapshot: PhyDcIqReadinessSnapshot,
    },
    ReadinessTimedOut(PhySignalPowerRequest),
    AccumulatorsRead {
        request: PhySignalPowerRequest,
        snapshot: PhySignalPowerAccumulatorSnapshot,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhySignalPowerTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhySignalPowerStep {
    EnableTxClock,
    EnableRxClock,
    DisablePreviousMeasurement,
    PreviousStopDelay,
    DisablePreviousStart,
    ConfigureEstimator,
    EnableStart,
    StartDelay,
    EnableMeasurement,
    AwaitReadiness,
    ReadAccumulators,
    DisableFailedMeasurement(PhySignalPowerFailure),
    FailedStopDelay(PhySignalPowerFailure),
    DisableFailedStart(PhySignalPowerFailure),
    Complete(PhySignalPowerOutcome),
    Failed(PhySignalPowerFailure),
}

/// Exact control halfword passed by ROM to `phy_iq_est_enable`.
///
/// Both the low-five-bit shift behavior and the following `u16` truncation
/// are explicit. The reachable crystal-duty request is `shift = 12`, which
/// yields `0x1000`.
pub const fn signal_power_estimator_control(shift: u8) -> u16 {
    1_u32.wrapping_shl((shift as u32) & 0x1f) as u16
}

/// Exact stateless arithmetic suffix of rev0 ROM `phy_get_rx_sig_pwr`.
pub fn calculate_signal_power(
    request: PhySignalPowerRequest,
    snapshot: PhySignalPowerAccumulatorSnapshot,
) -> i64 {
    let shift = u32::from(request.shift.wrapping_sub(2)) & 0x1f;
    let sum = (snapshot.sum_i >> shift).wrapping_add(snapshot.sum_q >> shift);
    let difference = (snapshot.difference_i >> shift).wrapping_sub(snapshot.difference_q >> shift);
    let sum = i64::from(sum);
    let difference = i64::from(difference);
    sum.wrapping_mul(sum)
        .wrapping_add(difference.wrapping_mul(difference))
}

/// Heap-free, caller-driven replacement for `phy_get_rx_sig_pwr`.
///
/// Success intentionally leaves the estimator enabled, matching the complete
/// ROM body. The next measurement begins with the ROM-equivalent disable
/// tail. A typed timeout performs that same disable tail before failing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhySignalPowerTransition {
    request: PhySignalPowerRequest,
    readiness_activity_edges: u16,
    readiness_samples: u16,
    step: PhySignalPowerStep,
}

impl PhySignalPowerTransition {
    pub const fn new(request: PhySignalPowerRequest) -> Self {
        Self {
            request,
            readiness_activity_edges: 0,
            readiness_samples: 0,
            step: PhySignalPowerStep::EnableTxClock,
        }
    }

    pub const fn action(self) -> PhySignalPowerAction {
        match self.step {
            PhySignalPowerStep::EnableTxClock => PhySignalPowerAction::ConfigureClock {
                request: self.request,
                clock: PhySignalPowerClock::Tx,
                enabled: true,
            },
            PhySignalPowerStep::EnableRxClock => PhySignalPowerAction::ConfigureClock {
                request: self.request,
                clock: PhySignalPowerClock::Rx,
                enabled: true,
            },
            PhySignalPowerStep::DisablePreviousMeasurement
            | PhySignalPowerStep::DisableFailedMeasurement(_) => {
                PhySignalPowerAction::SetEstimatorEnable {
                    request: self.request,
                    phase: PhyDcIqEnablePhase::Measurement,
                    enabled: false,
                }
            }
            PhySignalPowerStep::PreviousStopDelay | PhySignalPowerStep::FailedStopDelay(_) => {
                PhySignalPowerAction::DelayMicros {
                    request: self.request,
                    phase: PhyDcIqDelayPhase::Stop,
                    micros: 1,
                }
            }
            PhySignalPowerStep::DisablePreviousStart
            | PhySignalPowerStep::DisableFailedStart(_) => {
                PhySignalPowerAction::SetEstimatorEnable {
                    request: self.request,
                    phase: PhyDcIqEnablePhase::Start,
                    enabled: false,
                }
            }
            PhySignalPowerStep::ConfigureEstimator => PhySignalPowerAction::ConfigureEstimator {
                request: self.request,
                control: signal_power_estimator_control(self.request.shift),
            },
            PhySignalPowerStep::EnableStart => PhySignalPowerAction::SetEstimatorEnable {
                request: self.request,
                phase: PhyDcIqEnablePhase::Start,
                enabled: true,
            },
            PhySignalPowerStep::StartDelay => PhySignalPowerAction::DelayMicros {
                request: self.request,
                phase: PhyDcIqDelayPhase::Start,
                micros: 1,
            },
            PhySignalPowerStep::EnableMeasurement => PhySignalPowerAction::SetEstimatorEnable {
                request: self.request,
                phase: PhyDcIqEnablePhase::Measurement,
                enabled: true,
            },
            PhySignalPowerStep::AwaitReadiness => PhySignalPowerAction::AwaitReadinessEdge {
                request: self.request,
                readiness_activity_edges: self.readiness_activity_edges,
                readiness_samples: self.readiness_samples,
            },
            PhySignalPowerStep::ReadAccumulators => {
                PhySignalPowerAction::ReadAccumulators(self.request)
            }
            PhySignalPowerStep::Complete(outcome) => PhySignalPowerAction::Complete(outcome),
            PhySignalPowerStep::Failed(failure) => PhySignalPowerAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhySignalPowerCompletion,
    ) -> Result<(), PhySignalPowerTransitionError> {
        self.step = match (self.step, completion) {
            (
                PhySignalPowerStep::EnableTxClock,
                PhySignalPowerCompletion::ClockConfigured {
                    request,
                    clock: PhySignalPowerClock::Tx,
                    enabled: true,
                },
            ) if request == self.request => PhySignalPowerStep::EnableRxClock,
            (
                PhySignalPowerStep::EnableRxClock,
                PhySignalPowerCompletion::ClockConfigured {
                    request,
                    clock: PhySignalPowerClock::Rx,
                    enabled: true,
                },
            ) if request == self.request => PhySignalPowerStep::DisablePreviousMeasurement,
            (
                PhySignalPowerStep::DisablePreviousMeasurement,
                PhySignalPowerCompletion::EstimatorEnableSet {
                    request,
                    phase: PhyDcIqEnablePhase::Measurement,
                    enabled: false,
                },
            ) if request == self.request => PhySignalPowerStep::PreviousStopDelay,
            (
                PhySignalPowerStep::PreviousStopDelay,
                PhySignalPowerCompletion::DelayElapsed {
                    request,
                    phase: PhyDcIqDelayPhase::Stop,
                    micros: 1,
                },
            ) if request == self.request => PhySignalPowerStep::DisablePreviousStart,
            (
                PhySignalPowerStep::DisablePreviousStart,
                PhySignalPowerCompletion::EstimatorEnableSet {
                    request,
                    phase: PhyDcIqEnablePhase::Start,
                    enabled: false,
                },
            ) if request == self.request => PhySignalPowerStep::ConfigureEstimator,
            (
                PhySignalPowerStep::ConfigureEstimator,
                PhySignalPowerCompletion::EstimatorConfigured { request, control },
            ) if request == self.request
                && control == signal_power_estimator_control(self.request.shift) =>
            {
                PhySignalPowerStep::EnableStart
            }
            (
                PhySignalPowerStep::EnableStart,
                PhySignalPowerCompletion::EstimatorEnableSet {
                    request,
                    phase: PhyDcIqEnablePhase::Start,
                    enabled: true,
                },
            ) if request == self.request => PhySignalPowerStep::StartDelay,
            (
                PhySignalPowerStep::StartDelay,
                PhySignalPowerCompletion::DelayElapsed {
                    request,
                    phase: PhyDcIqDelayPhase::Start,
                    micros: 1,
                },
            ) if request == self.request => PhySignalPowerStep::EnableMeasurement,
            (
                PhySignalPowerStep::EnableMeasurement,
                PhySignalPowerCompletion::EstimatorEnableSet {
                    request,
                    phase: PhyDcIqEnablePhase::Measurement,
                    enabled: true,
                },
            ) if request == self.request => PhySignalPowerStep::AwaitReadiness,
            (
                PhySignalPowerStep::AwaitReadiness,
                PhySignalPowerCompletion::ReadinessObserved {
                    request,
                    snapshot: PhyDcIqReadinessSnapshot { ready: true, .. },
                },
            ) if request == self.request => {
                self.readiness_samples = self.readiness_samples.saturating_add(1);
                PhySignalPowerStep::ReadAccumulators
            }
            (
                PhySignalPowerStep::AwaitReadiness,
                PhySignalPowerCompletion::ReadinessObserved {
                    request,
                    snapshot:
                        PhyDcIqReadinessSnapshot {
                            ready: false,
                            activity,
                        },
                },
            ) if request == self.request => {
                self.readiness_samples = self.readiness_samples.saturating_add(1);
                if activity {
                    self.readiness_activity_edges = self.readiness_activity_edges.wrapping_add(1);
                }
                PhySignalPowerStep::AwaitReadiness
            }
            (
                PhySignalPowerStep::AwaitReadiness,
                PhySignalPowerCompletion::ReadinessTimedOut(request),
            ) if request == self.request => PhySignalPowerStep::DisableFailedMeasurement(
                PhySignalPowerFailure::ReadinessTimedOut {
                    request,
                    readiness_activity_edges: self.readiness_activity_edges,
                },
            ),
            (
                PhySignalPowerStep::ReadAccumulators,
                PhySignalPowerCompletion::AccumulatorsRead { request, snapshot },
            ) if request == self.request => PhySignalPowerStep::Complete(PhySignalPowerOutcome {
                request,
                value: calculate_signal_power(request, snapshot),
                readiness_activity_edges: self.readiness_activity_edges,
            }),
            (
                PhySignalPowerStep::DisableFailedMeasurement(failure),
                PhySignalPowerCompletion::EstimatorEnableSet {
                    request,
                    phase: PhyDcIqEnablePhase::Measurement,
                    enabled: false,
                },
            ) if request == self.request => PhySignalPowerStep::FailedStopDelay(failure),
            (
                PhySignalPowerStep::FailedStopDelay(failure),
                PhySignalPowerCompletion::DelayElapsed {
                    request,
                    phase: PhyDcIqDelayPhase::Stop,
                    micros: 1,
                },
            ) if request == self.request => PhySignalPowerStep::DisableFailedStart(failure),
            (
                PhySignalPowerStep::DisableFailedStart(failure),
                PhySignalPowerCompletion::EstimatorEnableSet {
                    request,
                    phase: PhyDcIqEnablePhase::Start,
                    enabled: false,
                },
            ) if request == self.request => PhySignalPowerStep::Failed(failure),
            (PhySignalPowerStep::Complete(_), _) | (PhySignalPowerStep::Failed(_), _) => {
                return Err(PhySignalPowerTransitionError::AlreadyComplete);
            }
            _ => return Err(PhySignalPowerTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests;
