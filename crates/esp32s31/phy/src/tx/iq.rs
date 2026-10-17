//! Rust-owned ESP32-S31 transmit-IQ calibration.
//!
//! The mandatory archive root is `phy_txiq_cal_init`, size 332 bytes. Its
//! algorithmic ROM graph is `phy_rfcal_txiq`, `phy_txiq_cover`,
//! `phy_txiq_get_mis_pwr`, `phy_txtone_linear_pwr`, and the finite register,
//! PBus and I2C leaves named by their actions below. Formatting-only branches
//! are deliberately absent.
//!
//! Every SAR acquisition, two-microsecond interval, PHY-I2C access, PBus
//! command and RFPLL operation is an externally completed edge. The seven
//! cover iterations are explicit state, so advancing this module never polls,
//! sleeps, allocates, or runs an unbounded loop.

use crate::{
    analog::{
        i2c::{PhyI2cAddress, PhyI2cField, analog_registers},
        pbus::PhyPbusForceTest,
        rfpll::{
            RfpllFrequencyAction, RfpllFrequencyCompletion, RfpllFrequencyFailure,
            RfpllFrequencyRequest, RfpllFrequencyTransition,
        },
        temperature::{
            PhyTemperatureAction, PhyTemperatureCompletion, PhyTemperatureFailure,
            PhyTemperatureOutcome, PhyTemperatureTransition,
        },
    },
    tx::{
        calibration::{
            PhyPowerAttenuationAction, PhyPowerAttenuationCompletion, PhyPowerAttenuationRequest,
            PhyPowerAttenuationTransition, PhyToneSarAction, PhyToneSarCompletion,
            PhyToneSarFailure, PhyToneSarRequest, PhyToneSarTransition,
            PhyTxCalibrationEnvironment, PhyTxCalibrationEnvironmentAction,
            PhyTxCalibrationEnvironmentCompletion, PhyTxCalibrationEnvironmentFailure,
            PhyTxCalibrationEnvironmentTransition, PhyTxCalibrationParameters,
        },
        power_detector::sar_signal_reference,
    },
};

const TX_CAP_ADDRESS: PhyI2cAddress = analog_registers::TX_CAPACITOR_BANKS;
const D_CODE_0_ADDRESS: PhyI2cAddress = analog_registers::RFPLL_EXTERNAL_DCODE_0.address();
const D_CODE_1_ADDRESS: PhyI2cAddress = analog_registers::RFPLL_EXTERNAL_DCODE_1.address();
const TXIQ_COVER_ITERATIONS: u8 = 7;

