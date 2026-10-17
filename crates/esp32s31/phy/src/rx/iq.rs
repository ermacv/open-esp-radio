//! Rust-owned ESP32-S31 receive-IQ calibration.
//!
//! The mandatory archive root is `phy_rxiq_cal_init`, size 408 bytes in the
//! pinned `libphy.a[phy_rx_gain.o]`. Its reachable ROM calibration graph is
//! represented here as finite, externally completed transitions. Diagnostic
//! `ets_printf` branches are intentionally absent: their controlling
//! arguments are zero in the pinned `phy_bb_init` call and they have no radio
//! state effect.
//!
//! A hardware-ready observation never causes this module to poll itself.
//! Each observation is an identity-bound external edge, and an owner-supplied
//! deadline can enter the complete estimator cleanup path.

use crate::{
    analog::{
        i2c::{PhyI2cAddress, PhyI2cField, analog_registers},
        pbus::PhyPbusForceTest,
        rfpll::{
            RfpllFrequencyAction, RfpllFrequencyCompletion, RfpllFrequencyFailure,
            RfpllFrequencyRequest, RfpllFrequencyTransition,
        },
    },
    calibration::estimator::{PhyDcIqDelayPhase, PhyDcIqEnablePhase, PhyDcIqReadinessSnapshot},
    rx::dc_offset::{
        PhyRxDcoAction, PhyRxDcoCompletion, PhyRxDcoFailure, PhyRxDcoRequest, PhyRxDcoTransition,
    },
    tx::iq::{
        PhyTxIqCoefficientKind, PhyTxIqLoopbackAction, PhyTxIqLoopbackCompletion,
        PhyTxIqLoopbackTransition,
    },
};

const EXTERNAL_DCODE_0: PhyI2cAddress = analog_registers::RFPLL_EXTERNAL_DCODE_0.address();
const EXTERNAL_DCODE_1: PhyI2cAddress = analog_registers::RFPLL_EXTERNAL_DCODE_1.address();

