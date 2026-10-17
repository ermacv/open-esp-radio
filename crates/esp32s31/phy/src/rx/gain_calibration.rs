//! Rust-owned ESP32-S31 RX gain DC-calibration primitives.
//!
//! The one-step control flow is recovered from archive
//! `phy_pbus_rx_dco_cal_1step_new`; its measurement children are rev0 ROM
//! `phy_rxdc_est_min` and `phy_dc_iq_est`.
//! Debug-only `ets_printf` branches are intentionally absent. Hardware
//! commands, ten-microsecond intervals, and DC/IQ readiness are explicit
//! caller-driven actions.

mod quality;
pub use quality::PhyRxGainDcQuality;

use crate::{
    analog::{
        i2c::analog_registers,
        pbus::PhyPbusForceTest,
        rfpll::{
            RfpllFrequencyAction, RfpllFrequencyCompletion, RfpllFrequencyFailure,
            RfpllFrequencyRequest, RfpllFrequencyTransition,
        },
    },
    rx::dc_offset::{
        PhyRxDcMinimumAction, PhyRxDcMinimumCompletion, PhyRxDcMinimumFailure,
        PhyRxDcMinimumOutcome, PhyRxDcMinimumRequest, PhyRxDcMinimumTransition,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcCalibrationStage {
    Radio,
    Baseband,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxDcCalibrationRequest {
    /// Selects the Wi-Fi (`false`) or shared-radio (`true`) calibration bank.
    pub shared_radio: bool,
    pub stage: PhyRxDcCalibrationStage,
    pub control: u16,
    pub initial: [u16; 2],
    pub gain_index: u8,
    pub rx_saturation_detected: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxDcCalibrationOutcome {
    pub request: PhyRxDcCalibrationRequest,
    pub configuration: [u16; 2],
    pub iterations: u8,
    pub converged: bool,
    pub readiness_activity_edges: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcCalibrationFailure {
    PbusForceTimedOut(PhyPbusForceTest),
    Minimum(PhyRxDcMinimumFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcCalibrationAction {
    PrepareControlRestore,
    ReadPbus { selector: u8, path: u8 },
    ForcePbus(PhyPbusForceTest),
    DelayMicros { measurement: u8, micros: u32 },
    Minimum(PhyRxDcMinimumAction),
    RestoreControl,
    Complete(PhyRxDcCalibrationOutcome),
    Failed(PhyRxDcCalibrationFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcCalibrationCompletion {
    ControlRestorePrepared,
    PbusRead { selector: u8, path: u8, value: u32 },
    PbusForceCompleted(PhyPbusForceTest),
    PbusForceTimedOut(PhyPbusForceTest),
    DelayElapsed { measurement: u8, micros: u32 },
    Minimum(PhyRxDcMinimumCompletion),
    ControlRestored,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcCalibrationTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Terminal {
    Complete(PhyRxDcCalibrationOutcome),
    Failed(PhyRxDcCalibrationFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    PrepareControlRestore,
    ReadPbus,
    ForceI,
    ForceQ,
    ForceRadioLevel {
        high: bool,
    },
    Delay {
        high: bool,
    },
    Minimum {
        high: bool,
        transition: PhyRxDcMinimumTransition,
    },
    CleanupI {
        terminal: Terminal,
    },
    CleanupQ {
        terminal: Terminal,
    },
    Restore {
        terminal: Terminal,
    },
    Complete(PhyRxDcCalibrationOutcome),
    Failed(PhyRxDcCalibrationFailure),
}

const fn saturate_9bit(value: i32) -> u16 {
    if value < 0 {
        0
    } else if value > 0x1ff {
        0x1ff
    } else {
        value as u16
    }
}

const fn saturate_signed_5(value: i32) -> i32 {
    if value < -5 {
        -5
    } else if value > 5 {
        5
    } else {
        value
    }
}

/// Exact non-I/O correction arithmetic from `phy_pbus_rx_dco_cal_1step_new`:
/// an observation below the threshold contributes no correction.
pub const fn rx_dc_calibration_correction(delta: i32, threshold: i32, shift: u8) -> i32 {
    if delta.wrapping_abs() >= threshold {
        delta >> shift
    } else {
        0
    }
}

/// Heap-free translation of one complete RX DC-calibration step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxDcCalibrationTransition {
    request: PhyRxDcCalibrationRequest,
    step: Step,
    current: [u16; 2],
    population: u8,
    threshold: i32,
    iteration: u8,
    low: crate::calibration::estimator::PhyDcIqEstimate,
    measurement: crate::calibration::estimator::PhyDcIqEstimate,
    readiness_activity_edges: u16,
}

/// ROM `phy_pbus_rx_dco_cal_1step_new` keeps one output slot for the
/// baseband low search and one for the radio or baseband high search. Both
/// start cleared and keep the last search's estimate across iterations,
/// because `phy_rxdc_est_min` writes a slot only for an admitted estimate.
/// The ROM never initializes the radio slot, so it reads stack contents
/// before the first admitted radio estimate; Rust starts it cleared.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PhyRxDcCalibrationPolicyState {
    current: [u16; 2],
    iteration: u8,
    low: crate::calibration::estimator::PhyDcIqEstimate,
    measurement: crate::calibration::estimator::PhyDcIqEstimate,
    readiness_activity_edges: u16,
}

impl PhyRxDcCalibrationPolicyState {
    #[cfg(any(target_arch = "riscv32", test))]
    const fn new(initial: [u16; 2]) -> Self {
        Self {
            current: initial,
            iteration: 0,
            low: crate::calibration::estimator::PhyDcIqEstimate {
                i: 0,
                q: 0,
                power: 0,
            },
            measurement: crate::calibration::estimator::PhyDcIqEstimate {
                i: 0,
                q: 0,
                power: 0,
            },
            readiness_activity_edges: 0,
        }
    }

    fn accept(
        &mut self,
        request: PhyRxDcCalibrationRequest,
        population: u8,
        threshold: i32,
        high: bool,
        estimate: crate::calibration::estimator::PhyDcIqEstimate,
        readiness_activity_edges: u16,
    ) -> Option<PhyRxDcCalibrationOutcome> {
        self.readiness_activity_edges = self
            .readiness_activity_edges
            .wrapping_add(readiness_activity_edges);
        if request.stage == PhyRxDcCalibrationStage::Baseband && !high {
            self.low = estimate;
            return None;
        }
        self.measurement = estimate;

        let (delta_i, delta_q, power, shift) = match request.stage {
            PhyRxDcCalibrationStage::Radio => (
                estimate.i,
                estimate.q,
                estimate.power,
                population.max(2) - 2,
            ),
            PhyRxDcCalibrationStage::Baseband => (
                estimate.i.wrapping_sub(self.low.i),
                estimate.q.wrapping_sub(self.low.q),
                estimate.power.max(self.low.power),
                if request.shared_radio {
                    SHARED_BASEBAND_SHIFT
                } else {
                    WIFI_BASEBAND_SHIFT
                },
            ),
        };

        let mut correction_i = rx_dc_calibration_correction(delta_i, threshold, shift);
        let mut correction_q = rx_dc_calibration_correction(delta_q, threshold, shift);
        if request.stage == PhyRxDcCalibrationStage::Baseband {
            if power >= 45 {
                correction_i = 0;
                correction_q = 0;
            }
            if !request.shared_radio && request.gain_index > 1 {
                correction_i = saturate_signed_5(correction_i);
                correction_q = saturate_signed_5(correction_q);
            }
        }

        let converged = delta_i.wrapping_abs() <= threshold
            && delta_q.wrapping_abs() <= threshold
            && power < 46;
        // ROM `phy_pbus_rx_dco_cal_1step_new` stores the codes it forces at
        // the start of each iteration into its caller's output and returns
        // that output, so the correction of the final unconverged iteration
        // is computed but never published.
        let last = self.iteration + 1 >= maximum_iterations(request.stage);
        if !converged && !last {
            if delta_i.wrapping_abs() > threshold {
                self.current[0] =
                    saturate_9bit(i32::from(self.current[0]).wrapping_sub(correction_i));
            }
            if delta_q.wrapping_abs() > threshold {
                self.current[1] =
                    saturate_9bit(i32::from(self.current[1]).wrapping_sub(correction_q));
            }
        }

        self.iteration += 1;
        if !converged && self.iteration < maximum_iterations(request.stage) {
            return None;
        }
        let configuration = if !converged && request.stage == PhyRxDcCalibrationStage::Baseband {
            request.initial
        } else {
            self.current
        };
        Some(PhyRxDcCalibrationOutcome {
            request,
            configuration,
            iterations: self.iteration,
            converged,
            readiness_activity_edges: self.readiness_activity_edges,
        })
    }
}

/// Right shifts applied to baseband observations of each bank.
const SHARED_BASEBAND_SHIFT: u8 = 5;
const WIFI_BASEBAND_SHIFT: u8 = 2;
/// Baseband observation thresholds of each bank.
const SHARED_BASEBAND_THRESHOLD: i32 = 25;
const WIFI_BASEBAND_THRESHOLD: i32 = 3;

/// A radio-stage threshold grows with the population of the six low control
/// bits read before the search: `2 * population + 1`.
const fn calibration_threshold(request: PhyRxDcCalibrationRequest, population: u8) -> i32 {
    match request.stage {
        PhyRxDcCalibrationStage::Radio => 2 * population as i32 + 1,
        PhyRxDcCalibrationStage::Baseband if request.shared_radio => SHARED_BASEBAND_THRESHOLD,
        PhyRxDcCalibrationStage::Baseband => WIFI_BASEBAND_THRESHOLD,
    }
}

const fn calibration_path(stage: PhyRxDcCalibrationStage) -> u8 {
    match stage {
        PhyRxDcCalibrationStage::Radio => 2,
        PhyRxDcCalibrationStage::Baseband => 1,
    }
}

const fn maximum_iterations(stage: PhyRxDcCalibrationStage) -> u8 {
    match stage {
        PhyRxDcCalibrationStage::Radio => 8,
        PhyRxDcCalibrationStage::Baseband => 12,
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub(crate) struct PhyRxDcCalibrationTargetStats {
    pub minimum_searches: u32,
    pub minimum_operations: u32,
    pub settle_1us: u32,
    pub settle_10us: u32,
}

#[cfg(target_arch = "riscv32")]
fn execute_minimum_target<D: crate::executor::wait::PhyShortDelay>(
    request: PhyRxDcMinimumRequest,
    registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    budget: &mut crate::target_executor::DirectOperationBudget,
    stats: &mut PhyRxDcCalibrationTargetStats,
) -> Result<
    Result<PhyRxDcMinimumOutcome, PhyRxDcMinimumFailure>,
    crate::target_executor::PhyTargetPortError,
> {
    let completion = crate::rx::dc_offset::PhyRxDcMinimumTargetTransaction::new(request)
        .execute_target::<D>(budget.remaining(), registers)?
        .ok_or(crate::target_executor::PhyTargetPortError::RfOperationLimit)?;
    if !budget.consume(completion.operations()) {
        return Err(crate::target_executor::PhyTargetPortError::RfOperationLimit);
    }
    stats.minimum_searches += 1;
    stats.minimum_operations += completion.operations();
    stats.settle_1us += 2 * u32::from(completion.estimators());
    Ok(completion.into_terminal())
}

impl PhyRxDcCalibrationTransition {
    /// Execute the complete current-vendor one-step RX-DC routine as one
    /// blocking target transaction. The event-driven transition remains the
    /// executable host model; production does not dispatch its individual
    /// actions through an async executor.
    #[cfg(target_arch = "riscv32")]
    #[inline(never)]
    pub(crate) fn execute_target_direct<D: crate::executor::wait::PhyShortDelay>(
        &mut self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyContext,
        budget: &mut crate::target_executor::DirectOperationBudget,
    ) -> Result<PhyRxDcCalibrationTargetStats, crate::target_executor::PhyTargetPortError> {
        use crate::target_executor::{PhyTargetPortError, force_pbus_direct};

        if !matches!(self.step, Step::PrepareControlRestore) {
            return Err(PhyTargetPortError::UnexpectedBinding);
        }
        oer_esp32s31_hal::phy::rx_dco::prepare_control_restore(registers)
            .map_err(|_| PhyTargetPortError::HardwareInvariant)?;

        let population = oer_esp32s31_hal::phy::pbus::read_result(registers, 1, 2)
            .ok_or(PhyTargetPortError::HardwareInvariant)? as u8
            & 0x3f;
        let population = population.count_ones() as u8;
        let threshold = calibration_threshold(self.request, population);
        let path = calibration_path(self.request.stage);
        let initial = self.request.initial;
        let mut policy = PhyRxDcCalibrationPolicyState::new(initial);
        let mut stats = PhyRxDcCalibrationTargetStats::default();

        let terminal = loop {
            let force_i = PhyPbusForceTest::new(2, path, policy.current[0]);
            if !force_pbus_direct(registers, force_i) {
                break Terminal::Failed(PhyRxDcCalibrationFailure::PbusForceTimedOut(force_i));
            }
            let force_q = PhyPbusForceTest::new(3, path, policy.current[1]);
            if !force_pbus_direct(registers, force_q) {
                break Terminal::Failed(PhyRxDcCalibrationFailure::PbusForceTimedOut(force_q));
            }

            let low = if self.request.stage == PhyRxDcCalibrationStage::Baseband {
                let level = PhyPbusForceTest::new(1, 2, 0);
                if !force_pbus_direct(registers, level) {
                    break Terminal::Failed(PhyRxDcCalibrationFailure::PbusForceTimedOut(level));
                }
                if !D::settle_micros(10) {
                    return Err(PhyTargetPortError::HardwareCapabilityUnavailable);
                }
                stats.settle_10us += 1;
                match execute_minimum_target::<D>(
                    Self::minimum_request_at(self.request, policy.iteration, false, policy.low),
                    registers,
                    budget,
                    &mut stats,
                )? {
                    Ok(outcome) => Some((outcome.estimate, outcome.readiness_activity_edges)),
                    Err(failure) => {
                        break Terminal::Failed(PhyRxDcCalibrationFailure::Minimum(failure));
                    }
                }
            } else {
                None
            };

            let measurement = if self.request.stage == PhyRxDcCalibrationStage::Baseband {
                let level = PhyPbusForceTest::new(1, 2, 0x20);
                if !force_pbus_direct(registers, level) {
                    break Terminal::Failed(PhyRxDcCalibrationFailure::PbusForceTimedOut(level));
                }
                if !D::settle_micros(10) {
                    return Err(PhyTargetPortError::HardwareCapabilityUnavailable);
                }
                stats.settle_10us += 1;
                match execute_minimum_target::<D>(
                    Self::minimum_request_at(
                        self.request,
                        policy.iteration,
                        true,
                        policy.measurement,
                    ),
                    registers,
                    budget,
                    &mut stats,
                )? {
                    Ok(outcome) => outcome,
                    Err(failure) => {
                        break Terminal::Failed(PhyRxDcCalibrationFailure::Minimum(failure));
                    }
                }
            } else {
                if !D::settle_micros(10) {
                    return Err(PhyTargetPortError::HardwareCapabilityUnavailable);
                }
                stats.settle_10us += 1;
                match execute_minimum_target::<D>(
                    Self::minimum_request_at(
                        self.request,
                        policy.iteration,
                        false,
                        policy.measurement,
                    ),
                    registers,
                    budget,
                    &mut stats,
                )? {
                    Ok(outcome) => outcome,
                    Err(failure) => {
                        break Terminal::Failed(PhyRxDcCalibrationFailure::Minimum(failure));
                    }
                }
            };

            if let Some((low, low_activity_edges)) = low {
                let _ = policy.accept(
                    self.request,
                    population,
                    threshold,
                    false,
                    low,
                    low_activity_edges,
                );
            }
            let high = self.request.stage == PhyRxDcCalibrationStage::Baseband;
            if let Some(outcome) = policy.accept(
                self.request,
                population,
                threshold,
                high,
                measurement.estimate,
                measurement.readiness_activity_edges,
            ) {
                break Terminal::Complete(outcome);
            }
        };

        let cleanup = match terminal {
            Terminal::Complete(outcome) => outcome.configuration,
            Terminal::Failed(_) => initial,
        };
        let _ = force_pbus_direct(registers, PhyPbusForceTest::new(2, path, cleanup[0]));
        let _ = force_pbus_direct(registers, PhyPbusForceTest::new(3, path, cleanup[1]));
        oer_esp32s31_hal::phy::rx_dco::restore_control(registers)
            .map_err(|_| PhyTargetPortError::HardwareInvariant)?;

        self.step = match terminal {
            Terminal::Complete(outcome) => Step::Complete(outcome),
            Terminal::Failed(failure) => Step::Failed(failure),
        };
        Ok(stats)
    }

    #[cfg(test)]
    pub(crate) fn minimum_mut(&mut self) -> Option<&mut PhyRxDcMinimumTransition> {
        match &mut self.step {
            Step::Minimum { transition, .. } => Some(transition),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn finish_minimum(&mut self) -> Result<(), PhyRxDcCalibrationTransitionError> {
        let Step::Minimum {
            high,
            ref transition,
        } = self.step
        else {
            return Err(PhyRxDcCalibrationTransitionError::WrongCompletion);
        };
        match transition.terminal() {
            Some(Ok(outcome)) => self.accept_measurement(high, outcome),
            Some(Err(failure)) => self.fail(PhyRxDcCalibrationFailure::Minimum(failure)),
            None => return Err(PhyRxDcCalibrationTransitionError::WrongCompletion),
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn lower_external(
        &self,
    ) -> Result<PhyRxDcCalibrationExternalBinding, PhyRxGainCalibrationBindingError> {
        if let Step::Minimum { ref transition, .. } = self.step {
            return crate::rx::dc_offset::PhyRxDcMinimumExternalBinding::lower(transition.action())
                .map(PhyRxDcCalibrationExternalBinding::Minimum)
                .map_err(|_| PhyRxGainCalibrationBindingError::UnsupportedAction);
        }
        PhyRxDcCalibrationExternalBinding::lower(self.action())
    }

    /// Inspect completion without constructing a nested hardware action.
    pub(crate) fn terminal(
        &self,
    ) -> Option<Result<PhyRxDcCalibrationOutcome, PhyRxDcCalibrationFailure>> {
        match self.step {
            Step::Complete(outcome) => Some(Ok(outcome)),
            Step::Failed(failure) => Some(Err(failure)),
            _ => None,
        }
    }

    pub const fn new(request: PhyRxDcCalibrationRequest) -> Self {
        Self {
            request,
            step: Step::PrepareControlRestore,
            current: request.initial,
            population: 0,
            threshold: 0,
            iteration: 0,
            low: crate::calibration::estimator::PhyDcIqEstimate {
                i: 0,
                q: 0,
                power: 0,
            },
            measurement: crate::calibration::estimator::PhyDcIqEstimate {
                i: 0,
                q: 0,
                power: 0,
            },
            readiness_activity_edges: 0,
        }
    }

    const fn path(&self) -> u8 {
        calibration_path(self.request.stage)
    }

    const fn measurement_identity(&self, high: bool) -> u8 {
        self.iteration.wrapping_mul(2).wrapping_add(high as u8)
    }

    const fn minimum_request(&self, high: bool) -> PhyRxDcMinimumRequest {
        let previous = if matches!(self.request.stage, PhyRxDcCalibrationStage::Baseband) && !high {
            self.low
        } else {
            self.measurement
        };
        Self::minimum_request_at(self.request, self.iteration, high, previous)
    }

    const fn minimum_request_at(
        request: PhyRxDcCalibrationRequest,
        iteration: u8,
        high: bool,
        previous: crate::calibration::estimator::PhyDcIqEstimate,
    ) -> PhyRxDcMinimumRequest {
        PhyRxDcMinimumRequest {
            measurement: iteration.wrapping_mul(2).wrapping_add(high as u8),
            control: request.control,
            // The fourth `phy_rxdc_est_min` argument is one for both baseband
            // searches and zero for the radio search. ROM forwards it to
            // `phy_dc_iq_est`; its second argument is unused.
            mode: match request.stage {
                PhyRxDcCalibrationStage::Baseband => 1,
                PhyRxDcCalibrationStage::Radio => 0,
            },
            rx_saturation_detected: request.rx_saturation_detected,
            previous,
        }
    }

    const fn current_force(&self, selector: u8) -> PhyPbusForceTest {
        let value = if selector == 2 {
            self.current[0]
        } else {
            self.current[1]
        };
        PhyPbusForceTest::new(selector, self.path(), value)
    }

    const fn cleanup_configuration(&self, terminal: Terminal) -> [u16; 2] {
        match terminal {
            Terminal::Complete(outcome) => outcome.configuration,
            Terminal::Failed(_) => self.request.initial,
        }
    }

    pub const fn action(&self) -> PhyRxDcCalibrationAction {
        match self.step {
            Step::PrepareControlRestore => PhyRxDcCalibrationAction::PrepareControlRestore,
            Step::ReadPbus => PhyRxDcCalibrationAction::ReadPbus {
                selector: 1,
                path: 2,
            },
            Step::ForceI => PhyRxDcCalibrationAction::ForcePbus(self.current_force(2)),
            Step::ForceQ => PhyRxDcCalibrationAction::ForcePbus(self.current_force(3)),
            Step::ForceRadioLevel { high, .. } => PhyRxDcCalibrationAction::ForcePbus(
                PhyPbusForceTest::new(1, 2, if high { 0x20 } else { 0 }),
            ),
            Step::Delay { high, .. } => PhyRxDcCalibrationAction::DelayMicros {
                measurement: self.measurement_identity(high),
                micros: 10,
            },
            Step::Minimum { ref transition, .. } => {
                PhyRxDcCalibrationAction::Minimum(transition.action())
            }
            Step::CleanupI { terminal, .. } => PhyRxDcCalibrationAction::ForcePbus(
                PhyPbusForceTest::new(2, self.path(), self.cleanup_configuration(terminal)[0]),
            ),
            Step::CleanupQ { terminal, .. } => PhyRxDcCalibrationAction::ForcePbus(
                PhyPbusForceTest::new(3, self.path(), self.cleanup_configuration(terminal)[1]),
            ),
            Step::Restore { .. } => PhyRxDcCalibrationAction::RestoreControl,
            Step::Complete(outcome) => PhyRxDcCalibrationAction::Complete(outcome),
            Step::Failed(failure) => PhyRxDcCalibrationAction::Failed(failure),
        }
    }

    fn begin_cleanup(&mut self, terminal: Terminal) {
        self.step = Step::CleanupI { terminal };
    }

    fn fail(&mut self, failure: PhyRxDcCalibrationFailure) {
        self.begin_cleanup(Terminal::Failed(failure));
    }

    fn accept_measurement(&mut self, high: bool, outcome: PhyRxDcMinimumOutcome) {
        let mut policy = PhyRxDcCalibrationPolicyState {
            current: self.current,
            iteration: self.iteration,
            low: self.low,
            measurement: self.measurement,
            readiness_activity_edges: self.readiness_activity_edges,
        };
        let terminal = policy.accept(
            self.request,
            self.population,
            self.threshold,
            high,
            outcome.estimate,
            outcome.readiness_activity_edges,
        );
        self.current = policy.current;
        self.iteration = policy.iteration;
        self.low = policy.low;
        self.measurement = policy.measurement;
        self.readiness_activity_edges = policy.readiness_activity_edges;
        if self.request.stage == PhyRxDcCalibrationStage::Baseband && !high {
            self.step = Step::ForceRadioLevel { high: true };
        } else if let Some(outcome) = terminal {
            self.begin_cleanup(Terminal::Complete(outcome));
        } else {
            self.step = Step::ForceI;
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyRxDcCalibrationCompletion,
    ) -> Result<(), PhyRxDcCalibrationTransitionError> {
        // Child rejection is transactional. Keep its cursor in the parent instead
        // of copying the nested transition for every hardware completion.
        if let (
            &mut Step::Minimum {
                high,
                ref mut transition,
            },
            PhyRxDcCalibrationCompletion::Minimum(completion),
        ) = (&mut self.step, completion)
        {
            transition
                .advance(completion)
                .map_err(|_| PhyRxDcCalibrationTransitionError::WrongCompletion)?;
            match transition.terminal() {
                Some(Ok(outcome)) => self.accept_measurement(high, outcome),
                Some(Err(failure)) => self.fail(PhyRxDcCalibrationFailure::Minimum(failure)),
                None => {}
            }
            return Ok(());
        }
        match (self.step, completion) {
            (Step::PrepareControlRestore, PhyRxDcCalibrationCompletion::ControlRestorePrepared) => {
                self.step = Step::ReadPbus;
            }
            (
                Step::ReadPbus,
                PhyRxDcCalibrationCompletion::PbusRead {
                    selector: 1,
                    path: 2,
                    value,
                },
            ) => {
                self.population = (value as u8 & 0x3f).count_ones() as u8;
                self.threshold = calibration_threshold(self.request, self.population);
                self.step = Step::ForceI;
            }
            (Step::ForceI, PhyRxDcCalibrationCompletion::PbusForceCompleted(transaction))
                if transaction == self.current_force(2) =>
            {
                self.step = Step::ForceQ;
            }
            (Step::ForceQ, PhyRxDcCalibrationCompletion::PbusForceCompleted(transaction))
                if transaction == self.current_force(3) =>
            {
                self.step = match self.request.stage {
                    PhyRxDcCalibrationStage::Radio => Step::Delay { high: false },
                    PhyRxDcCalibrationStage::Baseband => Step::ForceRadioLevel { high: false },
                };
            }
            (
                Step::ForceRadioLevel { high },
                PhyRxDcCalibrationCompletion::PbusForceCompleted(transaction),
            ) if transaction == PhyPbusForceTest::new(1, 2, if high { 0x20 } else { 0 }) => {
                self.step = Step::Delay { high };
            }
            (
                Step::Delay { high },
                PhyRxDcCalibrationCompletion::DelayElapsed {
                    measurement,
                    micros: 10,
                },
            ) if measurement == self.measurement_identity(high) => {
                self.step = Step::Minimum {
                    high,
                    transition: PhyRxDcMinimumTransition::new(self.minimum_request(high)),
                };
            }

            (
                Step::ForceI | Step::ForceQ | Step::ForceRadioLevel { .. },
                PhyRxDcCalibrationCompletion::PbusForceTimedOut(transaction),
            ) => self.fail(PhyRxDcCalibrationFailure::PbusForceTimedOut(transaction)),
            (
                Step::CleanupI { terminal },
                PhyRxDcCalibrationCompletion::PbusForceCompleted(transaction)
                | PhyRxDcCalibrationCompletion::PbusForceTimedOut(transaction),
            ) if transaction
                == PhyPbusForceTest::new(
                    2,
                    self.path(),
                    self.cleanup_configuration(terminal)[0],
                ) =>
            {
                self.step = Step::CleanupQ { terminal };
            }
            (
                Step::CleanupQ { terminal },
                PhyRxDcCalibrationCompletion::PbusForceCompleted(transaction)
                | PhyRxDcCalibrationCompletion::PbusForceTimedOut(transaction),
            ) if transaction
                == PhyPbusForceTest::new(
                    3,
                    self.path(),
                    self.cleanup_configuration(terminal)[1],
                ) =>
            {
                self.step = Step::Restore { terminal };
            }
            (Step::Restore { terminal }, PhyRxDcCalibrationCompletion::ControlRestored) => {
                self.step = match terminal {
                    Terminal::Complete(outcome) => Step::Complete(outcome),
                    Terminal::Failed(failure) => Step::Failed(failure),
                };
            }
            (Step::Complete(_), _) | (Step::Failed(_), _) => {
                return Err(PhyRxDcCalibrationTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyRxDcCalibrationTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

// Exact halfword tables at `.LANCHOR0 + 0x08` and `.LANCHOR0 + 0x18`
// in vendor `phy_rx_cal.o`.  The final shared entries select the extended
// 0x027f and 0x017f gain encodings; truncating them to 0x007f skips four
// calibration points later consumed by the RX gain-memory setup.
pub(crate) const WIFI_CALIBRATION_GAIN: [u16; 8] = [0x40, 0x41, 0x43, 0x6e, 0x78, 0x79, 0x7b, 0x7f];
pub(crate) const SHARED_CALIBRATION_GAIN: [u16; 11] = [
    0x40, 0x41, 0x42, 0x43, 0x6e, 0x78, 0x79, 0x7b, 0x027f, 0x017f, 0x007f,
];
const RX_ON_COUNT: u8 = 7;
const RX_OFF_COUNT: u8 = 3;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainDcBank {
    Wifi,
    Shared,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxGainDcParameters {
    pub crystal_selector: u8,
    pub pbus_rx_path_value: u8,
    pub rx_saturation_detected: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxGainDcOutcome {
    /// Per-search convergence, distinct from successful transaction completion.
    pub quality: PhyRxGainDcQuality,
    pub wifi_index_dc: [[u16; 2]; 8],
    /// Five `phy_rxdc_fine_cal` radio DC pairs beginning at
    /// `phy_param[0x16e]`, one per [`fine_code`]. The first is the radio DC
    /// base of every later Wi-Fi gain; RX gain memory selects one per entry.
    pub wifi_fine_dc: [[u16; 2]; FINE_CODES],
    /// Eleven calibrated entries beginning at `phy_param[0x1b4]`.
    pub shared_index_dc: [[u16; 2]; 11],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainDcFailure {
    Rfpll(RfpllFrequencyFailure),
    Pbus {
        bank: PhyRxGainDcBank,
        transaction: PhyPbusForceTest,
    },
    Calibration(PhyRxDcCalibrationFailure),
    Minimum(PhyRxDcMinimumFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainDcClock {
    Rx,
    Tx,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainDcDelayPhase {
    PbusWorkMode,
    PbusWorkModePulse,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainDcAction {
    ConfigureRegisters {
        enabled: bool,
    },
    Rfpll(RfpllFrequencyAction),
    ConfigurePbusDebugMode,
    ForcePbus {
        bank: PhyRxGainDcBank,
        transaction: PhyPbusForceTest,
    },
    ConfigureClock {
        clock: PhyRxGainDcClock,
        enabled: bool,
    },
    ReadPbus {
        selector: u8,
        path: u8,
    },
    I2c(oer_radio_analog::Action<crate::analog::i2c::PhyI2cAddress, ()>),
    Calibration(PhyRxDcCalibrationAction),
    ConfigurePbusWorkMode,
    DelayMicros {
        phase: PhyRxGainDcDelayPhase,
        micros: u32,
    },
    ConfigurePbusWorkModePulse,
    ClearPbusWorkModePulse,
    Complete,
    Failed(PhyRxGainDcFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainDcCompletion {
    RegistersConfigured {
        enabled: bool,
    },
    Rfpll(RfpllFrequencyCompletion),
    PbusDebugModeConfigured,
    PbusCompleted {
        bank: PhyRxGainDcBank,
        transaction: PhyPbusForceTest,
    },
    PbusTimedOut {
        bank: PhyRxGainDcBank,
        transaction: PhyPbusForceTest,
    },
    ClockConfigured {
        clock: PhyRxGainDcClock,
        enabled: bool,
    },
    PbusRead {
        selector: u8,
        path: u8,
        value: u32,
    },
    I2c(oer_radio_analog::Completion<crate::analog::i2c::PhyI2cAddress>),
    Calibration(PhyRxDcCalibrationCompletion),
    PbusWorkModeConfigured {
        settle_required: bool,
    },
    DelayElapsed {
        phase: PhyRxGainDcDelayPhase,
        micros: u32,
    },
    PbusWorkModePulseConfigured,
    PbusWorkModePulseCleared,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainDcTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DcTerminal {
    ContinueWifi,
    Complete,
    Failed(PhyRxGainDcFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DcStep {
    ConfigureRegisters,
    Rfpll(RfpllFrequencyTransition),
    Debug,
    WifiConfigureRegisters,
    WifiRfpll(RfpllFrequencyTransition),
    WifiDebug,
    RxOn {
        index: u8,
    },
    Clock {
        clock: PhyRxGainDcClock,
        enabled: bool,
    },
    SharedReadPbus,
    SharedForcePbus {
        value: u16,
    },
    SharedI2c(oer_radio_analog::FieldWriteTransition<crate::analog::i2c::PhyI2cAddress>),
    FineCode {
        index: u8,
    },
    FineCalibration {
        index: u8,
        transition: PhyRxDcCalibrationTransition,
    },
    SetupRadioI {
        bank: PhyRxGainDcBank,
        index: u8,
    },
    SetupRadioQ {
        bank: PhyRxGainDcBank,
        index: u8,
    },
    SetupBasebandI {
        bank: PhyRxGainDcBank,
        index: u8,
        value: [u16; 2],
    },
    SetupBasebandQ {
        bank: PhyRxGainDcBank,
        index: u8,
        value: [u16; 2],
    },
    SetGain {
        bank: PhyRxGainDcBank,
        index: u8,
        value: [u16; 2],
        transaction: u8,
    },
    SetSharedMixerDgain {
        index: u8,
    },
    CalibrateBaseband {
        bank: PhyRxGainDcBank,
        index: u8,
        transition: PhyRxDcCalibrationTransition,
    },
    SharedRestoreI2c(oer_radio_analog::FieldWriteTransition<crate::analog::i2c::PhyI2cAddress>),
    WifiRxOn {
        index: u8,
    },
    WifiClock {
        clock: PhyRxGainDcClock,
    },
    ClockOff(PhyRxGainDcClock, DcTerminal),
    RxOff {
        index: u8,
        terminal: DcTerminal,
    },
    WorkMode(DcTerminal),
    WorkModeDelay(DcTerminal),
    WorkModePulse(DcTerminal),
    WorkModePulseDelay(DcTerminal),
    WorkModePulseClear(DcTerminal),
    ClearRegisters(DcTerminal),
    Complete,
    Failed(PhyRxGainDcFailure),
}

pub(crate) const fn rx_on(index: u8, pbus_rx_path_value: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(4, 1, 0),
        1 => PhyPbusForceTest::new(4, 2, 1),
        2 => PhyPbusForceTest::new(5, 1, 0),
        3 => PhyPbusForceTest::new(0, 1, 0x40),
        4 => PhyPbusForceTest::new(0, 2, pbus_rx_path_value as u16),
        5 => PhyPbusForceTest::new(1, 1, 0x189),
        _ => PhyPbusForceTest::new(1, 2, 0),
    }
}

pub(crate) const fn rx_off(index: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(0, 1, 0),
        1 => PhyPbusForceTest::new(1, 1, 0),
        _ => PhyPbusForceTest::new(1, 2, 0),
    }
}

/// Radio control codes `phy_rxdc_fine_cal` forces before each of its searches.
const FINE_CODE_TABLE: [u16; FINE_CODES] = [0x00, 0x20, 0x30, 0x38, 0x3c];
/// Searches of `phy_rxdc_fine_cal`.
pub const FINE_CODES: usize = 5;

pub(crate) const fn fine_code(index: u8) -> u16 {
    FINE_CODE_TABLE[index as usize]
}

const fn bank_count(bank: PhyRxGainDcBank) -> u8 {
    match bank {
        PhyRxGainDcBank::Wifi => WIFI_CALIBRATION_GAIN.len() as u8,
        PhyRxGainDcBank::Shared => SHARED_CALIBRATION_GAIN.len() as u8,
    }
}

const fn gain(bank: PhyRxGainDcBank, index: u8) -> u16 {
    match bank {
        PhyRxGainDcBank::Wifi => WIFI_CALIBRATION_GAIN[index as usize],
        PhyRxGainDcBank::Shared => SHARED_CALIBRATION_GAIN[index as usize],
    }
}

/// Expand complete ROM `phy_pbus_set_rxgain`, size 92, into its three
/// independently completed PBus commands. The final command's former
/// `phy_param[0x002]` dependency is an explicit Rust-owned input.
pub(crate) const fn set_rx_gain_transaction(
    encoded_gain: u32,
    pbus_rx_path_value: u8,
    transaction: u8,
) -> PhyPbusForceTest {
    match transaction {
        0 => PhyPbusForceTest::new(
            1,
            2,
            (((encoded_gain << 6) & 0x1c0) | ((encoded_gain >> 4) & 0x3f)) as u16,
        ),
        1 => PhyPbusForceTest::new(
            0,
            1,
            (((encoded_gain >> 12) & 0x38) | ((encoded_gain >> 12) & 7) | 0x40) as u16,
        ),
        _ => PhyPbusForceTest::new(0, 2, pbus_rx_path_value as u16),
    }
}

/// Exact stack-local lookup constructed by vendor `phy_bt_rx_mx_dgain`.
///
/// Indices 0..=8 select zero, index 9 selects four, and index 10 (plus the
/// out-of-range fallback) selects seven.
const fn shared_mixer_dgain(index: u8) -> u8 {
    if index >= 10 {
        7
    } else if index == 9 {
        4
    } else {
        0
    }
}

pub(crate) const fn shared_mixer_dgain_transaction(
    index: u8,
    pbus_rx_path_value: u8,
) -> PhyPbusForceTest {
    PhyPbusForceTest::new(
        0,
        2,
        ((pbus_rx_path_value & 0xf8) | shared_mixer_dgain(index)) as u16,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxGainDcTransition {
    quality: PhyRxGainDcQuality,
    parameters: PhyRxGainDcParameters,
    step: DcStep,
    wifi_index_dc: [[u16; 2]; 8],
    wifi_fine_dc: [[u16; 2]; FINE_CODES],
    shared_index_dc: [[u16; 2]; 11],
    /// Pair carried between the fine searches: vendor reuses one buffer.
    fine_current: [u16; 2],
}

impl PhyRxGainDcTransition {
    pub(crate) fn finish_calibration(&mut self) -> Result<(), PhyRxGainDcTransitionError> {
        enum Finished {
            Fine(
                u8,
                Result<PhyRxDcCalibrationOutcome, PhyRxDcCalibrationFailure>,
            ),
            Baseband(
                PhyRxGainDcBank,
                u8,
                Result<PhyRxDcCalibrationOutcome, PhyRxDcCalibrationFailure>,
            ),
        }

        let finished = match &self.step {
            DcStep::FineCalibration { index, transition } => Finished::Fine(
                *index,
                transition
                    .terminal()
                    .ok_or(PhyRxGainDcTransitionError::WrongCompletion)?,
            ),
            DcStep::CalibrateBaseband {
                bank,
                index,
                transition,
            } => Finished::Baseband(
                *bank,
                *index,
                transition
                    .terminal()
                    .ok_or(PhyRxGainDcTransitionError::WrongCompletion)?,
            ),
            _ => return Err(PhyRxGainDcTransitionError::WrongCompletion),
        };

        match finished {
            Finished::Fine(index, Ok(outcome)) => {
                self.quality.record_wifi_fine(index, outcome.converged);
                self.fine_current = outcome.configuration;
                self.wifi_fine_dc[index as usize] = outcome.configuration;
                if usize::from(index) + 1 == FINE_CODES {
                    self.next_gain(PhyRxGainDcBank::Wifi, 0);
                } else {
                    self.step = DcStep::FineCode { index: index + 1 };
                }
            }
            Finished::Baseband(bank, index, Ok(outcome)) => {
                self.store_calibration(bank, index, outcome);
            }
            Finished::Fine(_, Err(failure)) | Finished::Baseband(_, _, Err(failure)) => {
                self.fail(PhyRxGainDcFailure::Calibration(failure));
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn minimum_mut(&mut self) -> Option<&mut PhyRxDcMinimumTransition> {
        match &mut self.step {
            DcStep::FineCalibration { transition, .. }
            | DcStep::CalibrateBaseband { transition, .. } => transition.minimum_mut(),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn finish_minimum(&mut self) -> Result<(), PhyRxGainDcTransitionError> {
        match &mut self.step {
            DcStep::FineCalibration { transition, .. }
            | DcStep::CalibrateBaseband { transition, .. } => transition
                .finish_minimum()
                .map_err(|_| PhyRxGainDcTransitionError::WrongCompletion),
            _ => Err(PhyRxGainDcTransitionError::WrongCompletion),
        }
    }

    #[cfg(test)]
    pub(crate) fn lower_external(
        &self,
    ) -> Result<PhyRxGainDcExternalBinding, PhyRxGainCalibrationBindingError> {
        match self.step {
            DcStep::FineCalibration { ref transition, .. }
            | DcStep::CalibrateBaseband { ref transition, .. } => transition
                .lower_external()
                .map(PhyRxGainDcExternalBinding::Calibration),
            _ => PhyRxGainDcExternalBinding::lower(self.action()),
        }
    }

    /// Inspect completion without constructing a nested hardware action.
    pub(crate) fn terminal(&self) -> Option<Result<PhyRxGainDcOutcome, PhyRxGainDcFailure>> {
        match self.step {
            DcStep::Complete => Some(Ok(self.outcome())),
            DcStep::Failed(failure) => Some(Err(failure)),
            _ => None,
        }
    }

    pub const fn new(parameters: PhyRxGainDcParameters) -> Self {
        Self {
            parameters,
            step: DcStep::ConfigureRegisters,
            quality: PhyRxGainDcQuality::EMPTY,
            wifi_index_dc: [[0; 2]; 8],
            wifi_fine_dc: [[0; 2]; FINE_CODES],
            shared_index_dc: [[0; 2]; 11],
            fine_current: [0x100; 2],
        }
    }

    const fn outcome(&self) -> PhyRxGainDcOutcome {
        PhyRxGainDcOutcome {
            quality: self.quality,
            wifi_index_dc: self.wifi_index_dc,
            wifi_fine_dc: self.wifi_fine_dc,
            shared_index_dc: self.shared_index_dc,
        }
    }

    const fn previous(&self, bank: PhyRxGainDcBank, index: u8) -> [u16; 2] {
        if index == 0 {
            [0x100; 2]
        } else {
            // Vendor keeps a2 fixed at the beginning of the output bank and
            // reloads that first pair for every later gain. Its advancing s5
            // pointer is output-only; this is not a previous-index chain.
            match bank {
                PhyRxGainDcBank::Wifi => self.wifi_index_dc[0],
                PhyRxGainDcBank::Shared => self.shared_index_dc[0],
            }
        }
    }

    const fn baseband(&self, bank: PhyRxGainDcBank, index: u8) -> [u16; 2] {
        match bank {
            PhyRxGainDcBank::Wifi if index != 0 => self.wifi_fine_dc[0],
            _ => [0x100; 2],
        }
    }

    pub const fn action(&self) -> PhyRxGainDcAction {
        match self.step {
            DcStep::ConfigureRegisters | DcStep::WifiConfigureRegisters => {
                PhyRxGainDcAction::ConfigureRegisters { enabled: true }
            }
            DcStep::Rfpll(ref transition) | DcStep::WifiRfpll(ref transition) => {
                PhyRxGainDcAction::Rfpll(transition.action())
            }
            DcStep::Debug | DcStep::WifiDebug => PhyRxGainDcAction::ConfigurePbusDebugMode,
            DcStep::RxOn { index } => PhyRxGainDcAction::ForcePbus {
                bank: PhyRxGainDcBank::Shared,
                transaction: rx_on(index, self.parameters.pbus_rx_path_value),
            },
            DcStep::Clock { clock, enabled } => {
                PhyRxGainDcAction::ConfigureClock { clock, enabled }
            }
            DcStep::ClockOff(clock, _) => PhyRxGainDcAction::ConfigureClock {
                clock,
                enabled: false,
            },
            DcStep::SharedReadPbus => PhyRxGainDcAction::ReadPbus {
                selector: 1,
                path: 1,
            },
            DcStep::SharedForcePbus { value } => PhyRxGainDcAction::ForcePbus {
                bank: PhyRxGainDcBank::Shared,
                transaction: PhyPbusForceTest::new(1, 1, value),
            },
            DcStep::SharedI2c(ref transition) | DcStep::SharedRestoreI2c(ref transition) => {
                PhyRxGainDcAction::I2c(transition.action())
            }
            DcStep::WifiRxOn { index } => PhyRxGainDcAction::ForcePbus {
                bank: PhyRxGainDcBank::Wifi,
                transaction: rx_on(index, self.parameters.pbus_rx_path_value),
            },
            DcStep::WifiClock { clock } => PhyRxGainDcAction::ConfigureClock {
                clock,
                enabled: true,
            },
            DcStep::FineCode { index } => PhyRxGainDcAction::ForcePbus {
                bank: PhyRxGainDcBank::Wifi,
                transaction: PhyPbusForceTest::new(1, 2, fine_code(index)),
            },
            DcStep::FineCalibration { ref transition, .. } => {
                PhyRxGainDcAction::Calibration(transition.action())
            }
            DcStep::SetupRadioI { bank, .. } => PhyRxGainDcAction::ForcePbus {
                bank,
                transaction: PhyPbusForceTest::new(2, 1, 0x100),
            },
            DcStep::SetupRadioQ { bank, .. } => PhyRxGainDcAction::ForcePbus {
                bank,
                transaction: PhyPbusForceTest::new(3, 1, 0x100),
            },
            DcStep::SetupBasebandI { bank, value, .. } => PhyRxGainDcAction::ForcePbus {
                bank,
                transaction: PhyPbusForceTest::new(2, 2, value[0]),
            },
            DcStep::SetupBasebandQ { bank, value, .. } => PhyRxGainDcAction::ForcePbus {
                bank,
                transaction: PhyPbusForceTest::new(3, 2, value[1]),
            },
            DcStep::SetGain {
                bank,
                index,
                transaction,
                ..
            } => PhyRxGainDcAction::ForcePbus {
                bank,
                transaction: set_rx_gain_transaction(
                    (gain(bank, index) as u32) << 12,
                    self.parameters.pbus_rx_path_value,
                    transaction,
                ),
            },
            DcStep::SetSharedMixerDgain { index } => PhyRxGainDcAction::ForcePbus {
                bank: PhyRxGainDcBank::Shared,
                transaction: shared_mixer_dgain_transaction(
                    index,
                    self.parameters.pbus_rx_path_value,
                ),
            },
            DcStep::CalibrateBaseband { ref transition, .. } => {
                PhyRxGainDcAction::Calibration(transition.action())
            }
            DcStep::RxOff { index, .. } => PhyRxGainDcAction::ForcePbus {
                bank: PhyRxGainDcBank::Shared,
                transaction: rx_off(index),
            },
            DcStep::WorkMode(_) => PhyRxGainDcAction::ConfigurePbusWorkMode,
            DcStep::WorkModeDelay(_) => PhyRxGainDcAction::DelayMicros {
                phase: PhyRxGainDcDelayPhase::PbusWorkMode,
                micros: 1,
            },
            DcStep::WorkModePulse(_) => PhyRxGainDcAction::ConfigurePbusWorkModePulse,
            DcStep::WorkModePulseDelay(_) => PhyRxGainDcAction::DelayMicros {
                phase: PhyRxGainDcDelayPhase::PbusWorkModePulse,
                micros: 2,
            },
            DcStep::WorkModePulseClear(_) => PhyRxGainDcAction::ClearPbusWorkModePulse,
            DcStep::ClearRegisters(_) => PhyRxGainDcAction::ConfigureRegisters { enabled: false },
            DcStep::Complete => PhyRxGainDcAction::Complete,
            DcStep::Failed(failure) => PhyRxGainDcAction::Failed(failure),
        }
    }

    fn cleanup(&mut self, terminal: DcTerminal) {
        self.step = match terminal {
            DcTerminal::ContinueWifi | DcTerminal::Complete | DcTerminal::Failed(_) => {
                DcStep::ClockOff(PhyRxGainDcClock::Rx, terminal)
            }
        };
    }

    fn fail(&mut self, failure: PhyRxGainDcFailure) {
        self.cleanup(DcTerminal::Failed(failure));
    }

    fn store_calibration(
        &mut self,
        bank: PhyRxGainDcBank,
        index: u8,
        outcome: PhyRxDcCalibrationOutcome,
    ) {
        match bank {
            PhyRxGainDcBank::Wifi => {
                self.quality.record_wifi_baseband(index, outcome.converged);
                self.wifi_index_dc[index as usize] = outcome.configuration;
            }
            PhyRxGainDcBank::Shared => {
                self.quality.record_shared(index, outcome.converged);
                self.shared_index_dc[index as usize] = outcome.configuration;
            }
        }
        if bank == PhyRxGainDcBank::Wifi && index == 0 {
            // `phy_rxdc_fine_cal` follows the first Wi-Fi gain.
            self.step = DcStep::FineCode { index: 0 };
        } else {
            self.next_gain(bank, index);
        }
    }

    fn next_gain(&mut self, bank: PhyRxGainDcBank, index: u8) {
        if index + 1 != bank_count(bank) {
            self.step = DcStep::SetupRadioI {
                bank,
                index: index + 1,
            };
        } else if bank == PhyRxGainDcBank::Shared {
            self.step = DcStep::SharedRestoreI2c(crate::analog::i2c::field_write(
                analog_registers::SHARED_RX_GAIN_CALIBRATION_ENABLE,
                1,
            ));
        } else {
            self.cleanup(DcTerminal::Complete);
        }
    }

    fn baseband_calibration_step(&self, bank: PhyRxGainDcBank, index: u8) -> DcStep {
        DcStep::CalibrateBaseband {
            bank,
            index,
            transition: PhyRxDcCalibrationTransition::new(PhyRxDcCalibrationRequest {
                shared_radio: bank == PhyRxGainDcBank::Shared,
                stage: PhyRxDcCalibrationStage::Baseband,
                control: 0x800,
                initial: self.previous(bank, index),
                gain_index: index,
                rx_saturation_detected: self.parameters.rx_saturation_detected,
            }),
        }
    }

    fn after_work_mode(terminal: DcTerminal) -> DcStep {
        DcStep::ClearRegisters(terminal)
    }

    /// Rejected completions leave both the cursor and accumulated calibration
    /// products unchanged. Nested search cursors advance in place and retain their own rejection
    /// guarantees; coefficient publication follows only a terminal child.
    pub fn advance(
        &mut self,
        completion: PhyRxGainDcCompletion,
    ) -> Result<(), PhyRxGainDcTransitionError> {
        // Child rejection is transactional. Keep its cursor in the parent instead
        // of copying the nested transition for every hardware completion.
        if let (
            DcStep::FineCalibration { transition, .. }
            | DcStep::CalibrateBaseband { transition, .. },
            PhyRxGainDcCompletion::Calibration(completion),
        ) = (&mut self.step, completion)
        {
            transition
                .advance(completion)
                .map_err(|_| PhyRxGainDcTransitionError::WrongCompletion)?;
            if transition.terminal().is_some() {
                self.finish_calibration()?;
            }
            return Ok(());
        }
        match (self.step, completion) {
            (
                DcStep::ConfigureRegisters,
                PhyRxGainDcCompletion::RegistersConfigured { enabled: true },
            ) => {
                self.step =
                    DcStep::Rfpll(RfpllFrequencyTransition::channel(RfpllFrequencyRequest {
                        crystal_selector: self.parameters.crystal_selector,
                        frequency_code: 0x9b4,
                        offset: 0,
                    }));
            }
            (DcStep::Rfpll(mut transition), PhyRxGainDcCompletion::Rfpll(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxGainDcTransitionError::WrongCompletion)?;
                self.step = match transition.action() {
                    RfpllFrequencyAction::Complete(_) => DcStep::Debug,
                    RfpllFrequencyAction::Failed(failure) => {
                        self.fail(PhyRxGainDcFailure::Rfpll(failure));
                        return Ok(());
                    }
                    _ => DcStep::Rfpll(transition),
                };
            }
            (DcStep::Debug, PhyRxGainDcCompletion::PbusDebugModeConfigured) => {
                self.step = DcStep::RxOn { index: 0 };
            }
            (
                DcStep::WifiConfigureRegisters,
                PhyRxGainDcCompletion::RegistersConfigured { enabled: true },
            ) => {
                self.step =
                    DcStep::WifiRfpll(RfpllFrequencyTransition::channel(RfpllFrequencyRequest {
                        crystal_selector: self.parameters.crystal_selector,
                        frequency_code: 0x9b4,
                        offset: 0,
                    }));
            }
            (DcStep::WifiRfpll(mut transition), PhyRxGainDcCompletion::Rfpll(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxGainDcTransitionError::WrongCompletion)?;
                self.step = match transition.action() {
                    RfpllFrequencyAction::Complete(_) => DcStep::WifiDebug,
                    RfpllFrequencyAction::Failed(failure) => {
                        self.fail(PhyRxGainDcFailure::Rfpll(failure));
                        return Ok(());
                    }
                    _ => DcStep::WifiRfpll(transition),
                };
            }
            (DcStep::WifiDebug, PhyRxGainDcCompletion::PbusDebugModeConfigured) => {
                self.step = DcStep::WifiRxOn { index: 0 };
            }
            (
                DcStep::RxOn { index },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: PhyRxGainDcBank::Shared,
                    transaction,
                },
            ) if transaction == rx_on(index, self.parameters.pbus_rx_path_value) => {
                self.step = if index + 1 == RX_ON_COUNT {
                    DcStep::Clock {
                        clock: PhyRxGainDcClock::Rx,
                        enabled: true,
                    }
                } else {
                    DcStep::RxOn { index: index + 1 }
                };
            }
            (
                DcStep::Clock {
                    clock: PhyRxGainDcClock::Rx,
                    enabled: true,
                },
                PhyRxGainDcCompletion::ClockConfigured {
                    clock: PhyRxGainDcClock::Rx,
                    enabled: true,
                },
            ) => {
                self.step = DcStep::Clock {
                    clock: PhyRxGainDcClock::Tx,
                    enabled: true,
                };
            }
            (
                DcStep::Clock {
                    clock: PhyRxGainDcClock::Tx,
                    enabled: true,
                },
                PhyRxGainDcCompletion::ClockConfigured {
                    clock: PhyRxGainDcClock::Tx,
                    enabled: true,
                },
            ) => self.step = DcStep::SharedReadPbus,
            (
                DcStep::SharedReadPbus,
                PhyRxGainDcCompletion::PbusRead {
                    selector: 1,
                    path: 1,
                    value,
                },
            ) => {
                self.step = DcStep::SharedForcePbus {
                    value: value as u16 | 2,
                }
            }
            (
                DcStep::SharedForcePbus { value },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: PhyRxGainDcBank::Shared,
                    transaction,
                },
            ) if transaction == PhyPbusForceTest::new(1, 1, value) => {
                self.step = DcStep::SharedI2c(crate::analog::i2c::field_write(
                    analog_registers::SHARED_RX_GAIN_CALIBRATION_ENABLE,
                    0,
                ));
            }
            (DcStep::SharedI2c(mut transition), PhyRxGainDcCompletion::I2c(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxGainDcTransitionError::WrongCompletion)?;
                self.step = if transition.action() == oer_radio_analog::Action::Complete(()) {
                    DcStep::SetupRadioI {
                        bank: PhyRxGainDcBank::Shared,
                        index: 0,
                    }
                } else {
                    DcStep::SharedI2c(transition)
                };
            }
            (
                DcStep::FineCode { index },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: PhyRxGainDcBank::Wifi,
                    transaction,
                },
            ) if transaction == PhyPbusForceTest::new(1, 2, fine_code(index)) => {
                self.step = DcStep::FineCalibration {
                    index,
                    transition: PhyRxDcCalibrationTransition::new(PhyRxDcCalibrationRequest {
                        shared_radio: false,
                        stage: PhyRxDcCalibrationStage::Radio,
                        control: 0x800,
                        initial: self.fine_current,
                        gain_index: 0,
                        rx_saturation_detected: self.parameters.rx_saturation_detected,
                    }),
                };
            }

            (
                DcStep::SetupRadioI { bank, index },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: completed_bank,
                    transaction,
                },
            ) if bank == completed_bank && transaction == PhyPbusForceTest::new(2, 1, 0x100) => {
                self.step = DcStep::SetupRadioQ { bank, index };
            }
            (
                DcStep::SetupRadioQ { bank, index },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: completed_bank,
                    transaction,
                },
            ) if bank == completed_bank && transaction == PhyPbusForceTest::new(3, 1, 0x100) => {
                self.step = DcStep::SetupBasebandI {
                    bank,
                    index,
                    value: self.baseband(bank, index),
                };
            }
            (
                DcStep::SetupBasebandI { bank, index, value },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: completed_bank,
                    transaction,
                },
            ) if bank == completed_bank && transaction == PhyPbusForceTest::new(2, 2, value[0]) => {
                self.step = DcStep::SetupBasebandQ { bank, index, value };
            }
            (
                DcStep::SetupBasebandQ { bank, index, value },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: completed_bank,
                    transaction,
                },
            ) if bank == completed_bank && transaction == PhyPbusForceTest::new(3, 2, value[1]) => {
                self.step = DcStep::SetGain {
                    bank,
                    index,
                    value,
                    transaction: 0,
                };
            }
            (
                DcStep::SetGain {
                    bank,
                    index,
                    value,
                    transaction: transaction_index,
                },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: completed_bank,
                    transaction,
                },
            ) if bank == completed_bank
                && transaction
                    == set_rx_gain_transaction(
                        u32::from(gain(bank, index)) << 12,
                        self.parameters.pbus_rx_path_value,
                        transaction_index,
                    ) =>
            {
                if transaction_index == 2 {
                    self.step = if bank == PhyRxGainDcBank::Shared {
                        DcStep::SetSharedMixerDgain { index }
                    } else {
                        self.baseband_calibration_step(bank, index)
                    };
                } else {
                    self.step = DcStep::SetGain {
                        bank,
                        index,
                        value,
                        transaction: transaction_index + 1,
                    };
                }
            }
            (
                DcStep::SetSharedMixerDgain { index },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: PhyRxGainDcBank::Shared,
                    transaction,
                },
            ) if transaction
                == shared_mixer_dgain_transaction(index, self.parameters.pbus_rx_path_value) =>
            {
                self.step = self.baseband_calibration_step(PhyRxGainDcBank::Shared, index);
            }

            (DcStep::SharedRestoreI2c(mut transition), PhyRxGainDcCompletion::I2c(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxGainDcTransitionError::WrongCompletion)?;
                if transition.action() == oer_radio_analog::Action::Complete(()) {
                    self.cleanup(DcTerminal::ContinueWifi);
                } else {
                    self.step = DcStep::SharedRestoreI2c(transition);
                }
            }
            (
                DcStep::WifiRxOn { index },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: PhyRxGainDcBank::Wifi,
                    transaction,
                },
            ) if transaction == rx_on(index, self.parameters.pbus_rx_path_value) => {
                self.step = if index + 1 == RX_ON_COUNT {
                    DcStep::WifiClock {
                        clock: PhyRxGainDcClock::Rx,
                    }
                } else {
                    DcStep::WifiRxOn { index: index + 1 }
                };
            }
            (
                DcStep::WifiClock {
                    clock: PhyRxGainDcClock::Rx,
                },
                PhyRxGainDcCompletion::ClockConfigured {
                    clock: PhyRxGainDcClock::Rx,
                    enabled: true,
                },
            ) => {
                self.step = DcStep::WifiClock {
                    clock: PhyRxGainDcClock::Tx,
                };
            }
            (
                DcStep::WifiClock {
                    clock: PhyRxGainDcClock::Tx,
                },
                PhyRxGainDcCompletion::ClockConfigured {
                    clock: PhyRxGainDcClock::Tx,
                    enabled: true,
                },
            ) => {
                self.step = DcStep::SetupRadioI {
                    bank: PhyRxGainDcBank::Wifi,
                    index: 0,
                }
            }
            (
                DcStep::RxOn { index },
                PhyRxGainDcCompletion::PbusTimedOut {
                    bank: PhyRxGainDcBank::Shared,
                    transaction,
                },
            ) if transaction == rx_on(index, self.parameters.pbus_rx_path_value) => {
                self.fail(PhyRxGainDcFailure::Pbus {
                    bank: PhyRxGainDcBank::Shared,
                    transaction,
                });
            }
            (
                DcStep::SetupRadioI { bank, .. }
                | DcStep::SetupRadioQ { bank, .. }
                | DcStep::SetupBasebandI { bank, .. }
                | DcStep::SetupBasebandQ { bank, .. }
                | DcStep::SetGain { bank, .. },
                PhyRxGainDcCompletion::PbusTimedOut {
                    bank: completed_bank,
                    transaction,
                },
            ) if bank == completed_bank => {
                self.fail(PhyRxGainDcFailure::Pbus {
                    bank: completed_bank,
                    transaction,
                });
            }
            (
                DcStep::SetSharedMixerDgain { .. },
                PhyRxGainDcCompletion::PbusTimedOut {
                    bank: PhyRxGainDcBank::Shared,
                    transaction,
                },
            ) => {
                self.fail(PhyRxGainDcFailure::Pbus {
                    bank: PhyRxGainDcBank::Shared,
                    transaction,
                });
            }
            (
                DcStep::WifiRxOn { .. } | DcStep::FineCode { .. },
                PhyRxGainDcCompletion::PbusTimedOut {
                    bank: PhyRxGainDcBank::Wifi,
                    transaction,
                },
            ) => {
                self.fail(PhyRxGainDcFailure::Pbus {
                    bank: PhyRxGainDcBank::Wifi,
                    transaction,
                });
            }
            (
                DcStep::SharedForcePbus { .. },
                PhyRxGainDcCompletion::PbusTimedOut {
                    bank: PhyRxGainDcBank::Shared,
                    transaction,
                },
            ) => self.fail(PhyRxGainDcFailure::Pbus {
                bank: PhyRxGainDcBank::Shared,
                transaction,
            }),
            (
                DcStep::ClockOff(PhyRxGainDcClock::Rx, terminal),
                PhyRxGainDcCompletion::ClockConfigured {
                    clock: PhyRxGainDcClock::Rx,
                    enabled: false,
                },
            ) => self.step = DcStep::ClockOff(PhyRxGainDcClock::Tx, terminal),
            (
                DcStep::ClockOff(PhyRxGainDcClock::Tx, terminal),
                PhyRxGainDcCompletion::ClockConfigured {
                    clock: PhyRxGainDcClock::Tx,
                    enabled: false,
                },
            ) => self.step = DcStep::RxOff { index: 0, terminal },
            (
                DcStep::RxOff { index, terminal },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: PhyRxGainDcBank::Shared,
                    transaction,
                }
                | PhyRxGainDcCompletion::PbusTimedOut {
                    bank: PhyRxGainDcBank::Shared,
                    transaction,
                },
            ) if transaction == rx_off(index) => {
                self.step = if index + 1 == RX_OFF_COUNT {
                    DcStep::WorkMode(terminal)
                } else {
                    DcStep::RxOff {
                        index: index + 1,
                        terminal,
                    }
                };
            }
            (
                DcStep::WorkMode(terminal),
                PhyRxGainDcCompletion::PbusWorkModeConfigured {
                    settle_required: false,
                },
            ) => self.step = Self::after_work_mode(terminal),
            (
                DcStep::WorkMode(terminal),
                PhyRxGainDcCompletion::PbusWorkModeConfigured {
                    settle_required: true,
                },
            ) => self.step = DcStep::WorkModeDelay(terminal),
            (
                DcStep::WorkModeDelay(terminal),
                PhyRxGainDcCompletion::DelayElapsed {
                    phase: PhyRxGainDcDelayPhase::PbusWorkMode,
                    micros: 1,
                },
            ) => self.step = DcStep::WorkModePulse(terminal),
            (
                DcStep::WorkModePulse(terminal),
                PhyRxGainDcCompletion::PbusWorkModePulseConfigured,
            ) => self.step = DcStep::WorkModePulseDelay(terminal),
            (
                DcStep::WorkModePulseDelay(terminal),
                PhyRxGainDcCompletion::DelayElapsed {
                    phase: PhyRxGainDcDelayPhase::PbusWorkModePulse,
                    micros: 2,
                },
            ) => self.step = DcStep::WorkModePulseClear(terminal),
            (
                DcStep::WorkModePulseClear(terminal),
                PhyRxGainDcCompletion::PbusWorkModePulseCleared,
            ) => self.step = Self::after_work_mode(terminal),
            (
                DcStep::ClearRegisters(terminal),
                PhyRxGainDcCompletion::RegistersConfigured { enabled: false },
            ) => {
                self.step = match terminal {
                    DcTerminal::ContinueWifi => DcStep::WifiConfigureRegisters,
                    DcTerminal::Complete => DcStep::Complete,
                    DcTerminal::Failed(failure) => DcStep::Failed(failure),
                };
            }
            (DcStep::Complete, _) | (DcStep::Failed(_), _) => {
                return Err(PhyRxGainDcTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyRxGainDcTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainCalibrationBindingError {
    NotDirectMmio,
    UnsupportedAction,
    Pbus(crate::analog::pbus::PhyPbusHardwareBindingError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcCalibrationRestoreInvariant {
    OwnedByOtherCalibration,
    NestingExceeded,
    NotPending,
}

// Compact operation identity; coefficient products stay in the transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CalibrationMmio {
    PrepareControlRestore,
    ReadPbus { selector: u8, path: u8 },
    RestoreControl,
}

/// Non-cloneable token for one direct MMIO edge of
/// `phy_pbus_rx_dco_cal_1step`.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxDcCalibrationMmioBinding {
    operation: CalibrationMmio,
}

impl PhyRxDcCalibrationMmioBinding {
    pub fn new(action: PhyRxDcCalibrationAction) -> Result<Self, PhyRxGainCalibrationBindingError> {
        let operation = match action {
            PhyRxDcCalibrationAction::PrepareControlRestore => {
                CalibrationMmio::PrepareControlRestore
            }
            PhyRxDcCalibrationAction::ReadPbus { selector, path } => {
                CalibrationMmio::ReadPbus { selector, path }
            }
            PhyRxDcCalibrationAction::RestoreControl => CalibrationMmio::RestoreControl,
            _ => return Err(PhyRxGainCalibrationBindingError::NotDirectMmio),
        };
        Ok(Self { operation })
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn action(&self) -> PhyRxDcCalibrationAction {
        match self.operation {
            CalibrationMmio::PrepareControlRestore => {
                PhyRxDcCalibrationAction::PrepareControlRestore
            }
            CalibrationMmio::ReadPbus { selector, path } => {
                PhyRxDcCalibrationAction::ReadPbus { selector, path }
            }
            CalibrationMmio::RestoreControl => PhyRxDcCalibrationAction::RestoreControl,
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyContext,
    ) -> Result<PhyRxDcCalibrationCompletion, PhyRxDcCalibrationRestoreInvariant> {
        match self.operation {
            CalibrationMmio::PrepareControlRestore => {
                oer_esp32s31_hal::phy::rx_dco::prepare_control_restore(registers).map_err(
                    |error| match error {
                        oer_esp32s31_hal::types::RxDcoControlPrepareError::RestorePending => {
                            PhyRxDcCalibrationRestoreInvariant::OwnedByOtherCalibration
                        }
                        oer_esp32s31_hal::types::RxDcoControlPrepareError::RestoreStackFull => {
                            PhyRxDcCalibrationRestoreInvariant::NestingExceeded
                        }
                    },
                )?;
                Ok(PhyRxDcCalibrationCompletion::ControlRestorePrepared)
            }
            CalibrationMmio::ReadPbus { selector, path } => {
                Ok(PhyRxDcCalibrationCompletion::PbusRead {
                    selector,
                    path,
                    value: {
                        let result =
                            oer_esp32s31_hal::phy::pbus::read_result(registers, selector, path);
                        debug_assert!(
                            result.is_some(),
                            "RX-DC calibration emitted an unrecovered PBus selector"
                        );
                        u32::from(result.unwrap_or(0))
                    },
                })
            }
            CalibrationMmio::RestoreControl => {
                oer_esp32s31_hal::phy::rx_dco::restore_control(registers)
                    .map_err(|_| PhyRxDcCalibrationRestoreInvariant::NotPending)?;
                Ok(PhyRxDcCalibrationCompletion::ControlRestored)
            }
        }
    }
}

// Compact operation identity; coefficient products stay in the transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DcMmio {
    ConfigureRegisters {
        enabled: bool,
    },
    ConfigurePbusDebugMode,
    ConfigureClock {
        clock: PhyRxGainDcClock,
        enabled: bool,
    },
    ReadPbus {
        selector: u8,
        path: u8,
    },
    ConfigurePbusWorkMode,
    ConfigurePbusWorkModePulse,
    ClearPbusWorkModePulse,
}

/// Non-cloneable token for one direct MMIO edge of the composed RX-DC gain
/// calibration. PBus force commands, I2C commands, timers, RFPLL and nested
/// estimator observations retain their separate executor bindings.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxGainDcMmioBinding {
    operation: DcMmio,
}

impl PhyRxGainDcMmioBinding {
    pub fn new(action: PhyRxGainDcAction) -> Result<Self, PhyRxGainCalibrationBindingError> {
        let operation = match action {
            PhyRxGainDcAction::ConfigureRegisters { enabled } => {
                DcMmio::ConfigureRegisters { enabled }
            }
            PhyRxGainDcAction::ConfigurePbusDebugMode => DcMmio::ConfigurePbusDebugMode,
            PhyRxGainDcAction::ConfigureClock { clock, enabled } => {
                DcMmio::ConfigureClock { clock, enabled }
            }
            PhyRxGainDcAction::ReadPbus { selector, path } => DcMmio::ReadPbus { selector, path },
            PhyRxGainDcAction::ConfigurePbusWorkMode => DcMmio::ConfigurePbusWorkMode,
            PhyRxGainDcAction::ConfigurePbusWorkModePulse => DcMmio::ConfigurePbusWorkModePulse,
            PhyRxGainDcAction::ClearPbusWorkModePulse => DcMmio::ClearPbusWorkModePulse,
            _ => return Err(PhyRxGainCalibrationBindingError::NotDirectMmio),
        };
        Ok(Self { operation })
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn action(&self) -> PhyRxGainDcAction {
        match self.operation {
            DcMmio::ConfigureRegisters { enabled } => {
                PhyRxGainDcAction::ConfigureRegisters { enabled }
            }
            DcMmio::ConfigurePbusDebugMode => PhyRxGainDcAction::ConfigurePbusDebugMode,
            DcMmio::ConfigureClock { clock, enabled } => {
                PhyRxGainDcAction::ConfigureClock { clock, enabled }
            }
            DcMmio::ReadPbus { selector, path } => PhyRxGainDcAction::ReadPbus { selector, path },
            DcMmio::ConfigurePbusWorkMode => PhyRxGainDcAction::ConfigurePbusWorkMode,
            DcMmio::ConfigurePbusWorkModePulse => PhyRxGainDcAction::ConfigurePbusWorkModePulse,
            DcMmio::ClearPbusWorkModePulse => PhyRxGainDcAction::ClearPbusWorkModePulse,
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyContext,
    ) -> PhyRxGainDcCompletion {
        match self.operation {
            DcMmio::ConfigureRegisters { enabled } => {
                crate::hardware::configure_phy_rx_gain_dc_registers(registers, enabled);
                PhyRxGainDcCompletion::RegistersConfigured { enabled }
            }
            DcMmio::ConfigurePbusDebugMode => {
                oer_esp32s31_hal::phy::pbus::configure_debug_mode(registers);
                PhyRxGainDcCompletion::PbusDebugModeConfigured
            }
            DcMmio::ConfigureClock { clock, enabled } => {
                match clock {
                    PhyRxGainDcClock::Rx => {
                        oer_esp32s31_hal::phy::pbus::configure_rx_clock(registers, enabled)
                    }
                    PhyRxGainDcClock::Tx => {
                        oer_esp32s31_hal::phy::pbus::configure_tx_clock(registers, enabled)
                    }
                }
                PhyRxGainDcCompletion::ClockConfigured { clock, enabled }
            }
            DcMmio::ReadPbus { selector, path } => PhyRxGainDcCompletion::PbusRead {
                selector,
                path,
                value: {
                    let result =
                        oer_esp32s31_hal::phy::pbus::read_result(registers, selector, path);
                    debug_assert!(
                        result.is_some(),
                        "RX-gain DC transition emitted an unrecovered PBus selector"
                    );
                    u32::from(result.unwrap_or(0))
                },
            },
            DcMmio::ConfigurePbusWorkMode => PhyRxGainDcCompletion::PbusWorkModeConfigured {
                settle_required: oer_esp32s31_hal::phy::pbus::configure_work_mode(registers),
            },
            DcMmio::ConfigurePbusWorkModePulse => {
                oer_esp32s31_hal::phy::agc::configure_pbus_work_mode_pulse(registers);
                PhyRxGainDcCompletion::PbusWorkModePulseConfigured
            }
            DcMmio::ClearPbusWorkModePulse => {
                oer_esp32s31_hal::phy::agc::clear_pbus_work_mode_pulse(registers);
                PhyRxGainDcCompletion::PbusWorkModePulseCleared
            }
        }
    }
}

/// Linear owner of one PBus edge from `phy_pbus_rx_dco_cal_1step`.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxDcCalibrationPbusBinding {
    transaction: PhyPbusForceTest,
    hardware: crate::analog::pbus::PhyPbusHardwareBinding,
}

impl PhyRxDcCalibrationPbusBinding {
    pub fn new(action: PhyRxDcCalibrationAction) -> Result<Self, PhyRxGainCalibrationBindingError> {
        let PhyRxDcCalibrationAction::ForcePbus(transaction) = action else {
            return Err(PhyRxGainCalibrationBindingError::UnsupportedAction);
        };
        Ok(Self {
            transaction,
            hardware: crate::analog::pbus::PhyPbusHardwareBinding::new(transaction),
        })
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn action(&self) -> crate::analog::pbus::PhyPbusHardwareAction {
        self.hardware.action()
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub fn started(&mut self) -> Result<(), crate::analog::pbus::PhyPbusHardwareBindingError> {
        self.hardware.started()
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub fn observe_completed(
        &mut self,
        completed: bool,
    ) -> Result<
        crate::analog::pbus::PhyPbusHardwareObservation,
        crate::analog::pbus::PhyPbusHardwareBindingError,
    > {
        self.hardware.observe_completed(completed)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target(
        &mut self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> Result<(), crate::analog::pbus::PhyPbusHardwareBindingError> {
        self.hardware.start_target(registers)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge(
        &mut self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> Result<
        crate::analog::pbus::PhyPbusHardwareObservation,
        crate::analog::pbus::PhyPbusHardwareBindingError,
    > {
        self.hardware.observe_target_edge(registers)
    }

    pub fn into_completion(
        self,
    ) -> Result<PhyRxDcCalibrationCompletion, PhyRxGainCalibrationBindingError> {
        self.hardware
            .into_transaction()
            .map(PhyRxDcCalibrationCompletion::PbusForceCompleted)
            .map_err(PhyRxGainCalibrationBindingError::Pbus)
    }

    pub const fn into_timeout_completion(self) -> PhyRxDcCalibrationCompletion {
        PhyRxDcCalibrationCompletion::PbusForceTimedOut(self.transaction)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxDcCalibrationTimerBinding {
    measurement: u8,
    micros: u32,
}

impl PhyRxDcCalibrationTimerBinding {
    pub fn new(action: PhyRxDcCalibrationAction) -> Result<Self, PhyRxGainCalibrationBindingError> {
        match action {
            PhyRxDcCalibrationAction::DelayMicros {
                measurement,
                micros,
            } => Ok(Self {
                measurement,
                micros,
            }),
            _ => Err(PhyRxGainCalibrationBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyRxDcCalibrationCompletion {
        PhyRxDcCalibrationCompletion::DelayElapsed {
            measurement: self.measurement,
            micros: self.micros,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyRxDcCalibrationExternalBinding {
    Mmio(PhyRxDcCalibrationMmioBinding),
    Pbus(PhyRxDcCalibrationPbusBinding),
    Timer(PhyRxDcCalibrationTimerBinding),
    Minimum(crate::rx::dc_offset::PhyRxDcMinimumExternalBinding),
}

impl PhyRxDcCalibrationExternalBinding {
    pub fn lower(
        action: PhyRxDcCalibrationAction,
    ) -> Result<Self, PhyRxGainCalibrationBindingError> {
        if let PhyRxDcCalibrationAction::Minimum(action) = action {
            return crate::rx::dc_offset::PhyRxDcMinimumExternalBinding::lower(action)
                .map(Self::Minimum)
                .map_err(|_| PhyRxGainCalibrationBindingError::UnsupportedAction);
        }
        if let Ok(binding) = PhyRxDcCalibrationMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        if let Ok(binding) = PhyRxDcCalibrationPbusBinding::new(action) {
            return Ok(Self::Pbus(binding));
        }
        if let Ok(binding) = PhyRxDcCalibrationTimerBinding::new(action) {
            return Ok(Self::Timer(binding));
        }

        Err(PhyRxGainCalibrationBindingError::UnsupportedAction)
    }
}

/// Linear owner of one bank-qualified PBus edge in the composed RX-gain
/// calibration. The bank is retained in both normal and timeout completions.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxGainDcPbusBinding {
    bank: PhyRxGainDcBank,
    transaction: PhyPbusForceTest,
    hardware: crate::analog::pbus::PhyPbusHardwareBinding,
}

impl PhyRxGainDcPbusBinding {
    pub fn new(action: PhyRxGainDcAction) -> Result<Self, PhyRxGainCalibrationBindingError> {
        let PhyRxGainDcAction::ForcePbus { bank, transaction } = action else {
            return Err(PhyRxGainCalibrationBindingError::UnsupportedAction);
        };
        Ok(Self {
            bank,
            transaction,
            hardware: crate::analog::pbus::PhyPbusHardwareBinding::new(transaction),
        })
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn action(&self) -> crate::analog::pbus::PhyPbusHardwareAction {
        self.hardware.action()
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub fn started(&mut self) -> Result<(), crate::analog::pbus::PhyPbusHardwareBindingError> {
        self.hardware.started()
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub fn observe_completed(
        &mut self,
        completed: bool,
    ) -> Result<
        crate::analog::pbus::PhyPbusHardwareObservation,
        crate::analog::pbus::PhyPbusHardwareBindingError,
    > {
        self.hardware.observe_completed(completed)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target(
        &mut self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> Result<(), crate::analog::pbus::PhyPbusHardwareBindingError> {
        self.hardware.start_target(registers)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge(
        &mut self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> Result<
        crate::analog::pbus::PhyPbusHardwareObservation,
        crate::analog::pbus::PhyPbusHardwareBindingError,
    > {
        self.hardware.observe_target_edge(registers)
    }

    pub fn into_completion(
        self,
    ) -> Result<PhyRxGainDcCompletion, PhyRxGainCalibrationBindingError> {
        let bank = self.bank;
        self.hardware
            .into_transaction()
            .map(|transaction| PhyRxGainDcCompletion::PbusCompleted { bank, transaction })
            .map_err(PhyRxGainCalibrationBindingError::Pbus)
    }

    pub const fn into_timeout_completion(self) -> PhyRxGainDcCompletion {
        PhyRxGainDcCompletion::PbusTimedOut {
            bank: self.bank,
            transaction: self.transaction,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxGainDcTimerBinding {
    phase: PhyRxGainDcDelayPhase,
    micros: u32,
}

impl PhyRxGainDcTimerBinding {
    pub fn new(action: PhyRxGainDcAction) -> Result<Self, PhyRxGainCalibrationBindingError> {
        match action {
            PhyRxGainDcAction::DelayMicros { phase, micros } => Ok(Self { phase, micros }),
            _ => Err(PhyRxGainCalibrationBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyRxGainDcCompletion {
        PhyRxGainDcCompletion::DelayElapsed {
            phase: self.phase,
            micros: self.micros,
        }
    }
}

/// Exhaustive lowering of every non-terminal composed RX-gain DC action.
#[derive(Debug, Eq, PartialEq)]
pub enum PhyRxGainDcExternalBinding {
    Mmio(PhyRxGainDcMmioBinding),
    Rfpll(crate::analog::rfpll::RfpllFrequencyExternalBinding),
    Pbus(PhyRxGainDcPbusBinding),
    I2c(crate::analog::i2c::MaskedI2cWriteBinding),
    Calibration(PhyRxDcCalibrationExternalBinding),
    Timer(PhyRxGainDcTimerBinding),
}

impl PhyRxGainDcExternalBinding {
    pub fn lower(action: PhyRxGainDcAction) -> Result<Self, PhyRxGainCalibrationBindingError> {
        if let PhyRxGainDcAction::Calibration(action) = action {
            return PhyRxDcCalibrationExternalBinding::lower(action).map(Self::Calibration);
        }
        if let Ok(binding) = PhyRxGainDcMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        if let Ok(binding) = PhyRxGainDcPbusBinding::new(action) {
            return Ok(Self::Pbus(binding));
        }
        if let Ok(binding) = PhyRxGainDcTimerBinding::new(action) {
            return Ok(Self::Timer(binding));
        }
        match action {
            PhyRxGainDcAction::Rfpll(action) => {
                crate::analog::rfpll::RfpllFrequencyExternalBinding::lower(action)
                    .map(Self::Rfpll)
                    .map_err(|_| PhyRxGainCalibrationBindingError::UnsupportedAction)
            }
            PhyRxGainDcAction::I2c(action) => {
                crate::analog::i2c::MaskedI2cWriteBinding::new(action)
                    .map(Self::I2c)
                    .map_err(|_| PhyRxGainCalibrationBindingError::UnsupportedAction)
            }

            _ => Err(PhyRxGainCalibrationBindingError::UnsupportedAction),
        }
    }
}

#[cfg(test)]
mod tests;
