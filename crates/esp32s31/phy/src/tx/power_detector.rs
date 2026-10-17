//! Rust-owned ESP32-S31 power-detector reference-code calibration.
//!
//! The required root is rev0 ROM `phy_pwdet_code_cal` at `0x2f82_b432`,
//! size 76 bytes. Its complete child graph enters PBus debug mode, configures
//! the TX calibration path, takes two four-sample SAR measurements, restores
//! the radio, stores two signed reference codes, and sets one calibrated bit
//! through the global `phy_param` pointer.
//!
//! No completion interrupt for the recovered three-bit SAR-ready field is
//! evidenced in the ROM symbol graph or public S31 material. Rust therefore
//! preserves that hardware poll. Each [`PhyPwdetReadyBinding`] consumes
//! exactly one issued identity and performs one volatile read; an outer async
//! executor decides when to issue another sample and owns the finite
//! deadline. The ROM spin loop and synchronous microsecond delays are not
//! copied.

use crate::analog::pbus::PhyPbusForceTest;
#[cfg(any(test, feature = "validation-probes"))]
use crate::calibration::estimator::phy_linear_to_db;

// Complete rev0 ROM `phy_pwdet_ref_code+0x24` and `+0x50` load `a0 = 4`
// before calling the runtime-table `phy_get_tone_sar_dout_` leaf. That leaf
// repeats `phy_pwdet_tone_start`/`phy_read_sar_dout` exactly `a0` times and
// returns the unsigned average.
pub const PHY_PWDET_SAMPLES_PER_REFERENCE: u8 = 4;

/// Required pinned `libphy.a` vendor-ABI no-op leaf.
///
/// The ESP32-S31 archive body is exactly one `ret` instruction; power-detector
/// ownership is instead expressed by the surrounding Rust state machine.
#[inline]
#[cfg(feature = "validation-probes")]
pub const fn phy_pwdet_always_en() {}

/// Required pinned `libphy.a` vendor-ABI no-op leaf.
///
/// Like [`phy_pwdet_always_en`], the vendor body has no observable operation.
#[inline]
#[cfg(feature = "validation-probes")]
pub const fn phy_pwdet_onetime_en() {}