const fn txiq_coefficient(value: i16, kind: PhyTxIqCoefficientKind) -> i8 {
    match kind {
        PhyTxIqCoefficientKind::Gain => {
            crate::calibration::math::saturate_signed(value as i32, 31, -31) as i8
        }
        PhyTxIqCoefficientKind::Phase => {
            crate::calibration::math::saturate_signed(value as i32, 63, -63) as i8
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqLinearPowerRequest {
    pub identity: u8,
    pub reference_codes: [i16; 2],
    pub clear_tone_after_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqLinearPowerOutcome {
    pub identity: u8,
    pub power: i16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqLinearPowerAction {
    ToneSar(PhyToneSarAction),
    Complete(PhyTxIqLinearPowerOutcome),
    Failed(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqLinearPowerCompletion {
    ToneSar(PhyToneSarCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqLinearPowerTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LinearPowerStep {
    Sample {
        pass: u8,
        transition: PhyToneSarTransition,
    },
    Complete,
    Failed(PhyToneSarFailure),
}

/// Complete bounded translation of ROM `phy_txtone_linear_pwr`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqLinearPowerTransition {
    request: PhyTxIqLinearPowerRequest,
    step: LinearPowerStep,
    accumulator: i16,
}

impl PhyTxIqLinearPowerTransition {
    fn sample(request: PhyTxIqLinearPowerRequest, pass: u8) -> PhyToneSarTransition {
        PhyToneSarTransition::new_nonzero(PhyToneSarRequest {
            measurement: request.identity.wrapping_mul(2).wrapping_add(pass),
            samples: 2,
            clear_tone_after_ready: request.clear_tone_after_ready,
        })
    }

    pub fn new(request: PhyTxIqLinearPowerRequest) -> Self {
        Self {
            request,
            step: LinearPowerStep::Sample {
                pass: 0,
                transition: Self::sample(request, 0),
            },
            accumulator: 0,
        }
    }

    pub const fn action(&self) -> PhyTxIqLinearPowerAction {
        match self.step {
            LinearPowerStep::Sample { transition, .. } => {
                PhyTxIqLinearPowerAction::ToneSar(transition.action())
            }
            LinearPowerStep::Complete => {
                PhyTxIqLinearPowerAction::Complete(PhyTxIqLinearPowerOutcome {
                    identity: self.request.identity,
                    power: self.accumulator,
                })
            }
            LinearPowerStep::Failed(failure) => PhyTxIqLinearPowerAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyTxIqLinearPowerCompletion,
    ) -> Result<(), PhyTxIqLinearPowerTransitionError> {
        match (self.step, completion) {
            (
                LinearPowerStep::Sample {
                    pass,
                    mut transition,
                },
                PhyTxIqLinearPowerCompletion::ToneSar(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqLinearPowerTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyToneSarAction::Complete(outcome) => {
                        let signal =
                            sar_signal_reference(outcome.sample, self.request.reference_codes);
                        let denominator = if signal[1] == 0 { 1 } else { signal[1] };
                        let ratio = ((i32::from(signal[0]) << 10) / i32::from(denominator)) as i16;
                        self.accumulator = self.accumulator.wrapping_add(ratio);
                        self.step = if pass == 1 {
                            LinearPowerStep::Complete
                        } else {
                            LinearPowerStep::Sample {
                                pass: 1,
                                transition: Self::sample(self.request, 1),
                            }
                        };
                    }
                    PhyToneSarAction::Failed(failure) => {
                        self.step = LinearPowerStep::Failed(failure);
                    }
                    _ => self.step = LinearPowerStep::Sample { pass, transition },
                }
            }
            (LinearPowerStep::Complete | LinearPowerStep::Failed(_), _) => {
                return Err(PhyTxIqLinearPowerTransitionError::AlreadyComplete);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqMisPowerRequest {
    pub identity: u8,
    pub polarity: bool,
    pub attenuation: u8,
    pub selector: u16,
    pub reference_codes: [i16; 2],
    pub clear_tone_after_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqMisPowerOutcome {
    pub identity: u8,
    pub power: [i16; 2],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqMisPowerDelayPhase {
    FirstPolarity,
    SecondPolarity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqMisPowerAction {
    Configure {
        identity: u8,
        first: bool,
        polarity: bool,
        attenuation: u8,
        selector: u16,
    },
    DelayMicros {
        identity: u8,
        phase: PhyTxIqMisPowerDelayPhase,
        micros: u32,
    },
    LinearPower(PhyTxIqLinearPowerAction),
    Complete(PhyTxIqMisPowerOutcome),
    Failed(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqMisPowerCompletion {
    Configured {
        identity: u8,
        first: bool,
        polarity: bool,
        attenuation: u8,
        selector: u16,
    },
    DelayElapsed {
        identity: u8,
        phase: PhyTxIqMisPowerDelayPhase,
        micros: u32,
    },
    LinearPower(PhyTxIqLinearPowerCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqMisPowerTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MisPowerStep {
    ConfigureFirst,
    DelayFirst,
    First(PhyTxIqLinearPowerTransition),
    ConfigureSecond,
    DelaySecond,
    Second(PhyTxIqLinearPowerTransition),
    Complete,
    Failed(PhyToneSarFailure),
}

/// Complete translation of ROM `phy_txiq_get_mis_pwr`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqMisPowerTransition {
    request: PhyTxIqMisPowerRequest,
    step: MisPowerStep,
    power: [i16; 2],
}

impl PhyTxIqMisPowerTransition {
    pub const fn new(request: PhyTxIqMisPowerRequest) -> Self {
        Self {
            request,
            step: MisPowerStep::ConfigureFirst,
            power: [0; 2],
        }
    }

    fn linear(&self, phase: u8) -> PhyTxIqLinearPowerTransition {
        PhyTxIqLinearPowerTransition::new(PhyTxIqLinearPowerRequest {
            identity: self.request.identity.wrapping_mul(2).wrapping_add(phase),
            reference_codes: self.request.reference_codes,
            clear_tone_after_ready: self.request.clear_tone_after_ready,
        })
    }

    pub const fn action(&self) -> PhyTxIqMisPowerAction {
        match self.step {
            MisPowerStep::ConfigureFirst => PhyTxIqMisPowerAction::Configure {
                identity: self.request.identity,
                first: true,
                polarity: self.request.polarity,
                attenuation: self.request.attenuation,
                selector: self.request.selector,
            },
            MisPowerStep::DelayFirst => PhyTxIqMisPowerAction::DelayMicros {
                identity: self.request.identity,
                phase: PhyTxIqMisPowerDelayPhase::FirstPolarity,
                micros: 2,
            },
            MisPowerStep::First(transition) | MisPowerStep::Second(transition) => {
                PhyTxIqMisPowerAction::LinearPower(transition.action())
            }
            MisPowerStep::ConfigureSecond => PhyTxIqMisPowerAction::Configure {
                identity: self.request.identity,
                first: false,
                polarity: self.request.polarity,
                attenuation: self.request.attenuation,
                selector: self.request.selector,
            },
            MisPowerStep::DelaySecond => PhyTxIqMisPowerAction::DelayMicros {
                identity: self.request.identity,
                phase: PhyTxIqMisPowerDelayPhase::SecondPolarity,
                micros: 2,
            },
            MisPowerStep::Complete => PhyTxIqMisPowerAction::Complete(PhyTxIqMisPowerOutcome {
                identity: self.request.identity,
                power: self.power,
            }),
            MisPowerStep::Failed(failure) => PhyTxIqMisPowerAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyTxIqMisPowerCompletion,
    ) -> Result<(), PhyTxIqMisPowerTransitionError> {
        match (self.step, completion) {
            (
                MisPowerStep::ConfigureFirst,
                PhyTxIqMisPowerCompletion::Configured {
                    identity,
                    first: true,
                    polarity,
                    attenuation,
                    selector,
                },
            ) if identity == self.request.identity
                && polarity == self.request.polarity
                && attenuation == self.request.attenuation
                && selector == self.request.selector =>
            {
                self.step = MisPowerStep::DelayFirst;
            }
            (
                MisPowerStep::DelayFirst,
                PhyTxIqMisPowerCompletion::DelayElapsed {
                    identity,
                    phase: PhyTxIqMisPowerDelayPhase::FirstPolarity,
                    micros: 2,
                },
            ) if identity == self.request.identity => {
                self.step = MisPowerStep::First(self.linear(0));
            }
            (
                MisPowerStep::First(mut transition),
                PhyTxIqMisPowerCompletion::LinearPower(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqMisPowerTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxIqLinearPowerAction::Complete(outcome) => {
                        self.power[0] = outcome.power;
                        self.step = MisPowerStep::ConfigureSecond;
                    }
                    PhyTxIqLinearPowerAction::Failed(failure) => {
                        self.step = MisPowerStep::Failed(failure);
                    }
                    _ => self.step = MisPowerStep::First(transition),
                }
            }
            (
                MisPowerStep::ConfigureSecond,
                PhyTxIqMisPowerCompletion::Configured {
                    identity,
                    first: false,
                    polarity,
                    attenuation,
                    selector,
                },
            ) if identity == self.request.identity
                && polarity == self.request.polarity
                && attenuation == self.request.attenuation
                && selector == self.request.selector =>
            {
                self.step = MisPowerStep::DelaySecond;
            }
            (
                MisPowerStep::DelaySecond,
                PhyTxIqMisPowerCompletion::DelayElapsed {
                    identity,
                    phase: PhyTxIqMisPowerDelayPhase::SecondPolarity,
                    micros: 2,
                },
            ) if identity == self.request.identity => {
                self.step = MisPowerStep::Second(self.linear(1));
            }
            (
                MisPowerStep::Second(mut transition),
                PhyTxIqMisPowerCompletion::LinearPower(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqMisPowerTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxIqLinearPowerAction::Complete(outcome) => {
                        self.power[1] = outcome.power;
                        self.step = MisPowerStep::Complete;
                    }
                    PhyTxIqLinearPowerAction::Failed(failure) => {
                        self.step = MisPowerStep::Failed(failure);
                    }
                    _ => self.step = MisPowerStep::Second(transition),
                }
            }
            (MisPowerStep::Complete | MisPowerStep::Failed(_), _) => {
                return Err(PhyTxIqMisPowerTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxIqMisPowerTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqCoefficientKind {
    Gain,
    Phase,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqCoverRequest {
    pub identity: u8,
    pub attenuation: u8,
    pub selector: u16,
    pub reference_codes: [i16; 2],
    pub clear_tone_after_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqCoverOutcome {
    pub identity: u8,
    pub gain: i8,
    pub phase: i8,
    pub iterations: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqCoverAction {
    ConfigureCoefficient {
        identity: u8,
        iteration: u8,
        kind: PhyTxIqCoefficientKind,
        value: i8,
    },
    MisPower(PhyTxIqMisPowerAction),
    Complete(PhyTxIqCoverOutcome),
    Failed(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqCoverCompletion {
    CoefficientConfigured {
        identity: u8,
        iteration: u8,
        kind: PhyTxIqCoefficientKind,
        value: i8,
    },
    MisPower(PhyTxIqMisPowerCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqCoverTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CoverStep {
    Gain,
    Phase,
    GainMis(PhyTxIqMisPowerTransition),
    PhaseMis(PhyTxIqMisPowerTransition),
    FinalGain,
    FinalPhase,
    Complete,
    Failed(PhyToneSarFailure),
}

/// Exact seven-iteration coefficient solver from ROM `phy_txiq_cover`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqCoverTransition {
    request: PhyTxIqCoverRequest,
    step: CoverStep,
    iteration: u8,
    gain: i8,
    phase: i8,
    gain_accumulator: i8,
    phase_accumulator: i8,
    gain_delta: i8,
}

impl PhyTxIqCoverTransition {
    pub const fn new(request: PhyTxIqCoverRequest) -> Self {
        Self {
            request,
            step: CoverStep::Gain,
            iteration: 0,
            gain: 0,
            phase: 0,
            gain_accumulator: 0,
            phase_accumulator: 0,
            gain_delta: 0,
        }
    }

    const fn mis_identity(&self, phase: u8) -> u8 {
        self.request
            .identity
            .wrapping_mul(16)
            .wrapping_add(self.iteration.wrapping_mul(2))
            .wrapping_add(phase)
    }

    fn mis(&self, phase: u8) -> PhyTxIqMisPowerTransition {
        let gain_path = phase == 0;
        PhyTxIqMisPowerTransition::new(PhyTxIqMisPowerRequest {
            identity: self.mis_identity(phase),
            polarity: gain_path,
            attenuation: if gain_path {
                self.request.attenuation.saturating_sub(12)
            } else {
                self.request.attenuation
            },
            selector: self.request.selector,
            reference_codes: self.request.reference_codes,
            clear_tone_after_ready: self.request.clear_tone_after_ready,
        })
    }

    pub const fn action(&self) -> PhyTxIqCoverAction {
        match self.step {
            CoverStep::Gain | CoverStep::FinalGain => PhyTxIqCoverAction::ConfigureCoefficient {
                identity: self.request.identity,
                iteration: self.iteration,
                kind: PhyTxIqCoefficientKind::Gain,
                value: txiq_coefficient(self.gain as i16, PhyTxIqCoefficientKind::Gain),
            },
            CoverStep::Phase | CoverStep::FinalPhase => PhyTxIqCoverAction::ConfigureCoefficient {
                identity: self.request.identity,
                iteration: self.iteration,
                kind: PhyTxIqCoefficientKind::Phase,
                value: txiq_coefficient(self.phase as i16, PhyTxIqCoefficientKind::Phase),
            },
            CoverStep::GainMis(transition) | CoverStep::PhaseMis(transition) => {
                PhyTxIqCoverAction::MisPower(transition.action())
            }
            CoverStep::Complete => PhyTxIqCoverAction::Complete(PhyTxIqCoverOutcome {
                identity: self.request.identity,
                gain: self.gain,
                phase: self.phase,
                iterations: TXIQ_COVER_ITERATIONS,
            }),
            CoverStep::Failed(failure) => PhyTxIqCoverAction::Failed(failure),
        }
    }

    fn finish_iteration(&mut self, phase_delta: i8) {
        if self.iteration < 3 {
            self.gain = self.gain.wrapping_sub(self.gain_delta);
            self.phase = self.phase.wrapping_sub(phase_delta);
        } else {
            self.gain_accumulator = self.gain_accumulator.wrapping_add(self.gain_delta);
            self.phase_accumulator = self.phase_accumulator.wrapping_add(phase_delta);
            if self.iteration == 6 {
                let gain_average = self.gain_accumulator.wrapping_add(2) >> 2;
                let phase_average = self.phase_accumulator.wrapping_add(2) >> 2;
                self.gain = self.gain.wrapping_sub(gain_average);
                self.phase = self.phase.wrapping_sub(phase_average);
                self.gain = txiq_coefficient(self.gain as i16, PhyTxIqCoefficientKind::Gain);
                self.phase = txiq_coefficient(self.phase as i16, PhyTxIqCoefficientKind::Phase);
                self.step = CoverStep::FinalGain;
                return;
            }
        }
        self.iteration += 1;
        self.step = CoverStep::Gain;
    }

    pub fn advance(
        &mut self,
        completion: PhyTxIqCoverCompletion,
    ) -> Result<(), PhyTxIqCoverTransitionError> {
        match (self.step, completion) {
            (
                CoverStep::Gain,
                PhyTxIqCoverCompletion::CoefficientConfigured {
                    identity,
                    iteration,
                    kind: PhyTxIqCoefficientKind::Gain,
                    value,
                },
            ) if identity == self.request.identity
                && iteration == self.iteration
                && value == txiq_coefficient(self.gain as i16, PhyTxIqCoefficientKind::Gain) =>
            {
                self.gain = value;
                self.step = CoverStep::Phase;
            }
            (
                CoverStep::Phase,
                PhyTxIqCoverCompletion::CoefficientConfigured {
                    identity,
                    iteration,
                    kind: PhyTxIqCoefficientKind::Phase,
                    value,
                },
            ) if identity == self.request.identity
                && iteration == self.iteration
                && value == txiq_coefficient(self.phase as i16, PhyTxIqCoefficientKind::Phase) =>
            {
                self.phase = value;
                self.step = CoverStep::GainMis(self.mis(0));
            }
            (CoverStep::GainMis(mut transition), PhyTxIqCoverCompletion::MisPower(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqCoverTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxIqMisPowerAction::Complete(outcome) => {
                        let minimum = i32::from(outcome.power[0].min(outcome.power[1]));
                        let minimum = if minimum == 0 { 1 } else { minimum };
                        let delta = ((i32::from(outcome.power[1]) - i32::from(outcome.power[0]))
                            * 0x800
                            / minimum
                            + 0x10)
                            >> 5;
                        self.gain_delta = delta as i8;
                        self.step = CoverStep::PhaseMis(self.mis(1));
                    }
                    PhyTxIqMisPowerAction::Failed(failure) => {
                        self.step = CoverStep::Failed(failure);
                    }
                    _ => self.step = CoverStep::GainMis(transition),
                }
            }
            (CoverStep::PhaseMis(mut transition), PhyTxIqCoverCompletion::MisPower(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqCoverTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxIqMisPowerAction::Complete(outcome) => {
                        let sum = outcome.power[0].wrapping_add(outcome.power[1]);
                        let denominator = if sum == 0 { 1 } else { sum };
                        let delta = ((i32::from(outcome.power[0]) - i32::from(outcome.power[1]))
                            * 0x1000
                            / i32::from(denominator)
                            + 8)
                            >> 4;
                        self.finish_iteration(delta as i8);
                    }
                    PhyTxIqMisPowerAction::Failed(failure) => {
                        self.step = CoverStep::Failed(failure);
                    }
                    _ => self.step = CoverStep::PhaseMis(transition),
                }
            }
            (
                CoverStep::FinalGain,
                PhyTxIqCoverCompletion::CoefficientConfigured {
                    identity,
                    iteration: 6,
                    kind: PhyTxIqCoefficientKind::Gain,
                    value,
                },
            ) if identity == self.request.identity && value == self.gain => {
                self.step = CoverStep::FinalPhase;
            }
            (
                CoverStep::FinalPhase,
                PhyTxIqCoverCompletion::CoefficientConfigured {
                    identity,
                    iteration: 6,
                    kind: PhyTxIqCoefficientKind::Phase,
                    value,
                },
            ) if identity == self.request.identity && value == self.phase => {
                self.step = CoverStep::Complete;
            }
            (CoverStep::Complete | CoverStep::Failed(_), _) => {
                return Err(PhyTxIqCoverTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxIqCoverTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqLoopbackAction {
    I2c(oer_radio_analog::Action<crate::analog::i2c::PhyI2cAddress, ()>),
    ConfigureTxClock { enabled: bool },
    ConfigureRxClock { enabled: bool },
    Complete { enabled: bool },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqLoopbackCompletion {
    I2c(oer_radio_analog::Completion<crate::analog::i2c::PhyI2cAddress>),
    TxClockConfigured { enabled: bool },
    RxClockConfigured { enabled: bool },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqLoopbackTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoopbackStep {
    I2c(oer_radio_analog::FieldWriteTransition<crate::analog::i2c::PhyI2cAddress>),
    TxClock,
    RxClock,
    Complete,
}

/// Exact three-edge expansion of ROM `phy_loopback_mode_en`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqLoopbackTransition {
    enabled: bool,
    step: LoopbackStep,
}

impl PhyTxIqLoopbackTransition {
    pub fn new(enabled: bool) -> Self {
        let transition =
            crate::analog::i2c::field_write(analog_registers::TX_IQ_LOOPBACK_ENABLE, enabled as u8);
        Self {
            enabled,
            step: LoopbackStep::I2c(transition),
        }
    }

    pub const fn action(&self) -> PhyTxIqLoopbackAction {
        match self.step {
            LoopbackStep::I2c(transition) => PhyTxIqLoopbackAction::I2c(transition.action()),
            LoopbackStep::TxClock => PhyTxIqLoopbackAction::ConfigureTxClock {
                enabled: self.enabled,
            },
            LoopbackStep::RxClock => PhyTxIqLoopbackAction::ConfigureRxClock {
                enabled: self.enabled,
            },
            LoopbackStep::Complete => PhyTxIqLoopbackAction::Complete {
                enabled: self.enabled,
            },
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyTxIqLoopbackCompletion,
    ) -> Result<(), PhyTxIqLoopbackTransitionError> {
        match (self.step, completion) {
            (LoopbackStep::I2c(mut transition), PhyTxIqLoopbackCompletion::I2c(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqLoopbackTransitionError::WrongCompletion)?;
                self.step = if transition.action() == oer_radio_analog::Action::Complete(()) {
                    LoopbackStep::TxClock
                } else {
                    LoopbackStep::I2c(transition)
                };
            }
            (LoopbackStep::TxClock, PhyTxIqLoopbackCompletion::TxClockConfigured { enabled })
                if enabled == self.enabled =>
            {
                self.step = LoopbackStep::RxClock;
            }
            (LoopbackStep::RxClock, PhyTxIqLoopbackCompletion::RxClockConfigured { enabled })
                if enabled == self.enabled =>
            {
                self.step = LoopbackStep::Complete;
            }
            (LoopbackStep::Complete, _) => {
                return Err(PhyTxIqLoopbackTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxIqLoopbackTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqCalibrationVariant {
    Initial,
    Loopback,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqCalibrationRequest {
    pub identity: u8,
    pub variant: PhyTxIqCalibrationVariant,
    pub environment: PhyTxCalibrationParameters,
    pub attenuation: u8,
    pub selector: u16,
    pub power_offset: i16,
    pub reference_codes: [i16; 2],
    pub clear_tone_after_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqCalibrationOutcome {
    pub identity: u8,
    pub coefficient: u16,
    pub gain: i8,
    pub phase: i8,
    pub attenuation: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqCalibrationFailure {
    Environment(PhyTxCalibrationEnvironmentFailure),
    PbusTimedOut(PhyPbusForceTest),
    ToneSar(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqCalibrationAction {
    ConfigureCorrection { begin: bool },
    ConfigurePbusDebugMode,
    Loopback(PhyTxIqLoopbackAction),
    ForcePbus(PhyPbusForceTest),
    Environment(PhyTxCalibrationEnvironmentAction),
    PrepareToneControlRestore,
    PowerAttenuation(PhyPowerAttenuationAction),
    Cover(PhyTxIqCoverAction),
    RestoreToneControl,
    Complete(PhyTxIqCalibrationOutcome),
    Failed(PhyTxIqCalibrationFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqCalibrationCompletion {
    CorrectionConfigured { begin: bool },
    PbusDebugModeConfigured,
    Loopback(PhyTxIqLoopbackCompletion),
    PbusCompleted(PhyPbusForceTest),
    PbusTimedOut(PhyPbusForceTest),
    Environment(PhyTxCalibrationEnvironmentCompletion),
    ToneControlRestorePrepared,
    PowerAttenuation(PhyPowerAttenuationCompletion),
    Cover(PhyTxIqCoverCompletion),
    ToneControlRestored,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqCalibrationTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CalibrationTerminal {
    Complete,
    Failed(PhyTxIqCalibrationFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CalibrationStep {
    Begin,
    Enter(PhyTxCalibrationEnvironmentTransition),
    PbusDebug,
    EnableLoopback(PhyTxIqLoopbackTransition),
    LoopbackGain {
        index: u8,
    },
    PrepareRestore,
    PowerAttenuation(PhyPowerAttenuationTransition),
    Cover(PhyTxIqCoverTransition),
    Exit {
        terminal: CalibrationTerminal,
        transition: PhyTxCalibrationEnvironmentTransition,
    },
    Restore {
        terminal: CalibrationTerminal,
    },
    DisableLoopback {
        terminal: CalibrationTerminal,
        transition: PhyTxIqLoopbackTransition,
    },
    Finish {
        terminal: CalibrationTerminal,
    },
    Complete,
    Failed(PhyTxIqCalibrationFailure),
}

const fn loopback_gain_transaction(index: u8, parameter_002: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(4, 2, 1),
        1 => PhyPbusForceTest::new(4, 1, 0x83),
        2 => PhyPbusForceTest::new(5, 1, 0x1df),
        3 => PhyPbusForceTest::new(0, 1, 0),
        4 => PhyPbusForceTest::new(0, 2, parameter_002 as u16),
        5 => PhyPbusForceTest::new(1, 1, 0x1f9),
        _ => PhyPbusForceTest::new(1, 2, 0),
    }
}

/// Complete heap-free composition of ROM `phy_rfcal_txiq`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqCalibrationTransition {
    request: PhyTxIqCalibrationRequest,
    step: CalibrationStep,
    tone_control_restore_prepared: bool,
    attenuation: u8,
    gain: i8,
    phase: i8,
}

impl PhyTxIqCalibrationTransition {
    pub const fn new(request: PhyTxIqCalibrationRequest) -> Self {
        Self {
            request,
            step: CalibrationStep::Begin,
            tone_control_restore_prepared: false,
            attenuation: request.attenuation,
            gain: 0,
            phase: 0,
        }
    }

    const fn coefficient(&self) -> u16 {
        ((self.gain as u8 as u16 & 0x3f) << 7) | (self.phase as u8 as u16 & 0x7f)
    }

    fn fail(&mut self, failure: PhyTxIqCalibrationFailure) {
        self.step = CalibrationStep::Exit {
            terminal: CalibrationTerminal::Failed(failure),
            transition: PhyTxCalibrationEnvironmentTransition::exit(self.request.environment),
        };
    }

    fn after_exit(&mut self, terminal: CalibrationTerminal) {
        self.step = if self.tone_control_restore_prepared {
            CalibrationStep::Restore { terminal }
        } else if self.request.variant == PhyTxIqCalibrationVariant::Loopback {
            CalibrationStep::DisableLoopback {
                terminal,
                transition: PhyTxIqLoopbackTransition::new(false),
            }
        } else {
            CalibrationStep::Finish { terminal }
        };
    }

    fn after_restore(&mut self, terminal: CalibrationTerminal) {
        self.step = if self.request.variant == PhyTxIqCalibrationVariant::Loopback {
            CalibrationStep::DisableLoopback {
                terminal,
                transition: PhyTxIqLoopbackTransition::new(false),
            }
        } else {
            CalibrationStep::Finish { terminal }
        };
    }

    pub const fn action(&self) -> PhyTxIqCalibrationAction {
        match self.step {
            CalibrationStep::Begin => PhyTxIqCalibrationAction::ConfigureCorrection { begin: true },
            CalibrationStep::Enter(transition) | CalibrationStep::Exit { transition, .. } => {
                PhyTxIqCalibrationAction::Environment(transition.action())
            }
            CalibrationStep::PbusDebug => PhyTxIqCalibrationAction::ConfigurePbusDebugMode,
            CalibrationStep::EnableLoopback(transition)
            | CalibrationStep::DisableLoopback { transition, .. } => {
                PhyTxIqCalibrationAction::Loopback(transition.action())
            }
            CalibrationStep::LoopbackGain { index } => PhyTxIqCalibrationAction::ForcePbus(
                loopback_gain_transaction(index, self.request.environment.pbus_rx_path_value),
            ),
            CalibrationStep::PrepareRestore => PhyTxIqCalibrationAction::PrepareToneControlRestore,
            CalibrationStep::PowerAttenuation(transition) => {
                PhyTxIqCalibrationAction::PowerAttenuation(transition.action())
            }
            CalibrationStep::Cover(transition) => {
                PhyTxIqCalibrationAction::Cover(transition.action())
            }
            CalibrationStep::Restore { .. } => PhyTxIqCalibrationAction::RestoreToneControl,
            CalibrationStep::Finish { .. } => {
                PhyTxIqCalibrationAction::ConfigureCorrection { begin: false }
            }
            CalibrationStep::Complete => {
                PhyTxIqCalibrationAction::Complete(PhyTxIqCalibrationOutcome {
                    identity: self.request.identity,
                    coefficient: self.coefficient(),
                    gain: self.gain,
                    phase: self.phase,
                    attenuation: self.attenuation,
                })
            }
            CalibrationStep::Failed(failure) => PhyTxIqCalibrationAction::Failed(failure),
        }
    }

    fn power_attenuation(&self) -> PhyPowerAttenuationTransition {
        PhyPowerAttenuationTransition::new(PhyPowerAttenuationRequest {
            tone_selector: self.request.selector,
            initial_attenuation: self.request.attenuation,
            target_power: 0x40,
            power_offset: self.request.power_offset,
            reference_codes: self.request.reference_codes,
        })
    }

    fn cover(&self) -> PhyTxIqCoverTransition {
        PhyTxIqCoverTransition::new(PhyTxIqCoverRequest {
            identity: self.request.identity,
            attenuation: self.attenuation,
            selector: self.request.selector,
            reference_codes: self.request.reference_codes,
            clear_tone_after_ready: self.request.clear_tone_after_ready,
        })
    }

    pub fn advance(
        &mut self,
        completion: PhyTxIqCalibrationCompletion,
    ) -> Result<(), PhyTxIqCalibrationTransitionError> {
        match (self.step, completion) {
            (
                CalibrationStep::Begin,
                PhyTxIqCalibrationCompletion::CorrectionConfigured { begin: true },
            ) => {
                self.step = match self.request.variant {
                    PhyTxIqCalibrationVariant::Initial => CalibrationStep::Enter(
                        PhyTxCalibrationEnvironmentTransition::enter(self.request.environment),
                    ),
                    PhyTxIqCalibrationVariant::Loopback => CalibrationStep::PbusDebug,
                };
            }
            (
                CalibrationStep::Enter(mut transition),
                PhyTxIqCalibrationCompletion::Environment(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqCalibrationTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxCalibrationEnvironmentAction::Complete(
                        PhyTxCalibrationEnvironment::Debug,
                    ) => self.step = CalibrationStep::PrepareRestore,
                    PhyTxCalibrationEnvironmentAction::Failed(failure) => {
                        self.fail(PhyTxIqCalibrationFailure::Environment(failure));
                    }
                    _ => self.step = CalibrationStep::Enter(transition),
                }
            }
            (CalibrationStep::PbusDebug, PhyTxIqCalibrationCompletion::PbusDebugModeConfigured) => {
                self.step = CalibrationStep::EnableLoopback(PhyTxIqLoopbackTransition::new(true));
            }
            (
                CalibrationStep::EnableLoopback(mut transition),
                PhyTxIqCalibrationCompletion::Loopback(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqCalibrationTransitionError::WrongCompletion)?;
                self.step = if matches!(
                    transition.action(),
                    PhyTxIqLoopbackAction::Complete { enabled: true }
                ) {
                    CalibrationStep::LoopbackGain { index: 0 }
                } else {
                    CalibrationStep::EnableLoopback(transition)
                };
            }
            (
                CalibrationStep::LoopbackGain { index },
                PhyTxIqCalibrationCompletion::PbusCompleted(transaction),
            ) if transaction
                == loopback_gain_transaction(
                    index,
                    self.request.environment.pbus_rx_path_value,
                ) =>
            {
                self.step = if index == 6 {
                    CalibrationStep::PrepareRestore
                } else {
                    CalibrationStep::LoopbackGain { index: index + 1 }
                };
            }
            (
                CalibrationStep::LoopbackGain { index },
                PhyTxIqCalibrationCompletion::PbusTimedOut(transaction),
            ) if transaction
                == loopback_gain_transaction(
                    index,
                    self.request.environment.pbus_rx_path_value,
                ) =>
            {
                self.fail(PhyTxIqCalibrationFailure::PbusTimedOut(transaction));
            }
            (
                CalibrationStep::PrepareRestore,
                PhyTxIqCalibrationCompletion::ToneControlRestorePrepared,
            ) => {
                self.tone_control_restore_prepared = true;
                self.step = CalibrationStep::PowerAttenuation(self.power_attenuation());
            }
            (
                CalibrationStep::PowerAttenuation(mut transition),
                PhyTxIqCalibrationCompletion::PowerAttenuation(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqCalibrationTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyPowerAttenuationAction::Complete(outcome) => {
                        self.attenuation = outcome.attenuation;
                        self.step = CalibrationStep::Cover(self.cover());
                    }
                    PhyPowerAttenuationAction::Failed(failure) => {
                        self.fail(PhyTxIqCalibrationFailure::ToneSar(failure));
                    }
                    _ => self.step = CalibrationStep::PowerAttenuation(transition),
                }
            }
            (
                CalibrationStep::Cover(mut transition),
                PhyTxIqCalibrationCompletion::Cover(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqCalibrationTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxIqCoverAction::Complete(outcome) => {
                        self.gain =
                            txiq_coefficient(outcome.gain as i16, PhyTxIqCoefficientKind::Gain);
                        self.phase =
                            txiq_coefficient(outcome.phase as i16, PhyTxIqCoefficientKind::Phase);
                        self.step = CalibrationStep::Exit {
                            terminal: CalibrationTerminal::Complete,
                            transition: PhyTxCalibrationEnvironmentTransition::exit(
                                self.request.environment,
                            ),
                        };
                    }
                    PhyTxIqCoverAction::Failed(failure) => {
                        self.fail(PhyTxIqCalibrationFailure::ToneSar(failure));
                    }
                    _ => self.step = CalibrationStep::Cover(transition),
                }
            }
            (
                CalibrationStep::Exit {
                    terminal,
                    mut transition,
                },
                PhyTxIqCalibrationCompletion::Environment(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqCalibrationTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxCalibrationEnvironmentAction::Complete(
                        PhyTxCalibrationEnvironment::Work,
                    ) => self.after_exit(terminal),
                    PhyTxCalibrationEnvironmentAction::Failed(failure) => {
                        self.after_exit(CalibrationTerminal::Failed(
                            PhyTxIqCalibrationFailure::Environment(failure),
                        ));
                    }
                    _ => {
                        self.step = CalibrationStep::Exit {
                            terminal,
                            transition,
                        }
                    }
                }
            }
            (
                CalibrationStep::Restore { terminal },
                PhyTxIqCalibrationCompletion::ToneControlRestored,
            ) => {
                self.tone_control_restore_prepared = false;
                self.after_restore(terminal);
            }
            (
                CalibrationStep::DisableLoopback {
                    terminal,
                    mut transition,
                },
                PhyTxIqCalibrationCompletion::Loopback(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqCalibrationTransitionError::WrongCompletion)?;
                self.step = if matches!(
                    transition.action(),
                    PhyTxIqLoopbackAction::Complete { enabled: false }
                ) {
                    CalibrationStep::Finish { terminal }
                } else {
                    CalibrationStep::DisableLoopback {
                        terminal,
                        transition,
                    }
                };
            }
            (
                CalibrationStep::Finish { terminal },
                PhyTxIqCalibrationCompletion::CorrectionConfigured { begin: false },
            ) => {
                self.step = match terminal {
                    CalibrationTerminal::Complete => CalibrationStep::Complete,
                    CalibrationTerminal::Failed(failure) => CalibrationStep::Failed(failure),
                };
            }
            (CalibrationStep::Complete | CalibrationStep::Failed(_), _) => {
                return Err(PhyTxIqCalibrationTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxIqCalibrationTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqInitParameters {
    pub already_calibrated: bool,
    pub crystal_selector: u8,
    pub environment: PhyTxCalibrationParameters,
    pub capacitance: [u8; 6],
    pub channel_6_dcode: [u8; 2],
    pub initial_attenuation: i8,
    pub power_offset: i16,
    pub reference_codes: [i16; 2],
    pub clear_tone_after_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqInitOutcome {
    pub coefficient: [u16; 2],
    pub external_dcode: [u8; 2],
    pub temperature: Option<PhyTemperatureOutcome>,
    pub calibration_performed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqInitFailure {
    Rfpll(RfpllFrequencyFailure),
    Calibration(PhyTxIqCalibrationFailure),
    Temperature(PhyTemperatureFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqInitAction {
    Rfpll(RfpllFrequencyAction),
    WriteI2c { address: PhyI2cAddress, value: u8 },
    ReadI2cMasked { field: PhyI2cField },
    Calibration(PhyTxIqCalibrationAction),
    Temperature(PhyTemperatureAction),
    Complete(PhyTxIqInitOutcome),
    Failed(PhyTxIqInitFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqInitCompletion {
    Rfpll(RfpllFrequencyCompletion),
    I2cWritten { address: PhyI2cAddress, value: u8 },
    I2cMaskedRead { field: PhyI2cField, value: u8 },
    Calibration(PhyTxIqCalibrationCompletion),
    Temperature(PhyTemperatureCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqInitTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InitStep {
    Rfpll(RfpllFrequencyTransition),
    TxCap,
    Dcode0,
    Dcode1,
    ReadDcodeMode,
    ReadDcode0 {
        external: bool,
    },
    ReadDcode1 {
        external: bool,
    },
    Calibration {
        phase: u8,
        transition: PhyTxIqCalibrationTransition,
    },
    Temperature(PhyTemperatureTransition),
    Complete,
    Failed(PhyTxIqInitFailure),
}

/// Complete Rust-owned composition of archive `phy_txiq_cal_init`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqInitTransition {
    parameters: PhyTxIqInitParameters,
    step: InitStep,
    attenuation: u8,
    coefficient: [u16; 2],
    external_dcode: [u8; 2],
    temperature: Option<PhyTemperatureOutcome>,
}

impl PhyTxIqInitTransition {
    pub const fn new(parameters: PhyTxIqInitParameters) -> Self {
        let attenuation = crate::calibration::math::saturate_signed(
            parameters.initial_attenuation as i32,
            0x78,
            0,
        ) as u8;
        let step = if parameters.already_calibrated {
            InitStep::Complete
        } else {
            InitStep::Rfpll(RfpllFrequencyTransition::new(RfpllFrequencyRequest {
                crystal_selector: parameters.crystal_selector,
                frequency_code: 0x985,
                offset: 0,
            }))
        };
        Self {
            parameters,
            step,
            attenuation,
            coefficient: [0; 2],
            external_dcode: [0; 2],
            temperature: None,
        }
    }

    const fn outcome(&self) -> PhyTxIqInitOutcome {
        PhyTxIqInitOutcome {
            coefficient: self.coefficient,
            external_dcode: self.external_dcode,
            temperature: self.temperature,
            calibration_performed: !self.parameters.already_calibrated,
        }
    }

    fn calibration(&self, phase: u8) -> PhyTxIqCalibrationTransition {
        PhyTxIqCalibrationTransition::new(PhyTxIqCalibrationRequest {
            identity: phase,
            variant: if phase == 0 {
                PhyTxIqCalibrationVariant::Initial
            } else {
                PhyTxIqCalibrationVariant::Loopback
            },
            environment: self.parameters.environment,
            attenuation: if phase == 0 {
                self.attenuation
            } else {
                self.attenuation.saturating_sub(0x28)
            },
            selector: 0x80,
            power_offset: self.parameters.power_offset,
            reference_codes: self.parameters.reference_codes,
            clear_tone_after_ready: self.parameters.clear_tone_after_ready,
        })
    }

    pub const fn action(&self) -> PhyTxIqInitAction {
        match self.step {
            InitStep::Rfpll(transition) => PhyTxIqInitAction::Rfpll(transition.action()),
            InitStep::TxCap => PhyTxIqInitAction::WriteI2c {
                address: TX_CAP_ADDRESS,
                value: self.parameters.capacitance[2] | 0xc0,
            },
            InitStep::Dcode0 => PhyTxIqInitAction::WriteI2c {
                address: D_CODE_0_ADDRESS,
                value: (self.parameters.channel_6_dcode[0] & 0x3f) | 0x40,
            },
            InitStep::Dcode1 => PhyTxIqInitAction::WriteI2c {
                address: D_CODE_1_ADDRESS,
                value: (self.parameters.channel_6_dcode[1] & 0x3f) | 0x40,
            },
            InitStep::ReadDcodeMode => PhyTxIqInitAction::ReadI2cMasked {
                field: analog_registers::RFPLL_DCODE_0_SOURCE_SELECT,
            },
            InitStep::ReadDcode0 { external } => PhyTxIqInitAction::ReadI2cMasked {
                field: if external {
                    analog_registers::RFPLL_EXTERNAL_DCODE_0
                } else {
                    analog_registers::RFPLL_INTERNAL_DCODE_0
                },
            },
            InitStep::ReadDcode1 { external } => PhyTxIqInitAction::ReadI2cMasked {
                field: if external {
                    analog_registers::RFPLL_EXTERNAL_DCODE_1
                } else {
                    analog_registers::RFPLL_INTERNAL_DCODE_1
                },
            },
            InitStep::Calibration { transition, .. } => {
                PhyTxIqInitAction::Calibration(transition.action())
            }
            InitStep::Temperature(transition) => {
                PhyTxIqInitAction::Temperature(transition.action())
            }
            InitStep::Complete => PhyTxIqInitAction::Complete(self.outcome()),
            InitStep::Failed(failure) => PhyTxIqInitAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyTxIqInitCompletion,
    ) -> Result<(), PhyTxIqInitTransitionError> {
        match (self.step, completion) {
            (InitStep::Rfpll(mut transition), PhyTxIqInitCompletion::Rfpll(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqInitTransitionError::WrongCompletion)?;
                match transition.action() {
                    RfpllFrequencyAction::Complete(_) => self.step = InitStep::TxCap,
                    RfpllFrequencyAction::Failed(failure) => {
                        self.step = InitStep::Failed(PhyTxIqInitFailure::Rfpll(failure));
                    }
                    _ => self.step = InitStep::Rfpll(transition),
                }
            }
            (
                InitStep::TxCap,
                PhyTxIqInitCompletion::I2cWritten {
                    address: TX_CAP_ADDRESS,
                    value,
                },
            ) if value == self.parameters.capacitance[2] | 0xc0 => {
                self.step = if self.parameters.channel_6_dcode[0] != 0
                    && self.parameters.channel_6_dcode[1] != 0
                {
                    InitStep::Dcode0
                } else {
                    InitStep::ReadDcodeMode
                };
            }
            (
                InitStep::Dcode0,
                PhyTxIqInitCompletion::I2cWritten {
                    address: D_CODE_0_ADDRESS,
                    value,
                },
            ) if value == (self.parameters.channel_6_dcode[0] & 0x3f) | 0x40 => {
                self.step = InitStep::Dcode1;
            }
            (
                InitStep::Dcode1,
                PhyTxIqInitCompletion::I2cWritten {
                    address: D_CODE_1_ADDRESS,
                    value,
                },
            ) if value == (self.parameters.channel_6_dcode[1] & 0x3f) | 0x40 => {
                self.step = InitStep::ReadDcodeMode;
            }
            (
                InitStep::ReadDcodeMode,
                PhyTxIqInitCompletion::I2cMaskedRead {
                    field: analog_registers::RFPLL_DCODE_0_SOURCE_SELECT,
                    value,
                },
            ) => {
                self.step = InitStep::ReadDcode0 {
                    external: value & 1 != 0,
                };
            }
            (
                InitStep::ReadDcode0 { external },
                PhyTxIqInitCompletion::I2cMaskedRead { field, value },
            ) if field
                == if external {
                    analog_registers::RFPLL_EXTERNAL_DCODE_0
                } else {
                    analog_registers::RFPLL_INTERNAL_DCODE_0
                } =>
            {
                self.external_dcode[0] = value & 0x3f;
                self.step = InitStep::ReadDcode1 { external };
            }
            (
                InitStep::ReadDcode1 { external },
                PhyTxIqInitCompletion::I2cMaskedRead { field, value },
            ) if field
                == if external {
                    analog_registers::RFPLL_EXTERNAL_DCODE_1
                } else {
                    analog_registers::RFPLL_INTERNAL_DCODE_1
                } =>
            {
                self.external_dcode[1] = value & 0x3f;
                self.step = InitStep::Calibration {
                    phase: 0,
                    transition: self.calibration(0),
                };
            }
            (
                InitStep::Calibration {
                    phase,
                    mut transition,
                },
                PhyTxIqInitCompletion::Calibration(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqInitTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxIqCalibrationAction::Complete(outcome) => {
                        self.coefficient[phase as usize] = outcome.coefficient;
                        if phase == 0 {
                            self.step = InitStep::Temperature(PhyTemperatureTransition::new());
                        } else {
                            self.step = InitStep::Complete;
                        }
                    }
                    PhyTxIqCalibrationAction::Failed(failure) => {
                        self.step = InitStep::Failed(PhyTxIqInitFailure::Calibration(failure));
                    }
                    _ => self.step = InitStep::Calibration { phase, transition },
                }
            }
            (
                InitStep::Temperature(mut transition),
                PhyTxIqInitCompletion::Temperature(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqInitTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTemperatureAction::Complete(outcome) => {
                        self.temperature = Some(outcome);
                        self.step = InitStep::Calibration {
                            phase: 1,
                            transition: self.calibration(1),
                        };
                    }
                    PhyTemperatureAction::Failed(failure) => {
                        self.step = InitStep::Failed(PhyTxIqInitFailure::Temperature(failure));
                    }
                    _ => self.step = InitStep::Temperature(transition),
                }
            }
            (InitStep::Complete | InitStep::Failed(_), _) => {
                return Err(PhyTxIqInitTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxIqInitTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqBindingError {
    NotDirectMmio,
}

/// A TX-IQ restore-slot invariant was violated by target execution.
///
/// This is not a model completion. The target runner must retain and poison
/// the unique hardware epoch instead of attempting ordinary cleanup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqHardwareInvariant {
    /// A second calibration tried to replace a pending restore image.
    RestoreAlreadyPending,
    /// Cleanup reached restore without a successful prepare operation.
    RestoreNotPending,
}

/// Non-cloneable direct-MMIO token for TXIQ-specific register edges.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxIqMmioBinding {
    action: PhyTxIqCalibrationAction,
}

impl PhyTxIqMmioBinding {
    pub fn new(action: PhyTxIqCalibrationAction) -> Result<Self, PhyTxIqBindingError> {
        match action {
            PhyTxIqCalibrationAction::ConfigureCorrection { .. }
            | PhyTxIqCalibrationAction::ConfigurePbusDebugMode
            | PhyTxIqCalibrationAction::PrepareToneControlRestore
            | PhyTxIqCalibrationAction::RestoreToneControl => Ok(Self { action }),
            _ => Err(PhyTxIqBindingError::NotDirectMmio),
        }
    }

    #[cfg(feature = "validation-probes")]
    pub const fn action(&self) -> PhyTxIqCalibrationAction {
        self.action
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> Result<PhyTxIqCalibrationCompletion, PhyTxIqHardwareInvariant> {
        match self.action {
            PhyTxIqCalibrationAction::ConfigureCorrection { begin } => {
                crate::hardware::configure_phy_txiq_correction(registers, begin);
                Ok(PhyTxIqCalibrationCompletion::CorrectionConfigured { begin })
            }
            PhyTxIqCalibrationAction::ConfigurePbusDebugMode => {
                oer_esp32s31_hal::phy::pbus::configure_debug_mode(registers);
                Ok(PhyTxIqCalibrationCompletion::PbusDebugModeConfigured)
            }
            PhyTxIqCalibrationAction::PrepareToneControlRestore => {
                crate::hardware::prepare_phy_txiq_tone_control_restore(registers)
                    .map_err(|_| PhyTxIqHardwareInvariant::RestoreAlreadyPending)?;
                Ok(PhyTxIqCalibrationCompletion::ToneControlRestorePrepared)
            }
            PhyTxIqCalibrationAction::RestoreToneControl => {
                crate::hardware::restore_phy_txiq_tone_control(registers)
                    .map_err(|_| PhyTxIqHardwareInvariant::RestoreNotPending)?;
                Ok(PhyTxIqCalibrationCompletion::ToneControlRestored)
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxIqMisPowerMmioBinding {
    action: PhyTxIqMisPowerAction,
}

impl PhyTxIqMisPowerMmioBinding {
    pub fn new(action: PhyTxIqMisPowerAction) -> Result<Self, PhyTxIqBindingError> {
        match action {
            PhyTxIqMisPowerAction::Configure { .. } => Ok(Self { action }),
            _ => Err(PhyTxIqBindingError::NotDirectMmio),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> PhyTxIqMisPowerCompletion {
        match self.action {
            PhyTxIqMisPowerAction::Configure {
                identity,
                first,
                polarity,
                attenuation,
                selector,
            } => {
                crate::hardware::configure_phy_txiq_mis_power(
                    registers,
                    first,
                    polarity,
                    attenuation,
                    selector,
                );
                PhyTxIqMisPowerCompletion::Configured {
                    identity,
                    first,
                    polarity,
                    attenuation,
                    selector,
                }
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxIqCoverMmioBinding {
    action: PhyTxIqCoverAction,
}

impl PhyTxIqCoverMmioBinding {
    pub fn new(action: PhyTxIqCoverAction) -> Result<Self, PhyTxIqBindingError> {
        match action {
            PhyTxIqCoverAction::ConfigureCoefficient { .. } => Ok(Self { action }),
            _ => Err(PhyTxIqBindingError::NotDirectMmio),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> PhyTxIqCoverCompletion {
        match self.action {
            PhyTxIqCoverAction::ConfigureCoefficient {
                identity,
                iteration,
                kind,
                value,
            } => {
                crate::hardware::configure_phy_txiq_coefficient(registers, kind, value);
                PhyTxIqCoverCompletion::CoefficientConfigured {
                    identity,
                    iteration,
                    kind,
                    value,
                }
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxIqLoopbackMmioBinding {
    action: PhyTxIqLoopbackAction,
}

impl PhyTxIqLoopbackMmioBinding {
    pub fn new(action: PhyTxIqLoopbackAction) -> Result<Self, PhyTxIqBindingError> {
        match action {
            PhyTxIqLoopbackAction::ConfigureTxClock { .. }
            | PhyTxIqLoopbackAction::ConfigureRxClock { .. } => Ok(Self { action }),
            _ => Err(PhyTxIqBindingError::NotDirectMmio),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> PhyTxIqLoopbackCompletion {
        match self.action {
            PhyTxIqLoopbackAction::ConfigureTxClock { enabled } => {
                oer_esp32s31_hal::phy::pbus::configure_tx_clock(registers, enabled);
                PhyTxIqLoopbackCompletion::TxClockConfigured { enabled }
            }
            PhyTxIqLoopbackAction::ConfigureRxClock { enabled } => {
                oer_esp32s31_hal::phy::pbus::configure_rx_clock(registers, enabled);
                PhyTxIqLoopbackCompletion::RxClockConfigured { enabled }
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqExternalBindingError {
    UnsupportedAction,
    IncompleteTransaction,
    UnexpectedOutcome,
    Pbus(crate::analog::pbus::PhyPbusHardwareBindingError),
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxIqLinearPowerExternalBinding {
    ToneSar(crate::tx::calibration::PhyToneSarExternalBinding),
}

impl PhyTxIqLinearPowerExternalBinding {
    pub fn lower(action: PhyTxIqLinearPowerAction) -> Result<Self, PhyTxIqExternalBindingError> {
        match action {
            PhyTxIqLinearPowerAction::ToneSar(action) => {
                crate::tx::calibration::PhyToneSarExternalBinding::lower(action)
                    .map(Self::ToneSar)
                    .map_err(|_| PhyTxIqExternalBindingError::UnsupportedAction)
            }
            PhyTxIqLinearPowerAction::Complete(_) | PhyTxIqLinearPowerAction::Failed(_) => {
                Err(PhyTxIqExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxIqMisPowerTimerBinding {
    identity: u8,
    phase: PhyTxIqMisPowerDelayPhase,
    micros: u32,
}

impl PhyTxIqMisPowerTimerBinding {
    pub fn new(action: PhyTxIqMisPowerAction) -> Result<Self, PhyTxIqExternalBindingError> {
        match action {
            PhyTxIqMisPowerAction::DelayMicros {
                identity,
                phase,
                micros,
            } => Ok(Self {
                identity,
                phase,
                micros,
            }),
            _ => Err(PhyTxIqExternalBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyTxIqMisPowerCompletion {
        PhyTxIqMisPowerCompletion::DelayElapsed {
            identity: self.identity,
            phase: self.phase,
            micros: self.micros,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxIqMisPowerExternalBinding {
    Mmio(PhyTxIqMisPowerMmioBinding),
    Timer(PhyTxIqMisPowerTimerBinding),
    LinearPower(PhyTxIqLinearPowerExternalBinding),
}

impl PhyTxIqMisPowerExternalBinding {
    pub fn lower(action: PhyTxIqMisPowerAction) -> Result<Self, PhyTxIqExternalBindingError> {
        match action {
            PhyTxIqMisPowerAction::Configure { .. } => PhyTxIqMisPowerMmioBinding::new(action)
                .map(Self::Mmio)
                .map_err(|_| PhyTxIqExternalBindingError::UnsupportedAction),
            PhyTxIqMisPowerAction::DelayMicros { .. } => {
                PhyTxIqMisPowerTimerBinding::new(action).map(Self::Timer)
            }
            PhyTxIqMisPowerAction::LinearPower(action) => {
                PhyTxIqLinearPowerExternalBinding::lower(action).map(Self::LinearPower)
            }
            PhyTxIqMisPowerAction::Complete(_) | PhyTxIqMisPowerAction::Failed(_) => {
                Err(PhyTxIqExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxIqCoverExternalBinding {
    Mmio(PhyTxIqCoverMmioBinding),
    MisPower(PhyTxIqMisPowerExternalBinding),
}

impl PhyTxIqCoverExternalBinding {
    pub fn lower(action: PhyTxIqCoverAction) -> Result<Self, PhyTxIqExternalBindingError> {
        match action {
            PhyTxIqCoverAction::ConfigureCoefficient { .. } => PhyTxIqCoverMmioBinding::new(action)
                .map(Self::Mmio)
                .map_err(|_| PhyTxIqExternalBindingError::UnsupportedAction),
            PhyTxIqCoverAction::MisPower(action) => {
                PhyTxIqMisPowerExternalBinding::lower(action).map(Self::MisPower)
            }
            PhyTxIqCoverAction::Complete(_) | PhyTxIqCoverAction::Failed(_) => {
                Err(PhyTxIqExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxIqLoopbackExternalBinding {
    I2c(crate::analog::i2c::MaskedI2cWriteBinding),
    Mmio(PhyTxIqLoopbackMmioBinding),
}

impl PhyTxIqLoopbackExternalBinding {
    pub fn lower(action: PhyTxIqLoopbackAction) -> Result<Self, PhyTxIqExternalBindingError> {
        match action {
            PhyTxIqLoopbackAction::I2c(action) => {
                crate::analog::i2c::MaskedI2cWriteBinding::new(action)
                    .map(Self::I2c)
                    .map_err(|_| PhyTxIqExternalBindingError::UnsupportedAction)
            }
            PhyTxIqLoopbackAction::ConfigureTxClock { .. }
            | PhyTxIqLoopbackAction::ConfigureRxClock { .. } => {
                PhyTxIqLoopbackMmioBinding::new(action)
                    .map(Self::Mmio)
                    .map_err(|_| PhyTxIqExternalBindingError::UnsupportedAction)
            }
            PhyTxIqLoopbackAction::Complete { .. } => {
                Err(PhyTxIqExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxIqPbusBinding {
    transaction: PhyPbusForceTest,
    hardware: crate::analog::pbus::PhyPbusHardwareBinding,
}

impl PhyTxIqPbusBinding {
    pub fn new(action: PhyTxIqCalibrationAction) -> Result<Self, PhyTxIqExternalBindingError> {
        let PhyTxIqCalibrationAction::ForcePbus(transaction) = action else {
            return Err(PhyTxIqExternalBindingError::UnsupportedAction);
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
    ) -> Result<PhyTxIqCalibrationCompletion, PhyTxIqExternalBindingError> {
        self.hardware
            .into_transaction()
            .map(PhyTxIqCalibrationCompletion::PbusCompleted)
            .map_err(PhyTxIqExternalBindingError::Pbus)
    }

    pub const fn into_timeout_completion(self) -> PhyTxIqCalibrationCompletion {
        PhyTxIqCalibrationCompletion::PbusTimedOut(self.transaction)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxIqCalibrationExternalBinding {
    Mmio(PhyTxIqMmioBinding),
    Loopback(PhyTxIqLoopbackExternalBinding),
    Pbus(PhyTxIqPbusBinding),
    Environment(crate::tx::calibration::PhyTxCalibrationEnvironmentExternalBinding),
    PowerAttenuation(crate::tx::calibration::PhyPowerAttenuationExternalBinding),
    Cover(PhyTxIqCoverExternalBinding),
}

impl PhyTxIqCalibrationExternalBinding {
    pub fn lower(action: PhyTxIqCalibrationAction) -> Result<Self, PhyTxIqExternalBindingError> {
        if let Ok(binding) = PhyTxIqMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        match action {
            PhyTxIqCalibrationAction::Loopback(action) => {
                PhyTxIqLoopbackExternalBinding::lower(action).map(Self::Loopback)
            }
            PhyTxIqCalibrationAction::ForcePbus(_) => {
                PhyTxIqPbusBinding::new(action).map(Self::Pbus)
            }
            PhyTxIqCalibrationAction::Environment(action) => {
                crate::tx::calibration::PhyTxCalibrationEnvironmentExternalBinding::lower(action)
                    .map(Self::Environment)
                    .map_err(|_| PhyTxIqExternalBindingError::UnsupportedAction)
            }
            PhyTxIqCalibrationAction::PowerAttenuation(action) => {
                crate::tx::calibration::PhyPowerAttenuationExternalBinding::lower(action)
                    .map(Self::PowerAttenuation)
                    .map_err(|_| PhyTxIqExternalBindingError::UnsupportedAction)
            }
            PhyTxIqCalibrationAction::Cover(action) => {
                PhyTxIqCoverExternalBinding::lower(action).map(Self::Cover)
            }
            PhyTxIqCalibrationAction::Complete(_) | PhyTxIqCalibrationAction::Failed(_) => {
                Err(PhyTxIqExternalBindingError::UnsupportedAction)
            }
            _ => Err(PhyTxIqExternalBindingError::UnsupportedAction),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxIqInitI2cBinding {
    outer_action: PhyTxIqInitAction,
    transaction: crate::calibration::cold::PhyColdI2cTransaction,
}

impl PhyTxIqInitI2cBinding {
    pub fn new(action: PhyTxIqInitAction) -> Result<Self, PhyTxIqExternalBindingError> {
        let request = match action {
            PhyTxIqInitAction::WriteI2c { address, value } => {
                crate::calibration::cold::PhyColdI2cRequest::write_byte(address, value)
            }
            PhyTxIqInitAction::ReadI2cMasked { field } => {
                crate::calibration::cold::PhyColdI2cRequest::read_field(field)
            }
            _ => return Err(PhyTxIqExternalBindingError::UnsupportedAction),
        };
        Ok(Self {
            outer_action: action,
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
    pub fn write_started(&mut self) -> Result<(), crate::calibration::cold::PhyColdI2cError> {
        self.transaction.write_started()
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

    pub fn into_completion(self) -> Result<PhyTxIqInitCompletion, PhyTxIqExternalBindingError> {
        let crate::calibration::cold::PhyColdI2cAction::Complete(outcome) =
            self.transaction.action()
        else {
            return Err(PhyTxIqExternalBindingError::IncompleteTransaction);
        };
        match (self.outer_action, outcome) {
            (
                PhyTxIqInitAction::WriteI2c { address, value },
                crate::calibration::cold::PhyColdI2cOutcome::Written { address: completed },
            ) if completed == address => Ok(PhyTxIqInitCompletion::I2cWritten { address, value }),
            (
                PhyTxIqInitAction::ReadI2cMasked { field },
                crate::calibration::cold::PhyColdI2cOutcome::Read {
                    address: completed,
                    value,
                },
            ) if completed == field.address() => {
                Ok(PhyTxIqInitCompletion::I2cMaskedRead { field, value })
            }
            _ => Err(PhyTxIqExternalBindingError::UnexpectedOutcome),
        }
    }
}

/// Exhaustive lowering of every non-terminal `phy_txiq_cal_init` action.
#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxIqInitExternalBinding {
    Rfpll(crate::analog::rfpll::RfpllFrequencyExternalBinding),
    I2c(PhyTxIqInitI2cBinding),
    Calibration(PhyTxIqCalibrationExternalBinding),
    Temperature(crate::analog::temperature::PhyTemperatureExternalBinding),
}

impl PhyTxIqInitExternalBinding {
    pub fn lower(action: PhyTxIqInitAction) -> Result<Self, PhyTxIqExternalBindingError> {
        match action {
            PhyTxIqInitAction::Rfpll(action) => {
                crate::analog::rfpll::RfpllFrequencyExternalBinding::lower(action)
                    .map(Self::Rfpll)
                    .map_err(|_| PhyTxIqExternalBindingError::UnsupportedAction)
            }
            PhyTxIqInitAction::WriteI2c { .. } | PhyTxIqInitAction::ReadI2cMasked { .. } => {
                PhyTxIqInitI2cBinding::new(action).map(Self::I2c)
            }
            PhyTxIqInitAction::Calibration(action) => {
                PhyTxIqCalibrationExternalBinding::lower(action).map(Self::Calibration)
            }
            PhyTxIqInitAction::Temperature(action) => {
                crate::analog::temperature::PhyTemperatureExternalBinding::lower(action)
                    .map(Self::Temperature)
                    .map_err(|_| PhyTxIqExternalBindingError::UnsupportedAction)
            }
            PhyTxIqInitAction::Complete(_) | PhyTxIqInitAction::Failed(_) => {
                Err(PhyTxIqExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[cfg(test)]
mod tests;
