//! Event-driven replacement for `phy_check_rx_sat`.
//!
//! The pinned archive body performs eleven PBus commands, calls
//! `ets_delay_us(5)`, then reads the PAC activity field exactly 100 times.
//! No dedicated completion interrupt is evidenced in the available S31
//! PAC/SVD or ROM symbols. Rust therefore retains the required polling as 100
//! separately completed one-shot samples. The executor may yield or arm an
//! async timer between samples; neither this transition nor its MMIO leaf
//! contains a spin loop.

use crate::analog::pbus::PhyPbusForceTest;

pub const PHY_RX_SATURATION_DELAY_MICROS: u32 = 5;
pub const PHY_RX_SATURATION_SAMPLE_COUNT: u8 = 100;

const PHY_RX_SATURATION_PBUS_COUNT: u8 = 11;

const fn pbus_transaction(index: u8, parameter_002: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(4, 1, 0),
        1 => PhyPbusForceTest::new(4, 2, 1),
        2 => PhyPbusForceTest::new(5, 1, 0),
        3 => PhyPbusForceTest::new(0, 1, 0x40),
        4 => PhyPbusForceTest::new(0, 2, parameter_002 as u16),
        5 => PhyPbusForceTest::new(1, 1, 0x189),
        6 => PhyPbusForceTest::new(1, 2, 0),
        7 => PhyPbusForceTest::new(2, 1, 0x100),
        8 => PhyPbusForceTest::new(3, 1, 0x100),
        9 => PhyPbusForceTest::new(2, 2, 0x100),
        _ => PhyPbusForceTest::new(3, 2, 0x100),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxSaturationOutcome {
    Measured { saturated_samples: u8, samples: u8 },
    PbusTimedOut(PhyPbusForceTest),
    CaptureTimedOut,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxSaturationAction {
    ConfigureDebugMode,
    ForcePbus(PhyPbusForceTest),
    DelayMicros { micros: u32 },
    SampleStatus { sample_index: u8, samples: u8 },
    ConfigureWorkMode,
    Complete(PhyRxSaturationOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxSaturationCompletion {
    DebugModeConfigured,
    PbusCompleted(PhyPbusForceTest),
    PbusTimedOut(PhyPbusForceTest),
    DelayElapsed {
        micros: u32,
    },
    StatusSampled {
        sample_index: u8,
        active: bool,
    },
    #[cfg_attr(
        not(any(test, feature = "validation-probes")),
        allow(
            dead_code,
            reason = "only the validation binding constructs this timeout"
        )
    )]
    CaptureTimedOut,
    WorkModeConfigured,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxSaturationTransitionError {
    WrongCompletion,
    InvalidCapture,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyRxSaturationStep {
    DebugMode,
    Pbus {
        index: u8,
    },
    Delay,
    Sample {
        sample_index: u8,
        saturated_samples: u8,
    },
    WorkMode(PhyRxSaturationOutcome),
    Complete(PhyRxSaturationOutcome),
}

/// Caller-driven `phy_check_rx_sat` state machine.
///
/// Each status read is a separate action/completion pair. This preserves the
/// reference's bounded 100-sample policy while allowing the Rust executor to
/// yield between samples.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxSaturationTransition {
    parameter_002: u8,
    step: PhyRxSaturationStep,
}

impl PhyRxSaturationTransition {
    pub const fn new(parameter_002: u8) -> Self {
        Self {
            parameter_002,
            step: PhyRxSaturationStep::DebugMode,
        }
    }

    pub const fn action(self) -> PhyRxSaturationAction {
        match self.step {
            PhyRxSaturationStep::DebugMode => PhyRxSaturationAction::ConfigureDebugMode,
            PhyRxSaturationStep::Pbus { index } => {
                PhyRxSaturationAction::ForcePbus(pbus_transaction(index, self.parameter_002))
            }
            PhyRxSaturationStep::Delay => PhyRxSaturationAction::DelayMicros {
                micros: PHY_RX_SATURATION_DELAY_MICROS,
            },
            PhyRxSaturationStep::Sample { sample_index, .. } => {
                PhyRxSaturationAction::SampleStatus {
                    sample_index,
                    samples: PHY_RX_SATURATION_SAMPLE_COUNT,
                }
            }
            PhyRxSaturationStep::WorkMode(_) => PhyRxSaturationAction::ConfigureWorkMode,
            PhyRxSaturationStep::Complete(outcome) => PhyRxSaturationAction::Complete(outcome),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyRxSaturationCompletion,
    ) -> Result<(), PhyRxSaturationTransitionError> {
        self.step = match (self.step, completion) {
            (PhyRxSaturationStep::DebugMode, PhyRxSaturationCompletion::DebugModeConfigured) => {
                PhyRxSaturationStep::Pbus { index: 0 }
            }
            (
                PhyRxSaturationStep::Pbus { index },
                PhyRxSaturationCompletion::PbusCompleted(completed),
            ) if completed == pbus_transaction(index, self.parameter_002) => {
                let next = index + 1;
                if next == PHY_RX_SATURATION_PBUS_COUNT {
                    PhyRxSaturationStep::Delay
                } else {
                    PhyRxSaturationStep::Pbus { index: next }
                }
            }
            (
                PhyRxSaturationStep::Pbus { index },
                PhyRxSaturationCompletion::PbusTimedOut(completed),
            ) if completed == pbus_transaction(index, self.parameter_002) => {
                PhyRxSaturationStep::WorkMode(PhyRxSaturationOutcome::PbusTimedOut(completed))
            }
            (
                PhyRxSaturationStep::Delay,
                PhyRxSaturationCompletion::DelayElapsed {
                    micros: PHY_RX_SATURATION_DELAY_MICROS,
                },
            ) => PhyRxSaturationStep::Sample {
                sample_index: 0,
                saturated_samples: 0,
            },
            (
                PhyRxSaturationStep::Sample {
                    sample_index,
                    saturated_samples,
                },
                PhyRxSaturationCompletion::StatusSampled {
                    sample_index: completed_index,
                    active,
                },
            ) if completed_index == sample_index => {
                let saturated_samples = saturated_samples.wrapping_add(active as u8);
                let next = sample_index + 1;
                if next == PHY_RX_SATURATION_SAMPLE_COUNT {
                    PhyRxSaturationStep::WorkMode(PhyRxSaturationOutcome::Measured {
                        saturated_samples,
                        samples: PHY_RX_SATURATION_SAMPLE_COUNT,
                    })
                } else {
                    PhyRxSaturationStep::Sample {
                        sample_index: next,
                        saturated_samples,
                    }
                }
            }
            (
                PhyRxSaturationStep::Sample { .. },
                PhyRxSaturationCompletion::StatusSampled { .. },
            ) => return Err(PhyRxSaturationTransitionError::InvalidCapture),
            (PhyRxSaturationStep::Sample { .. }, PhyRxSaturationCompletion::CaptureTimedOut) => {
                PhyRxSaturationStep::WorkMode(PhyRxSaturationOutcome::CaptureTimedOut)
            }
            (
                PhyRxSaturationStep::WorkMode(outcome),
                PhyRxSaturationCompletion::WorkModeConfigured,
            ) => PhyRxSaturationStep::Complete(outcome),
            (PhyRxSaturationStep::Complete(_), _) => {
                return Err(PhyRxSaturationTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyRxSaturationTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxSaturationSampleBindingError {
    NotStatusSample,
}

/// A non-cloneable token for exactly one polling sample.
///
/// Repeating the poll is a state-machine/executor decision. The target leaf
/// itself performs one volatile read and cannot spin.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxSaturationSampleBinding {
    sample_index: u8,
}

impl PhyRxSaturationSampleBinding {
    pub fn new(action: PhyRxSaturationAction) -> Result<Self, PhyRxSaturationSampleBindingError> {
        match action {
            PhyRxSaturationAction::SampleStatus {
                sample_index,
                samples: PHY_RX_SATURATION_SAMPLE_COUNT,
            } => Ok(Self { sample_index }),
            _ => Err(PhyRxSaturationSampleBindingError::NotStatusSample),
        }
    }

    /// Perform one volatile sample and consume the issued identity.
    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyContext,
    ) -> PhyRxSaturationCompletion {
        PhyRxSaturationCompletion::StatusSampled {
            sample_index: self.sample_index,
            active: oer_esp32s31_hal::phy::iq_estimator::sample_activity(registers),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxSaturationBindingError {
    UnsupportedAction,
    Pbus(crate::analog::pbus::PhyPbusHardwareBindingError),
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxSaturationMmioBinding {
    action: PhyRxSaturationAction,
}

impl PhyRxSaturationMmioBinding {
    pub fn new(action: PhyRxSaturationAction) -> Result<Self, PhyRxSaturationBindingError> {
        match action {
            PhyRxSaturationAction::ConfigureDebugMode
            | PhyRxSaturationAction::ConfigureWorkMode => Ok(Self { action }),
            _ => Err(PhyRxSaturationBindingError::UnsupportedAction),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyContext,
    ) -> PhyRxSaturationCompletion {
        match self.action {
            PhyRxSaturationAction::ConfigureDebugMode => {
                oer_esp32s31_hal::phy::pbus::configure_debug_mode(registers);
                PhyRxSaturationCompletion::DebugModeConfigured
            }
            PhyRxSaturationAction::ConfigureWorkMode => {
                let _ = oer_esp32s31_hal::phy::pbus::configure_work_mode(registers);
                PhyRxSaturationCompletion::WorkModeConfigured
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxSaturationPbusBinding {
    transaction: PhyPbusForceTest,
    hardware: crate::analog::pbus::PhyPbusHardwareBinding,
}

impl PhyRxSaturationPbusBinding {
    pub fn new(action: PhyRxSaturationAction) -> Result<Self, PhyRxSaturationBindingError> {
        let PhyRxSaturationAction::ForcePbus(transaction) = action else {
            return Err(PhyRxSaturationBindingError::UnsupportedAction);
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

    pub fn into_completion(self) -> Result<PhyRxSaturationCompletion, PhyRxSaturationBindingError> {
        self.hardware
            .into_transaction()
            .map(PhyRxSaturationCompletion::PbusCompleted)
            .map_err(PhyRxSaturationBindingError::Pbus)
    }

    pub const fn into_timeout_completion(self) -> PhyRxSaturationCompletion {
        PhyRxSaturationCompletion::PbusTimedOut(self.transaction)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxSaturationTimerBinding {
    micros: u32,
}

impl PhyRxSaturationTimerBinding {
    pub fn new(action: PhyRxSaturationAction) -> Result<Self, PhyRxSaturationBindingError> {
        match action {
            PhyRxSaturationAction::DelayMicros { micros } => Ok(Self { micros }),
            _ => Err(PhyRxSaturationBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyRxSaturationCompletion {
        PhyRxSaturationCompletion::DelayElapsed {
            micros: self.micros,
        }
    }
}

/// Exhaustive lowering for one bounded RX-saturation action.
#[derive(Debug, Eq, PartialEq)]
pub enum PhyRxSaturationExternalBinding {
    Mmio(PhyRxSaturationMmioBinding),
    Pbus(PhyRxSaturationPbusBinding),
    Timer(PhyRxSaturationTimerBinding),
    Sample(PhyRxSaturationSampleBinding),
}

impl PhyRxSaturationExternalBinding {
    pub fn lower(action: PhyRxSaturationAction) -> Result<Self, PhyRxSaturationBindingError> {
        if let Ok(binding) = PhyRxSaturationMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        if let Ok(binding) = PhyRxSaturationPbusBinding::new(action) {
            return Ok(Self::Pbus(binding));
        }
        if let Ok(binding) = PhyRxSaturationTimerBinding::new(action) {
            return Ok(Self::Timer(binding));
        }
        if let Ok(binding) = PhyRxSaturationSampleBinding::new(action) {
            return Ok(Self::Sample(binding));
        }
        Err(PhyRxSaturationBindingError::UnsupportedAction)
    }
}

#[cfg(test)]
mod tests;
