//! Event-driven ESP32-S31 crystal-duty search.
//!
//! Reference: `libphy.a[phy_rx_cal.o]::phy_xtal_duty_cal`.
//! The cold caller passes `debug = 0`, so both vendor
//! `phy_printf` branches are dead and are intentionally absent here.

use crate::{
    analog::i2c::PhyI2cAddress,
    analog::pbus::PhyPbusForceTest,
    analog::rfpll::{
        RfpllFrequencyAction, RfpllFrequencyCompletion, RfpllFrequencyFailure,
        RfpllFrequencyRequest, RfpllFrequencyTransition,
    },
    rx::dc_offset::{
        PhyRxDcoAction, PhyRxDcoCompletion, PhyRxDcoFailure, PhyRxDcoOutcome, PhyRxDcoRequest,
        PhyRxDcoTransition,
    },
    rx::signal_power::{
        PhySignalPowerAction, PhySignalPowerCompletion, PhySignalPowerFailure,
        PhySignalPowerRequest, PhySignalPowerTransition,
    },
};

const CANDIDATE_COUNT: u8 = 24;
const INITIAL_SAMPLE_COUNT: u8 = 4;
const SIGNAL_POWER_SHIFT: u8 = 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutySampleKind {
    Initial(u8),
    FirstReplacement(u8),
    SecondReplacement(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutySearchAction {
    WriteCandidate {
        address: PhyI2cAddress,
        candidate: u8,
    },
    DelayMicros {
        candidate: u8,
        micros: u32,
    },
    SignalPower(PhySignalPowerAction),
    Complete(XtalDutySearchOutcome),
    Failed(PhySignalPowerFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutySearchCompletion {
    CandidateWritten {
        address: PhyI2cAddress,
        candidate: u8,
    },
    DelayElapsed {
        candidate: u8,
    },
    SignalPower(PhySignalPowerCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutySearchTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XtalDutySearchOutcome {
    pub best_candidate: u8,
    pub best_filtered_power: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum XtalDutySearchStep {
    WriteCandidate {
        candidate: u8,
    },
    Delay {
        candidate: u8,
    },
    InitialSamples {
        candidate: u8,
        samples: [i64; INITIAL_SAMPLE_COUNT as usize],
        count: u8,
    },
    Review {
        candidate: u8,
        samples: [i64; INITIAL_SAMPLE_COUNT as usize],
        lower: i64,
        upper: i64,
        index: u8,
        filtered_sum: i64,
    },
    FirstReplacement {
        candidate: u8,
        samples: [i64; INITIAL_SAMPLE_COUNT as usize],
        lower: i64,
        upper: i64,
        index: u8,
        filtered_sum: i64,
    },
    SecondReplacement {
        candidate: u8,
        samples: [i64; INITIAL_SAMPLE_COUNT as usize],
        lower: i64,
        upper: i64,
        index: u8,
        filtered_sum: i64,
    },
    Complete(XtalDutySearchOutcome),
    Failed(PhySignalPowerFailure),
}

/// Fixed-size translation of the vendor crystal-duty candidate search.
///
/// The transition owns all samples. It can request at most six measurements
/// for each of the 24 candidates and cannot advance from `poll`: every
/// hardware measurement and 20-microsecond interval requires an explicit
/// external completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XtalDutySearchTransition {
    step: XtalDutySearchStep,
    signal_power: Option<PhySignalPowerTransition>,
    best_candidate: u8,
    best_filtered_power: i64,
    has_best: bool,
    last_candidate: u8,
}

impl XtalDutySearchTransition {
    const DUTY_ADDRESS: PhyI2cAddress = crate::analog::i2c::analog_registers::XTAL_DUTY_CANDIDATE;

    pub const fn new() -> Self {
        Self::from_initial_duty(44)
    }

    /// Search a bounded window around the measured crystal-duty seed.
    pub const fn from_initial_duty(initial: u8) -> Self {
        let first = if initial <= 11 {
            initial
        } else if initial <= 47 {
            initial - 12
        } else {
            initial - 24
        };
        Self {
            step: XtalDutySearchStep::WriteCandidate { candidate: first },
            signal_power: None,
            best_candidate: first,
            best_filtered_power: 0,
            has_best: false,
            last_candidate: first.wrapping_add(CANDIDATE_COUNT - 1),
        }
    }

    pub const fn action(self) -> XtalDutySearchAction {
        if let Some(transition) = self.signal_power {
            return XtalDutySearchAction::SignalPower(transition.action());
        }
        match self.step {
            XtalDutySearchStep::WriteCandidate { candidate } => {
                XtalDutySearchAction::WriteCandidate {
                    address: Self::DUTY_ADDRESS,
                    candidate,
                }
            }
            XtalDutySearchStep::Delay { candidate } => XtalDutySearchAction::DelayMicros {
                candidate,
                micros: 20,
            },
            XtalDutySearchStep::InitialSamples { .. }
            | XtalDutySearchStep::Review { .. }
            | XtalDutySearchStep::FirstReplacement { .. }
            | XtalDutySearchStep::SecondReplacement { .. } => {
                panic!()
            }
            XtalDutySearchStep::Complete(outcome) => XtalDutySearchAction::Complete(outcome),
            XtalDutySearchStep::Failed(failure) => XtalDutySearchAction::Failed(failure),
        }
    }

    const fn measurement_request(candidate: u8, kind: XtalDutySampleKind) -> PhySignalPowerRequest {
        let kind = match kind {
            XtalDutySampleKind::Initial(index) => index,
            XtalDutySampleKind::FirstReplacement(index) => 4 + index,
            XtalDutySampleKind::SecondReplacement(index) => 8 + index,
        };
        PhySignalPowerRequest {
            measurement: ((candidate as u16) << 4) | kind as u16,
            shift: SIGNAL_POWER_SHIFT,
        }
    }

    fn arm_signal_power(&mut self) {
        if self.signal_power.is_some() {
            return;
        }
        let request = match self.step {
            XtalDutySearchStep::InitialSamples {
                candidate, count, ..
            } => Self::measurement_request(candidate, XtalDutySampleKind::Initial(count)),
            XtalDutySearchStep::FirstReplacement {
                candidate, index, ..
            } => Self::measurement_request(candidate, XtalDutySampleKind::FirstReplacement(index)),
            XtalDutySearchStep::SecondReplacement {
                candidate, index, ..
            } => Self::measurement_request(candidate, XtalDutySampleKind::SecondReplacement(index)),
            _ => return,
        };
        self.signal_power = Some(PhySignalPowerTransition::new(request));
    }

    fn outlier(value: i64, lower: i64, upper: i64) -> bool {
        value < lower || value > upper
    }

    fn finish_candidate(&mut self, candidate: u8, filtered_sum: i64) {
        let filtered_power = filtered_sum / i64::from(INITIAL_SAMPLE_COUNT);
        if !self.has_best || filtered_power < self.best_filtered_power {
            self.has_best = true;
            self.best_candidate = candidate;
            self.best_filtered_power = filtered_power;
        }
        self.step = if candidate == self.last_candidate {
            XtalDutySearchStep::Complete(XtalDutySearchOutcome {
                best_candidate: self.best_candidate,
                best_filtered_power: self.best_filtered_power,
            })
        } else {
            XtalDutySearchStep::WriteCandidate {
                candidate: candidate.wrapping_add(1),
            }
        };
    }

    fn normalize_review(&mut self) {
        loop {
            let XtalDutySearchStep::Review {
                candidate,
                samples,
                lower,
                upper,
                index,
                filtered_sum,
            } = self.step
            else {
                return;
            };
            if index == INITIAL_SAMPLE_COUNT {
                self.finish_candidate(candidate, filtered_sum);
                return;
            }
            let sample = samples[index as usize];
            if Self::outlier(sample, lower, upper) {
                self.step = XtalDutySearchStep::FirstReplacement {
                    candidate,
                    samples,
                    lower,
                    upper,
                    index,
                    filtered_sum,
                };
                return;
            }
            self.step = XtalDutySearchStep::Review {
                candidate,
                samples,
                lower,
                upper,
                index: index + 1,
                filtered_sum: filtered_sum.wrapping_add(sample),
            };
        }
    }

    fn accept_signal_power(&mut self, value: i64) -> Result<(), XtalDutySearchTransitionError> {
        self.step = match self.step {
            XtalDutySearchStep::InitialSamples {
                candidate,
                mut samples,
                count,
            } => {
                samples[count as usize] = value;
                if count + 1 != INITIAL_SAMPLE_COUNT {
                    XtalDutySearchStep::InitialSamples {
                        candidate,
                        samples,
                        count: count + 1,
                    }
                } else {
                    let sum = samples
                        .into_iter()
                        .fold(0_i64, |sum, sample| sum.wrapping_add(sample));
                    let mean = sum / i64::from(INITIAL_SAMPLE_COUNT);
                    XtalDutySearchStep::Review {
                        candidate,
                        samples,
                        lower: mean.wrapping_mul(2) / 3,
                        upper: mean.wrapping_mul(3) / 2,
                        index: 0,
                        filtered_sum: 0,
                    }
                }
            }
            XtalDutySearchStep::FirstReplacement {
                candidate,
                samples,
                lower,
                upper,
                index,
                filtered_sum,
            } => {
                if Self::outlier(value, lower, upper) {
                    XtalDutySearchStep::SecondReplacement {
                        candidate,
                        samples,
                        lower,
                        upper,
                        index,
                        filtered_sum,
                    }
                } else {
                    XtalDutySearchStep::Review {
                        candidate,
                        samples,
                        lower,
                        upper,
                        index: index + 1,
                        filtered_sum: filtered_sum.wrapping_add(value),
                    }
                }
            }
            XtalDutySearchStep::SecondReplacement {
                candidate,
                samples,
                lower,
                upper,
                index,
                filtered_sum,
            } => XtalDutySearchStep::Review {
                candidate,
                samples,
                lower,
                upper,
                index: index + 1,
                filtered_sum: filtered_sum.wrapping_add(value),
            },
            _ => return Err(XtalDutySearchTransitionError::WrongCompletion),
        };
        self.normalize_review();
        self.arm_signal_power();
        Ok(())
    }

    pub fn advance(
        &mut self,
        completion: XtalDutySearchCompletion,
    ) -> Result<(), XtalDutySearchTransitionError> {
        if let Some(mut transition) = self.signal_power {
            let XtalDutySearchCompletion::SignalPower(completion) = completion else {
                return Err(XtalDutySearchTransitionError::WrongCompletion);
            };
            transition
                .advance(completion)
                .map_err(|_| XtalDutySearchTransitionError::WrongCompletion)?;
            return match transition.action() {
                PhySignalPowerAction::Complete(outcome) => {
                    self.signal_power = None;
                    self.accept_signal_power(outcome.value)
                }
                PhySignalPowerAction::Failed(failure) => {
                    self.signal_power = None;
                    self.step = XtalDutySearchStep::Failed(failure);
                    Ok(())
                }
                _ => {
                    self.signal_power = Some(transition);
                    Ok(())
                }
            };
        }
        self.step = match (self.step, completion) {
            (
                XtalDutySearchStep::WriteCandidate { candidate },
                XtalDutySearchCompletion::CandidateWritten {
                    address,
                    candidate: completed,
                },
            ) if address == Self::DUTY_ADDRESS && candidate == completed => {
                XtalDutySearchStep::Delay { candidate }
            }
            (
                XtalDutySearchStep::Delay { candidate },
                XtalDutySearchCompletion::DelayElapsed {
                    candidate: completed,
                },
            ) if candidate == completed => XtalDutySearchStep::InitialSamples {
                candidate,
                samples: [0; INITIAL_SAMPLE_COUNT as usize],
                count: 0,
            },
            (XtalDutySearchStep::Complete(_), _) | (XtalDutySearchStep::Failed(_), _) => {
                return Err(XtalDutySearchTransitionError::AlreadyComplete);
            }
            _ => return Err(XtalDutySearchTransitionError::WrongCompletion),
        };
        self.normalize_review();
        self.arm_signal_power();
        Ok(())
    }
}

impl Default for XtalDutySearchTransition {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XtalDutyCalibrationParameters {
    /// Middle-band duty restored after each measurement pass.
    pub restore_duty: u8,
    /// `phy_param[0x4f]`, passed to the RFPLL frequency calculation.
    pub rf_frequency_offset_base: u8,
    /// Byte two of the parameter image published through the rev0 ROM
    /// `phy_param_rom` pointer cell at `0x2f07_fc40`.
    ///
    /// ROM `phy_pbus_xpd_rx_on` forwards this value to PBus selector zero,
    /// path two. Its electrical meaning is not yet evidenced, so the name
    /// deliberately describes the observed use rather than guessing.
    pub pbus_rx_path_value: u8,
}

const fn prepare_pbus_transaction(index: u8, pbus_rx_path_value: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(4, 1, 0),
        1 => PhyPbusForceTest::new(4, 2, 1),
        2 => PhyPbusForceTest::new(5, 1, 0),
        3 => PhyPbusForceTest::new(0, 1, 0x40),
        4 => PhyPbusForceTest::new(0, 2, pbus_rx_path_value as u16),
        5 => PhyPbusForceTest::new(1, 1, 0x189),
        6 => PhyPbusForceTest::new(1, 2, 0xf0),
        7 => PhyPbusForceTest::new(0, 1, 0x43),
        8 => PhyPbusForceTest::new(1, 1, 0x38),
        _ => PhyPbusForceTest::new(1, 1, 0x189),
    }
}

const fn restore_pbus_transaction(index: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(0, 1, 0),
        1 => PhyPbusForceTest::new(1, 1, 0),
        _ => PhyPbusForceTest::new(1, 2, 0),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyHardwareFailure {
    Rfpll(RfpllFrequencyFailure),
    PbusForceTestTimedOut(PhyPbusForceTest),
    RxDco(PhyRxDcoFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyPrepareAction {
    /// Complete Rust-owned RFPLL I2C/timer transition.
    Rfpll(RfpllFrequencyAction),
    /// Complete Rust MMIO replacement for
    /// `phy_start_tx_tone_step_new(1, 0x80, 0, 0, 0, 0)`, including both
    /// former `g_phyFuns + 0x30` callback invocations.
    ConfigureCalibrationTone {
        enabled: bool,
        selector: u8,
        step: u8,
    },
    ConfigureRxClock {
        enabled: bool,
    },
    ConfigureTxClock {
        enabled: bool,
    },
    ConfigurePbusDebugMode,
    ForcePbus(PhyPbusForceTest),
    /// Retain the field in the route restore slot and clear it in the same serialized
    /// radio-owner operation.
    PrepareRxDcoControlRestore,
    /// Complete Rust-owned RX-DCO and IQ-estimator graph.
    RxDco(PhyRxDcoAction),
    RestoreRxDcoControl,
    Complete(PhyRxDcoOutcome),
    Failed(XtalDutyHardwareFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyPrepareCompletion {
    Rfpll(RfpllFrequencyCompletion),
    CalibrationToneConfigured {
        enabled: bool,
        selector: u8,
        step: u8,
    },
    RxClockConfigured {
        enabled: bool,
    },
    TxClockConfigured {
        enabled: bool,
    },
    PbusDebugModeConfigured,
    PbusForceCompleted(PhyPbusForceTest),
    #[cfg_attr(
        not(any(test, feature = "validation-probes")),
        allow(dead_code, reason = "only validation bindings construct this variant")
    )]
    PbusForceTimedOut(PhyPbusForceTest),
    RxDcoControlRestorePrepared,
    RxDco(PhyRxDcoCompletion),
    RxDcoControlRestored,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyPrepareTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum XtalDutyPrepareStep {
    Rfpll(RfpllFrequencyTransition),
    StartTone,
    EnableRxClock,
    EnableTxClock,
    PbusDebugMode,
    PbusForce(u8),
    PrepareRxDcoControlRestore,
    RxDco { transition: PhyRxDcoTransition },
    RestoreRxDcoControl { outcome: PhyRxDcoOutcome },
    RestoreRxDcoControlAfterFailure { failure: PhyRxDcoFailure },
    Complete(PhyRxDcoOutcome),
    Failed(XtalDutyHardwareFailure),
}

/// Exact finite preparation order before the crystal-duty candidate search.
///
/// PBus commands are individual externally completed operations. Nothing in
/// this transition retries a busy register, polls, delays, allocates, or
/// invokes a callback. RFPLL is a complete nested I2C/timer transition, tone
/// is a complete Rust MMIO leaf, and RX-DCO with its IQ estimator is a
/// complete nested register/timer transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XtalDutyPrepareTransition {
    parameter: XtalDutyCalibrationParameters,
    step: XtalDutyPrepareStep,
}

impl XtalDutyPrepareTransition {
    pub const fn new(frequency_code: u16, parameter: XtalDutyCalibrationParameters) -> Self {
        Self {
            parameter,
            step: XtalDutyPrepareStep::Rfpll(RfpllFrequencyTransition::new(
                RfpllFrequencyRequest {
                    crystal_selector: parameter.rf_frequency_offset_base,
                    frequency_code: frequency_code.wrapping_sub(5),
                    offset: 0,
                },
            )),
        }
    }

    pub const fn action(self) -> XtalDutyPrepareAction {
        match self.step {
            XtalDutyPrepareStep::Rfpll(transition) => {
                XtalDutyPrepareAction::Rfpll(transition.action())
            }
            XtalDutyPrepareStep::StartTone => XtalDutyPrepareAction::ConfigureCalibrationTone {
                enabled: true,
                selector: 0x80,
                step: 0,
            },
            XtalDutyPrepareStep::EnableRxClock => {
                XtalDutyPrepareAction::ConfigureRxClock { enabled: true }
            }
            XtalDutyPrepareStep::EnableTxClock => {
                XtalDutyPrepareAction::ConfigureTxClock { enabled: true }
            }
            XtalDutyPrepareStep::PbusDebugMode => XtalDutyPrepareAction::ConfigurePbusDebugMode,
            XtalDutyPrepareStep::PbusForce(index) => XtalDutyPrepareAction::ForcePbus(
                prepare_pbus_transaction(index, self.parameter.pbus_rx_path_value),
            ),
            XtalDutyPrepareStep::PrepareRxDcoControlRestore => {
                XtalDutyPrepareAction::PrepareRxDcoControlRestore
            }
            XtalDutyPrepareStep::RxDco { transition, .. } => {
                XtalDutyPrepareAction::RxDco(transition.action())
            }
            XtalDutyPrepareStep::RestoreRxDcoControl { .. }
            | XtalDutyPrepareStep::RestoreRxDcoControlAfterFailure { .. } => {
                XtalDutyPrepareAction::RestoreRxDcoControl
            }
            XtalDutyPrepareStep::Complete(outcome) => XtalDutyPrepareAction::Complete(outcome),
            XtalDutyPrepareStep::Failed(failure) => XtalDutyPrepareAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: XtalDutyPrepareCompletion,
    ) -> Result<(), XtalDutyPrepareTransitionError> {
        self.step = match (self.step, completion) {
            (
                XtalDutyPrepareStep::Rfpll(mut transition),
                XtalDutyPrepareCompletion::Rfpll(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| XtalDutyPrepareTransitionError::WrongCompletion)?;
                match transition.action() {
                    RfpllFrequencyAction::Complete(_) => XtalDutyPrepareStep::StartTone,
                    RfpllFrequencyAction::Failed(failure) => {
                        XtalDutyPrepareStep::Failed(XtalDutyHardwareFailure::Rfpll(failure))
                    }
                    _ => XtalDutyPrepareStep::Rfpll(transition),
                }
            }
            (
                XtalDutyPrepareStep::StartTone,
                XtalDutyPrepareCompletion::CalibrationToneConfigured {
                    enabled: true,
                    selector: 0x80,
                    step: 0,
                },
            ) => XtalDutyPrepareStep::EnableRxClock,
            (
                XtalDutyPrepareStep::EnableRxClock,
                XtalDutyPrepareCompletion::RxClockConfigured { enabled: true },
            ) => XtalDutyPrepareStep::EnableTxClock,
            (
                XtalDutyPrepareStep::EnableTxClock,
                XtalDutyPrepareCompletion::TxClockConfigured { enabled: true },
            ) => XtalDutyPrepareStep::PbusDebugMode,
            (
                XtalDutyPrepareStep::PbusDebugMode,
                XtalDutyPrepareCompletion::PbusDebugModeConfigured,
            ) => XtalDutyPrepareStep::PbusForce(0),
            (
                XtalDutyPrepareStep::PbusForce(index),
                XtalDutyPrepareCompletion::PbusForceCompleted(transaction),
            ) if transaction
                == prepare_pbus_transaction(index, self.parameter.pbus_rx_path_value) =>
            {
                if index == 9 {
                    XtalDutyPrepareStep::PrepareRxDcoControlRestore
                } else {
                    XtalDutyPrepareStep::PbusForce(index + 1)
                }
            }
            (
                XtalDutyPrepareStep::PbusForce(index),
                XtalDutyPrepareCompletion::PbusForceTimedOut(transaction),
            ) if transaction
                == prepare_pbus_transaction(index, self.parameter.pbus_rx_path_value) =>
            {
                XtalDutyPrepareStep::Failed(XtalDutyHardwareFailure::PbusForceTestTimedOut(
                    transaction,
                ))
            }
            (
                XtalDutyPrepareStep::PrepareRxDcoControlRestore,
                XtalDutyPrepareCompletion::RxDcoControlRestorePrepared,
            ) => XtalDutyPrepareStep::RxDco {
                transition: PhyRxDcoTransition::new(PhyRxDcoRequest::XTAL_DUTY),
            },
            (
                XtalDutyPrepareStep::RxDco { mut transition },
                XtalDutyPrepareCompletion::RxDco(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| XtalDutyPrepareTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyRxDcoAction::Complete(outcome) => {
                        XtalDutyPrepareStep::RestoreRxDcoControl { outcome }
                    }
                    PhyRxDcoAction::Failed(failure) => {
                        XtalDutyPrepareStep::RestoreRxDcoControlAfterFailure { failure }
                    }
                    _ => XtalDutyPrepareStep::RxDco { transition },
                }
            }
            (
                XtalDutyPrepareStep::RestoreRxDcoControl { outcome },
                XtalDutyPrepareCompletion::RxDcoControlRestored,
            ) => XtalDutyPrepareStep::Complete(outcome),
            (
                XtalDutyPrepareStep::RestoreRxDcoControlAfterFailure { failure },
                XtalDutyPrepareCompletion::RxDcoControlRestored,
            ) => XtalDutyPrepareStep::Failed(XtalDutyHardwareFailure::RxDco(failure)),
            (XtalDutyPrepareStep::Complete(_), _) | (XtalDutyPrepareStep::Failed(_), _) => {
                return Err(XtalDutyPrepareTransitionError::AlreadyComplete);
            }
            _ => return Err(XtalDutyPrepareTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyRestoreAction {
    /// Complete Rust MMIO replacement for
    /// `phy_start_tx_tone_step_new(0, 0x80, 0x28, 0, 0, 0)`, including both
    /// former `g_phyFuns + 0x30` callback invocations.
    ConfigureCalibrationTone {
        enabled: bool,
        selector: u8,
        step: u8,
    },
    ConfigureRxClock {
        enabled: bool,
    },
    ConfigureTxClock {
        enabled: bool,
    },
    ForcePbus(PhyPbusForceTest),
    ConfigurePbusWorkMode,
    DelayMicros(u32),
    ConfigurePbusWorkModePulse,
    ClearPbusWorkModePulse,
    Complete,
    Failed(XtalDutyHardwareFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyRestoreCompletion {
    CalibrationToneConfigured {
        enabled: bool,
        selector: u8,
        step: u8,
    },
    RxClockConfigured {
        enabled: bool,
    },
    TxClockConfigured {
        enabled: bool,
    },
    PbusForceCompleted(PhyPbusForceTest),
    #[cfg_attr(
        not(any(test, feature = "validation-probes")),
        allow(dead_code, reason = "only validation bindings construct this variant")
    )]
    PbusForceTimedOut(PhyPbusForceTest),
    PbusWorkModeConfigured {
        settle_required: bool,
    },
    DelayElapsed {
        micros: u32,
    },
    PbusWorkModePulseConfigured,
    PbusWorkModePulseCleared,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyRestoreTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum XtalDutyRestoreStep {
    StopTone,
    DisableRxClock,
    DisableTxClock,
    PbusForce(u8),
    WorkMode,
    SettleDelay,
    WorkModePulse,
    PulseDelay,
    ClearWorkModePulse,
    Complete,
    Failed(XtalDutyHardwareFailure),
}

/// Exact finite restoration tail of one crystal-duty pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XtalDutyRestoreTransition {
    step: XtalDutyRestoreStep,
}

impl XtalDutyRestoreTransition {
    pub const fn new() -> Self {
        Self {
            step: XtalDutyRestoreStep::StopTone,
        }
    }

    pub const fn action(self) -> XtalDutyRestoreAction {
        match self.step {
            XtalDutyRestoreStep::StopTone => XtalDutyRestoreAction::ConfigureCalibrationTone {
                enabled: false,
                selector: 0x80,
                step: 0x28,
            },
            XtalDutyRestoreStep::DisableRxClock => {
                XtalDutyRestoreAction::ConfigureRxClock { enabled: false }
            }
            XtalDutyRestoreStep::DisableTxClock => {
                XtalDutyRestoreAction::ConfigureTxClock { enabled: false }
            }
            XtalDutyRestoreStep::PbusForce(index) => {
                XtalDutyRestoreAction::ForcePbus(restore_pbus_transaction(index))
            }
            XtalDutyRestoreStep::WorkMode => XtalDutyRestoreAction::ConfigurePbusWorkMode,
            XtalDutyRestoreStep::SettleDelay => XtalDutyRestoreAction::DelayMicros(1),
            XtalDutyRestoreStep::WorkModePulse => XtalDutyRestoreAction::ConfigurePbusWorkModePulse,
            XtalDutyRestoreStep::PulseDelay => XtalDutyRestoreAction::DelayMicros(2),
            XtalDutyRestoreStep::ClearWorkModePulse => {
                XtalDutyRestoreAction::ClearPbusWorkModePulse
            }
            XtalDutyRestoreStep::Complete => XtalDutyRestoreAction::Complete,
            XtalDutyRestoreStep::Failed(failure) => XtalDutyRestoreAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: XtalDutyRestoreCompletion,
    ) -> Result<(), XtalDutyRestoreTransitionError> {
        self.step = match (self.step, completion) {
            (
                XtalDutyRestoreStep::StopTone,
                XtalDutyRestoreCompletion::CalibrationToneConfigured {
                    enabled: false,
                    selector: 0x80,
                    step: 0x28,
                },
            ) => XtalDutyRestoreStep::DisableRxClock,
            (
                XtalDutyRestoreStep::DisableRxClock,
                XtalDutyRestoreCompletion::RxClockConfigured { enabled: false },
            ) => XtalDutyRestoreStep::DisableTxClock,
            (
                XtalDutyRestoreStep::DisableTxClock,
                XtalDutyRestoreCompletion::TxClockConfigured { enabled: false },
            ) => XtalDutyRestoreStep::PbusForce(0),
            (
                XtalDutyRestoreStep::PbusForce(index),
                XtalDutyRestoreCompletion::PbusForceCompleted(transaction),
            ) if transaction == restore_pbus_transaction(index) => {
                if index == 2 {
                    XtalDutyRestoreStep::WorkMode
                } else {
                    XtalDutyRestoreStep::PbusForce(index + 1)
                }
            }
            (
                XtalDutyRestoreStep::PbusForce(index),
                XtalDutyRestoreCompletion::PbusForceTimedOut(transaction),
            ) if transaction == restore_pbus_transaction(index) => XtalDutyRestoreStep::Failed(
                XtalDutyHardwareFailure::PbusForceTestTimedOut(transaction),
            ),
            (
                XtalDutyRestoreStep::WorkMode,
                XtalDutyRestoreCompletion::PbusWorkModeConfigured {
                    settle_required: false,
                },
            ) => XtalDutyRestoreStep::Complete,
            (
                XtalDutyRestoreStep::WorkMode,
                XtalDutyRestoreCompletion::PbusWorkModeConfigured {
                    settle_required: true,
                },
            ) => XtalDutyRestoreStep::SettleDelay,
            (
                XtalDutyRestoreStep::SettleDelay,
                XtalDutyRestoreCompletion::DelayElapsed { micros: 1 },
            ) => XtalDutyRestoreStep::WorkModePulse,
            (
                XtalDutyRestoreStep::WorkModePulse,
                XtalDutyRestoreCompletion::PbusWorkModePulseConfigured,
            ) => XtalDutyRestoreStep::PulseDelay,
            (
                XtalDutyRestoreStep::PulseDelay,
                XtalDutyRestoreCompletion::DelayElapsed { micros: 2 },
            ) => XtalDutyRestoreStep::ClearWorkModePulse,
            (
                XtalDutyRestoreStep::ClearWorkModePulse,
                XtalDutyRestoreCompletion::PbusWorkModePulseCleared,
            ) => XtalDutyRestoreStep::Complete,
            (XtalDutyRestoreStep::Complete, _) | (XtalDutyRestoreStep::Failed(_), _) => {
                return Err(XtalDutyRestoreTransitionError::AlreadyComplete);
            }
            _ => return Err(XtalDutyRestoreTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

impl Default for XtalDutyRestoreTransition {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XtalDutyPassOutcome {
    pub frequency_code: u16,
    pub best_candidate: u8,
    pub best_filtered_power: i64,
}

/// Terminal child failure, retaining the calibration phase that owns it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyFailure {
    Prepare(XtalDutyHardwareFailure),
    Search(PhySignalPowerFailure),
    Restore(XtalDutyHardwareFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyPassAction {
    WriteMasked {
        field: crate::analog::i2c::PhyI2cField,
        value: u8,
    },
    WriteByte {
        address: PhyI2cAddress,
        value: u8,
    },
    Prepare(XtalDutyPrepareAction),
    Search(XtalDutySearchAction),
    Restore(XtalDutyRestoreAction),
    Complete(XtalDutyPassOutcome),
    Failed(XtalDutyFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyPassCompletion {
    MaskedWrite {
        field: crate::analog::i2c::PhyI2cField,
    },
    ByteWrite {
        address: PhyI2cAddress,
    },
    Prepare(XtalDutyPrepareCompletion),
    Search(XtalDutySearchCompletion),
    Restore(XtalDutyRestoreCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyPassTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum XtalDutyPassStep {
    DisablePath,
    WriteInitialDuty,
    Prepare(XtalDutyPrepareTransition),
    Search(XtalDutySearchTransition),
    RestoreSavedDuty(XtalDutySearchOutcome),
    Restore {
        transition: XtalDutyRestoreTransition,
        search: XtalDutySearchOutcome,
    },
    Complete(XtalDutyPassOutcome),
    Failed(XtalDutyFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XtalDutyPassTransition {
    frequency_code: u16,
    initial_duty: u8,
    parameter: XtalDutyCalibrationParameters,
    step: XtalDutyPassStep,
}

impl XtalDutyPassTransition {
    const DUTY_ADDRESS: PhyI2cAddress = crate::analog::i2c::analog_registers::XTAL_DUTY_CANDIDATE;

    pub const fn new(
        frequency_code: u16,
        initial_duty: u8,
        parameter: XtalDutyCalibrationParameters,
    ) -> Self {
        Self {
            frequency_code,
            initial_duty,
            parameter,
            step: XtalDutyPassStep::DisablePath,
        }
    }

    pub const fn action(self) -> XtalDutyPassAction {
        match self.step {
            XtalDutyPassStep::DisablePath => XtalDutyPassAction::WriteMasked {
                field: crate::analog::i2c::analog_registers::XTAL_DUTY_CALIBRATION_PATH_ENABLE,
                value: 0,
            },
            XtalDutyPassStep::WriteInitialDuty => XtalDutyPassAction::WriteByte {
                address: Self::DUTY_ADDRESS,
                value: self.initial_duty,
            },
            XtalDutyPassStep::Prepare(transition) => {
                XtalDutyPassAction::Prepare(transition.action())
            }
            XtalDutyPassStep::Search(transition) => XtalDutyPassAction::Search(transition.action()),
            XtalDutyPassStep::RestoreSavedDuty(_) => XtalDutyPassAction::WriteByte {
                address: Self::DUTY_ADDRESS,
                value: self.parameter.restore_duty,
            },
            XtalDutyPassStep::Restore { transition, .. } => {
                XtalDutyPassAction::Restore(transition.action())
            }
            XtalDutyPassStep::Complete(outcome) => XtalDutyPassAction::Complete(outcome),
            XtalDutyPassStep::Failed(failure) => XtalDutyPassAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: XtalDutyPassCompletion,
    ) -> Result<(), XtalDutyPassTransitionError> {
        self.step = match (self.step, completion) {
            (
                XtalDutyPassStep::DisablePath,
                XtalDutyPassCompletion::MaskedWrite {
                    field: crate::analog::i2c::analog_registers::XTAL_DUTY_CALIBRATION_PATH_ENABLE,
                },
            ) => XtalDutyPassStep::WriteInitialDuty,
            (XtalDutyPassStep::WriteInitialDuty, XtalDutyPassCompletion::ByteWrite { address })
                if address == Self::DUTY_ADDRESS =>
            {
                XtalDutyPassStep::Prepare(XtalDutyPrepareTransition::new(
                    self.frequency_code,
                    self.parameter,
                ))
            }
            (
                XtalDutyPassStep::Prepare(mut transition),
                XtalDutyPassCompletion::Prepare(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| XtalDutyPassTransitionError::WrongCompletion)?;
                match transition.action() {
                    XtalDutyPrepareAction::Complete(_) => XtalDutyPassStep::Search(
                        XtalDutySearchTransition::from_initial_duty(self.initial_duty),
                    ),
                    XtalDutyPrepareAction::Failed(failure) => {
                        XtalDutyPassStep::Failed(XtalDutyFailure::Prepare(failure))
                    }
                    _ => XtalDutyPassStep::Prepare(transition),
                }
            }
            (
                XtalDutyPassStep::Search(mut transition),
                XtalDutyPassCompletion::Search(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| XtalDutyPassTransitionError::WrongCompletion)?;
                match transition.action() {
                    XtalDutySearchAction::Complete(outcome) => {
                        XtalDutyPassStep::RestoreSavedDuty(outcome)
                    }
                    XtalDutySearchAction::Failed(failure) => {
                        XtalDutyPassStep::Failed(XtalDutyFailure::Search(failure))
                    }
                    _ => XtalDutyPassStep::Search(transition),
                }
            }
            (
                XtalDutyPassStep::RestoreSavedDuty(outcome),
                XtalDutyPassCompletion::ByteWrite { address },
            ) if address == Self::DUTY_ADDRESS => XtalDutyPassStep::Restore {
                transition: XtalDutyRestoreTransition::new(),
                search: outcome,
            },
            (
                XtalDutyPassStep::Restore {
                    mut transition,
                    search,
                },
                XtalDutyPassCompletion::Restore(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| XtalDutyPassTransitionError::WrongCompletion)?;
                match transition.action() {
                    XtalDutyRestoreAction::Complete => {
                        XtalDutyPassStep::Complete(XtalDutyPassOutcome {
                            frequency_code: self.frequency_code,
                            best_candidate: search.best_candidate,
                            best_filtered_power: search.best_filtered_power,
                        })
                    }
                    XtalDutyRestoreAction::Failed(failure) => {
                        XtalDutyPassStep::Failed(XtalDutyFailure::Restore(failure))
                    }
                    _ => XtalDutyPassStep::Restore { transition, search },
                }
            }
            (XtalDutyPassStep::Complete(_) | XtalDutyPassStep::Failed(_), _) => {
                return Err(XtalDutyPassTransitionError::AlreadyComplete);
            }
            _ => return Err(XtalDutyPassTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XtalDutyCalibrationOutcome {
    pub initial_duty: u8,
    pub low_frequency: XtalDutyPassOutcome,
    pub high_frequency: XtalDutyPassOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyCalibrationAction {
    ReadInitialDuty {
        field: crate::analog::i2c::PhyI2cField,
    },
    DisableCalibrationPath {
        field: crate::analog::i2c::PhyI2cField,
        value: u8,
    },
    Pass(XtalDutyPassAction),
    Complete(XtalDutyCalibrationOutcome),
    Failed(XtalDutyFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyCalibrationCompletion {
    InitialDutyRead {
        field: crate::analog::i2c::PhyI2cField,
        value: u8,
    },
    CalibrationPathDisabled {
        field: crate::analog::i2c::PhyI2cField,
    },
    Pass(XtalDutyPassCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyCalibrationTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum XtalDutyCalibrationStep {
    ReadInitialDuty,
    DisableCalibrationPath {
        initial_duty: u8,
    },
    LowFrequencyPass(XtalDutyPassTransition),
    HighFrequencyPass {
        transition: XtalDutyPassTransition,
        low_frequency: XtalDutyPassOutcome,
        initial_duty: u8,
    },
    Complete(XtalDutyCalibrationOutcome),
    Failed(XtalDutyFailure),
}

/// Complete wrapper order for pinned `phy_xtal_duty_cal_init(0)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XtalDutyCalibrationTransition {
    parameter: XtalDutyCalibrationParameters,
    step: XtalDutyCalibrationStep,
}

impl XtalDutyCalibrationTransition {
    pub const fn new(parameter: XtalDutyCalibrationParameters) -> Self {
        Self {
            parameter,
            step: XtalDutyCalibrationStep::ReadInitialDuty,
        }
    }

    pub const fn action(self) -> XtalDutyCalibrationAction {
        match self.step {
            XtalDutyCalibrationStep::ReadInitialDuty => {
                XtalDutyCalibrationAction::ReadInitialDuty {
                    field: crate::analog::i2c::analog_registers::XTAL_DUTY_INITIAL,
                }
            }
            XtalDutyCalibrationStep::DisableCalibrationPath { .. } => {
                XtalDutyCalibrationAction::DisableCalibrationPath {
                    field: crate::analog::i2c::analog_registers::XTAL_DUTY_CALIBRATION_PATH_ENABLE,
                    value: 0,
                }
            }
            XtalDutyCalibrationStep::LowFrequencyPass(transition)
            | XtalDutyCalibrationStep::HighFrequencyPass { transition, .. } => {
                XtalDutyCalibrationAction::Pass(transition.action())
            }
            XtalDutyCalibrationStep::Complete(outcome) => {
                XtalDutyCalibrationAction::Complete(outcome)
            }
            XtalDutyCalibrationStep::Failed(failure) => XtalDutyCalibrationAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: XtalDutyCalibrationCompletion,
    ) -> Result<(), XtalDutyCalibrationTransitionError> {
        self.step = match (self.step, completion) {
            (
                XtalDutyCalibrationStep::ReadInitialDuty,
                XtalDutyCalibrationCompletion::InitialDutyRead {
                    field: crate::analog::i2c::analog_registers::XTAL_DUTY_INITIAL,
                    value,
                },
            ) => XtalDutyCalibrationStep::DisableCalibrationPath {
                initial_duty: value,
            },
            (
                XtalDutyCalibrationStep::DisableCalibrationPath { initial_duty },
                XtalDutyCalibrationCompletion::CalibrationPathDisabled {
                    field: crate::analog::i2c::analog_registers::XTAL_DUTY_CALIBRATION_PATH_ENABLE,
                },
            ) => XtalDutyCalibrationStep::LowFrequencyPass(XtalDutyPassTransition::new(
                0x988,
                initial_duty,
                self.parameter,
            )),
            (
                XtalDutyCalibrationStep::LowFrequencyPass(mut transition),
                XtalDutyCalibrationCompletion::Pass(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| XtalDutyCalibrationTransitionError::WrongCompletion)?;
                match transition.action() {
                    XtalDutyPassAction::Complete(low_frequency) => {
                        XtalDutyCalibrationStep::HighFrequencyPass {
                            transition: XtalDutyPassTransition::new(
                                0x9b0,
                                transition.initial_duty,
                                XtalDutyCalibrationParameters {
                                    restore_duty: low_frequency.best_candidate,
                                    ..self.parameter
                                },
                            ),
                            low_frequency,
                            initial_duty: transition.initial_duty,
                        }
                    }
                    XtalDutyPassAction::Failed(failure) => XtalDutyCalibrationStep::Failed(failure),
                    _ => XtalDutyCalibrationStep::LowFrequencyPass(transition),
                }
            }
            (
                XtalDutyCalibrationStep::HighFrequencyPass {
                    mut transition,
                    low_frequency,
                    initial_duty,
                },
                XtalDutyCalibrationCompletion::Pass(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| XtalDutyCalibrationTransitionError::WrongCompletion)?;
                match transition.action() {
                    XtalDutyPassAction::Complete(high_frequency) => {
                        XtalDutyCalibrationStep::Complete(XtalDutyCalibrationOutcome {
                            initial_duty,
                            low_frequency,
                            high_frequency,
                        })
                    }
                    XtalDutyPassAction::Failed(failure) => XtalDutyCalibrationStep::Failed(failure),
                    _ => XtalDutyCalibrationStep::HighFrequencyPass {
                        transition,
                        low_frequency,
                        initial_duty,
                    },
                }
            }
            (XtalDutyCalibrationStep::Complete(_) | XtalDutyCalibrationStep::Failed(_), _) => {
                return Err(XtalDutyCalibrationTransitionError::AlreadyComplete);
            }
            _ => return Err(XtalDutyCalibrationTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests;
