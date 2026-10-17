//! Shared heap-free primitives for ESP32-S31 TX calibration.
//!
//! The first recovered child is complete rev0 ROM `phy_get_power_atten`,
//! size 278 bytes. Its six-iteration search used a blocking two-microsecond
//! delay and a synchronous SAR callback. Rust exposes both as one externally
//! completed operation per transition step.

/// Complete pinned debug helper; this ESP32-S31 archive always returns zero.
#[inline]
#[cfg(feature = "validation-probes")]
pub const fn get_bias_ref_code() -> u32 {
    0
}

/// Apply complete pinned `phy_tx_atten_comp` wrapping byte arithmetic.
#[inline]
#[cfg(feature = "validation-probes")]
pub fn phy_tx_atten_comp(values: &mut [u8; 3]) {
    values[1] = values[1].wrapping_add(3);
    values[2] = values[2].wrapping_add(4);
}

use crate::{
    analog::{
        i2c::analog_registers,
        pbus::PhyPbusForceTest,
        rfpll::{
            RfpllFrequencyAction, RfpllFrequencyCompletion, RfpllFrequencyFailure,
            RfpllFrequencyRequest, RfpllFrequencyTransition,
        },
    },
    calibration::estimator::phy_linear_to_db,
    tx::power_detector::sar_signal_reference,
};

