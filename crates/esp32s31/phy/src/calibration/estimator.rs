//! Event-driven ESP32-S31 DC/IQ estimator.
//!
//! Primary references in `esp32s31_rev0_rom.elf`:
//!
//! - `phy_iq_est_enable` at `0x2f82_89d4`, size `0xb4`;
//! - `phy_iq_est_disable` at `0x2f82_8a88`, size `0x2c`;
//! - `phy_dc_iq_est` at `0x2f82_8ab4`, size `0x84`;
//! - `phy_linear_to_db` at `0x2f82_6542`, size `0x7c`.
//!
//! ROM spins on a hardware-ready bit and implements both one-microsecond
//! intervals with synchronous `ets_delay_us`. This module retains the exact
//! register ordering and arithmetic as caller-driven actions. It can advance
//! only from externally delivered readiness/timer completions.

/// Split one packed DC value exactly as pinned archive `get_dc_value`.
#[inline]
#[cfg(feature = "validation-probes")]
pub fn get_dc_value(output: &mut [u16; 2], value: u32) {
    output[0] = (value >> 16) as u16;
    output[1] = value as u16;
}

const LINEAR_TO_DB_FRACTION: [u8; 16] =
    [0, 4, 8, 12, 16, 19, 22, 25, 28, 31, 34, 36, 39, 41, 44, 46];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyDcIqEstimateRequest {
    /// Parent-owned identity. ROM does not receive this field.
    pub iteration: u8,
    pub chain: u8,
    pub control: u16,
    pub mode: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyDcIqAccumulatorSnapshot {
    pub i: i32,
    pub q: i32,
    pub power: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyDcIqReadinessSnapshot {
    /// PAC `ESTIMATOR_READY_STATUS.READY`.
    pub ready: bool,
    /// Whether PAC `ESTIMATOR_ACTIVITY_STATUS.BITS_20_21_OPAQUE` is nonzero.
    pub activity: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyDcIqEstimate {
    pub i: i32,
    pub q: i32,
    pub power: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyDcIqEstimateOutcome {
    pub request: PhyDcIqEstimateRequest,
    pub estimate: PhyDcIqEstimate,
    /// Rust-owned replacement for the diagnostic halfword that ROM mutates
    /// through `phy_param_rom + 0x1ac`.
    pub readiness_activity_edges: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcIqFailure {
    ReadinessTimedOut {
        request: PhyDcIqEstimateRequest,
        readiness_activity_edges: u16,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcIqEnablePhase {
    Start,
    Measurement,
}

#[cfg(target_arch = "riscv32")]
pub(crate) fn configure_target(
    registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    control: u16,
) {
    oer_esp32s31_hal::phy::iq_estimator::configure(registers, control);
}

#[cfg(target_arch = "riscv32")]
pub(crate) fn set_enable_target(
    registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    phase: PhyDcIqEnablePhase,
    enabled: bool,
) {
    match phase {
        PhyDcIqEnablePhase::Start => {
            oer_esp32s31_hal::phy::iq_estimator::set_start_enabled(registers, enabled);
        }
        PhyDcIqEnablePhase::Measurement => {
            oer_esp32s31_hal::phy::iq_estimator::set_measurement_enabled(registers, enabled);
        }
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) fn sample_readiness_target(
    registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
) -> PhyDcIqReadinessSnapshot {
    let snapshot = oer_esp32s31_hal::phy::iq_estimator::sample_readiness(registers);
    PhyDcIqReadinessSnapshot {
        ready: snapshot.ready,
        activity: snapshot.activity,
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) fn read_accumulators_target(
    registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
) -> PhyDcIqAccumulatorSnapshot {
    let snapshot = oer_esp32s31_hal::phy::iq_estimator::read_dc_iq_accumulators(registers);
    PhyDcIqAccumulatorSnapshot {
        i: snapshot.i,
        q: snapshot.q,
        power: snapshot.power,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcIqDelayPhase {
    Start,
    Stop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcIqAction {
    Configure(PhyDcIqEstimateRequest),
    SetEnable {
        request: PhyDcIqEstimateRequest,
        phase: PhyDcIqEnablePhase,
        enabled: bool,
    },
    DelayMicros {
        request: PhyDcIqEstimateRequest,
        phase: PhyDcIqDelayPhase,
        micros: u32,
    },
    AwaitReadinessEdge {
        request: PhyDcIqEstimateRequest,
        readiness_activity_edges: u16,
        readiness_samples: u16,
    },
    ReadAccumulators(PhyDcIqEstimateRequest),
    Complete(PhyDcIqEstimateOutcome),
    Failed(PhyDcIqFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyDcIqReadinessBatch {
    request: PhyDcIqEstimateRequest,
    observations: u16,
    activity_edges: u16,
    ready: bool,
    accumulators: Option<PhyDcIqAccumulatorSnapshot>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcIqCompletion {
    Configured(PhyDcIqEstimateRequest),
    EnableSet {
        request: PhyDcIqEstimateRequest,
        phase: PhyDcIqEnablePhase,
        enabled: bool,
    },
    DelayElapsed {
        request: PhyDcIqEstimateRequest,
        phase: PhyDcIqDelayPhase,
        micros: u32,
    },
    ReadinessObserved {
        request: PhyDcIqEstimateRequest,
        snapshot: PhyDcIqReadinessSnapshot,
    },
    /// One uninterrupted, bounded execution of the ROM-shaped readiness
    /// polling loop. The private payload can only be minted by its binding.
    #[cfg_attr(
        not(any(test, feature = "validation-probes")),
        allow(dead_code, reason = "only validation bindings construct this variant")
    )]
    ReadinessBatchObserved(PhyDcIqReadinessBatch),
    ReadinessTimedOut(PhyDcIqEstimateRequest),
    AccumulatorsRead {
        request: PhyDcIqEstimateRequest,
        snapshot: PhyDcIqAccumulatorSnapshot,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcIqTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyDcIqTerminal {
    Complete(PhyDcIqEstimateOutcome),
    Failed(PhyDcIqFailure),
}

/// One complete ROM-shaped estimator transaction lowered only from its
/// initial state. The target executor keeps the register sequence, both
/// one-microsecond settles and the readiness loop inside this narrow value;
/// the state transition is committed once after hardware cleanup.
#[derive(Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct PhyDcIqTargetTransaction {
    request: PhyDcIqEstimateRequest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct PhyDcIqTargetCompletion {
    request: PhyDcIqEstimateRequest,
    terminal: PhyDcIqTerminal,
    operations: u16,
}

#[cfg(any(target_arch = "riscv32", test))]
impl PhyDcIqTargetCompletion {
    pub(crate) const fn operations(&self) -> u16 {
        self.operations
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn into_terminal(self) -> Result<PhyDcIqEstimateOutcome, PhyDcIqFailure> {
        match self.terminal {
            PhyDcIqTerminal::Complete(outcome) => Ok(outcome),
            PhyDcIqTerminal::Failed(failure) => Err(failure),
        }
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl PhyDcIqTargetTransaction {
    pub(crate) const fn new(request: PhyDcIqEstimateRequest) -> Self {
        Self { request }
    }

    /// Execute the exact complete `phy_dc_iq_est` hardware envelope.
    ///
    /// The ready register and three accumulators remain adjacent, as in ROM.
    /// A finite readiness bound is the sole intentional safety extension. A
    /// timeout still executes measurement-disable, settle and start-disable
    /// before returning its typed failure completion.
    #[cfg(any(target_arch = "riscv32", test))]
    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    pub(crate) fn execute_with<C, E>(
        self,
        maximum_samples: u16,
        context: &mut C,
        mut configure: impl FnMut(&mut C, u16),
        mut set_enable: impl FnMut(&mut C, PhyDcIqEnablePhase, bool),
        mut settle: impl FnMut(&mut C, u32) -> Result<(), E>,
        mut read: impl FnMut(&mut C) -> PhyDcIqReadinessSnapshot,
        mut read_accumulators: impl FnMut(&mut C) -> PhyDcIqAccumulatorSnapshot,
    ) -> Result<PhyDcIqTargetCompletion, E> {
        configure(context, self.request.control);
        set_enable(context, PhyDcIqEnablePhase::Start, true);
        settle(context, 1)?;
        set_enable(context, PhyDcIqEnablePhase::Measurement, true);

        let mut observations = 0_u16;
        let mut activity_edges = 0_u16;
        let terminal = loop {
            if observations == maximum_samples {
                break PhyDcIqTerminal::Failed(PhyDcIqFailure::ReadinessTimedOut {
                    request: self.request,
                    readiness_activity_edges: activity_edges,
                });
            }
            let snapshot = read(context);
            observations += 1;
            if snapshot.ready {
                let accumulators = read_accumulators(context);
                break PhyDcIqTerminal::Complete(PhyDcIqEstimateOutcome {
                    request: self.request,
                    estimate: calculate_dc_iq_estimate(self.request, accumulators),
                    readiness_activity_edges: activity_edges,
                });
            }
            if snapshot.activity {
                activity_edges = activity_edges.wrapping_add(1);
            }
        };

        set_enable(context, PhyDcIqEnablePhase::Measurement, false);
        settle(context, 1)?;
        set_enable(context, PhyDcIqEnablePhase::Start, false);
        let accumulator_read = u16::from(matches!(terminal, PhyDcIqTerminal::Complete(_)));
        Ok(PhyDcIqTargetCompletion {
            request: self.request,
            terminal,
            // Configure, start-enable, first settle, measurement-enable,
            // readiness reads, optional accumulator read, measurement-disable,
            // second settle and start-disable.
            operations: observations
                .saturating_add(accumulator_read)
                .saturating_add(7),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyDcIqStep {
    Configure,
    EnableStart,
    StartDelay,
    EnableMeasurement,
    AwaitReadiness,
    ReadAccumulators,
    DisableMeasurement(PhyDcIqTerminal),
    StopDelay(PhyDcIqTerminal),
    DisableStart(PhyDcIqTerminal),
    Complete(PhyDcIqEstimateOutcome),
    Failed(PhyDcIqFailure),
}

/// Exact stateless translation of rev0 ROM `phy_linear_to_db`.
///
/// The lookup table is the sixteen-byte image at ROM address `0x2f84_832c`.
/// Shift counts retain RISC-V's low-five-bit behavior.
pub fn phy_linear_to_db(value: i32, scale: u8) -> i32 {
    let scaled = if scale <= 2 {
        value.wrapping_shl(u32::from(3 - scale))
    } else {
        value >> (u32::from(scale - 3) & 0x1f)
    };
    let exponent = (28_i32.wrapping_sub((scaled as u32).leading_zeros() as i32)) as i8 as i32;
    let (exponent, fraction) = if exponent > 0 {
        (
            exponent,
            ((scaled >> ((exponent - 1) as u32)) & 0x0f) as usize,
        )
    } else {
        (0, (scaled & 0x0f) as usize)
    };
    let result = (exponent as u16)
        .wrapping_mul(48)
        .wrapping_add(u16::from(LINEAR_TO_DB_FRACTION[fraction]));
    result as i16 as i32
}

/// Convert the three exact accumulator words read after the ready edge.
pub fn calculate_dc_iq_estimate(
    request: PhyDcIqEstimateRequest,
    snapshot: PhyDcIqAccumulatorSnapshot,
) -> PhyDcIqEstimate {
    let shift = if request.mode == 0 { 6 } else { 4 };
    let divisor = i32::from(request.control) + 1;
    let i = (snapshot.i >> shift) / divisor;
    let q = (snapshot.q >> shift) / divisor;
    let squares = i.wrapping_mul(i).wrapping_add(q.wrapping_mul(q));
    let squares = if request.mode == 0 {
        squares
    } else {
        squares >> 4
    };
    let linear = (snapshot.power / divisor)
        .wrapping_shl(3)
        .wrapping_sub(squares);
    let linear = if linear < 0 { 0 } else { linear };
    let power = phy_linear_to_db(linear, 0).wrapping_add(8) >> 4;
    PhyDcIqEstimate { i, q, power }
}

/// Heap-free, externally driven replacement for the complete DC/IQ estimator.
///
/// A false readiness observation represents an independently delivered
/// hardware edge; it never schedules or performs another observation. The
/// owner can instead deliver `ReadinessTimedOut`, after which the transition
/// executes the same disable/one-microsecond/disable tail before returning a
/// typed failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyDcIqEstimateTransition {
    request: PhyDcIqEstimateRequest,
    readiness_activity_edges: u16,
    readiness_samples: u16,
    step: PhyDcIqStep,
}

impl PhyDcIqEstimateTransition {
    pub const fn new(request: PhyDcIqEstimateRequest) -> Self {
        Self {
            request,
            readiness_activity_edges: 0,
            readiness_samples: 0,
            step: PhyDcIqStep::Configure,
        }
    }

    #[cfg(test)]
    pub(crate) fn lower_target_transaction(
        &self,
    ) -> Result<PhyDcIqTargetTransaction, PhyDcIqBindingError> {
        if !matches!(self.step, PhyDcIqStep::Configure) {
            return Err(PhyDcIqBindingError::UnsupportedAction);
        }
        Ok(PhyDcIqTargetTransaction::new(self.request))
    }

    #[cfg(test)]
    pub(crate) fn advance_target_transaction(
        &mut self,
        completion: PhyDcIqTargetCompletion,
    ) -> Result<(), PhyDcIqTransitionError> {
        if !matches!(self.step, PhyDcIqStep::Configure) {
            return Err(PhyDcIqTransitionError::WrongCompletion);
        }
        if completion.request != self.request {
            return Err(PhyDcIqTransitionError::WrongCompletion);
        }
        self.step = match completion.terminal {
            PhyDcIqTerminal::Complete(outcome) => PhyDcIqStep::Complete(outcome),
            PhyDcIqTerminal::Failed(failure) => PhyDcIqStep::Failed(failure),
        };
        Ok(())
    }

    pub const fn action(&self) -> PhyDcIqAction {
        match self.step {
            PhyDcIqStep::Configure => PhyDcIqAction::Configure(self.request),
            PhyDcIqStep::EnableStart => PhyDcIqAction::SetEnable {
                request: self.request,
                phase: PhyDcIqEnablePhase::Start,
                enabled: true,
            },
            PhyDcIqStep::StartDelay => PhyDcIqAction::DelayMicros {
                request: self.request,
                phase: PhyDcIqDelayPhase::Start,
                micros: 1,
            },
            PhyDcIqStep::EnableMeasurement => PhyDcIqAction::SetEnable {
                request: self.request,
                phase: PhyDcIqEnablePhase::Measurement,
                enabled: true,
            },
            PhyDcIqStep::AwaitReadiness => PhyDcIqAction::AwaitReadinessEdge {
                request: self.request,
                readiness_activity_edges: self.readiness_activity_edges,
                readiness_samples: self.readiness_samples,
            },
            PhyDcIqStep::ReadAccumulators => PhyDcIqAction::ReadAccumulators(self.request),
            PhyDcIqStep::DisableMeasurement(_) => PhyDcIqAction::SetEnable {
                request: self.request,
                phase: PhyDcIqEnablePhase::Measurement,
                enabled: false,
            },
            PhyDcIqStep::StopDelay(_) => PhyDcIqAction::DelayMicros {
                request: self.request,
                phase: PhyDcIqDelayPhase::Stop,
                micros: 1,
            },
            PhyDcIqStep::DisableStart(_) => PhyDcIqAction::SetEnable {
                request: self.request,
                phase: PhyDcIqEnablePhase::Start,
                enabled: false,
            },
            PhyDcIqStep::Complete(outcome) => PhyDcIqAction::Complete(outcome),
            PhyDcIqStep::Failed(failure) => PhyDcIqAction::Failed(failure),
        }
    }

    pub fn advance(&mut self, completion: PhyDcIqCompletion) -> Result<(), PhyDcIqTransitionError> {
        self.step = match (self.step, completion) {
            (PhyDcIqStep::Configure, PhyDcIqCompletion::Configured(request))
                if request == self.request =>
            {
                PhyDcIqStep::EnableStart
            }
            (
                PhyDcIqStep::EnableStart,
                PhyDcIqCompletion::EnableSet {
                    request,
                    phase: PhyDcIqEnablePhase::Start,
                    enabled: true,
                },
            ) if request == self.request => PhyDcIqStep::StartDelay,
            (
                PhyDcIqStep::StartDelay,
                PhyDcIqCompletion::DelayElapsed {
                    request,
                    phase: PhyDcIqDelayPhase::Start,
                    micros: 1,
                },
            ) if request == self.request => PhyDcIqStep::EnableMeasurement,
            (
                PhyDcIqStep::EnableMeasurement,
                PhyDcIqCompletion::EnableSet {
                    request,
                    phase: PhyDcIqEnablePhase::Measurement,
                    enabled: true,
                },
            ) if request == self.request => PhyDcIqStep::AwaitReadiness,
            (
                PhyDcIqStep::AwaitReadiness,
                PhyDcIqCompletion::ReadinessObserved {
                    request,
                    snapshot: PhyDcIqReadinessSnapshot { ready: true, .. },
                },
            ) if request == self.request => {
                self.readiness_samples = self.readiness_samples.saturating_add(1);
                PhyDcIqStep::ReadAccumulators
            }
            (
                PhyDcIqStep::AwaitReadiness,
                PhyDcIqCompletion::ReadinessObserved {
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
                PhyDcIqStep::AwaitReadiness
            }
            (PhyDcIqStep::AwaitReadiness, PhyDcIqCompletion::ReadinessBatchObserved(batch))
                if batch.request == self.request
                    && batch.observations != 0
                    && batch.ready == batch.accumulators.is_some() =>
            {
                self.readiness_samples = self.readiness_samples.saturating_add(batch.observations);
                self.readiness_activity_edges = self
                    .readiness_activity_edges
                    .wrapping_add(batch.activity_edges);
                match batch.accumulators {
                    Some(snapshot) => PhyDcIqStep::DisableMeasurement(PhyDcIqTerminal::Complete(
                        PhyDcIqEstimateOutcome {
                            request: self.request,
                            estimate: calculate_dc_iq_estimate(self.request, snapshot),
                            readiness_activity_edges: self.readiness_activity_edges,
                        },
                    )),
                    None => PhyDcIqStep::AwaitReadiness,
                }
            }
            (PhyDcIqStep::AwaitReadiness, PhyDcIqCompletion::ReadinessTimedOut(request))
                if request == self.request =>
            {
                PhyDcIqStep::DisableMeasurement(PhyDcIqTerminal::Failed(
                    PhyDcIqFailure::ReadinessTimedOut {
                        request,
                        readiness_activity_edges: self.readiness_activity_edges,
                    },
                ))
            }
            (
                PhyDcIqStep::ReadAccumulators,
                PhyDcIqCompletion::AccumulatorsRead { request, snapshot },
            ) if request == self.request => {
                PhyDcIqStep::DisableMeasurement(PhyDcIqTerminal::Complete(PhyDcIqEstimateOutcome {
                    request,
                    estimate: calculate_dc_iq_estimate(request, snapshot),
                    readiness_activity_edges: self.readiness_activity_edges,
                }))
            }
            (
                PhyDcIqStep::DisableMeasurement(terminal),
                PhyDcIqCompletion::EnableSet {
                    request,
                    phase: PhyDcIqEnablePhase::Measurement,
                    enabled: false,
                },
            ) if request == self.request => PhyDcIqStep::StopDelay(terminal),
            (
                PhyDcIqStep::StopDelay(terminal),
                PhyDcIqCompletion::DelayElapsed {
                    request,
                    phase: PhyDcIqDelayPhase::Stop,
                    micros: 1,
                },
            ) if request == self.request => PhyDcIqStep::DisableStart(terminal),
            (
                PhyDcIqStep::DisableStart(terminal),
                PhyDcIqCompletion::EnableSet {
                    request,
                    phase: PhyDcIqEnablePhase::Start,
                    enabled: false,
                },
            ) if request == self.request => match terminal {
                PhyDcIqTerminal::Complete(outcome) => PhyDcIqStep::Complete(outcome),
                PhyDcIqTerminal::Failed(failure) => PhyDcIqStep::Failed(failure),
            },
            (PhyDcIqStep::Complete(_), _) | (PhyDcIqStep::Failed(_), _) => {
                return Err(PhyDcIqTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyDcIqTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcIqBindingError {
    UnsupportedAction,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyDcIqMmioBinding {
    action: PhyDcIqAction,
}

impl PhyDcIqMmioBinding {
    pub fn new(action: PhyDcIqAction) -> Result<Self, PhyDcIqBindingError> {
        match action {
            PhyDcIqAction::Configure(_)
            | PhyDcIqAction::SetEnable { .. }
            | PhyDcIqAction::ReadAccumulators(_) => Ok(Self { action }),
            _ => Err(PhyDcIqBindingError::UnsupportedAction),
        }
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn action(&self) -> PhyDcIqAction {
        self.action
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> PhyDcIqCompletion {
        match self.action {
            PhyDcIqAction::Configure(request) => {
                configure_target(registers, request.control);
                PhyDcIqCompletion::Configured(request)
            }
            PhyDcIqAction::SetEnable {
                request,
                phase,
                enabled,
            } => {
                set_enable_target(registers, phase, enabled);
                PhyDcIqCompletion::EnableSet {
                    request,
                    phase,
                    enabled,
                }
            }
            PhyDcIqAction::ReadAccumulators(request) => PhyDcIqCompletion::AccumulatorsRead {
                request,
                snapshot: read_accumulators_target(registers),
            },
            _ => unreachable!(),
        }
    }
}

/// Non-cloneable async boundary for one scheduled readiness observation.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyDcIqReadinessBinding {
    action: PhyDcIqAction,
}

impl PhyDcIqReadinessBinding {
    pub fn new(action: PhyDcIqAction) -> Result<Self, PhyDcIqBindingError> {
        match action {
            PhyDcIqAction::AwaitReadinessEdge { .. } => Ok(Self { action }),
            _ => Err(PhyDcIqBindingError::UnsupportedAction),
        }
    }

    pub const fn samples(&self) -> u16 {
        match self.action {
            PhyDcIqAction::AwaitReadinessEdge {
                readiness_samples, ..
            } => readiness_samples,
            _ => unreachable!(),
        }
    }

    /// Rev0 ROM phy_iq_est_enable reads ready immediately after enabling the
    /// measurement (0x2f828a54 -> 0x2f828a5c). Its 1-us start settle precedes
    /// that enable. The retry loop also samples directly, without a delay.
    /// The finite sample bound is retained; it is not an elapsed-time budget.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn observe_with(
        self,
        maximum_samples: u16,
        read: impl FnOnce() -> PhyDcIqReadinessSnapshot,
    ) -> PhyDcIqCompletion {
        if self.samples() >= maximum_samples {
            return self.into_timeout_completion();
        }
        let PhyDcIqAction::AwaitReadinessEdge { request, .. } = self.action else {
            unreachable!();
        };
        PhyDcIqCompletion::ReadinessObserved {
            request,
            snapshot: read(),
        }
    }

    /// Execute the complete ROM-shaped direct polling loop behind one typed
    /// binding. `maximum_samples` is a total per-estimator bound, so callers
    /// can reduce it to preserve an enclosing operation budget. The returned
    /// charge includes the accumulator-read edge when readiness is observed.
    #[cfg(test)]
    pub(crate) fn observe_until_ready_with<C>(
        self,
        maximum_samples: u16,
        context: &mut C,
        mut read: impl FnMut(&mut C) -> PhyDcIqReadinessSnapshot,
        mut read_accumulators: impl FnMut(&mut C) -> PhyDcIqAccumulatorSnapshot,
    ) -> (PhyDcIqCompletion, u16) {
        let initial_samples = self.samples();
        if initial_samples >= maximum_samples {
            return (self.into_timeout_completion(), 0);
        }
        let PhyDcIqAction::AwaitReadinessEdge { request, .. } = self.action else {
            unreachable!();
        };
        let allowance = maximum_samples - initial_samples;
        let mut remaining = allowance;
        let mut activity_edges = 0_u16;
        loop {
            let snapshot = read(context);
            remaining -= 1;
            if snapshot.ready {
                let observations = allowance - remaining;
                let batch = PhyDcIqReadinessBatch {
                    request,
                    observations,
                    activity_edges,
                    ready: true,
                    // ROM reads all three accumulators immediately after the
                    // ready-return epilogue. Keep that temporal boundary in
                    // the same typed hardware transaction.
                    accumulators: Some(read_accumulators(context)),
                };
                return (
                    PhyDcIqCompletion::ReadinessBatchObserved(batch),
                    observations.saturating_add(1),
                );
            }
            if snapshot.activity {
                activity_edges = activity_edges.wrapping_add(1);
            }
            if remaining == 0 {
                break;
            }
        }
        let batch = PhyDcIqReadinessBatch {
            request,
            observations: allowance,
            activity_edges,
            ready: false,
            accumulators: None,
        };
        (PhyDcIqCompletion::ReadinessBatchObserved(batch), allowance)
    }

    #[cfg(target_arch = "riscv32")]
    #[cfg(any(test, feature = "validation-probes"))]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> PhyDcIqCompletion {
        let PhyDcIqAction::AwaitReadinessEdge { request, .. } = self.action else {
            unreachable!();
        };
        PhyDcIqCompletion::ReadinessObserved {
            request,
            snapshot: sample_readiness_target(registers),
        }
    }

    pub fn into_timeout_completion(self) -> PhyDcIqCompletion {
        let PhyDcIqAction::AwaitReadinessEdge { request, .. } = self.action else {
            unreachable!();
        };
        PhyDcIqCompletion::ReadinessTimedOut(request)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyDcIqTimerBinding {
    request: PhyDcIqEstimateRequest,
    phase: PhyDcIqDelayPhase,
    micros: u32,
}

impl PhyDcIqTimerBinding {
    pub fn new(action: PhyDcIqAction) -> Result<Self, PhyDcIqBindingError> {
        match action {
            PhyDcIqAction::DelayMicros {
                request,
                phase,
                micros,
            } => Ok(Self {
                request,
                phase,
                micros,
            }),
            _ => Err(PhyDcIqBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyDcIqCompletion {
        PhyDcIqCompletion::DelayElapsed {
            request: self.request,
            phase: self.phase,
            micros: self.micros,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyDcIqExternalBinding {
    Mmio(PhyDcIqMmioBinding),
    Timer(PhyDcIqTimerBinding),
    Readiness(PhyDcIqReadinessBinding),
}

impl PhyDcIqExternalBinding {
    pub fn lower(action: PhyDcIqAction) -> Result<Self, PhyDcIqBindingError> {
        if let Ok(binding) = PhyDcIqReadinessBinding::new(action) {
            return Ok(Self::Readiness(binding));
        }
        if let Ok(binding) = PhyDcIqMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        if let Ok(binding) = PhyDcIqTimerBinding::new(action) {
            return Ok(Self::Timer(binding));
        }
        Err(PhyDcIqBindingError::UnsupportedAction)
    }
}

#[cfg(test)]
mod tests;