const ENTER_PBUS_COUNT: u8 = 15;
const EXIT_PBUS_COUNT: u8 = 7;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPwdetParameters {
    pub already_calibrated: bool,
    pub pbus_tx_path_value: u8,
    pub pbus_rx_path_value: u8,
    pub dco: [u16; 4],
    pub clear_tone_after_ready: bool,
    pub reference_codes: [i16; 2],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPwdetOutcome {
    pub reference_codes: [i16; 2],
    pub calibrated: bool,
    pub measurement_performed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPwdetFailure {
    PbusTimedOut(PhyPbusForceTest),
    SarReadyDeadlineElapsed {
        measurement_index: u8,
        sample_index: u8,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPwdetDelayPhase {
    ToneArmed {
        measurement_index: u8,
        sample_index: u8,
    },
    SarTriggered {
        measurement_index: u8,
        sample_index: u8,
    },
    PbusWorkMode,
    PbusWorkModePulse,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPwdetAction {
    ConfigurePbusDebugMode,
    ForcePbus(PhyPbusForceTest),
    ConfigureTxClock {
        enabled: bool,
    },
    ConfigurePowerDetector,
    ConfigureCalibrationMode,
    ConfigureTone,
    WriteReferenceControl {
        value: u16,
    },
    ArmTone {
        measurement_index: u8,
        sample_index: u8,
    },
    DelayMicros {
        phase: PhyPwdetDelayPhase,
        micros: u32,
    },
    TriggerSar {
        measurement_index: u8,
        sample_index: u8,
    },
    PollSarReady {
        measurement_index: u8,
        sample_index: u8,
    },
    ClearToneArm {
        measurement_index: u8,
        sample_index: u8,
    },
    ReadSarSample {
        measurement_index: u8,
        sample_index: u8,
    },
    StopTone,
    ConfigurePbusWorkMode,
    ConfigurePbusWorkModePulse,
    ClearPbusWorkModePulse,
    Complete(PhyPwdetOutcome),
    Failed(PhyPwdetFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPwdetCompletion {
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
    ToneConfigured,
    ReferenceControlWritten {
        value: u16,
    },
    ToneArmed {
        measurement_index: u8,
        sample_index: u8,
    },
    DelayElapsed {
        phase: PhyPwdetDelayPhase,
        micros: u32,
    },
    SarTriggered {
        measurement_index: u8,
        sample_index: u8,
    },
    SarReadySampled {
        measurement_index: u8,
        sample_index: u8,
        ready: bool,
    },
    #[cfg_attr(
        not(any(test, feature = "validation-probes")),
        allow(
            dead_code,
            reason = "only the validation binding constructs this timeout"
        )
    )]
    SarReadyDeadlineElapsed {
        measurement_index: u8,
        sample_index: u8,
    },
    ToneArmCleared {
        measurement_index: u8,
        sample_index: u8,
    },
    SarSampled {
        measurement_index: u8,
        sample_index: u8,
        value: u16,
    },
    ToneStopped,
    PbusWorkModeConfigured {
        settle_required: bool,
    },
    PbusWorkModePulseConfigured,
    PbusWorkModePulseCleared,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPwdetTransitionError {
    WrongCompletion,
    InvalidSarSample,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyPwdetTerminal {
    Complete(PhyPwdetOutcome),
    Failed(PhyPwdetFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyPwdetStep {
    ConfigurePbusDebugMode,
    EnterPbus {
        index: u8,
    },
    EnableTxClock,
    ConfigurePowerDetector,
    ConfigureCalibrationMode,
    ConfigureTone,
    WriteReferenceControl {
        measurement_index: u8,
    },
    ArmTone {
        measurement_index: u8,
        sample_index: u8,
        sample_sum: u32,
    },
    ArmDelay {
        measurement_index: u8,
        sample_index: u8,
        sample_sum: u32,
    },
    TriggerSar {
        measurement_index: u8,
        sample_index: u8,
        sample_sum: u32,
    },
    TriggerDelay {
        measurement_index: u8,
        sample_index: u8,
        sample_sum: u32,
    },
    PollSarReady {
        measurement_index: u8,
        sample_index: u8,
        sample_sum: u32,
    },
    ClearToneArm {
        measurement_index: u8,
        sample_index: u8,
        sample_sum: u32,
    },
    ReadSarSample {
        measurement_index: u8,
        sample_index: u8,
        sample_sum: u32,
    },
    WriteFinalReferenceControl,
    StopTone(PhyPwdetTerminal),
    DisableTxClock(PhyPwdetTerminal),
    ExitPbus {
        index: u8,
        terminal: PhyPwdetTerminal,
    },
    ConfigurePbusWorkMode(PhyPwdetTerminal),
    PbusSettleDelay(PhyPwdetTerminal),
    ConfigurePbusWorkModePulse(PhyPwdetTerminal),
    PbusPulseDelay(PhyPwdetTerminal),
    ClearPbusWorkModePulse(PhyPwdetTerminal),
    Complete(PhyPwdetOutcome),
    Failed(PhyPwdetFailure),
}

const fn enter_pbus_transaction(index: u8, parameters: PhyPwdetParameters) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(0, 1, 0x080),
        1 => PhyPbusForceTest::new(0, 2, 0),
        2 => PhyPbusForceTest::new(4, 2, 0),
        3 => PhyPbusForceTest::new(1, 1, 0x07c),
        4 => PhyPbusForceTest::new(2, 1, 0x100),
        5 => PhyPbusForceTest::new(3, 1, 0x100),
        6 => PhyPbusForceTest::new(2, 2, 0x100),
        7 => PhyPbusForceTest::new(3, 2, 0x100),
        8 => PhyPbusForceTest::new(1, 2, 0),
        9 => PhyPbusForceTest::new(4, 1, 0x00b),
        10 => PhyPbusForceTest::new(
            5,
            1,
            (parameters.pbus_tx_path_value as u16).wrapping_add(0x1c0),
        ),
        11 => PhyPbusForceTest::new(2, 1, parameters.dco[0]),
        12 => PhyPbusForceTest::new(3, 1, parameters.dco[1]),
        13 => PhyPbusForceTest::new(2, 2, parameters.dco[2]),
        _ => PhyPbusForceTest::new(3, 2, parameters.dco[3]),
    }
}

const fn exit_pbus_transaction(index: u8, parameters: PhyPwdetParameters) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(4, 1, 0),
        1 => PhyPbusForceTest::new(4, 2, 1),
        2 => PhyPbusForceTest::new(5, 1, 0),
        3 => PhyPbusForceTest::new(0, 1, 0x040),
        4 => PhyPbusForceTest::new(0, 2, parameters.pbus_rx_path_value as u16),
        5 => PhyPbusForceTest::new(1, 1, 0x189),
        _ => PhyPbusForceTest::new(1, 2, 0),
    }
}

const fn reference_control(measurement_index: u8) -> u16 {
    if measurement_index == 0 { 0 } else { 0x5555 }
}

const fn delay_phase(armed: bool, measurement_index: u8, sample_index: u8) -> PhyPwdetDelayPhase {
    if armed {
        PhyPwdetDelayPhase::ToneArmed {
            measurement_index,
            sample_index,
        }
    } else {
        PhyPwdetDelayPhase::SarTriggered {
            measurement_index,
            sample_index,
        }
    }
}

const fn terminal_after_pbus_timeout(
    terminal: PhyPwdetTerminal,
    transaction: PhyPbusForceTest,
) -> PhyPwdetTerminal {
    match terminal {
        PhyPwdetTerminal::Complete(_) => {
            PhyPwdetTerminal::Failed(PhyPwdetFailure::PbusTimedOut(transaction))
        }
        failure => failure,
    }
}

const fn terminal_step(terminal: PhyPwdetTerminal) -> PhyPwdetStep {
    match terminal {
        PhyPwdetTerminal::Complete(outcome) => PhyPwdetStep::Complete(outcome),
        PhyPwdetTerminal::Failed(failure) => PhyPwdetStep::Failed(failure),
    }
}

/// Reproduce `phy_get_sar_sig_ref` using explicit Rust-owned reference codes.
pub const fn sar_signal_reference(sample: u16, reference_codes: [i16; 2]) -> [i16; 2] {
    let sample = sample.wrapping_add(25);
    let reference_0 = reference_codes[0] as u16;
    let reference_1 = reference_codes[1] as u16;
    [
        if sample >= reference_0 {
            sample.wrapping_sub(reference_0) as i16
        } else {
            0
        },
        if reference_1 >= reference_0 {
            reference_1.wrapping_sub(reference_0) as i16
        } else {
            0
        },
    ]
}

/// Reproduce the separate ROM `phy_get_power_db` transform.
///
/// This transform is consumed by later TX calibration code. Complete rev0
/// ROM `phy_pwdet_ref_code` does not call it: that root stores the raw
/// four-sample averages returned by `phy_get_tone_sar_dout_`.
#[cfg(feature = "validation-probes")]
pub fn calculate_pwdet_reference(sample_average: u16, reference_codes: [i16; 2]) -> i16 {
    let signal = sar_signal_reference(sample_average, reference_codes);
    phy_linear_to_db(i32::from(signal[0]), 3)
        .wrapping_add(4)
        .wrapping_sub(phy_linear_to_db(i32::from(signal[1]), 3)) as i16
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPwdetTransition {
    parameters: PhyPwdetParameters,
    reference_codes: [i16; 2],
    step: PhyPwdetStep,
}

impl PhyPwdetTransition {
    pub const fn new(parameters: PhyPwdetParameters) -> Self {
        let step = if parameters.already_calibrated {
            PhyPwdetStep::Complete(PhyPwdetOutcome {
                reference_codes: parameters.reference_codes,
                calibrated: true,
                measurement_performed: false,
            })
        } else {
            PhyPwdetStep::ConfigurePbusDebugMode
        };
        Self {
            parameters,
            reference_codes: parameters.reference_codes,
            step,
        }
    }

    pub const fn action(self) -> PhyPwdetAction {
        match self.step {
            PhyPwdetStep::ConfigurePbusDebugMode => PhyPwdetAction::ConfigurePbusDebugMode,
            PhyPwdetStep::EnterPbus { index } => {
                PhyPwdetAction::ForcePbus(enter_pbus_transaction(index, self.parameters))
            }
            PhyPwdetStep::EnableTxClock => PhyPwdetAction::ConfigureTxClock { enabled: true },
            PhyPwdetStep::ConfigurePowerDetector => PhyPwdetAction::ConfigurePowerDetector,
            PhyPwdetStep::ConfigureCalibrationMode => PhyPwdetAction::ConfigureCalibrationMode,
            PhyPwdetStep::ConfigureTone => PhyPwdetAction::ConfigureTone,
            PhyPwdetStep::WriteReferenceControl { measurement_index } => {
                PhyPwdetAction::WriteReferenceControl {
                    value: reference_control(measurement_index),
                }
            }
            PhyPwdetStep::ArmTone {
                measurement_index,
                sample_index,
                ..
            } => PhyPwdetAction::ArmTone {
                measurement_index,
                sample_index,
            },
            PhyPwdetStep::ArmDelay {
                measurement_index,
                sample_index,
                ..
            } => PhyPwdetAction::DelayMicros {
                phase: delay_phase(true, measurement_index, sample_index),
                micros: 1,
            },
            PhyPwdetStep::TriggerSar {
                measurement_index,
                sample_index,
                ..
            } => PhyPwdetAction::TriggerSar {
                measurement_index,
                sample_index,
            },
            PhyPwdetStep::TriggerDelay {
                measurement_index,
                sample_index,
                ..
            } => PhyPwdetAction::DelayMicros {
                phase: delay_phase(false, measurement_index, sample_index),
                micros: 2,
            },
            PhyPwdetStep::PollSarReady {
                measurement_index,
                sample_index,
                ..
            } => PhyPwdetAction::PollSarReady {
                measurement_index,
                sample_index,
            },
            PhyPwdetStep::ClearToneArm {
                measurement_index,
                sample_index,
                ..
            } => PhyPwdetAction::ClearToneArm {
                measurement_index,
                sample_index,
            },
            PhyPwdetStep::ReadSarSample {
                measurement_index,
                sample_index,
                ..
            } => PhyPwdetAction::ReadSarSample {
                measurement_index,
                sample_index,
            },
            PhyPwdetStep::WriteFinalReferenceControl => {
                PhyPwdetAction::WriteReferenceControl { value: 0xaaaa }
            }
            PhyPwdetStep::StopTone(_) => PhyPwdetAction::StopTone,
            PhyPwdetStep::DisableTxClock(_) => PhyPwdetAction::ConfigureTxClock { enabled: false },
            PhyPwdetStep::ExitPbus { index, .. } => {
                PhyPwdetAction::ForcePbus(exit_pbus_transaction(index, self.parameters))
            }
            PhyPwdetStep::ConfigurePbusWorkMode(_) => PhyPwdetAction::ConfigurePbusWorkMode,
            PhyPwdetStep::PbusSettleDelay(_) => PhyPwdetAction::DelayMicros {
                phase: PhyPwdetDelayPhase::PbusWorkMode,
                micros: 1,
            },
            PhyPwdetStep::ConfigurePbusWorkModePulse(_) => {
                PhyPwdetAction::ConfigurePbusWorkModePulse
            }
            PhyPwdetStep::PbusPulseDelay(_) => PhyPwdetAction::DelayMicros {
                phase: PhyPwdetDelayPhase::PbusWorkModePulse,
                micros: 2,
            },
            PhyPwdetStep::ClearPbusWorkModePulse(_) => PhyPwdetAction::ClearPbusWorkModePulse,
            PhyPwdetStep::Complete(outcome) => PhyPwdetAction::Complete(outcome),
            PhyPwdetStep::Failed(failure) => PhyPwdetAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyPwdetCompletion,
    ) -> Result<(), PhyPwdetTransitionError> {
        self.step = match (self.step, completion) {
            (PhyPwdetStep::ConfigurePbusDebugMode, PhyPwdetCompletion::PbusDebugModeConfigured) => {
                PhyPwdetStep::EnterPbus { index: 0 }
            }
            (PhyPwdetStep::EnterPbus { index }, PhyPwdetCompletion::PbusCompleted(completed))
                if completed == enter_pbus_transaction(index, self.parameters) =>
            {
                if index + 1 == ENTER_PBUS_COUNT {
                    PhyPwdetStep::EnableTxClock
                } else {
                    PhyPwdetStep::EnterPbus { index: index + 1 }
                }
            }
            (PhyPwdetStep::EnterPbus { index }, PhyPwdetCompletion::PbusTimedOut(completed))
                if completed == enter_pbus_transaction(index, self.parameters) =>
            {
                PhyPwdetStep::ConfigurePbusWorkMode(PhyPwdetTerminal::Failed(
                    PhyPwdetFailure::PbusTimedOut(completed),
                ))
            }
            (
                PhyPwdetStep::EnableTxClock,
                PhyPwdetCompletion::TxClockConfigured { enabled: true },
            ) => PhyPwdetStep::ConfigurePowerDetector,
            (PhyPwdetStep::ConfigurePowerDetector, PhyPwdetCompletion::PowerDetectorConfigured) => {
                PhyPwdetStep::ConfigureCalibrationMode
            }
            (
                PhyPwdetStep::ConfigureCalibrationMode,
                PhyPwdetCompletion::CalibrationModeConfigured,
            ) => PhyPwdetStep::ConfigureTone,
            (PhyPwdetStep::ConfigureTone, PhyPwdetCompletion::ToneConfigured) => {
                PhyPwdetStep::WriteReferenceControl {
                    measurement_index: 0,
                }
            }
            (
                PhyPwdetStep::WriteReferenceControl { measurement_index },
                PhyPwdetCompletion::ReferenceControlWritten { value },
            ) if value == reference_control(measurement_index) => PhyPwdetStep::ArmTone {
                measurement_index,
                sample_index: 0,
                sample_sum: 0,
            },
            (
                PhyPwdetStep::ArmTone {
                    measurement_index,
                    sample_index,
                    sample_sum,
                },
                PhyPwdetCompletion::ToneArmed {
                    measurement_index: completed_measurement,
                    sample_index: completed_sample,
                },
            ) if completed_measurement == measurement_index && completed_sample == sample_index => {
                PhyPwdetStep::ArmDelay {
                    measurement_index,
                    sample_index,
                    sample_sum,
                }
            }
            (
                PhyPwdetStep::ArmDelay {
                    measurement_index,
                    sample_index,
                    sample_sum,
                },
                PhyPwdetCompletion::DelayElapsed { phase, micros: 1 },
            ) if phase == delay_phase(true, measurement_index, sample_index) => {
                PhyPwdetStep::TriggerSar {
                    measurement_index,
                    sample_index,
                    sample_sum,
                }
            }
            (
                PhyPwdetStep::TriggerSar {
                    measurement_index,
                    sample_index,
                    sample_sum,
                },
                PhyPwdetCompletion::SarTriggered {
                    measurement_index: completed_measurement,
                    sample_index: completed_sample,
                },
            ) if completed_measurement == measurement_index && completed_sample == sample_index => {
                PhyPwdetStep::TriggerDelay {
                    measurement_index,
                    sample_index,
                    sample_sum,
                }
            }
            (
                PhyPwdetStep::TriggerDelay {
                    measurement_index,
                    sample_index,
                    sample_sum,
                },
                PhyPwdetCompletion::DelayElapsed { phase, micros: 2 },
            ) if phase == delay_phase(false, measurement_index, sample_index) => {
                PhyPwdetStep::PollSarReady {
                    measurement_index,
                    sample_index,
                    sample_sum,
                }
            }
            (
                PhyPwdetStep::PollSarReady {
                    measurement_index,
                    sample_index,
                    sample_sum,
                },
                PhyPwdetCompletion::SarReadySampled {
                    measurement_index: completed_measurement,
                    sample_index: completed_sample,
                    ready,
                },
            ) if completed_measurement == measurement_index && completed_sample == sample_index => {
                if !ready {
                    PhyPwdetStep::PollSarReady {
                        measurement_index,
                        sample_index,
                        sample_sum,
                    }
                } else if self.parameters.clear_tone_after_ready {
                    PhyPwdetStep::ClearToneArm {
                        measurement_index,
                        sample_index,
                        sample_sum,
                    }
                } else {
                    PhyPwdetStep::ReadSarSample {
                        measurement_index,
                        sample_index,
                        sample_sum,
                    }
                }
            }
            (
                PhyPwdetStep::PollSarReady {
                    measurement_index,
                    sample_index,
                    ..
                },
                PhyPwdetCompletion::SarReadyDeadlineElapsed {
                    measurement_index: completed_measurement,
                    sample_index: completed_sample,
                },
            ) if completed_measurement == measurement_index && completed_sample == sample_index => {
                PhyPwdetStep::StopTone(PhyPwdetTerminal::Failed(
                    PhyPwdetFailure::SarReadyDeadlineElapsed {
                        measurement_index,
                        sample_index,
                    },
                ))
            }
            (
                PhyPwdetStep::ClearToneArm {
                    measurement_index,
                    sample_index,
                    sample_sum,
                },
                PhyPwdetCompletion::ToneArmCleared {
                    measurement_index: completed_measurement,
                    sample_index: completed_sample,
                },
            ) if completed_measurement == measurement_index && completed_sample == sample_index => {
                PhyPwdetStep::ReadSarSample {
                    measurement_index,
                    sample_index,
                    sample_sum,
                }
            }
            (
                PhyPwdetStep::ReadSarSample {
                    measurement_index,
                    sample_index,
                    sample_sum,
                },
                PhyPwdetCompletion::SarSampled {
                    measurement_index: completed_measurement,
                    sample_index: completed_sample,
                    value,
                },
            ) if completed_measurement == measurement_index && completed_sample == sample_index => {
                let sample_sum = sample_sum.wrapping_add(u32::from(value));
                if sample_index + 1 != PHY_PWDET_SAMPLES_PER_REFERENCE {
                    PhyPwdetStep::ArmTone {
                        measurement_index,
                        sample_index: sample_index + 1,
                        sample_sum,
                    }
                } else {
                    let average = (sample_sum / u32::from(PHY_PWDET_SAMPLES_PER_REFERENCE)) as u16;
                    // Complete rev0 ROM `phy_pwdet_ref_code+0x26` and `+0x52`
                    // store the raw `phy_get_tone_sar_dout_(4)` return value
                    // directly at `phy_param[0x1a]` and `[0x1c]`.
                    self.reference_codes[measurement_index as usize] = average as i16;
                    if measurement_index == 0 {
                        PhyPwdetStep::WriteReferenceControl {
                            measurement_index: 1,
                        }
                    } else {
                        PhyPwdetStep::WriteFinalReferenceControl
                    }
                }
            }
            (PhyPwdetStep::ReadSarSample { .. }, PhyPwdetCompletion::SarSampled { .. }) => {
                return Err(PhyPwdetTransitionError::InvalidSarSample);
            }
            (
                PhyPwdetStep::WriteFinalReferenceControl,
                PhyPwdetCompletion::ReferenceControlWritten { value: 0xaaaa },
            ) => PhyPwdetStep::StopTone(PhyPwdetTerminal::Complete(PhyPwdetOutcome {
                reference_codes: self.reference_codes,
                calibrated: true,
                measurement_performed: true,
            })),
            (PhyPwdetStep::StopTone(terminal), PhyPwdetCompletion::ToneStopped) => {
                PhyPwdetStep::DisableTxClock(terminal)
            }
            (
                PhyPwdetStep::DisableTxClock(terminal),
                PhyPwdetCompletion::TxClockConfigured { enabled: false },
            ) => PhyPwdetStep::ExitPbus { index: 0, terminal },
            (
                PhyPwdetStep::ExitPbus { index, terminal },
                PhyPwdetCompletion::PbusCompleted(completed),
            ) if completed == exit_pbus_transaction(index, self.parameters) => {
                if index + 1 == EXIT_PBUS_COUNT {
                    PhyPwdetStep::ConfigurePbusWorkMode(terminal)
                } else {
                    PhyPwdetStep::ExitPbus {
                        index: index + 1,
                        terminal,
                    }
                }
            }
            (
                PhyPwdetStep::ExitPbus { index, terminal },
                PhyPwdetCompletion::PbusTimedOut(completed),
            ) if completed == exit_pbus_transaction(index, self.parameters) => {
                PhyPwdetStep::ConfigurePbusWorkMode(terminal_after_pbus_timeout(
                    terminal, completed,
                ))
            }
            (
                PhyPwdetStep::ConfigurePbusWorkMode(terminal),
                PhyPwdetCompletion::PbusWorkModeConfigured {
                    settle_required: false,
                },
            ) => terminal_step(terminal),
            (
                PhyPwdetStep::ConfigurePbusWorkMode(terminal),
                PhyPwdetCompletion::PbusWorkModeConfigured {
                    settle_required: true,
                },
            ) => PhyPwdetStep::PbusSettleDelay(terminal),
            (
                PhyPwdetStep::PbusSettleDelay(terminal),
                PhyPwdetCompletion::DelayElapsed {
                    phase: PhyPwdetDelayPhase::PbusWorkMode,
                    micros: 1,
                },
            ) => PhyPwdetStep::ConfigurePbusWorkModePulse(terminal),
            (
                PhyPwdetStep::ConfigurePbusWorkModePulse(terminal),
                PhyPwdetCompletion::PbusWorkModePulseConfigured,
            ) => PhyPwdetStep::PbusPulseDelay(terminal),
            (
                PhyPwdetStep::PbusPulseDelay(terminal),
                PhyPwdetCompletion::DelayElapsed {
                    phase: PhyPwdetDelayPhase::PbusWorkModePulse,
                    micros: 2,
                },
            ) => PhyPwdetStep::ClearPbusWorkModePulse(terminal),
            (
                PhyPwdetStep::ClearPbusWorkModePulse(terminal),
                PhyPwdetCompletion::PbusWorkModePulseCleared,
            ) => terminal_step(terminal),
            (PhyPwdetStep::Complete(_) | PhyPwdetStep::Failed(_), _) => {
                return Err(PhyPwdetTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyPwdetTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPwdetBindingError {
    NotDirectMmio,
    NotReadyPoll,
}

/// Non-cloneable lowering for one PWDET readiness poll.
///
/// A false completion deliberately returns to the same transition state. The
/// executor must yield or await its own async cadence/deadline before issuing
/// another token; this object itself cannot loop.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyPwdetReadyBinding {
    measurement_index: u8,
    sample_index: u8,
}

impl PhyPwdetReadyBinding {
    pub fn new(action: PhyPwdetAction) -> Result<Self, PhyPwdetBindingError> {
        match action {
            PhyPwdetAction::PollSarReady {
                measurement_index,
                sample_index,
            } => Ok(Self {
                measurement_index,
                sample_index,
            }),
            _ => Err(PhyPwdetBindingError::NotReadyPoll),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyContext,
    ) -> PhyPwdetCompletion {
        PhyPwdetCompletion::SarReadySampled {
            measurement_index: self.measurement_index,
            sample_index: self.sample_index,
            ready: oer_esp32s31_hal::phy::power_detector::sample_ready(registers),
        }
    }
}

/// Non-cloneable lowering for one finite PWDET MMIO action.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyPwdetMmioBinding {
    action: PhyPwdetAction,
}

impl PhyPwdetMmioBinding {
    pub fn new(action: PhyPwdetAction) -> Result<Self, PhyPwdetBindingError> {
        match action {
            PhyPwdetAction::ConfigurePbusDebugMode
            | PhyPwdetAction::ConfigureTxClock { .. }
            | PhyPwdetAction::ConfigurePowerDetector
            | PhyPwdetAction::ConfigureCalibrationMode
            | PhyPwdetAction::ConfigureTone
            | PhyPwdetAction::WriteReferenceControl { .. }
            | PhyPwdetAction::ArmTone { .. }
            | PhyPwdetAction::TriggerSar { .. }
            | PhyPwdetAction::ClearToneArm { .. }
            | PhyPwdetAction::ReadSarSample { .. }
            | PhyPwdetAction::StopTone
            | PhyPwdetAction::ConfigurePbusWorkMode
            | PhyPwdetAction::ConfigurePbusWorkModePulse
            | PhyPwdetAction::ClearPbusWorkModePulse => Ok(Self { action }),
            _ => Err(PhyPwdetBindingError::NotDirectMmio),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyContext,
    ) -> PhyPwdetCompletion {
        match self.action {
            PhyPwdetAction::ConfigurePbusDebugMode => {
                oer_esp32s31_hal::phy::pbus::configure_debug_mode(registers);
                PhyPwdetCompletion::PbusDebugModeConfigured
            }
            PhyPwdetAction::ConfigureTxClock { enabled } => {
                oer_esp32s31_hal::phy::pbus::configure_tx_clock(registers, enabled);
                PhyPwdetCompletion::TxClockConfigured { enabled }
            }
            PhyPwdetAction::ConfigurePowerDetector => {
                oer_esp32s31_hal::phy::power_detector::configure_enabled(registers);
                PhyPwdetCompletion::PowerDetectorConfigured
            }
            PhyPwdetAction::ConfigureCalibrationMode => {
                oer_esp32s31_hal::phy::power_detector::configure_calibration_mode(registers);
                PhyPwdetCompletion::CalibrationModeConfigured
            }
            PhyPwdetAction::ConfigureTone => {
                // Rev0 ROM `phy_pwdet_ref_code+0x1c` calls the original
                // `phy_start_tx_tone_step(1, 0x80, 0x50, 0, 0, 0)`.
                // Its measurement invariant requires DAC scale and TX-gain
                // compensation to remain disabled until `StopTone`.
                crate::hardware::configure_phy_power_control_tone(registers, 0x80, 0x50);
                PhyPwdetCompletion::ToneConfigured
            }
            PhyPwdetAction::WriteReferenceControl { value } => {
                oer_esp32s31_hal::phy::power_detector::write_reference(registers, value);
                PhyPwdetCompletion::ReferenceControlWritten { value }
            }
            PhyPwdetAction::ArmTone {
                measurement_index,
                sample_index,
            } => {
                crate::hardware::arm_phy_power_detector_tone(registers);
                PhyPwdetCompletion::ToneArmed {
                    measurement_index,
                    sample_index,
                }
            }
            PhyPwdetAction::TriggerSar {
                measurement_index,
                sample_index,
            } => {
                oer_esp32s31_hal::phy::power_detector::trigger_sar(registers);
                PhyPwdetCompletion::SarTriggered {
                    measurement_index,
                    sample_index,
                }
            }
            PhyPwdetAction::ClearToneArm {
                measurement_index,
                sample_index,
            } => {
                crate::hardware::clear_phy_power_detector_tone_arm(registers);
                PhyPwdetCompletion::ToneArmCleared {
                    measurement_index,
                    sample_index,
                }
            }
            PhyPwdetAction::ReadSarSample {
                measurement_index,
                sample_index,
            } => PhyPwdetCompletion::SarSampled {
                measurement_index,
                sample_index,
                value: oer_esp32s31_hal::phy::power_detector::sample_sar(registers),
            },
            PhyPwdetAction::StopTone => {
                crate::hardware::stop_phy_power_detector_tone(registers);
                PhyPwdetCompletion::ToneStopped
            }
            PhyPwdetAction::ConfigurePbusWorkMode => PhyPwdetCompletion::PbusWorkModeConfigured {
                settle_required: oer_esp32s31_hal::phy::pbus::configure_work_mode(registers),
            },
            PhyPwdetAction::ConfigurePbusWorkModePulse => {
                oer_esp32s31_hal::phy::agc::configure_pbus_work_mode_pulse(registers);
                PhyPwdetCompletion::PbusWorkModePulseConfigured
            }
            PhyPwdetAction::ClearPbusWorkModePulse => {
                oer_esp32s31_hal::phy::agc::clear_pbus_work_mode_pulse(registers);
                PhyPwdetCompletion::PbusWorkModePulseCleared
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(test, feature = "validation-probes"))]
pub enum PhyPwdetPbusBindingAction {
    Start(PhyPbusForceTest),
    SampleCompletion(PhyPbusForceTest),
    Complete(PhyPbusForceTest),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPwdetPbusObservation {
    StillPending,
    Completed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPwdetPbusBindingError {
    NotPbusAction,
    WrongEdge,
    AlreadyComplete,
    Incomplete,
}

/// Non-cloneable owner of one PBus publication and its completion polling.
///
/// No PBus completion IRQ has been proved. `sample_target_once` therefore
/// retains the status poll but performs exactly one volatile read. A busy
/// result preserves the phase; only the outer async executor may schedule
/// another sample or convert its finite deadline into a timeout completion.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyPwdetPbusBinding {
    transaction: PhyPbusForceTest,
    hardware: crate::analog::pbus::PhyPbusHardwareBinding,
}

impl PhyPwdetPbusBinding {
    pub fn new(action: PhyPwdetAction) -> Result<Self, PhyPwdetPbusBindingError> {
        match action {
            PhyPwdetAction::ForcePbus(transaction) => Ok(Self {
                transaction,
                hardware: crate::analog::pbus::PhyPbusHardwareBinding::new(transaction),
            }),
            _ => Err(PhyPwdetPbusBindingError::NotPbusAction),
        }
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn action(&self) -> PhyPwdetPbusBindingAction {
        match self.hardware.action() {
            crate::analog::pbus::PhyPbusHardwareAction::Start(_) => {
                PhyPwdetPbusBindingAction::Start(self.transaction)
            }
            crate::analog::pbus::PhyPbusHardwareAction::AwaitCompletionEdge(_) => {
                PhyPwdetPbusBindingAction::SampleCompletion(self.transaction)
            }
            crate::analog::pbus::PhyPbusHardwareAction::Complete(_) => {
                PhyPwdetPbusBindingAction::Complete(self.transaction)
            }
        }
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub fn started(&mut self) -> Result<(), PhyPwdetPbusBindingError> {
        self.hardware.started().map_err(map_pbus_hardware_error)
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub fn observe_completed(
        &mut self,
        completed: bool,
    ) -> Result<PhyPwdetPbusObservation, PhyPwdetPbusBindingError> {
        match self
            .hardware
            .observe_completed(completed)
            .map_err(map_pbus_hardware_error)?
        {
            crate::analog::pbus::PhyPbusHardwareObservation::StillPending => {
                Ok(PhyPwdetPbusObservation::StillPending)
            }
            crate::analog::pbus::PhyPbusHardwareObservation::EdgeConsumed => {
                Ok(PhyPwdetPbusObservation::Completed)
            }
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target(
        &mut self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> Result<(), PhyPwdetPbusBindingError> {
        self.hardware
            .start_target(registers)
            .map_err(map_pbus_hardware_error)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn sample_target_once(
        &mut self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> Result<PhyPwdetPbusObservation, PhyPwdetPbusBindingError> {
        match self
            .hardware
            .observe_target_edge(registers)
            .map_err(map_pbus_hardware_error)?
        {
            crate::analog::pbus::PhyPbusHardwareObservation::StillPending => {
                Ok(PhyPwdetPbusObservation::StillPending)
            }
            crate::analog::pbus::PhyPbusHardwareObservation::EdgeConsumed => {
                Ok(PhyPwdetPbusObservation::Completed)
            }
        }
    }

    pub fn into_completion(self) -> Result<PhyPwdetCompletion, PhyPwdetPbusBindingError> {
        self.hardware
            .into_transaction()
            .map(PhyPwdetCompletion::PbusCompleted)
            .map_err(map_pbus_hardware_error)
    }

    #[cfg(feature = "validation-probes")]
    pub const fn into_timeout_completion(self) -> PhyPwdetCompletion {
        PhyPwdetCompletion::PbusTimedOut(self.transaction)
    }
}

fn map_pbus_hardware_error(
    error: crate::analog::pbus::PhyPbusHardwareBindingError,
) -> PhyPwdetPbusBindingError {
    match error {
        crate::analog::pbus::PhyPbusHardwareBindingError::WrongEdge => {
            PhyPwdetPbusBindingError::WrongEdge
        }
        crate::analog::pbus::PhyPbusHardwareBindingError::AlreadyComplete => {
            PhyPwdetPbusBindingError::AlreadyComplete
        }
        crate::analog::pbus::PhyPbusHardwareBindingError::Incomplete => {
            PhyPwdetPbusBindingError::Incomplete
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPwdetTimerBindingError {
    NotTimerAction,
}

/// Consumed identity for one delay completed by an external Rust async timer.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyPwdetTimerBinding {
    phase: PhyPwdetDelayPhase,
    micros: u32,
}

impl PhyPwdetTimerBinding {
    pub fn new(action: PhyPwdetAction) -> Result<Self, PhyPwdetTimerBindingError> {
        match action {
            PhyPwdetAction::DelayMicros { phase, micros } => Ok(Self { phase, micros }),
            _ => Err(PhyPwdetTimerBindingError::NotTimerAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyPwdetCompletion {
        PhyPwdetCompletion::DelayElapsed {
            phase: self.phase,
            micros: self.micros,
        }
    }
}

/// Exhaustive lowering of every non-terminal power-detector action.
#[derive(Debug, Eq, PartialEq)]
pub enum PhyPwdetExternalBinding {
    Mmio(PhyPwdetMmioBinding),
    Ready(PhyPwdetReadyBinding),
    Pbus(PhyPwdetPbusBinding),
    Timer(PhyPwdetTimerBinding),
}

impl PhyPwdetExternalBinding {
    pub fn lower(action: PhyPwdetAction) -> Result<Self, PhyPwdetBindingError> {
        if let Ok(binding) = PhyPwdetMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        if let Ok(binding) = PhyPwdetReadyBinding::new(action) {
            return Ok(Self::Ready(binding));
        }
        if let Ok(binding) = PhyPwdetPbusBinding::new(action) {
            return Ok(Self::Pbus(binding));
        }
        if let Ok(binding) = PhyPwdetTimerBinding::new(action) {
            return Ok(Self::Timer(binding));
        }
        Err(PhyPwdetBindingError::NotDirectMmio)
    }
}

#[cfg(test)]
mod tests;