pub const PHY_POWER_ATTENUATION_MAX_ITERATIONS: u8 = 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxCalibrationParameters {
    pub pbus_tx_path_value: u8,
    pub pbus_rx_path_value: u8,
    pub dco: [u16; 4],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCalibrationEnvironment {
    Debug,
    Work,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCalibrationEnvironmentFailure {
    PbusTimedOut(PhyPbusForceTest),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCalibrationEnvironmentDelayPhase {
    PbusWorkMode,
    PbusWorkModePulse,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCalibrationEnvironmentAction {
    ConfigurePbusDebugMode,
    ForcePbus(PhyPbusForceTest),
    ConfigureTxClock {
        enabled: bool,
    },
    ConfigurePowerDetector,
    ConfigureCalibrationMode,
    StopTone,
    ConfigurePbusWorkMode,
    DelayMicros {
        phase: PhyTxCalibrationEnvironmentDelayPhase,
        micros: u32,
    },
    ConfigurePbusWorkModePulse,
    ClearPbusWorkModePulse,
    Complete(PhyTxCalibrationEnvironment),
    Failed(PhyTxCalibrationEnvironmentFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCalibrationEnvironmentCompletion {
    PbusDebugModeConfigured,
    PbusCompleted(PhyPbusForceTest),
    #[cfg_attr(
        not(any(test, feature = "validation-probes")),
        allow(
            dead_code,
            reason = "only the validation binding constructs this timeout"
        )
    )]
    PbusTimedOut(PhyPbusForceTest),
    TxClockConfigured {
        enabled: bool,
    },
    PowerDetectorConfigured,
    CalibrationModeConfigured,
    ToneStopped,
    PbusWorkModeConfigured {
        settle_required: bool,
    },
    DelayElapsed {
        phase: PhyTxCalibrationEnvironmentDelayPhase,
        micros: u32,
    },
    PbusWorkModePulseConfigured,
    PbusWorkModePulseCleared,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCalibrationEnvironmentTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EnvironmentStep {
    Debug,
    EnterPbus(u8),
    EnableClock,
    PowerDetector,
    CalibrationMode,
    StopTone,
    DisableClock,
    ExitPbus(u8),
    WorkMode,
    WorkModeDelay,
    WorkModePulse,
    WorkModePulseDelay,
    WorkModePulseClear,
    Complete(PhyTxCalibrationEnvironment),
    Failed(PhyTxCalibrationEnvironmentFailure),
}

const fn tx_debug_pbus(index: u8, parameters: PhyTxCalibrationParameters) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(0, 1, 0x80),
        1 => PhyPbusForceTest::new(0, 2, 0),
        2 => PhyPbusForceTest::new(4, 2, 0),
        3 => PhyPbusForceTest::new(1, 1, 0x7c),
        4 => PhyPbusForceTest::new(2, 1, 0x100),
        5 => PhyPbusForceTest::new(3, 1, 0x100),
        6 => PhyPbusForceTest::new(2, 2, 0x100),
        7 => PhyPbusForceTest::new(3, 2, 0x100),
        8 => PhyPbusForceTest::new(1, 2, 0),
        9 => PhyPbusForceTest::new(4, 1, 0x0b),
        10 => PhyPbusForceTest::new(5, 1, parameters.pbus_tx_path_value as u16 + 0x1c0),
        11 => PhyPbusForceTest::new(2, 1, parameters.dco[0]),
        12 => PhyPbusForceTest::new(3, 1, parameters.dco[1]),
        13 => PhyPbusForceTest::new(2, 2, parameters.dco[2]),
        _ => PhyPbusForceTest::new(3, 2, parameters.dco[3]),
    }
}

const fn tx_work_pbus(index: u8, parameters: PhyTxCalibrationParameters) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(4, 1, 0),
        1 => PhyPbusForceTest::new(4, 2, 1),
        2 => PhyPbusForceTest::new(5, 1, 0),
        3 => PhyPbusForceTest::new(0, 1, 0x40),
        4 => PhyPbusForceTest::new(0, 2, parameters.pbus_rx_path_value as u16),
        5 => PhyPbusForceTest::new(1, 1, 0x189),
        _ => PhyPbusForceTest::new(1, 2, 0),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxCalibrationEnvironmentTransition {
    parameters: PhyTxCalibrationParameters,
    step: EnvironmentStep,
}

impl PhyTxCalibrationEnvironmentTransition {
    pub const fn enter(parameters: PhyTxCalibrationParameters) -> Self {
        Self {
            parameters,
            step: EnvironmentStep::Debug,
        }
    }

    pub const fn exit(parameters: PhyTxCalibrationParameters) -> Self {
        Self {
            parameters,
            step: EnvironmentStep::StopTone,
        }
    }

    pub const fn action(self) -> PhyTxCalibrationEnvironmentAction {
        match self.step {
            EnvironmentStep::Debug => PhyTxCalibrationEnvironmentAction::ConfigurePbusDebugMode,
            EnvironmentStep::EnterPbus(index) => {
                PhyTxCalibrationEnvironmentAction::ForcePbus(tx_debug_pbus(index, self.parameters))
            }
            EnvironmentStep::EnableClock => {
                PhyTxCalibrationEnvironmentAction::ConfigureTxClock { enabled: true }
            }
            EnvironmentStep::PowerDetector => {
                PhyTxCalibrationEnvironmentAction::ConfigurePowerDetector
            }
            EnvironmentStep::CalibrationMode => {
                PhyTxCalibrationEnvironmentAction::ConfigureCalibrationMode
            }
            EnvironmentStep::StopTone => PhyTxCalibrationEnvironmentAction::StopTone,
            EnvironmentStep::DisableClock => {
                PhyTxCalibrationEnvironmentAction::ConfigureTxClock { enabled: false }
            }
            EnvironmentStep::ExitPbus(index) => {
                PhyTxCalibrationEnvironmentAction::ForcePbus(tx_work_pbus(index, self.parameters))
            }
            EnvironmentStep::WorkMode => PhyTxCalibrationEnvironmentAction::ConfigurePbusWorkMode,
            EnvironmentStep::WorkModeDelay => PhyTxCalibrationEnvironmentAction::DelayMicros {
                phase: PhyTxCalibrationEnvironmentDelayPhase::PbusWorkMode,
                micros: 1,
            },
            EnvironmentStep::WorkModePulse => {
                PhyTxCalibrationEnvironmentAction::ConfigurePbusWorkModePulse
            }
            EnvironmentStep::WorkModePulseDelay => {
                // SOURCE: complete rev0 ROM
                // `esp32s31_rev0_rom.elf::phy_pbus_force_mode(0)`
                // holds the second work-mode pulse for two microseconds. This
                // exit is reached by archive `phy_tx_cap_init`,
                // `phy_tx_pwctrl_init`, `phy_txdc_cal_pwdet_init` and TXIQ.
                PhyTxCalibrationEnvironmentAction::DelayMicros {
                    phase: PhyTxCalibrationEnvironmentDelayPhase::PbusWorkModePulse,
                    micros: 2,
                }
            }
            EnvironmentStep::WorkModePulseClear => {
                PhyTxCalibrationEnvironmentAction::ClearPbusWorkModePulse
            }
            EnvironmentStep::Complete(mode) => PhyTxCalibrationEnvironmentAction::Complete(mode),
            EnvironmentStep::Failed(failure) => PhyTxCalibrationEnvironmentAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyTxCalibrationEnvironmentCompletion,
    ) -> Result<(), PhyTxCalibrationEnvironmentTransitionError> {
        self.step = match (self.step, completion) {
            (
                EnvironmentStep::Debug,
                PhyTxCalibrationEnvironmentCompletion::PbusDebugModeConfigured,
            ) => EnvironmentStep::EnterPbus(0),
            (
                EnvironmentStep::EnterPbus(index),
                PhyTxCalibrationEnvironmentCompletion::PbusCompleted(transaction),
            ) if transaction == tx_debug_pbus(index, self.parameters) => {
                if index == 14 {
                    EnvironmentStep::EnableClock
                } else {
                    EnvironmentStep::EnterPbus(index + 1)
                }
            }
            (
                EnvironmentStep::EnterPbus(index),
                PhyTxCalibrationEnvironmentCompletion::PbusTimedOut(transaction),
            ) if transaction == tx_debug_pbus(index, self.parameters) => EnvironmentStep::Failed(
                PhyTxCalibrationEnvironmentFailure::PbusTimedOut(transaction),
            ),
            (
                EnvironmentStep::EnableClock,
                PhyTxCalibrationEnvironmentCompletion::TxClockConfigured { enabled: true },
            ) => EnvironmentStep::PowerDetector,
            (
                EnvironmentStep::PowerDetector,
                PhyTxCalibrationEnvironmentCompletion::PowerDetectorConfigured,
            ) => EnvironmentStep::CalibrationMode,
            (
                EnvironmentStep::CalibrationMode,
                PhyTxCalibrationEnvironmentCompletion::CalibrationModeConfigured,
            ) => EnvironmentStep::Complete(PhyTxCalibrationEnvironment::Debug),
            (EnvironmentStep::StopTone, PhyTxCalibrationEnvironmentCompletion::ToneStopped) => {
                EnvironmentStep::DisableClock
            }
            (
                EnvironmentStep::DisableClock,
                PhyTxCalibrationEnvironmentCompletion::TxClockConfigured { enabled: false },
            ) => EnvironmentStep::ExitPbus(0),
            (
                EnvironmentStep::ExitPbus(index),
                PhyTxCalibrationEnvironmentCompletion::PbusCompleted(transaction),
            ) if transaction == tx_work_pbus(index, self.parameters) => {
                if index == 6 {
                    EnvironmentStep::WorkMode
                } else {
                    EnvironmentStep::ExitPbus(index + 1)
                }
            }
            (
                EnvironmentStep::ExitPbus(index),
                PhyTxCalibrationEnvironmentCompletion::PbusTimedOut(transaction),
            ) if transaction == tx_work_pbus(index, self.parameters) => EnvironmentStep::Failed(
                PhyTxCalibrationEnvironmentFailure::PbusTimedOut(transaction),
            ),
            (
                EnvironmentStep::WorkMode,
                PhyTxCalibrationEnvironmentCompletion::PbusWorkModeConfigured {
                    settle_required: false,
                },
            ) => EnvironmentStep::Complete(PhyTxCalibrationEnvironment::Work),
            (
                EnvironmentStep::WorkMode,
                PhyTxCalibrationEnvironmentCompletion::PbusWorkModeConfigured {
                    settle_required: true,
                },
            ) => EnvironmentStep::WorkModeDelay,
            (
                EnvironmentStep::WorkModeDelay,
                PhyTxCalibrationEnvironmentCompletion::DelayElapsed {
                    phase: PhyTxCalibrationEnvironmentDelayPhase::PbusWorkMode,
                    micros: 1,
                },
            ) => EnvironmentStep::WorkModePulse,
            (
                EnvironmentStep::WorkModePulse,
                PhyTxCalibrationEnvironmentCompletion::PbusWorkModePulseConfigured,
            ) => EnvironmentStep::WorkModePulseDelay,
            (
                EnvironmentStep::WorkModePulseDelay,
                PhyTxCalibrationEnvironmentCompletion::DelayElapsed {
                    phase: PhyTxCalibrationEnvironmentDelayPhase::PbusWorkModePulse,
                    micros: 2,
                },
            ) => EnvironmentStep::WorkModePulseClear,
            (
                EnvironmentStep::WorkModePulseClear,
                PhyTxCalibrationEnvironmentCompletion::PbusWorkModePulseCleared,
            ) => EnvironmentStep::Complete(PhyTxCalibrationEnvironment::Work),
            (EnvironmentStep::Complete(_), _) | (EnvironmentStep::Failed(_), _) => {
                return Err(PhyTxCalibrationEnvironmentTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxCalibrationEnvironmentTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyToneSarRequest {
    pub measurement: u8,
    pub samples: u8,
    pub clear_tone_after_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyToneSarOutcome {
    pub request: PhyToneSarRequest,
    pub sample: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyToneSarFailure {
    ReadyDeadlineElapsed {
        measurement: u8,
        sample: u8,
    },
    /// Readiness was not observed within the bounded sample budget. This is
    /// independent of elapsed time and must not be reported as a timer expiry.
    ReadyObservationLimit {
        measurement: u8,
        sample: u8,
        observations: u16,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyToneSarDelayPhase {
    ToneArmed,
    SarTriggered,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyToneSarAction {
    ArmTone {
        measurement: u8,
        sample: u8,
    },
    DelayMicros {
        measurement: u8,
        sample: u8,
        phase: PhyToneSarDelayPhase,
        micros: u32,
    },
    TriggerSar {
        measurement: u8,
        sample: u8,
    },
    PollReady {
        measurement: u8,
        sample: u8,
    },
    ClearTone {
        measurement: u8,
        sample: u8,
    },
    ReadSar {
        measurement: u8,
        sample: u8,
    },
    Complete(PhyToneSarOutcome),
    Failed(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyToneSarCompletion {
    ToneArmed {
        measurement: u8,
        sample: u8,
    },
    DelayElapsed {
        measurement: u8,
        sample: u8,
        phase: PhyToneSarDelayPhase,
        micros: u32,
    },
    SarTriggered {
        measurement: u8,
        sample: u8,
    },
    ReadySampled {
        measurement: u8,
        sample: u8,
        ready: bool,
    },
    #[cfg_attr(
        not(any(test, feature = "validation-probes")),
        allow(dead_code, reason = "only validation bindings construct this variant")
    )]
    ReadyDeadlineElapsed {
        measurement: u8,
        sample: u8,
    },
    ToneCleared {
        measurement: u8,
        sample: u8,
    },
    SarRead {
        measurement: u8,
        sample: u8,
        value: u16,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyToneSarTransitionError {
    WrongCompletion,
    InvalidSampleCount,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ToneSarStep {
    Arm,
    ArmDelay,
    Trigger,
    TriggerDelay,
    Poll,
    Clear,
    Read,
    Complete(PhyToneSarOutcome),
    Failed(PhyToneSarFailure),
}

/// Complete event-driven translation of `phy_get_tone_sar_dout_`.
///
/// No completion interrupt is evidenced for the ready field. Each `PollReady`
/// action is exactly one volatile sample. The transition bounds unsuccessful
/// observations per conversion; an async owner may additionally supply an
/// external deadline. Neither mechanism fabricates a hardware completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyToneSarTransition {
    request: PhyToneSarRequest,
    step: ToneSarStep,
    sample: u8,
    sum: u32,
    ready_observations: u16,
}

impl PhyToneSarTransition {
    /// Construct a transition when the caller proves the sample count in its
    /// type-local control flow.
    ///
    /// Calibration roots use fixed counts of two or four. Keeping this
    /// constructor separate removes runtime `unwrap`/`expect` paths without
    /// weakening the checked public constructor used for dynamic requests.
    pub const fn new_nonzero(request: PhyToneSarRequest) -> Self {
        debug_assert!(request.samples != 0);
        Self {
            request,
            step: ToneSarStep::Arm,
            sample: 0,
            sum: 0,
            ready_observations: 0,
        }
    }

    pub const fn new(request: PhyToneSarRequest) -> Result<Self, PhyToneSarTransitionError> {
        if request.samples == 0 {
            Err(PhyToneSarTransitionError::InvalidSampleCount)
        } else {
            Ok(Self::new_nonzero(request))
        }
    }

    pub const fn action(self) -> PhyToneSarAction {
        let measurement = self.request.measurement;
        let sample = self.sample;
        match self.step {
            ToneSarStep::Arm => PhyToneSarAction::ArmTone {
                measurement,
                sample,
            },
            ToneSarStep::ArmDelay => PhyToneSarAction::DelayMicros {
                measurement,
                sample,
                phase: PhyToneSarDelayPhase::ToneArmed,
                micros: 1,
            },
            ToneSarStep::Trigger => PhyToneSarAction::TriggerSar {
                measurement,
                sample,
            },
            ToneSarStep::TriggerDelay => PhyToneSarAction::DelayMicros {
                measurement,
                sample,
                phase: PhyToneSarDelayPhase::SarTriggered,
                micros: 2,
            },
            ToneSarStep::Poll => PhyToneSarAction::PollReady {
                measurement,
                sample,
            },
            ToneSarStep::Clear => PhyToneSarAction::ClearTone {
                measurement,
                sample,
            },
            ToneSarStep::Read => PhyToneSarAction::ReadSar {
                measurement,
                sample,
            },
            ToneSarStep::Complete(outcome) => PhyToneSarAction::Complete(outcome),
            ToneSarStep::Failed(failure) => PhyToneSarAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyToneSarCompletion,
    ) -> Result<(), PhyToneSarTransitionError> {
        let measurement = self.request.measurement;
        let sample = self.sample;
        self.step = match (self.step, completion) {
            (
                ToneSarStep::Arm,
                PhyToneSarCompletion::ToneArmed {
                    measurement: completed,
                    sample: completed_sample,
                },
            ) if completed == measurement && completed_sample == sample => ToneSarStep::ArmDelay,
            (
                ToneSarStep::ArmDelay,
                PhyToneSarCompletion::DelayElapsed {
                    measurement: completed,
                    sample: completed_sample,
                    phase: PhyToneSarDelayPhase::ToneArmed,
                    micros: 1,
                },
            ) if completed == measurement && completed_sample == sample => ToneSarStep::Trigger,
            (
                ToneSarStep::Trigger,
                PhyToneSarCompletion::SarTriggered {
                    measurement: completed,
                    sample: completed_sample,
                },
            ) if completed == measurement && completed_sample == sample => {
                self.ready_observations = 0;
                ToneSarStep::TriggerDelay
            }
            (
                ToneSarStep::TriggerDelay,
                PhyToneSarCompletion::DelayElapsed {
                    measurement: completed,
                    sample: completed_sample,
                    phase: PhyToneSarDelayPhase::SarTriggered,
                    micros: 2,
                },
            ) if completed == measurement && completed_sample == sample => ToneSarStep::Poll,
            (
                ToneSarStep::Poll,
                PhyToneSarCompletion::ReadySampled {
                    measurement: completed,
                    sample: completed_sample,
                    ready,
                },
            ) if completed == measurement && completed_sample == sample => {
                if ready {
                    if self.request.clear_tone_after_ready {
                        ToneSarStep::Clear
                    } else {
                        ToneSarStep::Read
                    }
                } else {
                    self.ready_observations += 1;
                    if self.ready_observations == crate::HARDWARE_EDGE_LIMIT {
                        ToneSarStep::Failed(PhyToneSarFailure::ReadyObservationLimit {
                            measurement,
                            sample,
                            observations: self.ready_observations,
                        })
                    } else {
                        ToneSarStep::Poll
                    }
                }
            }
            (
                ToneSarStep::Poll,
                PhyToneSarCompletion::ReadyDeadlineElapsed {
                    measurement: completed,
                    sample: completed_sample,
                },
            ) if completed == measurement && completed_sample == sample => {
                ToneSarStep::Failed(PhyToneSarFailure::ReadyDeadlineElapsed {
                    measurement,
                    sample,
                })
            }
            (
                ToneSarStep::Clear,
                PhyToneSarCompletion::ToneCleared {
                    measurement: completed,
                    sample: completed_sample,
                },
            ) if completed == measurement && completed_sample == sample => ToneSarStep::Read,
            (
                ToneSarStep::Read,
                PhyToneSarCompletion::SarRead {
                    measurement: completed,
                    sample: completed_sample,
                    value,
                },
            ) if completed == measurement && completed_sample == sample => {
                self.sum = self.sum.wrapping_add(u32::from(value));
                if self.sample + 1 == self.request.samples {
                    ToneSarStep::Complete(PhyToneSarOutcome {
                        request: self.request,
                        sample: (self.sum / u32::from(self.request.samples)) as u16,
                    })
                } else {
                    self.sample += 1;
                    ToneSarStep::Arm
                }
            }
            (ToneSarStep::Complete(_), _) | (ToneSarStep::Failed(_), _) => {
                return Err(PhyToneSarTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyToneSarTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

pub fn phy_tx_power_db(sample: u16, reference_codes: [i16; 2], offset: i16) -> i16 {
    let signal = sar_signal_reference(sample, reference_codes);
    phy_linear_to_db(signal[0] as i32, 3)
        .wrapping_add(offset as i32)
        .wrapping_sub(phy_linear_to_db(signal[1] as i32, 3)) as i16
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPowerAttenuationRequest {
    pub tone_selector: u16,
    pub initial_attenuation: u8,
    pub target_power: i16,
    pub power_offset: i16,
    pub reference_codes: [i16; 2],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPowerAttenuationOutcome {
    pub attenuation: u8,
    pub iterations: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPowerAttenuationAction {
    ConfigureTone {
        iteration: u8,
        selector: u16,
        attenuation: u8,
    },
    ToneSar(PhyToneSarAction),
    Complete(PhyPowerAttenuationOutcome),
    Failed(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPowerAttenuationCompletion {
    ToneConfigured {
        iteration: u8,
        selector: u16,
        attenuation: u8,
    },
    ToneSar(PhyToneSarCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPowerAttenuationTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    Tone,
    ToneSar(PhyToneSarTransition),
    Complete(PhyPowerAttenuationOutcome),
    Failed(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPowerAttenuationTransition {
    request: PhyPowerAttenuationRequest,
    iteration: u8,
    attenuation: i16,
    previous_attenuation: i16,
    previous_power: i16,
    step: Step,
}

impl PhyPowerAttenuationTransition {
    pub const fn new(request: PhyPowerAttenuationRequest) -> Self {
        Self {
            request,
            iteration: 0,
            attenuation: request.initial_attenuation as i16,
            previous_attenuation: 0,
            previous_power: 0,
            step: Step::Tone,
        }
    }

    pub const fn action(self) -> PhyPowerAttenuationAction {
        match self.step {
            Step::Tone => PhyPowerAttenuationAction::ConfigureTone {
                iteration: self.iteration,
                selector: self.request.tone_selector,
                attenuation: self.attenuation as u8,
            },
            Step::ToneSar(transition) => PhyPowerAttenuationAction::ToneSar(transition.action()),
            Step::Complete(outcome) => PhyPowerAttenuationAction::Complete(outcome),
            Step::Failed(failure) => PhyPowerAttenuationAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyPowerAttenuationCompletion,
    ) -> Result<(), PhyPowerAttenuationTransitionError> {
        self.step = match (self.step, completion) {
            (
                Step::Tone,
                PhyPowerAttenuationCompletion::ToneConfigured {
                    iteration,
                    selector,
                    attenuation,
                },
            ) if iteration == self.iteration
                && selector == self.request.tone_selector
                && attenuation == self.attenuation as u8 =>
            {
                Step::ToneSar(
                    PhyToneSarTransition::new(PhyToneSarRequest {
                        measurement: self.iteration,
                        samples: 2,
                        clear_tone_after_ready: false,
                    })
                    .unwrap(),
                )
            }
            (Step::ToneSar(mut transition), PhyPowerAttenuationCompletion::ToneSar(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyPowerAttenuationTransitionError::WrongCompletion)?;
                let PhyToneSarAction::Complete(sample) = transition.action() else {
                    if let PhyToneSarAction::Failed(failure) = transition.action() {
                        return {
                            self.step = Step::Failed(failure);
                            Ok(())
                        };
                    }
                    self.step = Step::ToneSar(transition);
                    return Ok(());
                };
                let power = phy_tx_power_db(
                    sample.sample,
                    self.request.reference_codes,
                    self.request.power_offset,
                ) >> 2;
                let error = power.wrapping_sub(self.request.target_power);
                if self.iteration != 0
                    && self.previous_attenuation < self.attenuation
                    && self.previous_power < power
                {
                    self.attenuation = self.previous_attenuation.wrapping_sub(20);
                }
                let output = self.attenuation;
                if (-3..=3).contains(&error) {
                    Step::Complete(PhyPowerAttenuationOutcome {
                        attenuation: output as u8,
                        iterations: self.iteration + 1,
                    })
                } else {
                    let delta = if error < 1 {
                        error.wrapping_mul(3) / 4
                    } else {
                        error
                    };
                    let next = output.wrapping_add(delta).clamp(0, 120);
                    self.previous_attenuation = output;
                    self.previous_power = power;
                    self.attenuation = next;
                    self.iteration += 1;
                    if self.iteration == PHY_POWER_ATTENUATION_MAX_ITERATIONS {
                        Step::Complete(PhyPowerAttenuationOutcome {
                            attenuation: next as u8,
                            iterations: self.iteration,
                        })
                    } else {
                        Step::Tone
                    }
                }
            }
            (Step::Complete(_), _) | (Step::Failed(_), _) => {
                return Err(PhyPowerAttenuationTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyPowerAttenuationTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

const TX_CAP_CHANNELS: [u16; 3] = [1, 6, 11];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxCapSearchRequest {
    pub tone_selector: u16,
    pub attenuation: u8,
    pub clear_tone_after_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxCapSearchOutcome {
    pub capacitance: [u8; 2],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCapSearchFailure {
    ToneSar(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCapSearchAction {
    ConfigureTone {
        selector: u16,
        attenuation: u8,
        enabled: bool,
    },
    I2c(oer_radio_analog::Action<crate::analog::i2c::PhyI2cAddress, ()>),
    ToneSar(PhyToneSarAction),
    Complete(PhyTxCapSearchOutcome),
    Failed(PhyTxCapSearchFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCapSearchCompletion {
    ToneConfigured {
        selector: u16,
        attenuation: u8,
        enabled: bool,
    },
    I2c(oer_radio_analog::Completion<crate::analog::i2c::PhyI2cAddress>),
    ToneSar(PhyToneSarCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCapSearchTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TxCapSearchStep {
    ConfigureTone,
    WriteCandidate(oer_radio_analog::FieldWriteTransition<crate::analog::i2c::PhyI2cAddress>),
    Measure(PhyToneSarTransition),
    WriteBest(oer_radio_analog::FieldWriteTransition<crate::analog::i2c::PhyI2cAddress>),
    StopTone,
    Complete(PhyTxCapSearchOutcome),
    Failed(PhyTxCapSearchFailure),
}

const fn tx_cap_field(bank: u8) -> crate::analog::i2c::PhyI2cField {
    if bank == 0 {
        analog_registers::TX_CAPACITOR_LOW
    } else {
        analog_registers::TX_CAPACITOR_HIGH
    }
}

const fn tx_cap_max(bank: u8) -> u8 {
    if bank == 0 { 7 } else { 13 }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxCapSearchTransition {
    request: PhyTxCapSearchRequest,
    step: TxCapSearchStep,
    bank: u8,
    candidate: u8,
    best: [u8; 2],
    best_sample: u16,
}

impl PhyTxCapSearchTransition {
    pub const fn new(request: PhyTxCapSearchRequest) -> Self {
        Self {
            request,
            step: TxCapSearchStep::ConfigureTone,
            bank: 0,
            candidate: tx_cap_max(0),
            best: [tx_cap_max(0), tx_cap_max(1)],
            best_sample: 0,
        }
    }

    const fn write(
        self,
        value: u8,
    ) -> oer_radio_analog::FieldWriteTransition<crate::analog::i2c::PhyI2cAddress> {
        crate::analog::i2c::field_write(tx_cap_field(self.bank), value)
    }

    pub const fn action(self) -> PhyTxCapSearchAction {
        match self.step {
            TxCapSearchStep::ConfigureTone => PhyTxCapSearchAction::ConfigureTone {
                selector: self.request.tone_selector,
                attenuation: self.request.attenuation,
                enabled: true,
            },
            TxCapSearchStep::WriteCandidate(transition)
            | TxCapSearchStep::WriteBest(transition) => {
                PhyTxCapSearchAction::I2c(transition.action())
            }
            TxCapSearchStep::Measure(transition) => {
                PhyTxCapSearchAction::ToneSar(transition.action())
            }
            TxCapSearchStep::StopTone => PhyTxCapSearchAction::ConfigureTone {
                selector: self.request.tone_selector,
                attenuation: self.request.attenuation,
                enabled: false,
            },
            TxCapSearchStep::Complete(outcome) => PhyTxCapSearchAction::Complete(outcome),
            TxCapSearchStep::Failed(failure) => PhyTxCapSearchAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyTxCapSearchCompletion,
    ) -> Result<(), PhyTxCapSearchTransitionError> {
        match (self.step, completion) {
            (
                TxCapSearchStep::ConfigureTone,
                PhyTxCapSearchCompletion::ToneConfigured {
                    selector,
                    attenuation,
                    enabled: true,
                },
            ) if selector == self.request.tone_selector
                && attenuation == self.request.attenuation =>
            {
                self.step = TxCapSearchStep::WriteCandidate(self.write(self.candidate));
            }
            (
                TxCapSearchStep::WriteCandidate(mut transition),
                PhyTxCapSearchCompletion::I2c(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxCapSearchTransitionError::WrongCompletion)?;
                self.step = if transition.action() == oer_radio_analog::Action::Complete(()) {
                    TxCapSearchStep::Measure(
                        PhyToneSarTransition::new(PhyToneSarRequest {
                            measurement: self.bank.wrapping_mul(16).wrapping_add(self.candidate),
                            samples: 1,
                            clear_tone_after_ready: self.request.clear_tone_after_ready,
                        })
                        .unwrap(),
                    )
                } else {
                    TxCapSearchStep::WriteCandidate(transition)
                };
            }
            (
                TxCapSearchStep::Measure(mut transition),
                PhyTxCapSearchCompletion::ToneSar(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxCapSearchTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyToneSarAction::Complete(outcome) => {
                        if outcome.sample > self.best_sample {
                            self.best_sample = outcome.sample;
                            self.best[self.bank as usize] = self.candidate;
                        }
                        if self.candidate == 0 {
                            self.step = TxCapSearchStep::WriteBest(
                                self.write(self.best[self.bank as usize]),
                            );
                        } else {
                            self.candidate -= 1;
                            self.step = TxCapSearchStep::WriteCandidate(self.write(self.candidate));
                        }
                    }
                    PhyToneSarAction::Failed(failure) => {
                        self.step =
                            TxCapSearchStep::Failed(PhyTxCapSearchFailure::ToneSar(failure));
                    }
                    _ => self.step = TxCapSearchStep::Measure(transition),
                }
            }
            (
                TxCapSearchStep::WriteBest(mut transition),
                PhyTxCapSearchCompletion::I2c(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxCapSearchTransitionError::WrongCompletion)?;
                if transition.action() == oer_radio_analog::Action::Complete(()) {
                    if self.bank == 0 {
                        self.bank = 1;
                        self.candidate = tx_cap_max(1);
                        self.best_sample = 0;
                        self.step = TxCapSearchStep::WriteCandidate(self.write(self.candidate));
                    } else {
                        self.step = TxCapSearchStep::StopTone;
                    }
                } else {
                    self.step = TxCapSearchStep::WriteBest(transition);
                }
            }
            (
                TxCapSearchStep::StopTone,
                PhyTxCapSearchCompletion::ToneConfigured {
                    selector,
                    attenuation,
                    enabled: false,
                },
            ) if selector == self.request.tone_selector
                && attenuation == self.request.attenuation =>
            {
                self.step = TxCapSearchStep::Complete(PhyTxCapSearchOutcome {
                    capacitance: self.best,
                });
            }
            (TxCapSearchStep::Complete(_), _) | (TxCapSearchStep::Failed(_), _) => {
                return Err(PhyTxCapSearchTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxCapSearchTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxCapParameters {
    pub crystal_selector: u8,
    pub environment: PhyTxCalibrationParameters,
    pub clear_tone_after_ready: bool,
    pub reference_codes: [i16; 2],
    pub power_offset: i16,
    pub initial_attenuation: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxCapOutcome {
    pub capacitance: [u8; 6],
    pub attenuation: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCapFailure {
    Environment(PhyTxCalibrationEnvironmentFailure),
    Rfpll(RfpllFrequencyFailure),
    ToneSar(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCapAction {
    Environment(PhyTxCalibrationEnvironmentAction),
    Rfpll(RfpllFrequencyAction),
    I2c(oer_radio_analog::Action<crate::analog::i2c::PhyI2cAddress, ()>),
    Attenuation(PhyPowerAttenuationAction),
    Search(PhyTxCapSearchAction),
    Complete(PhyTxCapOutcome),
    Failed(PhyTxCapFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCapCompletion {
    Environment(PhyTxCalibrationEnvironmentCompletion),
    Rfpll(RfpllFrequencyCompletion),
    I2c(oer_radio_analog::Completion<crate::analog::i2c::PhyI2cAddress>),
    Attenuation(PhyPowerAttenuationCompletion),
    Search(PhyTxCapSearchCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCapTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TxCapTerminal {
    Complete,
    Failed(PhyTxCapFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TxCapStep {
    Enter(PhyTxCalibrationEnvironmentTransition),
    Rfpll(RfpllFrequencyTransition),
    SetLow(oer_radio_analog::FieldWriteTransition<crate::analog::i2c::PhyI2cAddress>),
    SetHigh(oer_radio_analog::FieldWriteTransition<crate::analog::i2c::PhyI2cAddress>),
    Attenuation(PhyPowerAttenuationTransition),
    Search(PhyTxCapSearchTransition),
    Exit {
        terminal: TxCapTerminal,
        transition: PhyTxCalibrationEnvironmentTransition,
    },
    Complete,
    Failed(PhyTxCapFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxCapTransition {
    parameters: PhyTxCapParameters,
    step: TxCapStep,
    channel: u8,
    attenuation: u8,
    capacitance: [u8; 6],
}

impl PhyTxCapTransition {
    pub const fn new(parameters: PhyTxCapParameters) -> Self {
        Self {
            parameters,
            step: TxCapStep::Enter(PhyTxCalibrationEnvironmentTransition::enter(
                parameters.environment,
            )),
            channel: 0,
            attenuation: parameters.initial_attenuation,
            capacitance: [0; 6],
        }
    }

    fn exit(&mut self, terminal: TxCapTerminal) {
        self.step = TxCapStep::Exit {
            terminal,
            transition: PhyTxCalibrationEnvironmentTransition::exit(self.parameters.environment),
        };
    }

    fn fail(&mut self, failure: PhyTxCapFailure) {
        self.exit(TxCapTerminal::Failed(failure));
    }

    pub const fn action(self) -> PhyTxCapAction {
        match self.step {
            TxCapStep::Enter(transition) | TxCapStep::Exit { transition, .. } => {
                PhyTxCapAction::Environment(transition.action())
            }
            TxCapStep::Rfpll(transition) => PhyTxCapAction::Rfpll(transition.action()),
            TxCapStep::SetLow(transition) | TxCapStep::SetHigh(transition) => {
                PhyTxCapAction::I2c(transition.action())
            }
            TxCapStep::Attenuation(transition) => PhyTxCapAction::Attenuation(transition.action()),
            TxCapStep::Search(transition) => PhyTxCapAction::Search(transition.action()),
            TxCapStep::Complete => PhyTxCapAction::Complete(PhyTxCapOutcome {
                capacitance: self.capacitance,
                attenuation: self.attenuation,
            }),
            TxCapStep::Failed(failure) => PhyTxCapAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyTxCapCompletion,
    ) -> Result<(), PhyTxCapTransitionError> {
        match (self.step, completion) {
            (TxCapStep::Enter(mut transition), PhyTxCapCompletion::Environment(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxCapTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxCalibrationEnvironmentAction::Complete(
                        PhyTxCalibrationEnvironment::Debug,
                    ) => {
                        self.step = TxCapStep::Rfpll(RfpllFrequencyTransition::channel(
                            RfpllFrequencyRequest {
                                crystal_selector: self.parameters.crystal_selector,
                                frequency_code: TX_CAP_CHANNELS[self.channel as usize],
                                offset: 0,
                            },
                        ));
                    }
                    PhyTxCalibrationEnvironmentAction::Failed(failure) => {
                        self.fail(PhyTxCapFailure::Environment(failure));
                    }
                    _ => self.step = TxCapStep::Enter(transition),
                }
            }
            (TxCapStep::Rfpll(mut transition), PhyTxCapCompletion::Rfpll(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxCapTransitionError::WrongCompletion)?;
                match transition.action() {
                    RfpllFrequencyAction::Complete(_) => {
                        self.step = TxCapStep::SetLow(crate::analog::i2c::field_write(
                            analog_registers::TX_CAPACITOR_LOW,
                            7,
                        ));
                    }
                    RfpllFrequencyAction::Failed(failure) => {
                        self.fail(PhyTxCapFailure::Rfpll(failure));
                    }
                    _ => self.step = TxCapStep::Rfpll(transition),
                }
            }
            (TxCapStep::SetLow(mut transition), PhyTxCapCompletion::I2c(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxCapTransitionError::WrongCompletion)?;
                self.step = if transition.action() == oer_radio_analog::Action::Complete(()) {
                    TxCapStep::SetHigh(crate::analog::i2c::field_write(
                        analog_registers::TX_CAPACITOR_HIGH,
                        13,
                    ))
                } else {
                    TxCapStep::SetLow(transition)
                };
            }
            (TxCapStep::SetHigh(mut transition), PhyTxCapCompletion::I2c(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxCapTransitionError::WrongCompletion)?;
                if transition.action() == oer_radio_analog::Action::Complete(()) {
                    self.step = if self.channel == 0 {
                        TxCapStep::Attenuation(PhyPowerAttenuationTransition::new(
                            PhyPowerAttenuationRequest {
                                tone_selector: 0x80,
                                initial_attenuation: self.attenuation,
                                target_power: 0x28,
                                power_offset: self.parameters.power_offset,
                                reference_codes: self.parameters.reference_codes,
                            },
                        ))
                    } else {
                        TxCapStep::Search(PhyTxCapSearchTransition::new(PhyTxCapSearchRequest {
                            tone_selector: 0x80,
                            attenuation: self.attenuation,
                            clear_tone_after_ready: self.parameters.clear_tone_after_ready,
                        }))
                    };
                } else {
                    self.step = TxCapStep::SetHigh(transition);
                }
            }
            (
                TxCapStep::Attenuation(mut transition),
                PhyTxCapCompletion::Attenuation(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxCapTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyPowerAttenuationAction::Complete(outcome) => {
                        self.attenuation = outcome.attenuation;
                        self.step = TxCapStep::Search(PhyTxCapSearchTransition::new(
                            PhyTxCapSearchRequest {
                                tone_selector: 0x80,
                                attenuation: self.attenuation,
                                clear_tone_after_ready: self.parameters.clear_tone_after_ready,
                            },
                        ));
                    }
                    PhyPowerAttenuationAction::Failed(failure) => {
                        self.fail(PhyTxCapFailure::ToneSar(failure));
                    }
                    _ => self.step = TxCapStep::Attenuation(transition),
                }
            }
            (TxCapStep::Search(mut transition), PhyTxCapCompletion::Search(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxCapTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxCapSearchAction::Complete(outcome) => {
                        let offset = self.channel as usize * 2;
                        self.capacitance[offset] = outcome.capacitance[0];
                        self.capacitance[offset + 1] = outcome.capacitance[1];
                        self.channel += 1;
                        if self.channel == 3 {
                            self.exit(TxCapTerminal::Complete);
                        } else {
                            self.step = TxCapStep::Rfpll(RfpllFrequencyTransition::channel(
                                RfpllFrequencyRequest {
                                    crystal_selector: self.parameters.crystal_selector,
                                    frequency_code: TX_CAP_CHANNELS[self.channel as usize],
                                    offset: 0,
                                },
                            ));
                        }
                    }
                    PhyTxCapSearchAction::Failed(PhyTxCapSearchFailure::ToneSar(failure)) => {
                        self.fail(PhyTxCapFailure::ToneSar(failure));
                    }
                    _ => self.step = TxCapStep::Search(transition),
                }
            }
            (
                TxCapStep::Exit {
                    terminal,
                    mut transition,
                },
                PhyTxCapCompletion::Environment(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxCapTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxCalibrationEnvironmentAction::Complete(
                        PhyTxCalibrationEnvironment::Work,
                    ) => {
                        self.step = match terminal {
                            TxCapTerminal::Complete => TxCapStep::Complete,
                            TxCapTerminal::Failed(failure) => TxCapStep::Failed(failure),
                        };
                    }
                    PhyTxCalibrationEnvironmentAction::Failed(failure) => {
                        self.step = TxCapStep::Failed(PhyTxCapFailure::Environment(failure));
                    }
                    _ => {
                        self.step = TxCapStep::Exit {
                            terminal,
                            transition,
                        };
                    }
                }
            }
            (TxCapStep::Complete, _) | (TxCapStep::Failed(_), _) => {
                return Err(PhyTxCapTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxCapTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPowerAttenuationBindingError {
    NotDirectMmio,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyPowerAttenuationMmioBinding {
    action: PhyPowerAttenuationAction,
}

impl PhyPowerAttenuationMmioBinding {
    pub fn new(action: PhyPowerAttenuationAction) -> Result<Self, PhyPowerAttenuationBindingError> {
        match action {
            PhyPowerAttenuationAction::ConfigureTone { .. } => Ok(Self { action }),
            _ => Err(PhyPowerAttenuationBindingError::NotDirectMmio),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyContext,
    ) -> PhyPowerAttenuationCompletion {
        match self.action {
            PhyPowerAttenuationAction::ConfigureTone {
                iteration,
                selector,
                attenuation,
            } => {
                crate::hardware::configure_phy_power_control_tone(registers, selector, attenuation);
                PhyPowerAttenuationCompletion::ToneConfigured {
                    iteration,
                    selector,
                    attenuation,
                }
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyToneSarBindingError {
    NotDirectMmio,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyToneSarMmioBinding {
    action: PhyToneSarAction,
}

impl PhyToneSarMmioBinding {
    pub fn new(action: PhyToneSarAction) -> Result<Self, PhyToneSarBindingError> {
        match action {
            PhyToneSarAction::ArmTone { .. }
            | PhyToneSarAction::TriggerSar { .. }
            | PhyToneSarAction::PollReady { .. }
            | PhyToneSarAction::ClearTone { .. }
            | PhyToneSarAction::ReadSar { .. } => Ok(Self { action }),
            _ => Err(PhyToneSarBindingError::NotDirectMmio),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> PhyToneSarCompletion {
        match self.action {
            PhyToneSarAction::ArmTone {
                measurement,
                sample,
            } => {
                crate::hardware::arm_phy_power_detector_tone(registers);
                PhyToneSarCompletion::ToneArmed {
                    measurement,
                    sample,
                }
            }
            PhyToneSarAction::TriggerSar {
                measurement,
                sample,
            } => {
                oer_esp32s31_hal::phy::power_detector::trigger_sar(registers);
                PhyToneSarCompletion::SarTriggered {
                    measurement,
                    sample,
                }
            }
            PhyToneSarAction::PollReady {
                measurement,
                sample,
            } => PhyToneSarCompletion::ReadySampled {
                measurement,
                sample,
                ready: oer_esp32s31_hal::phy::power_detector::sample_ready(registers),
            },
            PhyToneSarAction::ClearTone {
                measurement,
                sample,
            } => {
                crate::hardware::clear_phy_power_detector_tone_arm(registers);
                PhyToneSarCompletion::ToneCleared {
                    measurement,
                    sample,
                }
            }
            PhyToneSarAction::ReadSar {
                measurement,
                sample,
            } => PhyToneSarCompletion::SarRead {
                measurement,
                sample,
                value: oer_esp32s31_hal::phy::power_detector::sample_sar(registers),
            },
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCalibrationBindingError {
    NotDirectMmio,
}

/// Non-cloneable direct-MMIO token for entering or leaving TX calibration.
/// PBus force commands and both one-microsecond intervals stay separately
/// bound to the async executor.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxCalibrationEnvironmentMmioBinding {
    action: PhyTxCalibrationEnvironmentAction,
}

impl PhyTxCalibrationEnvironmentMmioBinding {
    pub fn new(
        action: PhyTxCalibrationEnvironmentAction,
    ) -> Result<Self, PhyTxCalibrationBindingError> {
        match action {
            PhyTxCalibrationEnvironmentAction::ConfigurePbusDebugMode
            | PhyTxCalibrationEnvironmentAction::ConfigureTxClock { .. }
            | PhyTxCalibrationEnvironmentAction::ConfigurePowerDetector
            | PhyTxCalibrationEnvironmentAction::ConfigureCalibrationMode
            | PhyTxCalibrationEnvironmentAction::StopTone
            | PhyTxCalibrationEnvironmentAction::ConfigurePbusWorkMode
            | PhyTxCalibrationEnvironmentAction::ConfigurePbusWorkModePulse
            | PhyTxCalibrationEnvironmentAction::ClearPbusWorkModePulse => Ok(Self { action }),
            _ => Err(PhyTxCalibrationBindingError::NotDirectMmio),
        }
    }

    #[cfg(feature = "validation-probes")]
    pub const fn action(&self) -> PhyTxCalibrationEnvironmentAction {
        self.action
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyContext,
    ) -> PhyTxCalibrationEnvironmentCompletion {
        match self.action {
            PhyTxCalibrationEnvironmentAction::ConfigurePbusDebugMode => {
                oer_esp32s31_hal::phy::pbus::configure_debug_mode(registers);
                PhyTxCalibrationEnvironmentCompletion::PbusDebugModeConfigured
            }
            PhyTxCalibrationEnvironmentAction::ConfigureTxClock { enabled } => {
                oer_esp32s31_hal::phy::pbus::configure_tx_clock(registers, enabled);
                PhyTxCalibrationEnvironmentCompletion::TxClockConfigured { enabled }
            }
            PhyTxCalibrationEnvironmentAction::ConfigurePowerDetector => {
                oer_esp32s31_hal::phy::power_detector::configure_enabled(registers);
                PhyTxCalibrationEnvironmentCompletion::PowerDetectorConfigured
            }
            PhyTxCalibrationEnvironmentAction::ConfigureCalibrationMode => {
                oer_esp32s31_hal::phy::power_detector::configure_calibration_mode(registers);
                PhyTxCalibrationEnvironmentCompletion::CalibrationModeConfigured
            }
            PhyTxCalibrationEnvironmentAction::StopTone => {
                crate::hardware::stop_phy_power_detector_tone(registers);
                PhyTxCalibrationEnvironmentCompletion::ToneStopped
            }
            PhyTxCalibrationEnvironmentAction::ConfigurePbusWorkMode => {
                PhyTxCalibrationEnvironmentCompletion::PbusWorkModeConfigured {
                    settle_required: oer_esp32s31_hal::phy::pbus::configure_work_mode(registers),
                }
            }
            PhyTxCalibrationEnvironmentAction::ConfigurePbusWorkModePulse => {
                oer_esp32s31_hal::phy::agc::configure_pbus_work_mode_pulse(registers);
                PhyTxCalibrationEnvironmentCompletion::PbusWorkModePulseConfigured
            }
            PhyTxCalibrationEnvironmentAction::ClearPbusWorkModePulse => {
                oer_esp32s31_hal::phy::agc::clear_pbus_work_mode_pulse(registers);
                PhyTxCalibrationEnvironmentCompletion::PbusWorkModePulseCleared
            }
            _ => unreachable!(),
        }
    }
}

/// Direct tone-programming edge of the TX-capacitance search.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxCapSearchMmioBinding {
    action: PhyTxCapSearchAction,
}

impl PhyTxCapSearchMmioBinding {
    pub fn new(action: PhyTxCapSearchAction) -> Result<Self, PhyTxCalibrationBindingError> {
        match action {
            PhyTxCapSearchAction::ConfigureTone { .. } => Ok(Self { action }),
            _ => Err(PhyTxCalibrationBindingError::NotDirectMmio),
        }
    }

    #[cfg(feature = "validation-probes")]
    pub const fn action(&self) -> PhyTxCapSearchAction {
        self.action
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> PhyTxCapSearchCompletion {
        match self.action {
            PhyTxCapSearchAction::ConfigureTone {
                selector,
                attenuation,
                enabled,
            } => {
                if enabled {
                    crate::hardware::configure_phy_power_control_tone(
                        registers,
                        selector,
                        attenuation,
                    );
                } else {
                    crate::hardware::stop_phy_power_detector_tone(registers);
                }
                PhyTxCapSearchCompletion::ToneConfigured {
                    selector,
                    attenuation,
                    enabled,
                }
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCapExternalBindingError {
    UnsupportedAction,
    Pbus(crate::analog::pbus::PhyPbusHardwareBindingError),
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxCalibrationEnvironmentPbusBinding {
    transaction: PhyPbusForceTest,
    hardware: crate::analog::pbus::PhyPbusHardwareBinding,
}

impl PhyTxCalibrationEnvironmentPbusBinding {
    pub fn new(
        action: PhyTxCalibrationEnvironmentAction,
    ) -> Result<Self, PhyTxCapExternalBindingError> {
        let PhyTxCalibrationEnvironmentAction::ForcePbus(transaction) = action else {
            return Err(PhyTxCapExternalBindingError::UnsupportedAction);
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
    ) -> Result<PhyTxCalibrationEnvironmentCompletion, PhyTxCapExternalBindingError> {
        self.hardware
            .into_transaction()
            .map(PhyTxCalibrationEnvironmentCompletion::PbusCompleted)
            .map_err(PhyTxCapExternalBindingError::Pbus)
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn into_timeout_completion(self) -> PhyTxCalibrationEnvironmentCompletion {
        PhyTxCalibrationEnvironmentCompletion::PbusTimedOut(self.transaction)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxCalibrationEnvironmentTimerBinding {
    phase: PhyTxCalibrationEnvironmentDelayPhase,
    micros: u32,
}

impl PhyTxCalibrationEnvironmentTimerBinding {
    pub fn new(
        action: PhyTxCalibrationEnvironmentAction,
    ) -> Result<Self, PhyTxCapExternalBindingError> {
        match action {
            PhyTxCalibrationEnvironmentAction::DelayMicros { phase, micros } => {
                Ok(Self { phase, micros })
            }
            _ => Err(PhyTxCapExternalBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyTxCalibrationEnvironmentCompletion {
        PhyTxCalibrationEnvironmentCompletion::DelayElapsed {
            phase: self.phase,
            micros: self.micros,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxCalibrationEnvironmentExternalBinding {
    Mmio(PhyTxCalibrationEnvironmentMmioBinding),
    Pbus(PhyTxCalibrationEnvironmentPbusBinding),
    Timer(PhyTxCalibrationEnvironmentTimerBinding),
}

impl PhyTxCalibrationEnvironmentExternalBinding {
    pub fn lower(
        action: PhyTxCalibrationEnvironmentAction,
    ) -> Result<Self, PhyTxCapExternalBindingError> {
        if let Ok(binding) = PhyTxCalibrationEnvironmentMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        if let Ok(binding) = PhyTxCalibrationEnvironmentPbusBinding::new(action) {
            return Ok(Self::Pbus(binding));
        }
        if let Ok(binding) = PhyTxCalibrationEnvironmentTimerBinding::new(action) {
            return Ok(Self::Timer(binding));
        }
        Err(PhyTxCapExternalBindingError::UnsupportedAction)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyToneSarTimerBinding {
    measurement: u8,
    sample: u8,
    phase: PhyToneSarDelayPhase,
    micros: u32,
}

impl PhyToneSarTimerBinding {
    pub fn new(action: PhyToneSarAction) -> Result<Self, PhyTxCapExternalBindingError> {
        match action {
            PhyToneSarAction::DelayMicros {
                measurement,
                sample,
                phase,
                micros,
            } => Ok(Self {
                measurement,
                sample,
                phase,
                micros,
            }),
            _ => Err(PhyTxCapExternalBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyToneSarCompletion {
        PhyToneSarCompletion::DelayElapsed {
            measurement: self.measurement,
            sample: self.sample,
            phase: self.phase,
            micros: self.micros,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyToneSarExternalBinding {
    Mmio(PhyToneSarMmioBinding),
    Timer(PhyToneSarTimerBinding),
}

impl PhyToneSarExternalBinding {
    pub fn lower(action: PhyToneSarAction) -> Result<Self, PhyTxCapExternalBindingError> {
        if let Ok(binding) = PhyToneSarMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        if let Ok(binding) = PhyToneSarTimerBinding::new(action) {
            return Ok(Self::Timer(binding));
        }
        Err(PhyTxCapExternalBindingError::UnsupportedAction)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyPowerAttenuationExternalBinding {
    Mmio(PhyPowerAttenuationMmioBinding),
    ToneSar(PhyToneSarExternalBinding),
}

impl PhyPowerAttenuationExternalBinding {
    pub fn lower(action: PhyPowerAttenuationAction) -> Result<Self, PhyTxCapExternalBindingError> {
        match action {
            PhyPowerAttenuationAction::ConfigureTone { .. } => {
                PhyPowerAttenuationMmioBinding::new(action)
                    .map(Self::Mmio)
                    .map_err(|_| PhyTxCapExternalBindingError::UnsupportedAction)
            }
            PhyPowerAttenuationAction::ToneSar(action) => {
                PhyToneSarExternalBinding::lower(action).map(Self::ToneSar)
            }
            PhyPowerAttenuationAction::Complete(_) | PhyPowerAttenuationAction::Failed(_) => {
                Err(PhyTxCapExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxCapSearchExternalBinding {
    Mmio(PhyTxCapSearchMmioBinding),
    I2c(crate::analog::i2c::MaskedI2cWriteBinding),
    ToneSar(PhyToneSarExternalBinding),
}

impl PhyTxCapSearchExternalBinding {
    pub fn lower(action: PhyTxCapSearchAction) -> Result<Self, PhyTxCapExternalBindingError> {
        match action {
            PhyTxCapSearchAction::ConfigureTone { .. } => PhyTxCapSearchMmioBinding::new(action)
                .map(Self::Mmio)
                .map_err(|_| PhyTxCapExternalBindingError::UnsupportedAction),
            PhyTxCapSearchAction::I2c(action) => {
                crate::analog::i2c::MaskedI2cWriteBinding::new(action)
                    .map(Self::I2c)
                    .map_err(|_| PhyTxCapExternalBindingError::UnsupportedAction)
            }
            PhyTxCapSearchAction::ToneSar(action) => {
                PhyToneSarExternalBinding::lower(action).map(Self::ToneSar)
            }
            PhyTxCapSearchAction::Complete(_) | PhyTxCapSearchAction::Failed(_) => {
                Err(PhyTxCapExternalBindingError::UnsupportedAction)
            }
        }
    }
}

/// Exhaustive lowering of every non-terminal `phy_tx_cap_init` action.
#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxCapExternalBinding {
    Environment(PhyTxCalibrationEnvironmentExternalBinding),
    Rfpll(crate::analog::rfpll::RfpllFrequencyExternalBinding),
    I2c(crate::analog::i2c::MaskedI2cWriteBinding),
    Attenuation(PhyPowerAttenuationExternalBinding),
    Search(PhyTxCapSearchExternalBinding),
}

impl PhyTxCapExternalBinding {
    pub fn lower(action: PhyTxCapAction) -> Result<Self, PhyTxCapExternalBindingError> {
        match action {
            PhyTxCapAction::Environment(action) => {
                PhyTxCalibrationEnvironmentExternalBinding::lower(action).map(Self::Environment)
            }
            PhyTxCapAction::Rfpll(action) => {
                crate::analog::rfpll::RfpllFrequencyExternalBinding::lower(action)
                    .map(Self::Rfpll)
                    .map_err(|_| PhyTxCapExternalBindingError::UnsupportedAction)
            }
            PhyTxCapAction::I2c(action) => crate::analog::i2c::MaskedI2cWriteBinding::new(action)
                .map(Self::I2c)
                .map_err(|_| PhyTxCapExternalBindingError::UnsupportedAction),
            PhyTxCapAction::Attenuation(action) => {
                PhyPowerAttenuationExternalBinding::lower(action).map(Self::Attenuation)
            }
            PhyTxCapAction::Search(action) => {
                PhyTxCapSearchExternalBinding::lower(action).map(Self::Search)
            }
            PhyTxCapAction::Complete(_) | PhyTxCapAction::Failed(_) => {
                Err(PhyTxCapExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[cfg(test)]
mod tests;
