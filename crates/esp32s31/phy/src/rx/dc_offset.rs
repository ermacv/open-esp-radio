//! Event-driven RX DC-offset calibration for ESP32-S31 rev0.
//!
//! Primary reference: `esp32s31_rev0_rom.elf::phy_pbus_rx_dco_cal` at
//! `0x2f82_8f44`, size `0x228`. The crystal-duty caller uses the fixed
//! argument tuple `(0x0fa0, configuration, 10, 0, 0)`. This module owns the
//! complete reachable twelve-iteration control loop. Its `phy_dc_iq_est`
//! child is another Rust-owned transition, so the ROM hardware-ready spin and
//! synchronous delays are absent from the complete reachable graph.

use crate::{
    analog::pbus::PhyPbusForceTest,
    calibration::estimator::{
        PhyDcIqAction, PhyDcIqCompletion, PhyDcIqEstimateOutcome, PhyDcIqEstimateTransition,
        PhyDcIqFailure,
    },
};

pub use crate::calibration::estimator::{PhyDcIqEstimate, PhyDcIqEstimateRequest};

const MAX_ITERATIONS: u8 = 12;
const RX_DC_MINIMUM_MAX_ATTEMPTS: u8 = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxDcoRequest {
    /// Measurement control passed as the second argument to
    /// `phy_dc_iq_est(1, control, ...)`.
    pub control: u16,
    /// Two words initialized by the caller. ROM treats the low and high
    /// halfwords of word zero as signed I/Q compensation values. Word one is
    /// retained byte-for-byte even though the reachable ROM body does not
    /// access it.
    pub configuration: [u32; 2],
    /// Timer interval before each IQ measurement.
    pub delay_micros: u32,
}