const fn channel_to_frequency(channel: u16) -> u16 {
    if channel > 14 {
        channel
    } else if channel == 14 {
        2484
    } else {
        2407_u16.wrapping_add(channel.wrapping_mul(5))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxIqMismatchSnapshot {
    pub sum_i: i32,
    pub difference_i: i32,
    pub difference_q: i32,
    pub sum_q: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqEstimatorKind {
    TotalPower,
    Mismatch { exponent: u8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxIqEstimatorRequest {
    pub identity: u8,
    pub control: u16,
    pub kind: PhyRxIqEstimatorKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqMeasurement {
    TotalPower(i32),
    Mismatch([i8; 2]),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxIqEstimatorOutcome {
    pub request: PhyRxIqEstimatorRequest,
    pub measurement: PhyRxIqMeasurement,
    pub readiness_activity_edges: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxIqEstimatorFailure {
    pub request: PhyRxIqEstimatorRequest,
    pub readiness_activity_edges: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqEstimatorAction {
    Configure(PhyRxIqEstimatorRequest),
    SetEnable {
        request: PhyRxIqEstimatorRequest,
        phase: PhyDcIqEnablePhase,
        enabled: bool,
    },
    DelayMicros {
        request: PhyRxIqEstimatorRequest,
        phase: PhyDcIqDelayPhase,
        micros: u32,
    },
    AwaitReadinessEdge {
        request: PhyRxIqEstimatorRequest,
        readiness_activity_edges: u16,
        readiness_samples: u16,
    },
    ReadTotalPower(PhyRxIqEstimatorRequest),
    ReadMismatch(PhyRxIqEstimatorRequest),
    Complete(PhyRxIqEstimatorOutcome),
    Failed(PhyRxIqEstimatorFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqEstimatorCompletion {
    Configured(PhyRxIqEstimatorRequest),
    EnableSet {
        request: PhyRxIqEstimatorRequest,
        phase: PhyDcIqEnablePhase,
        enabled: bool,
    },
    DelayElapsed {
        request: PhyRxIqEstimatorRequest,
        phase: PhyDcIqDelayPhase,
        micros: u32,
    },
    ReadinessObserved {
        request: PhyRxIqEstimatorRequest,
        snapshot: PhyDcIqReadinessSnapshot,
    },
    ReadinessTimedOut(PhyRxIqEstimatorRequest),
    TotalPowerRead {
        request: PhyRxIqEstimatorRequest,
        value: i32,
    },
    MismatchRead {
        request: PhyRxIqEstimatorRequest,
        snapshot: PhyRxIqMismatchSnapshot,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqEstimatorTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EstimatorTerminal {
    Complete(PhyRxIqMeasurement),
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EstimatorStep {
    Configure,
    EnableStart,
    StartDelay,
    EnableMeasurement,
    AwaitReadiness,
    Read,
    DisableMeasurement(EstimatorTerminal),
    StopDelay(EstimatorTerminal),
    DisableStart(EstimatorTerminal),
    Complete(PhyRxIqMeasurement),
    Failed,
}

/// Exact signed arithmetic from ROM `phy_rxiq_get_mis`.
pub fn rxiq_mismatch(exponent: u8, snapshot: PhyRxIqMismatchSnapshot) -> [i8; 2] {
    let shift = u32::from(exponent.wrapping_sub(2) & 0x1f);
    let sum_i = snapshot.sum_i >> shift;
    let difference_i = snapshot.difference_i >> shift;
    let difference_q = snapshot.difference_q >> shift;
    let sum_q = snapshot.sum_q >> shift;

    let real = sum_i.wrapping_add(sum_q);
    let imaginary = difference_i.wrapping_sub(difference_q);
    let conjugate_real = sum_i.wrapping_sub(sum_q);
    let conjugate_imaginary = difference_q.wrapping_add(difference_i);

    let mut denominator = i64::from(real)
        .wrapping_mul(i64::from(real))
        .wrapping_add(i64::from(imaginary).wrapping_mul(i64::from(imaginary)));
    if denominator == 0 {
        denominator = 1;
    }
    let gain_numerator = i64::from(real)
        .wrapping_mul(i64::from(conjugate_real))
        .wrapping_sub(i64::from(imaginary).wrapping_mul(i64::from(conjugate_imaginary)))
        .wrapping_mul(0x200);
    let phase_numerator = i64::from(real)
        .wrapping_mul(i64::from(conjugate_imaginary))
        .wrapping_add(i64::from(imaginary).wrapping_mul(i64::from(conjugate_real)))
        .wrapping_mul(0x400);

    let gain = (gain_numerator / denominator) as i8;
    let phase = (phase_numerator / denominator) as i8;
    [
        ((i16::from(gain) + 1) >> 1) as i8,
        ((i16::from(phase) + 1) >> 1) as i8,
    ]
}

/// Event-driven expansion of `phy_iq_est_enable`/read/`phy_iq_est_disable`
/// for the two RXIQ consumers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxIqEstimatorTransition {
    request: PhyRxIqEstimatorRequest,
    step: EstimatorStep,
    readiness_activity_edges: u16,
    readiness_samples: u16,
}

impl PhyRxIqEstimatorTransition {
    pub const fn new(request: PhyRxIqEstimatorRequest) -> Self {
        Self {
            request,
            step: EstimatorStep::Configure,
            readiness_activity_edges: 0,
            readiness_samples: 0,
        }
    }

    pub const fn action(self) -> PhyRxIqEstimatorAction {
        match self.step {
            EstimatorStep::Configure => PhyRxIqEstimatorAction::Configure(self.request),
            EstimatorStep::EnableStart => PhyRxIqEstimatorAction::SetEnable {
                request: self.request,
                phase: PhyDcIqEnablePhase::Start,
                enabled: true,
            },
            EstimatorStep::StartDelay => PhyRxIqEstimatorAction::DelayMicros {
                request: self.request,
                phase: PhyDcIqDelayPhase::Start,
                micros: 1,
            },
            EstimatorStep::EnableMeasurement => PhyRxIqEstimatorAction::SetEnable {
                request: self.request,
                phase: PhyDcIqEnablePhase::Measurement,
                enabled: true,
            },
            EstimatorStep::AwaitReadiness => PhyRxIqEstimatorAction::AwaitReadinessEdge {
                request: self.request,
                readiness_activity_edges: self.readiness_activity_edges,
                readiness_samples: self.readiness_samples,
            },
            EstimatorStep::Read => match self.request.kind {
                PhyRxIqEstimatorKind::TotalPower => {
                    PhyRxIqEstimatorAction::ReadTotalPower(self.request)
                }
                PhyRxIqEstimatorKind::Mismatch { .. } => {
                    PhyRxIqEstimatorAction::ReadMismatch(self.request)
                }
            },
            EstimatorStep::DisableMeasurement(_) => PhyRxIqEstimatorAction::SetEnable {
                request: self.request,
                phase: PhyDcIqEnablePhase::Measurement,
                enabled: false,
            },
            EstimatorStep::StopDelay(_) => PhyRxIqEstimatorAction::DelayMicros {
                request: self.request,
                phase: PhyDcIqDelayPhase::Stop,
                micros: 1,
            },
            EstimatorStep::DisableStart(_) => PhyRxIqEstimatorAction::SetEnable {
                request: self.request,
                phase: PhyDcIqEnablePhase::Start,
                enabled: false,
            },
            EstimatorStep::Complete(measurement) => {
                PhyRxIqEstimatorAction::Complete(PhyRxIqEstimatorOutcome {
                    request: self.request,
                    measurement,
                    readiness_activity_edges: self.readiness_activity_edges,
                })
            }
            EstimatorStep::Failed => PhyRxIqEstimatorAction::Failed(PhyRxIqEstimatorFailure {
                request: self.request,
                readiness_activity_edges: self.readiness_activity_edges,
            }),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyRxIqEstimatorCompletion,
    ) -> Result<(), PhyRxIqEstimatorTransitionError> {
        self.step = match (self.step, completion) {
            (EstimatorStep::Configure, PhyRxIqEstimatorCompletion::Configured(request))
                if request == self.request =>
            {
                EstimatorStep::EnableStart
            }
            (
                EstimatorStep::EnableStart,
                PhyRxIqEstimatorCompletion::EnableSet {
                    request,
                    phase: PhyDcIqEnablePhase::Start,
                    enabled: true,
                },
            ) if request == self.request => EstimatorStep::StartDelay,
            (
                EstimatorStep::StartDelay,
                PhyRxIqEstimatorCompletion::DelayElapsed {
                    request,
                    phase: PhyDcIqDelayPhase::Start,
                    micros: 1,
                },
            ) if request == self.request => EstimatorStep::EnableMeasurement,
            (
                EstimatorStep::EnableMeasurement,
                PhyRxIqEstimatorCompletion::EnableSet {
                    request,
                    phase: PhyDcIqEnablePhase::Measurement,
                    enabled: true,
                },
            ) if request == self.request => EstimatorStep::AwaitReadiness,
            (
                EstimatorStep::AwaitReadiness,
                PhyRxIqEstimatorCompletion::ReadinessObserved {
                    request,
                    snapshot: PhyDcIqReadinessSnapshot { ready: true, .. },
                },
            ) if request == self.request => {
                self.readiness_samples = self.readiness_samples.saturating_add(1);
                EstimatorStep::Read
            }
            (
                EstimatorStep::AwaitReadiness,
                PhyRxIqEstimatorCompletion::ReadinessObserved {
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
                EstimatorStep::AwaitReadiness
            }
            (
                EstimatorStep::AwaitReadiness,
                PhyRxIqEstimatorCompletion::ReadinessTimedOut(request),
            ) if request == self.request => {
                EstimatorStep::DisableMeasurement(EstimatorTerminal::Failed)
            }
            (
                EstimatorStep::Read,
                PhyRxIqEstimatorCompletion::TotalPowerRead { request, value },
            ) if request == self.request
                && self.request.kind == PhyRxIqEstimatorKind::TotalPower =>
            {
                EstimatorStep::DisableMeasurement(EstimatorTerminal::Complete(
                    PhyRxIqMeasurement::TotalPower(value >> 7),
                ))
            }
            (
                EstimatorStep::Read,
                PhyRxIqEstimatorCompletion::MismatchRead { request, snapshot },
            ) if request == self.request => match self.request.kind {
                PhyRxIqEstimatorKind::Mismatch { exponent } => {
                    EstimatorStep::DisableMeasurement(EstimatorTerminal::Complete(
                        PhyRxIqMeasurement::Mismatch(rxiq_mismatch(exponent, snapshot)),
                    ))
                }
                PhyRxIqEstimatorKind::TotalPower => {
                    return Err(PhyRxIqEstimatorTransitionError::WrongCompletion);
                }
            },
            (
                EstimatorStep::DisableMeasurement(terminal),
                PhyRxIqEstimatorCompletion::EnableSet {
                    request,
                    phase: PhyDcIqEnablePhase::Measurement,
                    enabled: false,
                },
            ) if request == self.request => EstimatorStep::StopDelay(terminal),
            (
                EstimatorStep::StopDelay(terminal),
                PhyRxIqEstimatorCompletion::DelayElapsed {
                    request,
                    phase: PhyDcIqDelayPhase::Stop,
                    micros: 1,
                },
            ) if request == self.request => EstimatorStep::DisableStart(terminal),
            (
                EstimatorStep::DisableStart(terminal),
                PhyRxIqEstimatorCompletion::EnableSet {
                    request,
                    phase: PhyDcIqEnablePhase::Start,
                    enabled: false,
                },
            ) if request == self.request => match terminal {
                EstimatorTerminal::Complete(measurement) => EstimatorStep::Complete(measurement),
                EstimatorTerminal::Failed => EstimatorStep::Failed,
            },
            (EstimatorStep::Complete(_) | EstimatorStep::Failed, _) => {
                return Err(PhyRxIqEstimatorTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyRxIqEstimatorTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxIqAdjustedTxParameters {
    pub coefficient: u16,
    pub current_channel: u16,
    pub current_temperature: u16,
    pub calibration_temperature: u16,
    pub calibration_dcode: [u8; 2],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxIqAdjustedTxOutcome {
    pub coefficient: [i8; 2],
    pub external_dcode: bool,
    pub observed_dcode: [u8; 2],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqAdjustedTxAction {
    ReadI2cMasked { field: PhyI2cField },
    Complete(PhyRxIqAdjustedTxOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqAdjustedTxCompletion {
    I2cMaskedRead { field: PhyI2cField, value: u8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqAdjustedTxTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AdjustedTxStep {
    ReadMode,
    ReadFirst { external: bool },
    ReadSecond { external: bool },
    Complete,
}

fn decode_txiq_coefficient(value: u16) -> [i8; 2] {
    let gain = (((value >> 7) & 0x3f) as u8) as i8;
    let gain = if gain & 0x20 != 0 {
        gain.wrapping_sub(0x40)
    } else {
        gain
    };
    let phase = (value as u8 & 0x7f) as i8;
    let phase = if phase & 0x40 != 0 {
        phase.wrapping_sub(0x80_u8 as i8)
    } else {
        phase
    };
    [gain, phase]
}

/// Exact non-I/O arithmetic from `phy_get_txiq_set`.
pub fn adjusted_txiq_coefficient(
    parameters: PhyRxIqAdjustedTxParameters,
    observed_dcode: [u8; 2],
) -> [i8; 2] {
    let mut coefficient = decode_txiq_coefficient(parameters.coefficient);
    let temperature_delta = parameters
        .current_temperature
        .wrapping_sub(parameters.calibration_temperature) as i16;
    let absolute_temperature =
        crate::calibration::math::absolute_temperature(i32::from(temperature_delta)) as i32;
    let temperature_correction = if absolute_temperature < 20 {
        temperature_delta / -3
    } else {
        ((i32::from(temperature_delta) / absolute_temperature) * 3
            - i32::from(temperature_delta) / 2) as i16
    };
    let second_low = i32::from(observed_dcode[1])
        .wrapping_sub(i32::from(parameters.calibration_dcode[1]))
        .wrapping_mul(3);
    let second_high = i32::from(observed_dcode[1] >> 3)
        .wrapping_sub(i32::from(parameters.calibration_dcode[1] >> 3))
        .wrapping_mul(9);
    let first = if absolute_temperature > 19 {
        i32::from(observed_dcode[0] >> 3)
            .wrapping_sub(i32::from(parameters.calibration_dcode[0] >> 3))
            .wrapping_mul(6)
            .wrapping_add(
                i32::from(parameters.calibration_dcode[0])
                    .wrapping_sub(i32::from(observed_dcode[0])),
            )
    } else {
        0
    };
    let frequency =
        (i32::from(0x985_u16) - i32::from(channel_to_frequency(parameters.current_channel))) / 5;
    let phase = frequency
        .wrapping_add(i32::from(coefficient[1]))
        .wrapping_add(second_low)
        .wrapping_add(second_high)
        .wrapping_add(i32::from(temperature_correction))
        .wrapping_add(first) as i16 as i32;
    coefficient[1] = crate::calibration::math::saturate_signed(phase, 60, -60) as i8;
    coefficient
}

/// Three separately completed I2C reads around the pure TXIQ adjustment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxIqAdjustedTxTransition {
    parameters: PhyRxIqAdjustedTxParameters,
    step: AdjustedTxStep,
    observed_dcode: [u8; 2],
    external: bool,
}

impl PhyRxIqAdjustedTxTransition {
    pub const fn new(parameters: PhyRxIqAdjustedTxParameters) -> Self {
        Self {
            parameters,
            step: AdjustedTxStep::ReadMode,
            observed_dcode: [0; 2],
            external: false,
        }
    }

    const fn field(external: bool, second: bool) -> PhyI2cField {
        match (external, second) {
            (false, false) => analog_registers::RFPLL_INTERNAL_DCODE_0,
            (false, true) => analog_registers::RFPLL_INTERNAL_DCODE_1,
            (true, false) => analog_registers::RFPLL_EXTERNAL_DCODE_0,
            (true, true) => analog_registers::RFPLL_EXTERNAL_DCODE_1,
        }
    }

    pub fn action(self) -> PhyRxIqAdjustedTxAction {
        match self.step {
            AdjustedTxStep::ReadMode => PhyRxIqAdjustedTxAction::ReadI2cMasked {
                field: analog_registers::RFPLL_DCODE_0_SOURCE_SELECT,
            },
            AdjustedTxStep::ReadFirst { external } => PhyRxIqAdjustedTxAction::ReadI2cMasked {
                field: Self::field(external, false),
            },
            AdjustedTxStep::ReadSecond { external } => PhyRxIqAdjustedTxAction::ReadI2cMasked {
                field: Self::field(external, true),
            },
            AdjustedTxStep::Complete => {
                PhyRxIqAdjustedTxAction::Complete(PhyRxIqAdjustedTxOutcome {
                    coefficient: adjusted_txiq_coefficient(self.parameters, self.observed_dcode),
                    external_dcode: self.external,
                    observed_dcode: self.observed_dcode,
                })
            }
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyRxIqAdjustedTxCompletion,
    ) -> Result<(), PhyRxIqAdjustedTxTransitionError> {
        match (self.step, completion) {
            (
                AdjustedTxStep::ReadMode,
                PhyRxIqAdjustedTxCompletion::I2cMaskedRead {
                    field: analog_registers::RFPLL_DCODE_0_SOURCE_SELECT,
                    value,
                },
            ) => {
                self.external = value & 1 != 0;
                self.step = AdjustedTxStep::ReadFirst {
                    external: self.external,
                };
            }
            (
                AdjustedTxStep::ReadFirst { external },
                PhyRxIqAdjustedTxCompletion::I2cMaskedRead { field, value },
            ) if field == Self::field(external, false) => {
                self.observed_dcode[0] = value & 0x3f;
                self.step = AdjustedTxStep::ReadSecond { external };
            }
            (
                AdjustedTxStep::ReadSecond { external },
                PhyRxIqAdjustedTxCompletion::I2cMaskedRead { field, value },
            ) if field == Self::field(external, true) => {
                self.observed_dcode[1] = value & 0x3f;
                self.step = AdjustedTxStep::Complete;
            }
            (AdjustedTxStep::Complete, _) => {
                return Err(PhyRxIqAdjustedTxTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyRxIqAdjustedTxTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqCoefficientKind {
    Gain,
    Phase,
}

const fn bounded_rxiq_coefficient(value: i8, kind: PhyRxIqCoefficientKind) -> i8 {
    match kind {
        PhyRxIqCoefficientKind::Gain => {
            crate::calibration::math::saturate_signed(value as i32, 31, -31) as i8
        }
        PhyRxIqCoefficientKind::Phase => {
            crate::calibration::math::saturate_signed(value as i32, 63, -63) as i8
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxIqCoverRequest {
    pub identity: u8,
    pub exponent: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxIqCoverOutcome {
    pub identity: u8,
    pub gain: i8,
    pub phase: i8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqCoverAction {
    ConfigureCoefficient {
        identity: u8,
        iteration: u8,
        kind: PhyRxIqCoefficientKind,
        value: i8,
        final_value: bool,
    },
    Estimator(PhyRxIqEstimatorAction),
    Complete(PhyRxIqCoverOutcome),
    Failed(PhyRxIqEstimatorFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqCoverCompletion {
    CoefficientConfigured {
        identity: u8,
        iteration: u8,
        kind: PhyRxIqCoefficientKind,
        value: i8,
        final_value: bool,
    },
    Estimator(PhyRxIqEstimatorCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqCoverTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CoverStep {
    Gain,
    Phase,
    Measure(PhyRxIqEstimatorTransition),
    FinalGain,
    FinalPhase,
    Complete,
    Failed(PhyRxIqEstimatorFailure),
}

/// Complete two-pass translation of ROM `phy_rxiq_cover_mg_mp`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxIqCoverTransition {
    request: PhyRxIqCoverRequest,
    step: CoverStep,
    iteration: u8,
    gain: i8,
    phase: i8,
}

impl PhyRxIqCoverTransition {
    pub const fn new(request: PhyRxIqCoverRequest) -> Self {
        Self {
            request,
            step: CoverStep::Gain,
            iteration: 0,
            gain: 0,
            phase: 0,
        }
    }

    const fn estimator(self) -> PhyRxIqEstimatorTransition {
        PhyRxIqEstimatorTransition::new(PhyRxIqEstimatorRequest {
            identity: self
                .request
                .identity
                .wrapping_mul(2)
                .wrapping_add(self.iteration),
            control: (1_u32.wrapping_shl((self.request.exponent & 0x1f) as u32) & 0xffff) as u16,
            kind: PhyRxIqEstimatorKind::Mismatch {
                exponent: self.request.exponent,
            },
        })
    }

    pub const fn action(self) -> PhyRxIqCoverAction {
        match self.step {
            CoverStep::Gain => PhyRxIqCoverAction::ConfigureCoefficient {
                identity: self.request.identity,
                iteration: self.iteration,
                kind: PhyRxIqCoefficientKind::Gain,
                value: bounded_rxiq_coefficient(self.gain, PhyRxIqCoefficientKind::Gain),
                final_value: false,
            },
            CoverStep::Phase => PhyRxIqCoverAction::ConfigureCoefficient {
                identity: self.request.identity,
                iteration: self.iteration,
                kind: PhyRxIqCoefficientKind::Phase,
                value: bounded_rxiq_coefficient(self.phase, PhyRxIqCoefficientKind::Phase),
                final_value: false,
            },
            CoverStep::Measure(transition) => PhyRxIqCoverAction::Estimator(transition.action()),
            CoverStep::FinalGain => PhyRxIqCoverAction::ConfigureCoefficient {
                identity: self.request.identity,
                iteration: 2,
                kind: PhyRxIqCoefficientKind::Gain,
                value: bounded_rxiq_coefficient(self.gain, PhyRxIqCoefficientKind::Gain),
                final_value: true,
            },
            CoverStep::FinalPhase => PhyRxIqCoverAction::ConfigureCoefficient {
                identity: self.request.identity,
                iteration: 2,
                kind: PhyRxIqCoefficientKind::Phase,
                value: bounded_rxiq_coefficient(self.phase, PhyRxIqCoefficientKind::Phase),
                final_value: true,
            },
            CoverStep::Complete => PhyRxIqCoverAction::Complete(PhyRxIqCoverOutcome {
                identity: self.request.identity,
                gain: bounded_rxiq_coefficient(self.gain, PhyRxIqCoefficientKind::Gain),
                phase: bounded_rxiq_coefficient(self.phase, PhyRxIqCoefficientKind::Phase),
            }),
            CoverStep::Failed(failure) => PhyRxIqCoverAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyRxIqCoverCompletion,
    ) -> Result<(), PhyRxIqCoverTransitionError> {
        match (self.step, completion) {
            (
                CoverStep::Gain,
                PhyRxIqCoverCompletion::CoefficientConfigured {
                    identity,
                    iteration,
                    kind: PhyRxIqCoefficientKind::Gain,
                    value,
                    final_value: false,
                },
            ) if identity == self.request.identity
                && iteration == self.iteration
                && value == bounded_rxiq_coefficient(self.gain, PhyRxIqCoefficientKind::Gain) =>
            {
                self.gain = value;
                self.step = CoverStep::Phase;
            }
            (
                CoverStep::Phase,
                PhyRxIqCoverCompletion::CoefficientConfigured {
                    identity,
                    iteration,
                    kind: PhyRxIqCoefficientKind::Phase,
                    value,
                    final_value: false,
                },
            ) if identity == self.request.identity
                && iteration == self.iteration
                && value == bounded_rxiq_coefficient(self.phase, PhyRxIqCoefficientKind::Phase) =>
            {
                self.phase = value;
                self.step = CoverStep::Measure(self.estimator());
            }
            (CoverStep::Measure(mut transition), PhyRxIqCoverCompletion::Estimator(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxIqCoverTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyRxIqEstimatorAction::Complete(PhyRxIqEstimatorOutcome {
                        measurement: PhyRxIqMeasurement::Mismatch(mismatch),
                        ..
                    }) => {
                        self.gain = self.gain.wrapping_add(mismatch[0]);
                        self.phase = self.phase.wrapping_sub(mismatch[1]);
                        self.iteration += 1;
                        self.step = if self.iteration == 2 {
                            CoverStep::FinalGain
                        } else {
                            CoverStep::Gain
                        };
                    }
                    PhyRxIqEstimatorAction::Failed(failure) => {
                        self.step = CoverStep::Failed(failure);
                    }
                    _ => self.step = CoverStep::Measure(transition),
                }
            }
            (
                CoverStep::FinalGain,
                PhyRxIqCoverCompletion::CoefficientConfigured {
                    identity,
                    iteration: 2,
                    kind: PhyRxIqCoefficientKind::Gain,
                    value,
                    final_value: true,
                },
            ) if identity == self.request.identity
                && value == bounded_rxiq_coefficient(self.gain, PhyRxIqCoefficientKind::Gain) =>
            {
                self.gain = value;
                self.step = CoverStep::FinalPhase;
            }
            (
                CoverStep::FinalPhase,
                PhyRxIqCoverCompletion::CoefficientConfigured {
                    identity,
                    iteration: 2,
                    kind: PhyRxIqCoefficientKind::Phase,
                    value,
                    final_value: true,
                },
            ) if identity == self.request.identity
                && value == bounded_rxiq_coefficient(self.phase, PhyRxIqCoefficientKind::Phase) =>
            {
                self.phase = value;
                self.step = CoverStep::Complete;
            }
            (CoverStep::Complete | CoverStep::Failed(_), _) => {
                return Err(PhyRxIqCoverTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyRxIqCoverTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxIqRfCalibrationRequest {
    pub identity: u8,
    pub selector: u16,
    pub attenuation: u8,
    pub exponent: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxIqRfCalibrationOutcome {
    pub request: PhyRxIqRfCalibrationRequest,
    pub coefficient: [i8; 2],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqRfCalibrationAction {
    ConfigureCalibrationMode,
    ConfigureTone {
        enabled: bool,
        selector: u16,
        attenuation: u8,
    },
    Cover(PhyRxIqCoverAction),
    Complete(PhyRxIqRfCalibrationOutcome),
    Failed(PhyRxIqEstimatorFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqRfCalibrationCompletion {
    CalibrationModeConfigured,
    ToneConfigured {
        enabled: bool,
        selector: u16,
        attenuation: u8,
    },
    Cover(PhyRxIqCoverCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqRfCalibrationTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RfCalibrationTerminal {
    Complete([i8; 2]),
    Failed(PhyRxIqEstimatorFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RfCalibrationStep {
    Configure,
    StartTone,
    Cover(PhyRxIqCoverTransition),
    StopTone(RfCalibrationTerminal),
    Complete([i8; 2]),
    Failed(PhyRxIqEstimatorFailure),
}

/// Exact composition of ROM `phy_rfcal_rxiq`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxIqRfCalibrationTransition {
    request: PhyRxIqRfCalibrationRequest,
    step: RfCalibrationStep,
}

impl PhyRxIqRfCalibrationTransition {
    pub const fn new(request: PhyRxIqRfCalibrationRequest) -> Self {
        Self {
            request,
            step: RfCalibrationStep::Configure,
        }
    }

    pub const fn action(self) -> PhyRxIqRfCalibrationAction {
        match self.step {
            RfCalibrationStep::Configure => PhyRxIqRfCalibrationAction::ConfigureCalibrationMode,
            RfCalibrationStep::StartTone => PhyRxIqRfCalibrationAction::ConfigureTone {
                enabled: true,
                selector: self.request.selector,
                attenuation: self.request.attenuation,
            },
            RfCalibrationStep::Cover(transition) => {
                PhyRxIqRfCalibrationAction::Cover(transition.action())
            }
            RfCalibrationStep::StopTone(_) => PhyRxIqRfCalibrationAction::ConfigureTone {
                enabled: false,
                selector: self.request.selector,
                attenuation: self.request.attenuation,
            },
            RfCalibrationStep::Complete(coefficient) => {
                PhyRxIqRfCalibrationAction::Complete(PhyRxIqRfCalibrationOutcome {
                    request: self.request,
                    coefficient,
                })
            }
            RfCalibrationStep::Failed(failure) => PhyRxIqRfCalibrationAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyRxIqRfCalibrationCompletion,
    ) -> Result<(), PhyRxIqRfCalibrationTransitionError> {
        match (self.step, completion) {
            (
                RfCalibrationStep::Configure,
                PhyRxIqRfCalibrationCompletion::CalibrationModeConfigured,
            ) => self.step = RfCalibrationStep::StartTone,
            (
                RfCalibrationStep::StartTone,
                PhyRxIqRfCalibrationCompletion::ToneConfigured {
                    enabled: true,
                    selector,
                    attenuation,
                },
            ) if selector == self.request.selector && attenuation == self.request.attenuation => {
                self.step =
                    RfCalibrationStep::Cover(PhyRxIqCoverTransition::new(PhyRxIqCoverRequest {
                        identity: self.request.identity,
                        exponent: self.request.exponent,
                    }));
            }
            (
                RfCalibrationStep::Cover(mut transition),
                PhyRxIqRfCalibrationCompletion::Cover(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxIqRfCalibrationTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyRxIqCoverAction::Complete(outcome) => {
                        self.step = RfCalibrationStep::StopTone(RfCalibrationTerminal::Complete([
                            outcome.gain,
                            outcome.phase,
                        ]));
                    }
                    PhyRxIqCoverAction::Failed(failure) => {
                        self.step =
                            RfCalibrationStep::StopTone(RfCalibrationTerminal::Failed(failure));
                    }
                    _ => self.step = RfCalibrationStep::Cover(transition),
                }
            }
            (
                RfCalibrationStep::StopTone(terminal),
                PhyRxIqRfCalibrationCompletion::ToneConfigured {
                    enabled: false,
                    selector,
                    attenuation,
                },
            ) if selector == self.request.selector && attenuation == self.request.attenuation => {
                self.step = match terminal {
                    RfCalibrationTerminal::Complete(coefficient) => {
                        RfCalibrationStep::Complete(coefficient)
                    }
                    RfCalibrationTerminal::Failed(failure) => RfCalibrationStep::Failed(failure),
                };
            }
            (RfCalibrationStep::Complete(_) | RfCalibrationStep::Failed(_), _) => {
                return Err(PhyRxIqRfCalibrationTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyRxIqRfCalibrationTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxIqDataRequest {
    pub selector: u16,
    pub attenuation: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxIqDataOutcome {
    pub coefficient: u16,
    pub gain: i8,
    pub phase: i8,
    pub attempts: u8,
    pub converged: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqDataAction {
    Calibration(PhyRxIqRfCalibrationAction),
    Complete(PhyRxIqDataOutcome),
    Failed(PhyRxIqEstimatorFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqDataCompletion {
    Calibration(PhyRxIqRfCalibrationCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqDataTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DataStep {
    Calibration(PhyRxIqRfCalibrationTransition),
    Complete(PhyRxIqDataOutcome),
    Failed(PhyRxIqEstimatorFailure),
}

/// Bounded four-sample translation of `phy_get_rfcal_rxiq_data`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxIqDataTransition {
    request: PhyRxIqDataRequest,
    step: DataStep,
    attempts: u8,
    previous: [i8; 2],
    sum: [i16; 2],
}

impl PhyRxIqDataTransition {
    pub const fn new(request: PhyRxIqDataRequest) -> Self {
        Self {
            request,
            step: DataStep::Calibration(Self::calibration(request, 0)),
            attempts: 0,
            previous: [0; 2],
            sum: [0; 2],
        }
    }

    const fn calibration(
        request: PhyRxIqDataRequest,
        attempt: u8,
    ) -> PhyRxIqRfCalibrationTransition {
        PhyRxIqRfCalibrationTransition::new(PhyRxIqRfCalibrationRequest {
            identity: attempt,
            selector: request.selector,
            attenuation: request.attenuation,
            exponent: 14,
        })
    }

    const fn outcome(gain: i8, phase: i8, attempts: u8, converged: bool) -> PhyRxIqDataOutcome {
        let gain = bounded_rxiq_coefficient(gain, PhyRxIqCoefficientKind::Gain);
        let phase = bounded_rxiq_coefficient(phase, PhyRxIqCoefficientKind::Phase);
        PhyRxIqDataOutcome {
            coefficient: ((gain as u8 as u16) << 8) | phase as u8 as u16,
            gain,
            phase,
            attempts,
            converged,
        }
    }

    pub const fn action(self) -> PhyRxIqDataAction {
        match self.step {
            DataStep::Calibration(transition) => {
                PhyRxIqDataAction::Calibration(transition.action())
            }
            DataStep::Complete(outcome) => PhyRxIqDataAction::Complete(outcome),
            DataStep::Failed(failure) => PhyRxIqDataAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyRxIqDataCompletion,
    ) -> Result<(), PhyRxIqDataTransitionError> {
        match (self.step, completion) {
            (
                DataStep::Calibration(mut transition),
                PhyRxIqDataCompletion::Calibration(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxIqDataTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyRxIqRfCalibrationAction::Complete(outcome) => {
                        let current = outcome.coefficient;
                        let stable = self.attempts != 0
                            && i16::from(self.previous[0])
                                .wrapping_sub(i16::from(current[0]))
                                .wrapping_abs()
                                < 2
                            && i16::from(self.previous[1])
                                .wrapping_sub(i16::from(current[1]))
                                .wrapping_abs()
                                < 2;
                        if stable {
                            let gain = ((i16::from(current[0]) + i16::from(self.previous[0]) + 1)
                                >> 1) as i8;
                            let phase = ((i16::from(current[1]) + i16::from(self.previous[1]) + 1)
                                >> 1) as i8;
                            self.step = DataStep::Complete(Self::outcome(
                                gain,
                                phase,
                                self.attempts + 1,
                                true,
                            ));
                            return Ok(());
                        }
                        self.previous = current;
                        self.attempts += 1;
                        self.sum[0] = self.sum[0].wrapping_add(i16::from(current[0]));
                        self.sum[1] = self.sum[1].wrapping_add(i16::from(current[1]));
                        if self.attempts == 4 {
                            let gain = self.sum[0].wrapping_add(2) >> 2;
                            let phase = self.sum[1].wrapping_add(2) >> 2;
                            self.step = DataStep::Complete(Self::outcome(
                                gain as i8,
                                phase as i8,
                                4,
                                false,
                            ));
                        } else {
                            self.step = DataStep::Calibration(Self::calibration(
                                self.request,
                                self.attempts,
                            ));
                        }
                    }
                    PhyRxIqRfCalibrationAction::Failed(failure) => {
                        self.step = DataStep::Failed(failure);
                    }
                    _ => self.step = DataStep::Calibration(transition),
                }
            }
            (DataStep::Complete(_) | DataStep::Failed(_), _) => {
                return Err(PhyRxIqDataTransitionError::AlreadyComplete);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxIqGainParameters {
    pub pbus_rx_path_value: u8,
    pub channel_6_dcode: [u8; 2],
    pub adjusted_tx: PhyRxIqAdjustedTxParameters,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxIqGainOutcome {
    pub coefficient: u16,
    pub attenuation: u8,
    pub baseband_gain: u8,
    pub dco_configuration: [u32; 2],
    pub rf_attempts: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqGainFailure {
    Pbus(PhyPbusForceTest),
    Dco(PhyRxDcoFailure),
    Estimator(PhyRxIqEstimatorFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqGainAction {
    ForcePbus {
        pass: u8,
        transaction: PhyPbusForceTest,
    },
    WriteI2c {
        address: PhyI2cAddress,
        value: u8,
    },
    AdjustTx(PhyRxIqAdjustedTxAction),
    ConfigureTxIq {
        kind: PhyTxIqCoefficientKind,
        value: i8,
    },
    Dco(PhyRxDcoAction),
    ConfigureTone {
        enabled: bool,
        selector: u16,
        attenuation: u8,
    },
    Estimator(PhyRxIqEstimatorAction),
    Data(PhyRxIqDataAction),
    Complete(PhyRxIqGainOutcome),
    Failed(PhyRxIqGainFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqGainCompletion {
    PbusCompleted {
        pass: u8,
        transaction: PhyPbusForceTest,
    },
    PbusTimedOut {
        pass: u8,
        transaction: PhyPbusForceTest,
    },
    I2cWritten {
        address: PhyI2cAddress,
        value: u8,
    },
    AdjustTx(PhyRxIqAdjustedTxCompletion),
    TxIqConfigured {
        kind: PhyTxIqCoefficientKind,
        value: i8,
    },
    Dco(PhyRxDcoCompletion),
    ToneConfigured {
        enabled: bool,
        selector: u16,
        attenuation: u8,
    },
    Estimator(PhyRxIqEstimatorCompletion),
    Data(PhyRxIqDataCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqGainTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GainToneTerminal {
    Power(i32),
    Failed(PhyRxIqGainFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GainStep {
    LoopbackGain {
        pass: u8,
        index: u8,
    },
    Dcode0,
    Dcode1,
    AdjustTx(PhyRxIqAdjustedTxTransition),
    ConfigureTxGain {
        coefficient: [i8; 2],
    },
    ConfigureTxPhase {
        coefficient: [i8; 2],
    },
    Dco {
        pass: u8,
        transition: PhyRxDcoTransition,
    },
    TonePbus {
        pass: u8,
        index: u8,
    },
    StartTone {
        pass: u8,
    },
    MeasurePower {
        pass: u8,
        transition: PhyRxIqEstimatorTransition,
    },
    StopTone {
        pass: u8,
        terminal: GainToneTerminal,
    },
    Data(PhyRxIqDataTransition),
    Complete(PhyRxIqGainOutcome),
    Failed(PhyRxIqGainFailure),
}

const fn loopback_gain_transaction(
    index: u8,
    pbus_rx_path_value: u8,
    baseband_gain: u8,
) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(4, 2, 1),
        1 => PhyPbusForceTest::new(4, 1, 0x83),
        2 => PhyPbusForceTest::new(5, 1, 0x1c0),
        3 => PhyPbusForceTest::new(0, 1, 0x43),
        4 => PhyPbusForceTest::new(0, 2, pbus_rx_path_value as u16),
        5 => PhyPbusForceTest::new(1, 1, 0x1f9),
        _ => PhyPbusForceTest::new(1, 2, baseband_gain as u16),
    }
}

const fn tone_pbus_transaction(index: u8) -> PhyPbusForceTest {
    if index == 0 {
        PhyPbusForceTest::new(1, 1, 0)
    } else {
        PhyPbusForceTest::new(1, 1, 0x1f9)
    }
}

/// Complete bounded translation of ROM `phy_set_rx_gain_cal_iq(0, ...)`.
///
/// The omitted `param_1` and debug arguments are constants in the archive
/// parent; their only extra effects are one temporary I2C mask and formatting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxIqGainTransition {
    parameters: PhyRxIqGainParameters,
    step: GainStep,
    attenuation: u8,
    baseband_gain: u8,
    dco_configuration: [u32; 2],
}

impl PhyRxIqGainTransition {
    pub const fn new(parameters: PhyRxIqGainParameters) -> Self {
        Self {
            parameters,
            step: GainStep::LoopbackGain { pass: 0, index: 0 },
            attenuation: 0x30,
            baseband_gain: 0x20,
            dco_configuration: [0x0100_0100, 0],
        }
    }

    const fn dco(self) -> PhyRxDcoTransition {
        PhyRxDcoTransition::new(PhyRxDcoRequest {
            control: 4000,
            configuration: self.dco_configuration,
            delay_micros: 10,
        })
    }

    const fn power_estimator(pass: u8) -> PhyRxIqEstimatorTransition {
        PhyRxIqEstimatorTransition::new(PhyRxIqEstimatorRequest {
            identity: pass,
            control: 0x3ff,
            kind: PhyRxIqEstimatorKind::TotalPower,
        })
    }

    pub fn action(self) -> PhyRxIqGainAction {
        match self.step {
            GainStep::LoopbackGain { pass, index } => PhyRxIqGainAction::ForcePbus {
                pass,
                transaction: loopback_gain_transaction(
                    index,
                    self.parameters.pbus_rx_path_value,
                    self.baseband_gain,
                ),
            },
            GainStep::Dcode0 => PhyRxIqGainAction::WriteI2c {
                address: EXTERNAL_DCODE_0,
                value: (self.parameters.channel_6_dcode[0] & 0x3f) | 0x40,
            },
            GainStep::Dcode1 => PhyRxIqGainAction::WriteI2c {
                address: EXTERNAL_DCODE_1,
                value: (self.parameters.channel_6_dcode[1] & 0x3f) | 0x40,
            },
            GainStep::AdjustTx(transition) => PhyRxIqGainAction::AdjustTx(transition.action()),
            GainStep::ConfigureTxGain { coefficient } => PhyRxIqGainAction::ConfigureTxIq {
                kind: PhyTxIqCoefficientKind::Gain,
                value: coefficient[0],
            },
            GainStep::ConfigureTxPhase { coefficient } => PhyRxIqGainAction::ConfigureTxIq {
                kind: PhyTxIqCoefficientKind::Phase,
                value: coefficient[1],
            },
            GainStep::Dco { transition, .. } => PhyRxIqGainAction::Dco(transition.action()),
            GainStep::TonePbus { pass, index } => PhyRxIqGainAction::ForcePbus {
                pass,
                transaction: tone_pbus_transaction(index),
            },
            GainStep::StartTone { .. } => PhyRxIqGainAction::ConfigureTone {
                enabled: true,
                selector: 0x80,
                attenuation: self.attenuation,
            },
            GainStep::MeasurePower { transition, .. } => {
                PhyRxIqGainAction::Estimator(transition.action())
            }
            GainStep::StopTone { .. } => PhyRxIqGainAction::ConfigureTone {
                enabled: false,
                selector: 0x80,
                attenuation: self.attenuation,
            },
            GainStep::Data(transition) => PhyRxIqGainAction::Data(transition.action()),
            GainStep::Complete(outcome) => PhyRxIqGainAction::Complete(outcome),
            GainStep::Failed(failure) => PhyRxIqGainAction::Failed(failure),
        }
    }

    fn after_power(&mut self, pass: u8, power: i32) {
        if pass == 1 || (power > 0x0fff && power <= 0x20_000) {
            self.step = GainStep::Data(PhyRxIqDataTransition::new(PhyRxIqDataRequest {
                selector: 0x80,
                attenuation: self.attenuation,
            }));
            return;
        }
        if power > 0x20_000 {
            if self.baseband_gain != 0 {
                self.baseband_gain = 0;
            } else {
                self.attenuation = self.attenuation.saturating_add(0x18).min(0x78);
            }
        } else {
            self.attenuation = self.attenuation.saturating_sub(0x18);
        }
        self.step = GainStep::LoopbackGain { pass: 1, index: 0 };
    }

    pub fn advance(
        &mut self,
        completion: PhyRxIqGainCompletion,
    ) -> Result<(), PhyRxIqGainTransitionError> {
        match (self.step, completion) {
            (
                GainStep::LoopbackGain { pass, index },
                PhyRxIqGainCompletion::PbusCompleted {
                    pass: completed_pass,
                    transaction,
                },
            ) if completed_pass == pass
                && transaction
                    == loopback_gain_transaction(
                        index,
                        self.parameters.pbus_rx_path_value,
                        self.baseband_gain,
                    ) =>
            {
                self.step = if index == 6 {
                    if pass == 0
                        && self.parameters.channel_6_dcode[0] != 0
                        && self.parameters.channel_6_dcode[1] != 0
                    {
                        GainStep::Dcode0
                    } else if pass == 0 {
                        GainStep::AdjustTx(PhyRxIqAdjustedTxTransition::new(
                            self.parameters.adjusted_tx,
                        ))
                    } else {
                        GainStep::Dco {
                            pass,
                            transition: self.dco(),
                        }
                    }
                } else {
                    GainStep::LoopbackGain {
                        pass,
                        index: index + 1,
                    }
                };
            }
            (
                GainStep::LoopbackGain { pass, .. } | GainStep::TonePbus { pass, .. },
                PhyRxIqGainCompletion::PbusTimedOut {
                    pass: completed_pass,
                    transaction,
                },
            ) if completed_pass == pass => {
                self.step = GainStep::Failed(PhyRxIqGainFailure::Pbus(transaction));
            }
            (
                GainStep::Dcode0,
                PhyRxIqGainCompletion::I2cWritten {
                    address: EXTERNAL_DCODE_0,
                    value,
                },
            ) if value == (self.parameters.channel_6_dcode[0] & 0x3f) | 0x40 => {
                self.step = GainStep::Dcode1;
            }
            (
                GainStep::Dcode1,
                PhyRxIqGainCompletion::I2cWritten {
                    address: EXTERNAL_DCODE_1,
                    value,
                },
            ) if value == (self.parameters.channel_6_dcode[1] & 0x3f) | 0x40 => {
                self.step = GainStep::AdjustTx(PhyRxIqAdjustedTxTransition::new(
                    self.parameters.adjusted_tx,
                ));
            }
            (GainStep::AdjustTx(mut transition), PhyRxIqGainCompletion::AdjustTx(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxIqGainTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyRxIqAdjustedTxAction::Complete(outcome) => {
                        self.step = GainStep::ConfigureTxGain {
                            coefficient: outcome.coefficient,
                        };
                    }
                    _ => self.step = GainStep::AdjustTx(transition),
                }
            }
            (
                GainStep::ConfigureTxGain { coefficient },
                PhyRxIqGainCompletion::TxIqConfigured {
                    kind: PhyTxIqCoefficientKind::Gain,
                    value,
                },
            ) if value == coefficient[0] => {
                self.step = GainStep::ConfigureTxPhase { coefficient };
            }
            (
                GainStep::ConfigureTxPhase { coefficient },
                PhyRxIqGainCompletion::TxIqConfigured {
                    kind: PhyTxIqCoefficientKind::Phase,
                    value,
                },
            ) if value == coefficient[1] => {
                self.step = GainStep::Dco {
                    pass: 0,
                    transition: self.dco(),
                };
            }
            (
                GainStep::Dco {
                    pass,
                    mut transition,
                },
                PhyRxIqGainCompletion::Dco(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxIqGainTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyRxDcoAction::Complete(outcome) => {
                        self.dco_configuration = outcome.configuration;
                        self.step = GainStep::TonePbus { pass, index: 0 };
                    }
                    PhyRxDcoAction::Failed(failure) => {
                        self.step = GainStep::Failed(PhyRxIqGainFailure::Dco(failure));
                    }
                    _ => self.step = GainStep::Dco { pass, transition },
                }
            }
            (
                GainStep::TonePbus { pass, index },
                PhyRxIqGainCompletion::PbusCompleted {
                    pass: completed_pass,
                    transaction,
                },
            ) if completed_pass == pass && transaction == tone_pbus_transaction(index) => {
                self.step = if index == 1 {
                    GainStep::StartTone { pass }
                } else {
                    GainStep::TonePbus {
                        pass,
                        index: index + 1,
                    }
                };
            }
            (
                GainStep::StartTone { pass },
                PhyRxIqGainCompletion::ToneConfigured {
                    enabled: true,
                    selector: 0x80,
                    attenuation,
                },
            ) if attenuation == self.attenuation => {
                self.step = GainStep::MeasurePower {
                    pass,
                    transition: Self::power_estimator(pass),
                };
            }
            (
                GainStep::MeasurePower {
                    pass,
                    mut transition,
                },
                PhyRxIqGainCompletion::Estimator(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxIqGainTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyRxIqEstimatorAction::Complete(PhyRxIqEstimatorOutcome {
                        measurement: PhyRxIqMeasurement::TotalPower(power),
                        ..
                    }) => {
                        self.step = GainStep::StopTone {
                            pass,
                            terminal: GainToneTerminal::Power(power),
                        };
                    }
                    PhyRxIqEstimatorAction::Failed(failure) => {
                        self.step = GainStep::StopTone {
                            pass,
                            terminal: GainToneTerminal::Failed(PhyRxIqGainFailure::Estimator(
                                failure,
                            )),
                        };
                    }
                    _ => self.step = GainStep::MeasurePower { pass, transition },
                }
            }
            (
                GainStep::StopTone { pass, terminal },
                PhyRxIqGainCompletion::ToneConfigured {
                    enabled: false,
                    selector: 0x80,
                    attenuation,
                },
            ) if attenuation == self.attenuation => match terminal {
                GainToneTerminal::Power(power) => self.after_power(pass, power),
                GainToneTerminal::Failed(failure) => self.step = GainStep::Failed(failure),
            },
            (GainStep::Data(mut transition), PhyRxIqGainCompletion::Data(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxIqGainTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyRxIqDataAction::Complete(outcome) => {
                        self.step = GainStep::Complete(PhyRxIqGainOutcome {
                            coefficient: outcome.coefficient,
                            attenuation: self.attenuation,
                            baseband_gain: self.baseband_gain,
                            dco_configuration: self.dco_configuration,
                            rf_attempts: outcome.attempts,
                        });
                    }
                    PhyRxIqDataAction::Failed(failure) => {
                        self.step = GainStep::Failed(PhyRxIqGainFailure::Estimator(failure));
                    }
                    _ => self.step = GainStep::Data(transition),
                }
            }
            (GainStep::Complete(_) | GainStep::Failed(_), _) => {
                return Err(PhyRxIqGainTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyRxIqGainTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxIqInitParameters {
    pub crystal_selector: u8,
    pub pbus_rx_path_value: u8,
    pub capacitance: [u8; 6],
    pub channel_6_dcode: [u8; 2],
    pub adjusted_tx: PhyRxIqAdjustedTxParameters,
    pub coefficients: [u16; 4],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxIqInitOutcome {
    pub coefficients: [u16; 4],
    pub current_channel: u16,
    pub gain: PhyRxIqGainOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqInitFailure {
    Rfpll(RfpllFrequencyFailure),
    Pbus(PhyPbusForceTest),
    Gain(PhyRxIqGainFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqWorkModeDelayPhase {
    Settle,
    Pulse,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqInitAction {
    Rfpll(RfpllFrequencyAction),
    WriteTxCap {
        address: PhyI2cAddress,
        value: u8,
    },
    ConfigureRootStatus,
    ConfigurePbusDebugMode,
    ForcePbus(PhyPbusForceTest),
    Loopback(PhyTxIqLoopbackAction),
    ConfigureCorrection {
        begin: bool,
    },
    Gain(PhyRxIqGainAction),
    ConfigurePbusWorkMode,
    DelayMicros {
        phase: PhyRxIqWorkModeDelayPhase,
        micros: u32,
    },
    ConfigurePbusWorkModePulse,
    ClearPbusWorkModePulse,
    Complete(PhyRxIqInitOutcome),
    Failed(PhyRxIqInitFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqInitCompletion {
    Rfpll(RfpllFrequencyCompletion),
    TxCapWritten {
        address: PhyI2cAddress,
        value: u8,
    },
    RootStatusConfigured,
    PbusDebugModeConfigured,
    PbusCompleted(PhyPbusForceTest),
    PbusTimedOut(PhyPbusForceTest),
    Loopback(PhyTxIqLoopbackCompletion),
    CorrectionConfigured {
        begin: bool,
    },
    Gain(PhyRxIqGainCompletion),
    PbusWorkModeConfigured {
        settle_required: bool,
    },
    DelayElapsed {
        phase: PhyRxIqWorkModeDelayPhase,
        micros: u32,
    },
    PbusWorkModePulseConfigured,
    PbusWorkModePulseCleared,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqInitTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InitTerminal {
    Complete,
    Failed(PhyRxIqInitFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InitStep {
    Rfpll(RfpllFrequencyTransition),
    TxCap,
    RootStatus,
    Debug,
    RxOn {
        index: u8,
    },
    EnableLoopback(PhyTxIqLoopbackTransition),
    CorrectionBegin,
    Gain(PhyRxIqGainTransition),
    CorrectionEnd(InitTerminal),
    DisableLoopback {
        terminal: InitTerminal,
        transition: PhyTxIqLoopbackTransition,
    },
    TxOff {
        terminal: InitTerminal,
        index: u8,
    },
    WorkMode(InitTerminal),
    WorkModeDelay(InitTerminal),
    WorkModePulse(InitTerminal),
    WorkModePulseDelay(InitTerminal),
    WorkModePulseClear(InitTerminal),
    Complete,
    Failed(PhyRxIqInitFailure),
}

const RXIQ_TX_CAP_ADDRESS: PhyI2cAddress = analog_registers::TX_CAPACITOR_BANKS;

const fn root_rx_on(index: u8, pbus_rx_path_value: u8) -> PhyPbusForceTest {
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

const fn root_tx_off(index: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(4, 1, 0),
        1 => PhyPbusForceTest::new(5, 1, 0),
        2 => PhyPbusForceTest::new(1, 1, 0),
        3 => PhyPbusForceTest::new(1, 2, 0),
        _ => PhyPbusForceTest::new(0, 1, 0),
    }
}

const fn convert_rxiq_coefficient(value: u16) -> u16 {
    ((value >> 1) & 0x1f80) | (value & 0x007f)
}

/// Complete fixed-argument translation of archive `phy_rxiq_cal_init`.
///
/// The live parent calls `(0, &flags, 0)`, so the optional PBus read/OR branch
/// and the skip-cleanup branch are not part of this Wi-Fi profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxIqInitTransition {
    parameters: PhyRxIqInitParameters,
    step: InitStep,
    gain: PhyRxIqGainOutcome,
    coefficients: [u16; 4],
}

impl PhyRxIqInitTransition {
    pub const fn new(parameters: PhyRxIqInitParameters) -> Self {
        Self {
            parameters,
            step: InitStep::Rfpll(RfpllFrequencyTransition::new(RfpllFrequencyRequest {
                crystal_selector: parameters.crystal_selector,
                frequency_code: 0x985,
                offset: 0,
            })),
            gain: PhyRxIqGainOutcome {
                coefficient: 0,
                attenuation: 0,
                baseband_gain: 0,
                dco_configuration: [0; 2],
                rf_attempts: 0,
            },
            coefficients: parameters.coefficients,
        }
    }

    const fn outcome(self) -> PhyRxIqInitOutcome {
        PhyRxIqInitOutcome {
            coefficients: self.coefficients,
            current_channel: 6,
            gain: self.gain,
        }
    }

    fn cleanup_after_correction(&mut self, terminal: InitTerminal) {
        self.step = InitStep::CorrectionEnd(terminal);
    }

    fn cleanup_before_loopback(&mut self, terminal: InitTerminal) {
        self.step = InitStep::TxOff { terminal, index: 0 };
    }

    fn finish_work_mode(&mut self, terminal: InitTerminal) {
        self.step = match terminal {
            InitTerminal::Complete => InitStep::Complete,
            InitTerminal::Failed(failure) => InitStep::Failed(failure),
        };
    }

    pub fn action(self) -> PhyRxIqInitAction {
        match self.step {
            InitStep::Rfpll(transition) => PhyRxIqInitAction::Rfpll(transition.action()),
            InitStep::TxCap => PhyRxIqInitAction::WriteTxCap {
                address: RXIQ_TX_CAP_ADDRESS,
                value: self.parameters.capacitance[2] | 0xc0,
            },
            InitStep::RootStatus => PhyRxIqInitAction::ConfigureRootStatus,
            InitStep::Debug => PhyRxIqInitAction::ConfigurePbusDebugMode,
            InitStep::RxOn { index } => {
                PhyRxIqInitAction::ForcePbus(root_rx_on(index, self.parameters.pbus_rx_path_value))
            }
            InitStep::EnableLoopback(transition) | InitStep::DisableLoopback { transition, .. } => {
                PhyRxIqInitAction::Loopback(transition.action())
            }
            InitStep::CorrectionBegin => PhyRxIqInitAction::ConfigureCorrection { begin: true },
            InitStep::Gain(transition) => PhyRxIqInitAction::Gain(transition.action()),
            InitStep::CorrectionEnd(_) => PhyRxIqInitAction::ConfigureCorrection { begin: false },
            InitStep::TxOff { index, .. } => PhyRxIqInitAction::ForcePbus(root_tx_off(index)),
            InitStep::WorkMode(_) => PhyRxIqInitAction::ConfigurePbusWorkMode,
            InitStep::WorkModeDelay(_) => PhyRxIqInitAction::DelayMicros {
                phase: PhyRxIqWorkModeDelayPhase::Settle,
                micros: 1,
            },
            InitStep::WorkModePulse(_) => PhyRxIqInitAction::ConfigurePbusWorkModePulse,
            InitStep::WorkModePulseDelay(_) => PhyRxIqInitAction::DelayMicros {
                phase: PhyRxIqWorkModeDelayPhase::Pulse,
                micros: 2,
            },
            InitStep::WorkModePulseClear(_) => PhyRxIqInitAction::ClearPbusWorkModePulse,
            InitStep::Complete => PhyRxIqInitAction::Complete(self.outcome()),
            InitStep::Failed(failure) => PhyRxIqInitAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyRxIqInitCompletion,
    ) -> Result<(), PhyRxIqInitTransitionError> {
        match (self.step, completion) {
            (InitStep::Rfpll(mut transition), PhyRxIqInitCompletion::Rfpll(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxIqInitTransitionError::WrongCompletion)?;
                match transition.action() {
                    RfpllFrequencyAction::Complete(_) => self.step = InitStep::TxCap,
                    RfpllFrequencyAction::Failed(failure) => {
                        self.step = InitStep::Failed(PhyRxIqInitFailure::Rfpll(failure));
                    }
                    _ => self.step = InitStep::Rfpll(transition),
                }
            }
            (
                InitStep::TxCap,
                PhyRxIqInitCompletion::TxCapWritten {
                    address: RXIQ_TX_CAP_ADDRESS,
                    value,
                },
            ) if value == self.parameters.capacitance[2] | 0xc0 => {
                self.step = InitStep::RootStatus;
            }
            (InitStep::RootStatus, PhyRxIqInitCompletion::RootStatusConfigured) => {
                self.step = InitStep::Debug;
            }
            (InitStep::Debug, PhyRxIqInitCompletion::PbusDebugModeConfigured) => {
                self.step = InitStep::RxOn { index: 0 };
            }
            (InitStep::RxOn { index }, PhyRxIqInitCompletion::PbusCompleted(transaction))
                if transaction == root_rx_on(index, self.parameters.pbus_rx_path_value) =>
            {
                self.step = if index == 6 {
                    InitStep::EnableLoopback(PhyTxIqLoopbackTransition::new(true))
                } else {
                    InitStep::RxOn { index: index + 1 }
                };
            }
            (InitStep::RxOn { index }, PhyRxIqInitCompletion::PbusTimedOut(transaction))
                if transaction == root_rx_on(index, self.parameters.pbus_rx_path_value) =>
            {
                self.cleanup_before_loopback(InitTerminal::Failed(PhyRxIqInitFailure::Pbus(
                    transaction,
                )));
            }
            (
                InitStep::EnableLoopback(mut transition),
                PhyRxIqInitCompletion::Loopback(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxIqInitTransitionError::WrongCompletion)?;
                self.step = if matches!(
                    transition.action(),
                    PhyTxIqLoopbackAction::Complete { enabled: true }
                ) {
                    InitStep::CorrectionBegin
                } else {
                    InitStep::EnableLoopback(transition)
                };
            }
            (
                InitStep::CorrectionBegin,
                PhyRxIqInitCompletion::CorrectionConfigured { begin: true },
            ) => {
                // The archive publishes channel 6 before its nested
                // `phy_get_txiq_set` call. Keep that calibration-only input
                // explicit without exposing a partially committed owner.
                let mut adjusted_tx = self.parameters.adjusted_tx;
                adjusted_tx.current_channel = 6;
                self.step = InitStep::Gain(PhyRxIqGainTransition::new(PhyRxIqGainParameters {
                    pbus_rx_path_value: self.parameters.pbus_rx_path_value,
                    channel_6_dcode: self.parameters.channel_6_dcode,
                    adjusted_tx,
                }));
            }
            (InitStep::Gain(mut transition), PhyRxIqInitCompletion::Gain(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxIqInitTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyRxIqGainAction::Complete(outcome) => {
                        self.gain = outcome;
                        self.coefficients[0] = outcome.coefficient;
                        let mut index = 0;
                        while index != self.coefficients.len() {
                            self.coefficients[index] =
                                convert_rxiq_coefficient(self.coefficients[index]);
                            index += 1;
                        }
                        self.cleanup_after_correction(InitTerminal::Complete);
                    }
                    PhyRxIqGainAction::Failed(failure) => {
                        self.cleanup_after_correction(InitTerminal::Failed(
                            PhyRxIqInitFailure::Gain(failure),
                        ));
                    }
                    _ => self.step = InitStep::Gain(transition),
                }
            }
            (
                InitStep::CorrectionEnd(terminal),
                PhyRxIqInitCompletion::CorrectionConfigured { begin: false },
            ) => {
                self.step = InitStep::DisableLoopback {
                    terminal,
                    transition: PhyTxIqLoopbackTransition::new(false),
                };
            }
            (
                InitStep::DisableLoopback {
                    terminal,
                    mut transition,
                },
                PhyRxIqInitCompletion::Loopback(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxIqInitTransitionError::WrongCompletion)?;
                self.step = if matches!(
                    transition.action(),
                    PhyTxIqLoopbackAction::Complete { enabled: false }
                ) {
                    InitStep::TxOff { terminal, index: 0 }
                } else {
                    InitStep::DisableLoopback {
                        terminal,
                        transition,
                    }
                };
            }
            (
                InitStep::TxOff { terminal, index },
                PhyRxIqInitCompletion::PbusCompleted(transaction)
                | PhyRxIqInitCompletion::PbusTimedOut(transaction),
            ) if transaction == root_tx_off(index) => {
                self.step = if index == 4 {
                    InitStep::WorkMode(terminal)
                } else {
                    InitStep::TxOff {
                        terminal,
                        index: index + 1,
                    }
                };
            }
            (
                InitStep::WorkMode(terminal),
                PhyRxIqInitCompletion::PbusWorkModeConfigured {
                    settle_required: false,
                },
            ) => self.finish_work_mode(terminal),
            (
                InitStep::WorkMode(terminal),
                PhyRxIqInitCompletion::PbusWorkModeConfigured {
                    settle_required: true,
                },
            ) => self.step = InitStep::WorkModeDelay(terminal),
            (
                InitStep::WorkModeDelay(terminal),
                PhyRxIqInitCompletion::DelayElapsed {
                    phase: PhyRxIqWorkModeDelayPhase::Settle,
                    micros: 1,
                },
            ) => self.step = InitStep::WorkModePulse(terminal),
            (
                InitStep::WorkModePulse(terminal),
                PhyRxIqInitCompletion::PbusWorkModePulseConfigured,
            ) => self.step = InitStep::WorkModePulseDelay(terminal),
            (
                InitStep::WorkModePulseDelay(terminal),
                PhyRxIqInitCompletion::DelayElapsed {
                    phase: PhyRxIqWorkModeDelayPhase::Pulse,
                    micros: 2,
                },
            ) => self.step = InitStep::WorkModePulseClear(terminal),
            (
                InitStep::WorkModePulseClear(terminal),
                PhyRxIqInitCompletion::PbusWorkModePulseCleared,
            ) => self.finish_work_mode(terminal),
            (InitStep::Complete | InitStep::Failed(_), _) => {
                return Err(PhyRxIqInitTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyRxIqInitTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqEstimatorBindingError {
    NotDirectMmio,
}

/// Non-cloneable target token for one direct estimator MMIO edge.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxIqEstimatorMmioBinding {
    action: PhyRxIqEstimatorAction,
}

impl PhyRxIqEstimatorMmioBinding {
    pub fn new(action: PhyRxIqEstimatorAction) -> Result<Self, PhyRxIqEstimatorBindingError> {
        match action {
            PhyRxIqEstimatorAction::Configure(_)
            | PhyRxIqEstimatorAction::SetEnable { .. }
            | PhyRxIqEstimatorAction::ReadTotalPower(_)
            | PhyRxIqEstimatorAction::ReadMismatch(_) => Ok(Self { action }),
            _ => Err(PhyRxIqEstimatorBindingError::NotDirectMmio),
        }
    }

    #[cfg(feature = "validation-probes")]
    pub const fn action(&self) -> PhyRxIqEstimatorAction {
        self.action
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyContext,
    ) -> PhyRxIqEstimatorCompletion {
        match self.action {
            PhyRxIqEstimatorAction::Configure(request) => {
                crate::calibration::estimator::configure_target(registers, request.control);
                PhyRxIqEstimatorCompletion::Configured(request)
            }
            PhyRxIqEstimatorAction::SetEnable {
                request,
                phase,
                enabled,
            } => {
                crate::calibration::estimator::set_enable_target(registers, phase, enabled);
                PhyRxIqEstimatorCompletion::EnableSet {
                    request,
                    phase,
                    enabled,
                }
            }
            PhyRxIqEstimatorAction::ReadTotalPower(request) => {
                PhyRxIqEstimatorCompletion::TotalPowerRead {
                    request,
                    value: oer_esp32s31_hal::phy::iq_estimator::read_total_power(registers),
                }
            }
            PhyRxIqEstimatorAction::ReadMismatch(request) => {
                let snapshot = oer_esp32s31_hal::phy::iq_estimator::read_rxiq_mismatch(registers);
                PhyRxIqEstimatorCompletion::MismatchRead {
                    request,
                    snapshot: PhyRxIqMismatchSnapshot {
                        sum_i: snapshot.sum_i,
                        difference_i: snapshot.difference_i,
                        difference_q: snapshot.difference_q,
                        sum_q: snapshot.sum_q,
                    },
                }
            }
            _ => unreachable!(),
        }
    }
}

/// Non-cloneable async boundary for one scheduled RXIQ readiness observation.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxIqEstimatorReadinessBinding {
    action: PhyRxIqEstimatorAction,
}

impl PhyRxIqEstimatorReadinessBinding {
    pub fn new(action: PhyRxIqEstimatorAction) -> Result<Self, PhyRxIqEstimatorBindingError> {
        match action {
            PhyRxIqEstimatorAction::AwaitReadinessEdge { .. } => Ok(Self { action }),
            _ => Err(PhyRxIqEstimatorBindingError::NotDirectMmio),
        }
    }

    pub const fn samples(&self) -> u16 {
        match self.action {
            PhyRxIqEstimatorAction::AwaitReadinessEdge {
                readiness_samples, ..
            } => readiness_samples,
            _ => unreachable!(),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> PhyRxIqEstimatorCompletion {
        let PhyRxIqEstimatorAction::AwaitReadinessEdge { request, .. } = self.action else {
            unreachable!();
        };
        PhyRxIqEstimatorCompletion::ReadinessObserved {
            request,
            snapshot: crate::calibration::estimator::sample_readiness_target(registers),
        }
    }

    pub fn into_timeout_completion(self) -> PhyRxIqEstimatorCompletion {
        let PhyRxIqEstimatorAction::AwaitReadinessEdge { request, .. } = self.action else {
            unreachable!();
        };
        PhyRxIqEstimatorCompletion::ReadinessTimedOut(request)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxIqEstimatorTimerBinding {
    request: PhyRxIqEstimatorRequest,
    phase: PhyDcIqDelayPhase,
    micros: u32,
}

impl PhyRxIqEstimatorTimerBinding {
    pub fn new(action: PhyRxIqEstimatorAction) -> Result<Self, PhyRxIqEstimatorBindingError> {
        match action {
            PhyRxIqEstimatorAction::DelayMicros {
                request,
                phase,
                micros,
            } => Ok(Self {
                request,
                phase,
                micros,
            }),
            _ => Err(PhyRxIqEstimatorBindingError::NotDirectMmio),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyRxIqEstimatorCompletion {
        PhyRxIqEstimatorCompletion::DelayElapsed {
            request: self.request,
            phase: self.phase,
            micros: self.micros,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyRxIqEstimatorExternalBinding {
    Mmio(PhyRxIqEstimatorMmioBinding),
    Timer(PhyRxIqEstimatorTimerBinding),
    Readiness(PhyRxIqEstimatorReadinessBinding),
}

impl PhyRxIqEstimatorExternalBinding {
    pub fn lower(action: PhyRxIqEstimatorAction) -> Result<Self, PhyRxIqEstimatorBindingError> {
        if let Ok(binding) = PhyRxIqEstimatorReadinessBinding::new(action) {
            return Ok(Self::Readiness(binding));
        }
        if let Ok(binding) = PhyRxIqEstimatorMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        if let Ok(binding) = PhyRxIqEstimatorTimerBinding::new(action) {
            return Ok(Self::Timer(binding));
        }
        Err(PhyRxIqEstimatorBindingError::NotDirectMmio)
    }
}

/// Non-cloneable target token for RXIQ mode and tone edges.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxIqCalibrationMmioBinding {
    action: PhyRxIqRfCalibrationAction,
}

impl PhyRxIqCalibrationMmioBinding {
    pub fn new(action: PhyRxIqRfCalibrationAction) -> Result<Self, PhyRxIqEstimatorBindingError> {
        match action {
            PhyRxIqRfCalibrationAction::ConfigureCalibrationMode
            | PhyRxIqRfCalibrationAction::ConfigureTone { .. } => Ok(Self { action }),
            _ => Err(PhyRxIqEstimatorBindingError::NotDirectMmio),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> PhyRxIqRfCalibrationCompletion {
        match self.action {
            PhyRxIqRfCalibrationAction::ConfigureCalibrationMode => {
                crate::hardware::configure_phy_rxiq_calibration_mode(registers);
                PhyRxIqRfCalibrationCompletion::CalibrationModeConfigured
            }
            PhyRxIqRfCalibrationAction::ConfigureTone {
                enabled,
                selector,
                attenuation,
            } => {
                crate::hardware::configure_phy_calibration_tone_wide(
                    registers,
                    enabled,
                    selector,
                    attenuation,
                );
                PhyRxIqRfCalibrationCompletion::ToneConfigured {
                    enabled,
                    selector,
                    attenuation,
                }
            }
            _ => unreachable!(),
        }
    }
}

/// Non-cloneable target token for one RXIQ coefficient publication.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxIqCoverMmioBinding {
    action: PhyRxIqCoverAction,
}

impl PhyRxIqCoverMmioBinding {
    pub fn new(action: PhyRxIqCoverAction) -> Result<Self, PhyRxIqEstimatorBindingError> {
        match action {
            PhyRxIqCoverAction::ConfigureCoefficient { .. } => Ok(Self { action }),
            _ => Err(PhyRxIqEstimatorBindingError::NotDirectMmio),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> PhyRxIqCoverCompletion {
        match self.action {
            PhyRxIqCoverAction::ConfigureCoefficient {
                identity,
                iteration,
                kind,
                value,
                final_value,
            } => {
                crate::hardware::configure_phy_rxiq_coefficient(registers, kind, value);
                PhyRxIqCoverCompletion::CoefficientConfigured {
                    identity,
                    iteration,
                    kind,
                    value,
                    final_value,
                }
            }
            _ => unreachable!(),
        }
    }
}

/// Non-cloneable direct-MMIO token for the power-selection portion of
/// `phy_set_rx_gain_cal_iq`.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxIqGainMmioBinding {
    action: PhyRxIqGainAction,
}

impl PhyRxIqGainMmioBinding {
    pub fn new(action: PhyRxIqGainAction) -> Result<Self, PhyRxIqEstimatorBindingError> {
        match action {
            PhyRxIqGainAction::ConfigureTxIq { .. } | PhyRxIqGainAction::ConfigureTone { .. } => {
                Ok(Self { action })
            }
            _ => Err(PhyRxIqEstimatorBindingError::NotDirectMmio),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> PhyRxIqGainCompletion {
        match self.action {
            PhyRxIqGainAction::ConfigureTxIq { kind, value } => {
                crate::hardware::configure_phy_txiq_coefficient(registers, kind, value);
                PhyRxIqGainCompletion::TxIqConfigured { kind, value }
            }
            PhyRxIqGainAction::ConfigureTone {
                enabled,
                selector,
                attenuation,
            } => {
                crate::hardware::configure_phy_calibration_tone_wide(
                    registers,
                    enabled,
                    selector,
                    attenuation,
                );
                PhyRxIqGainCompletion::ToneConfigured {
                    enabled,
                    selector,
                    attenuation,
                }
            }
            _ => unreachable!(),
        }
    }
}

/// Non-cloneable direct-MMIO token for the RXIQ archive root.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxIqInitMmioBinding {
    action: PhyRxIqInitAction,
}

impl PhyRxIqInitMmioBinding {
    pub fn new(action: PhyRxIqInitAction) -> Result<Self, PhyRxIqEstimatorBindingError> {
        match action {
            PhyRxIqInitAction::ConfigureRootStatus
            | PhyRxIqInitAction::ConfigurePbusDebugMode
            | PhyRxIqInitAction::ConfigureCorrection { .. }
            | PhyRxIqInitAction::ConfigurePbusWorkMode
            | PhyRxIqInitAction::ConfigurePbusWorkModePulse
            | PhyRxIqInitAction::ClearPbusWorkModePulse => Ok(Self { action }),
            _ => Err(PhyRxIqEstimatorBindingError::NotDirectMmio),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyContext,
    ) -> PhyRxIqInitCompletion {
        match self.action {
            PhyRxIqInitAction::ConfigureRootStatus => {
                oer_esp32s31_hal::phy::baseband::configure_rxiq_root_status(registers);
                PhyRxIqInitCompletion::RootStatusConfigured
            }
            PhyRxIqInitAction::ConfigurePbusDebugMode => {
                oer_esp32s31_hal::phy::pbus::configure_debug_mode(registers);
                PhyRxIqInitCompletion::PbusDebugModeConfigured
            }
            PhyRxIqInitAction::ConfigureCorrection { begin } => {
                oer_esp32s31_hal::phy::baseband::configure_rxiq_root_correction(registers, begin);
                PhyRxIqInitCompletion::CorrectionConfigured { begin }
            }
            PhyRxIqInitAction::ConfigurePbusWorkMode => {
                PhyRxIqInitCompletion::PbusWorkModeConfigured {
                    settle_required: oer_esp32s31_hal::phy::pbus::configure_work_mode(registers),
                }
            }
            PhyRxIqInitAction::ConfigurePbusWorkModePulse => {
                oer_esp32s31_hal::phy::agc::configure_pbus_work_mode_pulse(registers);
                PhyRxIqInitCompletion::PbusWorkModePulseConfigured
            }
            PhyRxIqInitAction::ClearPbusWorkModePulse => {
                oer_esp32s31_hal::phy::agc::clear_pbus_work_mode_pulse(registers);
                PhyRxIqInitCompletion::PbusWorkModePulseCleared
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxIqExternalBindingError {
    UnsupportedAction,
    IncompleteTransaction,
    UnexpectedOutcome,
    Pbus(crate::analog::pbus::PhyPbusHardwareBindingError),
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxIqAdjustedTxI2cBinding {
    field: PhyI2cField,
    transaction: crate::calibration::cold::PhyColdI2cTransaction,
}

impl PhyRxIqAdjustedTxI2cBinding {
    pub fn new(action: PhyRxIqAdjustedTxAction) -> Result<Self, PhyRxIqExternalBindingError> {
        let PhyRxIqAdjustedTxAction::ReadI2cMasked { field } = action else {
            return Err(PhyRxIqExternalBindingError::UnsupportedAction);
        };
        let request = crate::calibration::cold::PhyColdI2cRequest::read_field(field);
        Ok(Self {
            field,
            transaction: crate::calibration::cold::PhyColdI2cTransaction::new(request),
        })
    }

    pub const fn action(&self) -> crate::calibration::cold::PhyColdI2cAction {
        self.transaction.action()
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub fn read_started(&mut self) -> Result<(), crate::calibration::cold::PhyColdI2cError> {
        self.transaction.read_started()
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub fn observe_read_result(
        &mut self,
        result: Result<u8, crate::analog::i2c::PhyI2cError>,
    ) -> Result<
        crate::calibration::cold::PhyColdI2cObservation,
        crate::calibration::cold::PhyColdI2cError,
    > {
        self.transaction.observe_read_result(result)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target<P: oer_esp32s31_hal::owner::SharedPhyAccess>(
        &mut self,
        platform: &mut P,
    ) -> Result<(), crate::calibration::cold::PhyColdI2cError> {
        self.transaction.start_target(platform)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge<P: oer_esp32s31_hal::owner::SharedPhyAccess>(
        &mut self,
        platform: &mut P,
    ) -> Result<
        crate::calibration::cold::PhyColdI2cObservation,
        crate::calibration::cold::PhyColdI2cError,
    > {
        self.transaction.observe_target_edge(platform)
    }

    pub fn into_completion(
        self,
    ) -> Result<PhyRxIqAdjustedTxCompletion, PhyRxIqExternalBindingError> {
        match self.transaction.action() {
            crate::calibration::cold::PhyColdI2cAction::Complete(
                crate::calibration::cold::PhyColdI2cOutcome::Read { address, value },
            ) if address == self.field.address() => {
                Ok(PhyRxIqAdjustedTxCompletion::I2cMaskedRead {
                    field: self.field,
                    value,
                })
            }
            crate::calibration::cold::PhyColdI2cAction::Complete(_) => {
                Err(PhyRxIqExternalBindingError::UnexpectedOutcome)
            }
            _ => Err(PhyRxIqExternalBindingError::IncompleteTransaction),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyRxIqCoverExternalBinding {
    Mmio(PhyRxIqCoverMmioBinding),
    Estimator(PhyRxIqEstimatorExternalBinding),
}

impl PhyRxIqCoverExternalBinding {
    pub fn lower(action: PhyRxIqCoverAction) -> Result<Self, PhyRxIqExternalBindingError> {
        match action {
            PhyRxIqCoverAction::ConfigureCoefficient { .. } => PhyRxIqCoverMmioBinding::new(action)
                .map(Self::Mmio)
                .map_err(|_| PhyRxIqExternalBindingError::UnsupportedAction),
            PhyRxIqCoverAction::Estimator(action) => PhyRxIqEstimatorExternalBinding::lower(action)
                .map(Self::Estimator)
                .map_err(|_| PhyRxIqExternalBindingError::UnsupportedAction),
            PhyRxIqCoverAction::Complete(_) | PhyRxIqCoverAction::Failed(_) => {
                Err(PhyRxIqExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyRxIqRfCalibrationExternalBinding {
    Mmio(PhyRxIqCalibrationMmioBinding),
    Cover(PhyRxIqCoverExternalBinding),
}

impl PhyRxIqRfCalibrationExternalBinding {
    pub fn lower(action: PhyRxIqRfCalibrationAction) -> Result<Self, PhyRxIqExternalBindingError> {
        match action {
            PhyRxIqRfCalibrationAction::ConfigureCalibrationMode
            | PhyRxIqRfCalibrationAction::ConfigureTone { .. } => {
                PhyRxIqCalibrationMmioBinding::new(action)
                    .map(Self::Mmio)
                    .map_err(|_| PhyRxIqExternalBindingError::UnsupportedAction)
            }
            PhyRxIqRfCalibrationAction::Cover(action) => {
                PhyRxIqCoverExternalBinding::lower(action).map(Self::Cover)
            }
            PhyRxIqRfCalibrationAction::Complete(_) | PhyRxIqRfCalibrationAction::Failed(_) => {
                Err(PhyRxIqExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyRxIqDataExternalBinding {
    Calibration(PhyRxIqRfCalibrationExternalBinding),
}

impl PhyRxIqDataExternalBinding {
    pub fn lower(action: PhyRxIqDataAction) -> Result<Self, PhyRxIqExternalBindingError> {
        match action {
            PhyRxIqDataAction::Calibration(action) => {
                PhyRxIqRfCalibrationExternalBinding::lower(action).map(Self::Calibration)
            }
            PhyRxIqDataAction::Complete(_) | PhyRxIqDataAction::Failed(_) => {
                Err(PhyRxIqExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxIqGainPbusBinding {
    pass: u8,
    transaction: PhyPbusForceTest,
    hardware: crate::analog::pbus::PhyPbusHardwareBinding,
}

impl PhyRxIqGainPbusBinding {
    pub fn new(action: PhyRxIqGainAction) -> Result<Self, PhyRxIqExternalBindingError> {
        let PhyRxIqGainAction::ForcePbus { pass, transaction } = action else {
            return Err(PhyRxIqExternalBindingError::UnsupportedAction);
        };
        Ok(Self {
            pass,
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

    pub fn into_completion(self) -> Result<PhyRxIqGainCompletion, PhyRxIqExternalBindingError> {
        self.hardware
            .into_transaction()
            .map(|transaction| PhyRxIqGainCompletion::PbusCompleted {
                pass: self.pass,
                transaction,
            })
            .map_err(PhyRxIqExternalBindingError::Pbus)
    }

    pub const fn into_timeout_completion(self) -> PhyRxIqGainCompletion {
        PhyRxIqGainCompletion::PbusTimedOut {
            pass: self.pass,
            transaction: self.transaction,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxIqGainI2cBinding {
    address: PhyI2cAddress,
    value: u8,
    transaction: crate::calibration::cold::PhyColdI2cTransaction,
}

impl PhyRxIqGainI2cBinding {
    pub fn new(action: PhyRxIqGainAction) -> Result<Self, PhyRxIqExternalBindingError> {
        let PhyRxIqGainAction::WriteI2c { address, value } = action else {
            return Err(PhyRxIqExternalBindingError::UnsupportedAction);
        };
        Ok(Self {
            address,
            value,
            transaction: crate::calibration::cold::PhyColdI2cTransaction::new(
                crate::calibration::cold::PhyColdI2cRequest::write_byte(address, value),
            ),
        })
    }

    pub const fn action(&self) -> crate::calibration::cold::PhyColdI2cAction {
        self.transaction.action()
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub fn write_started(&mut self) -> Result<(), crate::calibration::cold::PhyColdI2cError> {
        self.transaction.write_started()
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub fn observe_write_result(
        &mut self,
        result: Result<(), crate::analog::i2c::PhyI2cError>,
    ) -> Result<
        crate::calibration::cold::PhyColdI2cObservation,
        crate::calibration::cold::PhyColdI2cError,
    > {
        self.transaction.observe_write_result(result)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target<P: oer_esp32s31_hal::owner::SharedPhyAccess>(
        &mut self,
        platform: &mut P,
    ) -> Result<(), crate::calibration::cold::PhyColdI2cError> {
        self.transaction.start_target(platform)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge<P: oer_esp32s31_hal::owner::SharedPhyAccess>(
        &mut self,
        platform: &mut P,
    ) -> Result<
        crate::calibration::cold::PhyColdI2cObservation,
        crate::calibration::cold::PhyColdI2cError,
    > {
        self.transaction.observe_target_edge(platform)
    }

    pub fn into_completion(self) -> Result<PhyRxIqGainCompletion, PhyRxIqExternalBindingError> {
        match self.transaction.action() {
            crate::calibration::cold::PhyColdI2cAction::Complete(
                crate::calibration::cold::PhyColdI2cOutcome::Written { address },
            ) if address == self.address => Ok(PhyRxIqGainCompletion::I2cWritten {
                address,
                value: self.value,
            }),
            crate::calibration::cold::PhyColdI2cAction::Complete(_) => {
                Err(PhyRxIqExternalBindingError::UnexpectedOutcome)
            }
            _ => Err(PhyRxIqExternalBindingError::IncompleteTransaction),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyRxIqGainExternalBinding {
    Pbus(PhyRxIqGainPbusBinding),
    I2c(PhyRxIqGainI2cBinding),
    AdjustTx(PhyRxIqAdjustedTxI2cBinding),
    Mmio(PhyRxIqGainMmioBinding),
    Dco(crate::rx::dc_offset::PhyRxDcoExternalBinding),
    Estimator(PhyRxIqEstimatorExternalBinding),
    Data(PhyRxIqDataExternalBinding),
}

impl PhyRxIqGainExternalBinding {
    pub fn lower(action: PhyRxIqGainAction) -> Result<Self, PhyRxIqExternalBindingError> {
        if let Ok(binding) = PhyRxIqGainMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        match action {
            PhyRxIqGainAction::ForcePbus { .. } => {
                PhyRxIqGainPbusBinding::new(action).map(Self::Pbus)
            }
            PhyRxIqGainAction::WriteI2c { .. } => PhyRxIqGainI2cBinding::new(action).map(Self::I2c),
            PhyRxIqGainAction::AdjustTx(action) => {
                PhyRxIqAdjustedTxI2cBinding::new(action).map(Self::AdjustTx)
            }
            PhyRxIqGainAction::Dco(action) => {
                crate::rx::dc_offset::PhyRxDcoExternalBinding::lower(action)
                    .map(Self::Dco)
                    .map_err(|_| PhyRxIqExternalBindingError::UnsupportedAction)
            }
            PhyRxIqGainAction::Estimator(action) => PhyRxIqEstimatorExternalBinding::lower(action)
                .map(Self::Estimator)
                .map_err(|_| PhyRxIqExternalBindingError::UnsupportedAction),
            PhyRxIqGainAction::Data(action) => {
                PhyRxIqDataExternalBinding::lower(action).map(Self::Data)
            }
            PhyRxIqGainAction::Complete(_) | PhyRxIqGainAction::Failed(_) => {
                Err(PhyRxIqExternalBindingError::UnsupportedAction)
            }
            _ => Err(PhyRxIqExternalBindingError::UnsupportedAction),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxIqInitPbusBinding {
    transaction: PhyPbusForceTest,
    hardware: crate::analog::pbus::PhyPbusHardwareBinding,
}

impl PhyRxIqInitPbusBinding {
    pub fn new(action: PhyRxIqInitAction) -> Result<Self, PhyRxIqExternalBindingError> {
        let PhyRxIqInitAction::ForcePbus(transaction) = action else {
            return Err(PhyRxIqExternalBindingError::UnsupportedAction);
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

    pub fn into_completion(self) -> Result<PhyRxIqInitCompletion, PhyRxIqExternalBindingError> {
        self.hardware
            .into_transaction()
            .map(PhyRxIqInitCompletion::PbusCompleted)
            .map_err(PhyRxIqExternalBindingError::Pbus)
    }

    pub const fn into_timeout_completion(self) -> PhyRxIqInitCompletion {
        PhyRxIqInitCompletion::PbusTimedOut(self.transaction)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxIqInitI2cBinding {
    address: PhyI2cAddress,
    value: u8,
    transaction: crate::calibration::cold::PhyColdI2cTransaction,
}

impl PhyRxIqInitI2cBinding {
    pub fn new(action: PhyRxIqInitAction) -> Result<Self, PhyRxIqExternalBindingError> {
        let PhyRxIqInitAction::WriteTxCap { address, value } = action else {
            return Err(PhyRxIqExternalBindingError::UnsupportedAction);
        };
        Ok(Self {
            address,
            value,
            transaction: crate::calibration::cold::PhyColdI2cTransaction::new(
                crate::calibration::cold::PhyColdI2cRequest::write_byte(address, value),
            ),
        })
    }

    pub const fn action(&self) -> crate::calibration::cold::PhyColdI2cAction {
        self.transaction.action()
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub fn write_started(&mut self) -> Result<(), crate::calibration::cold::PhyColdI2cError> {
        self.transaction.write_started()
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub fn observe_write_result(
        &mut self,
        result: Result<(), crate::analog::i2c::PhyI2cError>,
    ) -> Result<
        crate::calibration::cold::PhyColdI2cObservation,
        crate::calibration::cold::PhyColdI2cError,
    > {
        self.transaction.observe_write_result(result)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target<P: oer_esp32s31_hal::owner::SharedPhyAccess>(
        &mut self,
        platform: &mut P,
    ) -> Result<(), crate::calibration::cold::PhyColdI2cError> {
        self.transaction.start_target(platform)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge<P: oer_esp32s31_hal::owner::SharedPhyAccess>(
        &mut self,
        platform: &mut P,
    ) -> Result<
        crate::calibration::cold::PhyColdI2cObservation,
        crate::calibration::cold::PhyColdI2cError,
    > {
        self.transaction.observe_target_edge(platform)
    }

    pub fn into_completion(self) -> Result<PhyRxIqInitCompletion, PhyRxIqExternalBindingError> {
        match self.transaction.action() {
            crate::calibration::cold::PhyColdI2cAction::Complete(
                crate::calibration::cold::PhyColdI2cOutcome::Written { address },
            ) if address == self.address => Ok(PhyRxIqInitCompletion::TxCapWritten {
                address,
                value: self.value,
            }),
            crate::calibration::cold::PhyColdI2cAction::Complete(_) => {
                Err(PhyRxIqExternalBindingError::UnexpectedOutcome)
            }
            _ => Err(PhyRxIqExternalBindingError::IncompleteTransaction),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxIqInitTimerBinding {
    phase: PhyRxIqWorkModeDelayPhase,
    micros: u32,
}

impl PhyRxIqInitTimerBinding {
    pub fn new(action: PhyRxIqInitAction) -> Result<Self, PhyRxIqExternalBindingError> {
        match action {
            PhyRxIqInitAction::DelayMicros { phase, micros } => Ok(Self { phase, micros }),
            _ => Err(PhyRxIqExternalBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyRxIqInitCompletion {
        PhyRxIqInitCompletion::DelayElapsed {
            phase: self.phase,
            micros: self.micros,
        }
    }
}

/// Exhaustive lowering of every non-terminal `phy_rxiq_cal_init` action.
#[derive(Debug, Eq, PartialEq)]
pub enum PhyRxIqInitExternalBinding {
    Rfpll(crate::analog::rfpll::RfpllFrequencyExternalBinding),
    I2c(PhyRxIqInitI2cBinding),
    Mmio(PhyRxIqInitMmioBinding),
    Pbus(PhyRxIqInitPbusBinding),
    Loopback(crate::tx::iq::PhyTxIqLoopbackExternalBinding),
    Gain(PhyRxIqGainExternalBinding),
    Timer(PhyRxIqInitTimerBinding),
}

impl PhyRxIqInitExternalBinding {
    pub fn lower(action: PhyRxIqInitAction) -> Result<Self, PhyRxIqExternalBindingError> {
        if let Ok(binding) = PhyRxIqInitMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        match action {
            PhyRxIqInitAction::Rfpll(action) => {
                crate::analog::rfpll::RfpllFrequencyExternalBinding::lower(action)
                    .map(Self::Rfpll)
                    .map_err(|_| PhyRxIqExternalBindingError::UnsupportedAction)
            }
            PhyRxIqInitAction::WriteTxCap { .. } => {
                PhyRxIqInitI2cBinding::new(action).map(Self::I2c)
            }
            PhyRxIqInitAction::ForcePbus(_) => PhyRxIqInitPbusBinding::new(action).map(Self::Pbus),
            PhyRxIqInitAction::Loopback(action) => {
                crate::tx::iq::PhyTxIqLoopbackExternalBinding::lower(action)
                    .map(Self::Loopback)
                    .map_err(|_| PhyRxIqExternalBindingError::UnsupportedAction)
            }
            PhyRxIqInitAction::Gain(action) => {
                PhyRxIqGainExternalBinding::lower(action).map(Self::Gain)
            }
            PhyRxIqInitAction::DelayMicros { .. } => {
                PhyRxIqInitTimerBinding::new(action).map(Self::Timer)
            }
            PhyRxIqInitAction::Complete(_) | PhyRxIqInitAction::Failed(_) => {
                Err(PhyRxIqExternalBindingError::UnsupportedAction)
            }
            _ => Err(PhyRxIqExternalBindingError::UnsupportedAction),
        }
    }
}

#[cfg(test)]
mod tests;