impl PhyRxDcoRequest {
    pub const XTAL_DUTY: Self = Self {
        control: 0x0fa0,
        configuration: [0x0100_0100; 2],
        delay_micros: 10,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxDcoOutcome {
    pub configuration: [u32; 2],
    /// Number of completed IQ measurements, in `1..=12`.
    pub iterations: u8,
    /// False means ROM's bounded iteration limit was reached. It is still a
    /// normal vendor-compatible outcome, not an implicit retry request.
    pub converged: bool,
    pub last_estimate: PhyDcIqEstimate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcoFailure {
    PbusForceTimedOut(PhyPbusForceTest),
    DcIq(PhyDcIqFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcoAction {
    PrepareRxDcoControlRestore,
    ReadPbus { selector: u8, path: u8 },
    ForcePbus(PhyPbusForceTest),
    DelayMicros { iteration: u8, micros: u32 },
    DcIq(PhyDcIqAction),
    RestoreRxDcoControl,
    Complete(PhyRxDcoOutcome),
    Failed(PhyRxDcoFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcoCompletion {
    RxDcoControlRestorePrepared,
    PbusRead { selector: u8, path: u8, value: u32 },
    PbusForceCompleted(PhyPbusForceTest),
    PbusForceTimedOut(PhyPbusForceTest),
    DelayElapsed { iteration: u8, micros: u32 },
    DcIq(PhyDcIqCompletion),
    RxDcoControlRestored,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcoTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxDcMinimumRequest {
    /// Parent-owned identity. The ROM function has no corresponding field.
    pub measurement: u8,
    pub control: u16,
    pub mode: u8,
    /// Rust-owned replacement for `phy_param[0x1ae] == 1`.
    pub rx_saturation_detected: bool,
    /// Estimate the caller's output slot holds before the search. ROM
    /// `phy_rxdc_est_min` writes that slot only for an admitted estimate, so a
    /// search that admits none returns these I/Q values with the exhausted
    /// power sentinel.
    pub previous: PhyDcIqEstimate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxDcMinimumOutcome {
    pub request: PhyRxDcMinimumRequest,
    pub estimate: PhyDcIqEstimate,
    pub attempts: u8,
    /// Rust-owned replacement for the cumulative diagnostic halfword at
    /// `phy_param[0x1ac]`.
    pub readiness_activity_edges: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcMinimumFailure {
    DcIq(PhyDcIqFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcMinimumAction {
    DcIq(PhyDcIqAction),
    Complete(PhyRxDcMinimumOutcome),
    Failed(PhyRxDcMinimumFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcMinimumCompletion {
    DcIq(PhyDcIqCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcMinimumTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyRxDcMinimumStep {
    Measure(PhyDcIqEstimateTransition),
    Complete(PhyRxDcMinimumOutcome),
    Failed(PhyRxDcMinimumFailure),
}

/// Heap-free translation of rev0 ROM `phy_rxdc_est_min`.
///
/// ROM may invoke `phy_dc_iq_est` eight times. The only apparent polling in
/// that child is represented by externally delivered readiness edges with a
/// finite owner-supplied deadline. This parent performs no hidden retry,
/// delay, allocation, or global-state access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxDcMinimumTransition {
    request: PhyRxDcMinimumRequest,
    step: PhyRxDcMinimumStep,
    attempt: u8,
    best: PhyDcIqEstimate,
    minimum_power: i32,
    readiness_activity_edges: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PhyRxDcMinimumPolicyState {
    attempts: u8,
    best: PhyDcIqEstimate,
    minimum_power: i32,
    readiness_activity_edges: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyRxDcMinimumDecision {
    Continue,
    Complete(PhyRxDcMinimumOutcome),
}

impl PhyRxDcMinimumPolicyState {
    #[cfg(any(target_arch = "riscv32", test))]
    const fn new(previous: PhyDcIqEstimate) -> Self {
        Self {
            attempts: 0,
            best: previous,
            minimum_power: 100,
            readiness_activity_edges: 0,
        }
    }

    fn accept(
        &mut self,
        request: PhyRxDcMinimumRequest,
        outcome: PhyDcIqEstimateOutcome,
    ) -> PhyRxDcMinimumDecision {
        self.readiness_activity_edges = self
            .readiness_activity_edges
            .wrapping_add(outcome.readiness_activity_edges);

        // `phy_iq_est_enable` clears `phy_param[0x1ac]` before every child
        // estimate, so admission considers this attempt's activity only.
        if outcome.estimate.power < self.minimum_power
            && (outcome.readiness_activity_edges == 0 || request.rx_saturation_detected)
        {
            self.best = outcome.estimate;
            self.minimum_power = outcome.estimate.power;
        }

        self.attempts += 1;
        let complete = self.minimum_power < 36 || (self.attempts >= 3 && self.minimum_power < 48);
        if !complete && self.attempts < RX_DC_MINIMUM_MAX_ATTEMPTS {
            return PhyRxDcMinimumDecision::Continue;
        }

        let mut estimate = self.best;
        if !complete {
            // ROM overwrites output word two with this terminal sentinel.
            estimate.power = 0x38;
        }
        PhyRxDcMinimumDecision::Complete(PhyRxDcMinimumOutcome {
            request,
            estimate,
            attempts: self.attempts,
            readiness_activity_edges: self.readiness_activity_edges,
        })
    }
}

/// One complete ROM-shaped `phy_rxdc_est_min` transaction. The estimator
/// retry loop and selector stay below the RX calibration cursor.
#[derive(Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct PhyRxDcMinimumTargetTransaction {
    request: PhyRxDcMinimumRequest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct PhyRxDcMinimumTargetCompletion {
    terminal: Result<PhyRxDcMinimumOutcome, PhyRxDcMinimumFailure>,
    operations: u32,
    estimators: u8,
}

#[cfg(any(target_arch = "riscv32", test))]
impl PhyRxDcMinimumTargetCompletion {
    pub(crate) const fn operations(&self) -> u32 {
        self.operations
    }

    pub(crate) const fn estimators(&self) -> u8 {
        self.estimators
    }

    pub(crate) const fn into_terminal(
        self,
    ) -> Result<PhyRxDcMinimumOutcome, PhyRxDcMinimumFailure> {
        self.terminal
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl PhyRxDcMinimumTargetTransaction {
    pub(crate) const fn new(request: PhyRxDcMinimumRequest) -> Self {
        Self { request }
    }

    #[inline]
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn execute_target<D: crate::executor::wait::PhyShortDelay>(
        self,
        maximum_operations: u32,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> Result<Option<PhyRxDcMinimumTargetCompletion>, crate::target_executor::PhyTargetPortError>
    {
        self.execute_with(maximum_operations, |estimator, maximum_samples| {
            estimator.execute_with(
                maximum_samples.min(crate::HARDWARE_EDGE_LIMIT),
                registers,
                |registers, control| {
                    crate::calibration::estimator::configure_target(registers, control);
                },
                |registers, phase, enabled| {
                    crate::calibration::estimator::set_enable_target(registers, phase, enabled);
                },
                |_, micros| {
                    if micros <= D::MAX_MICROS && D::settle_micros(micros) {
                        Ok(())
                    } else {
                        Err(crate::target_executor::PhyTargetPortError::HardwareCapabilityUnavailable)
                    }
                },
                |registers| crate::calibration::estimator::sample_readiness_target(registers),
                |registers| crate::calibration::estimator::read_accumulators_target(registers),
            )
        })
    }

    /// Execute all estimator attempts behind one typed target transaction.
    /// None means the shared operation budget cannot admit another complete
    /// estimator envelope, so hardware is left untouched.
    pub(crate) fn execute_with<E>(
        self,
        maximum_operations: u32,
        mut execute: impl FnMut(
            crate::calibration::estimator::PhyDcIqTargetTransaction,
            u16,
        )
            -> Result<crate::calibration::estimator::PhyDcIqTargetCompletion, E>,
    ) -> Result<Option<PhyRxDcMinimumTargetCompletion>, E> {
        let mut operations = 0_u32;
        let mut policy = PhyRxDcMinimumPolicyState::new(self.request.previous);
        loop {
            let available = maximum_operations.saturating_sub(operations);
            let complete_estimator_allowance = u32::from(crate::HARDWARE_EDGE_LIMIT) + 8;
            if available < complete_estimator_allowance {
                return Ok(None);
            }
            let allowance = crate::HARDWARE_EDGE_LIMIT;
            let request = PhyRxDcMinimumTransition::estimate_request(self.request, policy.attempts);
            let completion = execute(
                crate::calibration::estimator::PhyDcIqTargetTransaction::new(request),
                allowance,
            )?;
            operations = operations.saturating_add(u32::from(completion.operations()));
            let outcome = match completion.into_terminal() {
                Ok(outcome) => outcome,
                Err(failure) => {
                    return Ok(Some(PhyRxDcMinimumTargetCompletion {
                        terminal: Err(PhyRxDcMinimumFailure::DcIq(failure)),
                        operations,
                        estimators: policy.attempts + 1,
                    }));
                }
            };
            if let PhyRxDcMinimumDecision::Complete(outcome) = policy.accept(self.request, outcome)
            {
                return Ok(Some(PhyRxDcMinimumTargetCompletion {
                    terminal: Ok(outcome),
                    operations,
                    estimators: policy.attempts,
                }));
            }
        }
    }
}

impl PhyRxDcMinimumTransition {
    pub(crate) fn terminal(&self) -> Option<Result<PhyRxDcMinimumOutcome, PhyRxDcMinimumFailure>> {
        match self.step {
            PhyRxDcMinimumStep::Complete(outcome) => Some(Ok(outcome)),
            PhyRxDcMinimumStep::Failed(failure) => Some(Err(failure)),
            _ => None,
        }
    }

    pub const fn new(request: PhyRxDcMinimumRequest) -> Self {
        Self {
            request,
            step: PhyRxDcMinimumStep::Measure(PhyDcIqEstimateTransition::new(
                Self::estimate_request(request, 0),
            )),
            attempt: 0,
            best: request.previous,
            minimum_power: 100,
            readiness_activity_edges: 0,
        }
    }

    const fn estimate_request(
        request: PhyRxDcMinimumRequest,
        attempt: u8,
    ) -> PhyDcIqEstimateRequest {
        PhyDcIqEstimateRequest {
            iteration: attempt,
            chain: 1,
            control: request.control,
            mode: request.mode,
        }
    }

    pub const fn action(&self) -> PhyRxDcMinimumAction {
        match self.step {
            PhyRxDcMinimumStep::Measure(ref transition) => {
                PhyRxDcMinimumAction::DcIq(transition.action())
            }
            PhyRxDcMinimumStep::Complete(outcome) => PhyRxDcMinimumAction::Complete(outcome),
            PhyRxDcMinimumStep::Failed(failure) => PhyRxDcMinimumAction::Failed(failure),
        }
    }

    fn accept_outcome(&mut self, outcome: PhyDcIqEstimateOutcome) {
        let mut policy = PhyRxDcMinimumPolicyState {
            attempts: self.attempt,
            best: self.best,
            minimum_power: self.minimum_power,
            readiness_activity_edges: self.readiness_activity_edges,
        };
        match policy.accept(self.request, outcome) {
            PhyRxDcMinimumDecision::Continue => {
                self.attempt = policy.attempts;
                self.best = policy.best;
                self.minimum_power = policy.minimum_power;
                self.readiness_activity_edges = policy.readiness_activity_edges;
                let attempts = policy.attempts;
                self.step = PhyRxDcMinimumStep::Measure(PhyDcIqEstimateTransition::new(
                    Self::estimate_request(self.request, attempts),
                ));
            }
            PhyRxDcMinimumDecision::Complete(outcome) => {
                self.attempt = policy.attempts;
                self.best = policy.best;
                self.minimum_power = policy.minimum_power;
                self.readiness_activity_edges = policy.readiness_activity_edges;
                self.step = PhyRxDcMinimumStep::Complete(outcome);
            }
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyRxDcMinimumCompletion,
    ) -> Result<(), PhyRxDcMinimumTransitionError> {
        match (&mut self.step, completion) {
            (
                PhyRxDcMinimumStep::Measure(transition),
                PhyRxDcMinimumCompletion::DcIq(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxDcMinimumTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyDcIqAction::Complete(outcome) => self.accept_outcome(outcome),
                    PhyDcIqAction::Failed(failure) => {
                        self.step =
                            PhyRxDcMinimumStep::Failed(PhyRxDcMinimumFailure::DcIq(failure));
                    }
                    _ => {}
                }
            }
            (PhyRxDcMinimumStep::Complete(_), _) | (PhyRxDcMinimumStep::Failed(_), _) => {
                return Err(PhyRxDcMinimumTransitionError::AlreadyComplete);
            }
        }
        Ok(())
    }
}

/// Exhaustive lowering of the non-terminal `phy_rxdc_est_min` surface.
///
/// The minimum selector owns no hardware edge of its own: every observable
/// operation belongs to the nested DC/IQ estimator. Keeping this wrapper
/// explicit prevents the cold-init executor from reaching into child state.
#[derive(Debug, Eq, PartialEq)]
pub enum PhyRxDcMinimumExternalBinding {
    DcIq(crate::calibration::estimator::PhyDcIqExternalBinding),
}

impl PhyRxDcMinimumExternalBinding {
    pub fn lower(action: PhyRxDcMinimumAction) -> Result<Self, PhyRxDcoBindingError> {
        let PhyRxDcMinimumAction::DcIq(action) = action else {
            return Err(PhyRxDcoBindingError::UnsupportedAction);
        };
        crate::calibration::estimator::PhyDcIqExternalBinding::lower(action)
            .map(Self::DcIq)
            .map_err(|_| PhyRxDcoBindingError::UnsupportedAction)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyRxDcoStep {
    PrepareControlRestore,
    ReadPbus,
    SetupPbus {
        index: u8,
    },
    ForceI,
    ForceQ,
    Delay,
    Measure {
        transition: PhyDcIqEstimateTransition,
    },
    RestoreSuccess {
        outcome: PhyRxDcoOutcome,
    },
    RestoreFailure {
        failure: PhyRxDcoFailure,
    },
    Complete(PhyRxDcoOutcome),
    Failed(PhyRxDcoFailure),
}

/// Exact fixed-input compensation helper recovered from
/// `phy_get_dco_comp(1, 0, measurement, previous, population, upper)`.
///
/// All arithmetic is explicit and stateless. `upper` is the upper byte of
/// the PBus value after shifting it right by six; its physical field name is
/// intentionally not guessed.
pub fn rx_dco_compensation_step(measurement: i32, previous: i32, population: u8, upper: u8) -> i32 {
    let upper_group = u32::from(upper >= 4);
    let shift = u32::from(population) + upper_group;
    let mut compensation = measurement >> shift;
    let delta = measurement.wrapping_sub(previous).wrapping_abs();
    let reference = measurement.wrapping_mul(3).wrapping_div(2).wrapping_abs();
    if reference < delta {
        compensation >>= 1;
    }
    if compensation == 0 {
        if measurement > 0 { 1 } else { -1 }
    } else {
        compensation
    }
}

const fn setup_transaction(index: u8, initial_i: i16, initial_q: i16) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(2, 1, initial_i as u16),
        1 => PhyPbusForceTest::new(3, 1, initial_q as u16),
        2 => PhyPbusForceTest::new(2, 2, 0x100),
        _ => PhyPbusForceTest::new(3, 2, 0x100),
    }
}

const fn threshold(population: u8) -> i32 {
    if population <= 1 {
        2
    } else if population <= 3 {
        4
    } else {
        10
    }
}

const fn initial_i(configuration: [u32; 2]) -> i16 {
    configuration[0] as u16 as i16
}

const fn initial_q(configuration: [u32; 2]) -> i16 {
    (configuration[0] >> 16) as u16 as i16
}

const fn output_configuration(configuration: [u32; 2], i: i16, q: i16) -> [u32; 2] {
    [
        (i as u16 as u32) | ((q as u16 as u32) << 16),
        configuration[1],
    ]
}

/// Caller-driven, heap-free translation of the reachable RX-DCO loop.
///
/// It has no future, waker, allocation, callback, internal delay, readiness
/// poll, or retry. Each PBus command, timer expiry, and IQ measurement is
/// accepted only through an identity-bound completion. Both success and
/// typed child failure restore the saved RX-DCO control field before becoming
/// terminal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxDcoTransition {
    request: PhyRxDcoRequest,
    step: PhyRxDcoStep,
    population: u8,
    upper: u8,
    threshold: i32,
    current_i: i16,
    current_q: i16,
    previous: PhyDcIqEstimate,
    iteration: u8,
}

impl PhyRxDcoTransition {
    pub const fn new(request: PhyRxDcoRequest) -> Self {
        Self {
            request,
            step: PhyRxDcoStep::PrepareControlRestore,
            population: 0,
            upper: 0,
            threshold: 0,
            current_i: initial_i(request.configuration),
            current_q: initial_q(request.configuration),
            previous: PhyDcIqEstimate {
                i: 0,
                q: 0,
                power: 0,
            },
            iteration: 0,
        }
    }

    const fn estimate_request(self) -> PhyDcIqEstimateRequest {
        PhyDcIqEstimateRequest {
            iteration: self.iteration,
            chain: 1,
            control: self.request.control,
            mode: 0,
        }
    }

    pub const fn action(self) -> PhyRxDcoAction {
        match self.step {
            PhyRxDcoStep::PrepareControlRestore => PhyRxDcoAction::PrepareRxDcoControlRestore,
            PhyRxDcoStep::ReadPbus => PhyRxDcoAction::ReadPbus {
                selector: 1,
                path: 2,
            },
            PhyRxDcoStep::SetupPbus { index, .. } => PhyRxDcoAction::ForcePbus(setup_transaction(
                index,
                initial_i(self.request.configuration),
                initial_q(self.request.configuration),
            )),
            PhyRxDcoStep::ForceI => {
                PhyRxDcoAction::ForcePbus(PhyPbusForceTest::new(2, 1, self.current_i as u16))
            }
            PhyRxDcoStep::ForceQ => {
                PhyRxDcoAction::ForcePbus(PhyPbusForceTest::new(3, 1, self.current_q as u16))
            }
            PhyRxDcoStep::Delay => PhyRxDcoAction::DelayMicros {
                iteration: self.iteration,
                micros: self.request.delay_micros,
            },
            PhyRxDcoStep::Measure { transition } => PhyRxDcoAction::DcIq(transition.action()),
            PhyRxDcoStep::RestoreSuccess { .. } | PhyRxDcoStep::RestoreFailure { .. } => {
                PhyRxDcoAction::RestoreRxDcoControl
            }
            PhyRxDcoStep::Complete(outcome) => PhyRxDcoAction::Complete(outcome),
            PhyRxDcoStep::Failed(failure) => PhyRxDcoAction::Failed(failure),
        }
    }

    fn pbus_failure(&mut self, transaction: PhyPbusForceTest) {
        self.step = PhyRxDcoStep::RestoreFailure {
            failure: PhyRxDcoFailure::PbusForceTimedOut(transaction),
        };
    }

    fn accept_estimate(&mut self, estimate: PhyDcIqEstimate) {
        let i_within = estimate.i.wrapping_abs() <= self.threshold;
        let q_within = estimate.q.wrapping_abs() <= self.threshold;
        let completed_iterations = self.iteration + 1;

        if i_within && q_within {
            self.step = PhyRxDcoStep::RestoreSuccess {
                outcome: PhyRxDcoOutcome {
                    configuration: output_configuration(
                        self.request.configuration,
                        self.current_i,
                        self.current_q,
                    ),
                    iterations: completed_iterations,
                    converged: true,
                    last_estimate: estimate,
                },
            };
            return;
        }

        let previous = if self.iteration == 0 {
            estimate
        } else {
            self.previous
        };
        if !i_within {
            let compensation =
                rx_dco_compensation_step(estimate.i, previous.i, self.population, self.upper);
            self.current_i = i32::from(self.current_i).wrapping_sub(compensation) as i16;
        }
        if !q_within {
            let compensation =
                rx_dco_compensation_step(estimate.q, previous.q, self.population, self.upper);
            self.current_q = i32::from(self.current_q).wrapping_sub(compensation) as i16;
        }
        self.previous = estimate;

        if completed_iterations == MAX_ITERATIONS {
            self.step = PhyRxDcoStep::RestoreSuccess {
                outcome: PhyRxDcoOutcome {
                    configuration: output_configuration(
                        self.request.configuration,
                        self.current_i,
                        self.current_q,
                    ),
                    iterations: completed_iterations,
                    converged: false,
                    last_estimate: estimate,
                },
            };
        } else {
            self.iteration = completed_iterations;
            self.step = PhyRxDcoStep::ForceI;
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyRxDcoCompletion,
    ) -> Result<(), PhyRxDcoTransitionError> {
        match (self.step, completion) {
            (
                PhyRxDcoStep::PrepareControlRestore,
                PhyRxDcoCompletion::RxDcoControlRestorePrepared,
            ) => {
                self.step = PhyRxDcoStep::ReadPbus;
            }
            (
                PhyRxDcoStep::ReadPbus,
                PhyRxDcoCompletion::PbusRead {
                    selector: 1,
                    path: 2,
                    value,
                },
            ) => {
                let low = (value & 0x3f) as u8;
                self.population = low.count_ones() as u8;
                self.upper = ((value >> 6) & 0xff) as u8;
                self.threshold = threshold(self.population);
                self.step = PhyRxDcoStep::SetupPbus { index: 0 };
            }
            (
                PhyRxDcoStep::SetupPbus { index },
                PhyRxDcoCompletion::PbusForceCompleted(transaction),
            ) if transaction
                == setup_transaction(
                    index,
                    initial_i(self.request.configuration),
                    initial_q(self.request.configuration),
                ) =>
            {
                self.step = if index == 3 {
                    PhyRxDcoStep::ForceI
                } else {
                    PhyRxDcoStep::SetupPbus { index: index + 1 }
                };
            }
            (
                PhyRxDcoStep::SetupPbus { index },
                PhyRxDcoCompletion::PbusForceTimedOut(transaction),
            ) if transaction
                == setup_transaction(
                    index,
                    initial_i(self.request.configuration),
                    initial_q(self.request.configuration),
                ) =>
            {
                self.pbus_failure(transaction);
            }
            (PhyRxDcoStep::ForceI, PhyRxDcoCompletion::PbusForceCompleted(transaction))
                if transaction == PhyPbusForceTest::new(2, 1, self.current_i as u16) =>
            {
                self.step = PhyRxDcoStep::ForceQ;
            }
            (PhyRxDcoStep::ForceI, PhyRxDcoCompletion::PbusForceTimedOut(transaction))
                if transaction == PhyPbusForceTest::new(2, 1, self.current_i as u16) =>
            {
                self.pbus_failure(transaction);
            }
            (PhyRxDcoStep::ForceQ, PhyRxDcoCompletion::PbusForceCompleted(transaction))
                if transaction == PhyPbusForceTest::new(3, 1, self.current_q as u16) =>
            {
                self.step = PhyRxDcoStep::Delay;
            }
            (PhyRxDcoStep::ForceQ, PhyRxDcoCompletion::PbusForceTimedOut(transaction))
                if transaction == PhyPbusForceTest::new(3, 1, self.current_q as u16) =>
            {
                self.pbus_failure(transaction);
            }
            (PhyRxDcoStep::Delay, PhyRxDcoCompletion::DelayElapsed { iteration, micros })
                if iteration == self.iteration && micros == self.request.delay_micros =>
            {
                self.step = PhyRxDcoStep::Measure {
                    transition: PhyDcIqEstimateTransition::new(self.estimate_request()),
                };
            }
            (PhyRxDcoStep::Measure { mut transition }, PhyRxDcoCompletion::DcIq(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxDcoTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyDcIqAction::Complete(outcome) => {
                        self.accept_estimate(outcome.estimate);
                    }
                    PhyDcIqAction::Failed(failure) => {
                        self.step = PhyRxDcoStep::RestoreFailure {
                            failure: PhyRxDcoFailure::DcIq(failure),
                        };
                    }
                    _ => {
                        self.step = PhyRxDcoStep::Measure { transition };
                    }
                }
            }
            (
                PhyRxDcoStep::RestoreSuccess { outcome },
                PhyRxDcoCompletion::RxDcoControlRestored,
            ) => {
                self.step = PhyRxDcoStep::Complete(outcome);
            }
            (
                PhyRxDcoStep::RestoreFailure { failure },
                PhyRxDcoCompletion::RxDcoControlRestored,
            ) => {
                self.step = PhyRxDcoStep::Failed(failure);
            }
            (PhyRxDcoStep::Complete(_), _) | (PhyRxDcoStep::Failed(_), _) => {
                return Err(PhyRxDcoTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyRxDcoTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcoBindingError {
    UnsupportedAction,
    Pbus(crate::analog::pbus::PhyPbusHardwareBindingError),
}

/// An RX-DCO restore-stack invariant was violated by target execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcoRestoreInvariant {
    /// Another calibration owns the shared route restore slot.
    OwnedByOtherCalibration,
    /// More than the reviewed outer-plus-inner nesting was attempted.
    NestingExceeded,
    /// Cleanup reached restore without a matching successful prepare.
    NotPending,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxDcoMmioBinding {
    action: PhyRxDcoAction,
}

impl PhyRxDcoMmioBinding {
    pub fn new(action: PhyRxDcoAction) -> Result<Self, PhyRxDcoBindingError> {
        match action {
            PhyRxDcoAction::PrepareRxDcoControlRestore
            | PhyRxDcoAction::ReadPbus { .. }
            | PhyRxDcoAction::RestoreRxDcoControl => Ok(Self { action }),
            _ => Err(PhyRxDcoBindingError::UnsupportedAction),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> Result<PhyRxDcoCompletion, PhyRxDcoRestoreInvariant> {
        match self.action {
            PhyRxDcoAction::PrepareRxDcoControlRestore => {
                oer_esp32s31_hal::phy::rx_dco::prepare_control_restore(registers).map_err(
                    |error| match error {
                        oer_esp32s31_hal::types::RxDcoControlPrepareError::RestorePending => {
                            PhyRxDcoRestoreInvariant::OwnedByOtherCalibration
                        }
                        oer_esp32s31_hal::types::RxDcoControlPrepareError::RestoreStackFull => {
                            PhyRxDcoRestoreInvariant::NestingExceeded
                        }
                    },
                )?;
                Ok(PhyRxDcoCompletion::RxDcoControlRestorePrepared)
            }
            PhyRxDcoAction::ReadPbus { selector, path } => Ok(PhyRxDcoCompletion::PbusRead {
                selector,
                path,
                value: {
                    let result =
                        oer_esp32s31_hal::phy::pbus::read_result(registers, selector, path);
                    debug_assert!(
                        result.is_some(),
                        "RX-DCO transition emitted an unrecovered PBus selector"
                    );
                    u32::from(result.unwrap_or(0))
                },
            }),
            PhyRxDcoAction::RestoreRxDcoControl => {
                oer_esp32s31_hal::phy::rx_dco::restore_control(registers)
                    .map_err(|_| PhyRxDcoRestoreInvariant::NotPending)?;
                Ok(PhyRxDcoCompletion::RxDcoControlRestored)
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxDcoPbusBinding {
    transaction: PhyPbusForceTest,
    hardware: crate::analog::pbus::PhyPbusHardwareBinding,
}

impl PhyRxDcoPbusBinding {
    pub fn new(action: PhyRxDcoAction) -> Result<Self, PhyRxDcoBindingError> {
        let PhyRxDcoAction::ForcePbus(transaction) = action else {
            return Err(PhyRxDcoBindingError::UnsupportedAction);
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

    pub fn into_completion(self) -> Result<PhyRxDcoCompletion, PhyRxDcoBindingError> {
        self.hardware
            .into_transaction()
            .map(PhyRxDcoCompletion::PbusForceCompleted)
            .map_err(PhyRxDcoBindingError::Pbus)
    }

    pub const fn into_timeout_completion(self) -> PhyRxDcoCompletion {
        PhyRxDcoCompletion::PbusForceTimedOut(self.transaction)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxDcoTimerBinding {
    iteration: u8,
    micros: u32,
}

impl PhyRxDcoTimerBinding {
    pub fn new(action: PhyRxDcoAction) -> Result<Self, PhyRxDcoBindingError> {
        match action {
            PhyRxDcoAction::DelayMicros { iteration, micros } => Ok(Self { iteration, micros }),
            _ => Err(PhyRxDcoBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyRxDcoCompletion {
        PhyRxDcoCompletion::DelayElapsed {
            iteration: self.iteration,
            micros: self.micros,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyRxDcoExternalBinding {
    Mmio(PhyRxDcoMmioBinding),
    Pbus(PhyRxDcoPbusBinding),
    Timer(PhyRxDcoTimerBinding),
    DcIq(crate::calibration::estimator::PhyDcIqExternalBinding),
}

impl PhyRxDcoExternalBinding {
    pub fn lower(action: PhyRxDcoAction) -> Result<Self, PhyRxDcoBindingError> {
        if let Ok(binding) = PhyRxDcoMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        if let Ok(binding) = PhyRxDcoPbusBinding::new(action) {
            return Ok(Self::Pbus(binding));
        }
        if let Ok(binding) = PhyRxDcoTimerBinding::new(action) {
            return Ok(Self::Timer(binding));
        }
        if let PhyRxDcoAction::DcIq(action) = action {
            return crate::calibration::estimator::PhyDcIqExternalBinding::lower(action)
                .map(Self::DcIq)
                .map_err(|_| PhyRxDcoBindingError::UnsupportedAction);
        }
        Err(PhyRxDcoBindingError::UnsupportedAction)
    }
}

#[cfg(test)]
mod tests;
