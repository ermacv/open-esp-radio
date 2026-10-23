//! One retained RX program; hardware leaves borrow this owner directly.
use super::{
    dc_offset::{
        PhyRxDcMinimumDecision, PhyRxDcMinimumFailure, PhyRxDcMinimumPolicyState,
        PhyRxDcMinimumRequest,
    },
    gain::{
        PhyRxGainInitFailure, PhyRxGainInitOutcome, PhyRxGainInitParameters,
        PhyRxGainPublishFailure, generated_table,
    },
    gain_calibration::{
        self as dc, PhyRxDcCalibrationFailure, PhyRxDcCalibrationOutcome,
        PhyRxDcCalibrationPolicyState, PhyRxDcCalibrationRequest, PhyRxDcCalibrationStage,
        PhyRxGainDcBank, PhyRxGainDcFailure, PhyRxGainDcOutcome, PhyRxGainDcParameters,
        PhyRxGainDcQuality,
    },
};
use crate::{
    analog::{
        i2c::{self, MaskedI2cWriteBinding},
        pbus::PhyPbusForceTest,
        rfpll::{
            RfpllFrequencyAction, RfpllFrequencyCompletion, RfpllFrequencyRequest,
            RfpllFrequencyTransition,
        },
    },
    calibration::{
        baseband::{
            PhyGainMemoryEntry, PhyRxGainBank, PhyRxGainMemoryParameters,
            phy_generated_rx_gain_memory_entry,
        },
        cold::{PhyColdI2cAction, PhyColdI2cTransaction},
        estimator::{
            PhyDcIqAction, PhyDcIqCompletion, PhyDcIqEstimateRequest, PhyDcIqEstimateTransition,
        },
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RxError {
    WrongCompletion,
    Hardware,
    TimedOut,
    Capability,
    OperationLimit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Pc {
    // Control phase. Keep these first: phase() uses the named endpoints.
    RootPrepare,
    RootRestore,
    Limits,
    IqCorrection,

    // Table publication. The final control tail uses the first group.
    PublishDebug,
    PublishRxOn,
    PublishRxClockOn,
    PublishTxClockOn,
    Entry,
    PublishRxClockOff,
    PublishTxClockOff,
    PublishRestoreRxOn,
    PublishWorkMode,

    // Calibration control commands.
    RegistersOn,
    RegistersOff,
    Debug,
    RxClockOn,
    TxClockOn,
    RxClockOff,
    TxClockOff,
    WorkMode,
    CalPrepare,
    CalRestore,

    // Calibration PBus commands.
    RxOn,
    SharedForce,
    RadioI,
    RadioQ,
    BasebandI,
    BasebandQ,
    Gain,
    SharedMixer,
    FineCode,
    CalI,
    CalQ,
    CalCleanupI,
    CalCleanupQ,
    CalLevel,
    RxOff,

    // Calibration reads, waits and retained bus transactions.
    Frequency,
    SharedRead,
    SharedI2c,
    CalRead,
    CalSettle,
    Estimate,
    SharedI2cRestore,

    // No observation phase remains once terminal.
    Complete,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RxPhase {
    Control,
    Dc,
    Publish,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AfterDc {
    Wifi,
    Complete,
    Failed,
}

#[derive(Debug, Eq, PartialEq)]
struct Cursor {
    pc: Pc,
    bank: PhyRxGainDcBank,
    gain: u8,
    index: u8,
    fine: Option<u8>,
    high: bool,
    after_dc: AfterDc,
}

#[derive(Debug, Eq, PartialEq)]
struct Calibration {
    request: PhyRxDcCalibrationRequest,
    policy: PhyRxDcCalibrationPolicyState,
    population: u8,
    threshold: i32,
    terminal: Option<Result<PhyRxDcCalibrationOutcome, PhyRxDcCalibrationFailure>>,
    minimum_request: PhyRxDcMinimumRequest,
    minimum: PhyRxDcMinimumPolicyState,
    estimator: PhyDcIqEstimateTransition,
}

// Only the currently active search owns scratch. There is no nested action,
// binding or completion transport around these arithmetic/frequency policies.
#[derive(Debug, Eq, PartialEq)]
enum Work {
    Idle,
    Frequency(RfpllFrequencyTransition),
    Field(oer_espressif_analog::FieldWriteTransition<i2c::Address>),
    Calibration(Calibration),
}

/// Logical obligations only. Physical saved registers stay with the HAL owner.
#[derive(Debug, Default, Eq, PartialEq)]
struct RestoreObligation {
    root: bool,
    calibration: bool,
    registers: bool,
    bus: bool,
}
/// Complete RX calibration and gain publication with one retained program PC.
/// Only the enclosing physical owner may run it or publish its terminal result.
///
/// ```compile_fail
/// use oer_esp32s31_phy::rx::gain::RxProgram;
/// let program = RxProgram::with_initialized_tables();
/// let moved = program;
/// let _ = program.terminal();
/// ```
#[derive(Debug, Eq, PartialEq)]
pub struct RxProgram {
    inputs: PhyRxGainDcParameters,
    dc_required: bool,
    generated_tables: bool,
    dc_complete: bool,
    memory: PhyRxGainMemoryParameters,
    quality: PhyRxGainDcQuality,
    fine_current: [u16; 2],
    shared_force: u16,
    cursor: Cursor,
    work: Work,
    bus: Option<MaskedI2cWriteBinding>,
    restore: RestoreObligation,
    failure: Option<PhyRxGainInitFailure>,
    fault: Option<RxError>,
    #[cfg(feature = "radio-health")]
    health_frequency: Option<u16>,
    #[cfg(feature = "radio-health")]
    health_correction: Option<crate::tracking::observation::health::Correction>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ControlCommand {
    PrepareRoot,
    RestoreRoot,
    PrepareCalibration,
    RestoreCalibration,
    Registers(bool),
    Debug,
    RxClock(bool),
    TxClock(bool),
    WorkMode,
    Limits(u8),
    IqCorrection,
}

pub(crate) enum RxStep<'a> {
    Pbus(PbusStep<'a>),
    ReadPbus(ReadPbusStep<'a>),
    I2c(I2cStep<'a>),
    Frequency(FrequencyStep<'a>),
    Estimator(EstimatorStep<'a>),
    GainEntry(GainEntryStep<'a>),
    Control(ControlStep<'a>),
    Settle(SettleStep<'a>),
    Complete,
}
pub(crate) struct PbusStep<'a> {
    program: &'a mut RxProgram,
    command: PhyPbusForceTest,
}
pub(crate) struct ReadPbusStep<'a> {
    program: &'a mut RxProgram,
    selector: u8,
    path: u8,
}
pub(crate) struct I2cStep<'a> {
    program: &'a mut RxProgram,
}
pub(crate) struct FrequencyStep<'a> {
    program: &'a mut RxProgram,
    command: RfpllFrequencyAction,
}
pub(crate) struct EstimatorStep<'a> {
    program: &'a mut RxProgram,
    command: PhyDcIqAction,
}
pub(crate) struct GainEntryStep<'a> {
    program: &'a mut RxProgram,
}
pub(crate) struct ControlStep<'a> {
    program: &'a mut RxProgram,
    command: ControlCommand,
}
pub(crate) struct SettleStep<'a> {
    program: &'a mut RxProgram,
    micros: u32,
}

impl RxProgram {
    pub fn new(parameters: PhyRxGainInitParameters) -> Self {
        let initialized = parameters.tables_initialized;
        let dc_required = !parameters.dc_calibrated && !initialized;
        Self {
            inputs: parameters.dc,
            dc_required,
            generated_tables: !initialized,
            dc_complete: false,
            memory: parameters.memory,
            quality: PhyRxGainDcQuality::EMPTY,
            fine_current: [0x100; 2],
            shared_force: 0,
            cursor: Cursor {
                pc: if initialized {
                    Pc::Limits
                } else if dc_required {
                    Pc::RootPrepare
                } else {
                    Pc::PublishDebug
                },
                bank: if dc_required {
                    PhyRxGainDcBank::Shared
                } else {
                    PhyRxGainDcBank::Wifi
                },
                gain: 0,
                index: 0,
                fine: None,
                high: false,
                after_dc: AfterDc::Complete,
            },
            work: Work::Idle,
            bus: None,
            restore: RestoreObligation::default(),
            failure: None,
            fault: None,
            #[cfg(feature = "radio-health")]
            health_frequency: None,
            #[cfg(feature = "radio-health")]
            health_correction: None,
        }
    }

    pub fn with_initialized_tables() -> Self {
        Self::new(PhyRxGainInitParameters {
            dc_calibrated: true,
            tables_initialized: true,
            dc: PhyRxGainDcParameters {
                crystal_selector: 0,
                pbus_rx_path_value: 0,
                rx_saturation_detected: false,
            },
            memory: PhyRxGainMemoryParameters {
                parameter_002: 0,
                wifi_index_dc: [[0; 2]; 8],
                wifi_fine_dc: [[0; 2]; dc::FINE_CODES],
                shared_index_dc: [[0; 2]; 11],
                wifi_auxiliary: 0,
            },
        })
    }

    pub fn terminal(&self) -> Option<Result<PhyRxGainInitOutcome, PhyRxGainInitFailure>> {
        match self.cursor.pc {
            Pc::Complete => Some(Ok(PhyRxGainInitOutcome {
                dc: if self.dc_complete {
                    Some(PhyRxGainDcOutcome {
                        quality: self.quality,
                        wifi_index_dc: self.memory.wifi_index_dc,
                        wifi_fine_dc: self.memory.wifi_fine_dc,
                        shared_index_dc: self.memory.shared_index_dc,
                    })
                } else {
                    None
                },
                generated_tables: self.generated_tables,
                wifi_last_index: generated_table(PhyRxGainBank::Wifi).last_index,
                shared_last_index: generated_table(PhyRxGainBank::Shared).last_index,
            })),
            Pc::Failed => self.failure.map(Err),
            _ => None,
        }
    }

    // Grouping the PC makes phase classification small enough to inline;
    // the action-tag switch below remains shared across poll instantiations.
    #[inline(always)]
    pub(crate) fn phase(&self) -> Option<RxPhase> {
        // The private PC groups keep this classification to range checks;
        // inlining a switch here duplicates jump tables in every poll caller.
        let pc = self.cursor.pc as u8;
        if pc <= Pc::IqCorrection as u8 {
            Some(RxPhase::Control)
        } else if pc <= Pc::PublishWorkMode as u8 {
            Some(RxPhase::Publish)
        } else if matches!(self.cursor.pc, Pc::Complete | Pc::Failed) {
            None
        } else {
            Some(RxPhase::Dc)
        }
    }

    pub(crate) fn quality(&self) -> Option<PhyRxGainDcQuality> {
        (self.dc_complete && self.cursor.pc == Pc::Complete).then_some(self.quality)
    }

    #[inline(never)]
    pub(crate) fn action_tag(&self) -> u8 {
        match self.cursor.pc {
            Pc::RootPrepare => 1,
            Pc::RootRestore => 2,
            Pc::Limits => 3,
            Pc::IqCorrection => 4,
            Pc::RegistersOn | Pc::RegistersOff => 5,
            Pc::Frequency => 6,
            Pc::Debug => 7,
            Pc::RxOn
            | Pc::SharedForce
            | Pc::RadioI
            | Pc::RadioQ
            | Pc::BasebandI
            | Pc::BasebandQ
            | Pc::Gain
            | Pc::SharedMixer
            | Pc::FineCode
            | Pc::RxOff => 8,
            Pc::RxClockOn | Pc::TxClockOn | Pc::RxClockOff | Pc::TxClockOff => 9,
            Pc::SharedRead => 10,
            Pc::SharedI2c | Pc::SharedI2cRestore => 11,
            Pc::WorkMode => 12,
            Pc::CalPrepare => 16,
            Pc::CalRead => 17,
            Pc::CalI | Pc::CalQ | Pc::CalLevel | Pc::CalCleanupI | Pc::CalCleanupQ => 18,
            Pc::CalSettle => 19,
            Pc::CalRestore => 20,
            Pc::Estimate => match &self.work {
                Work::Calibration(cal) => match cal.estimator.action() {
                    PhyDcIqAction::Configure(_) => 21,
                    PhyDcIqAction::SetEnable { .. } => 22,
                    PhyDcIqAction::DelayMicros { .. } => 23,
                    PhyDcIqAction::AwaitReadinessEdge { .. } => 24,
                    PhyDcIqAction::ReadAccumulators(_) => 25,
                    _ => 0,
                },
                _ => 0,
            },
            Pc::PublishDebug => 26,
            Pc::PublishRxOn | Pc::PublishRestoreRxOn => 27,
            Pc::PublishRxClockOn
            | Pc::PublishTxClockOn
            | Pc::PublishRxClockOff
            | Pc::PublishTxClockOff => 28,
            Pc::Entry => 29,
            Pc::PublishWorkMode => 30,
            Pc::Complete | Pc::Failed => 0,
        }
    }
    pub(crate) fn abandon(&mut self) {
        if !matches!(self.cursor.pc, Pc::Complete | Pc::Failed) {
            self.fault.get_or_insert(RxError::Hardware);
        }
    }
    pub(crate) fn fail_stop(&mut self, error: RxError) {
        self.fault.get_or_insert(error);
    }

    pub(crate) fn step(&mut self) -> Result<RxStep<'_>, RxError> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        use ControlCommand as C;
        let control = match self.cursor.pc {
            Pc::RootPrepare => Some(C::PrepareRoot),
            Pc::RootRestore => Some(C::RestoreRoot),
            Pc::RegistersOn => Some(C::Registers(true)),
            Pc::RegistersOff => Some(C::Registers(false)),
            Pc::Debug | Pc::PublishDebug => Some(C::Debug),
            Pc::RxClockOn | Pc::PublishRxClockOn => Some(C::RxClock(true)),
            Pc::TxClockOn | Pc::PublishTxClockOn => Some(C::TxClock(true)),
            Pc::RxClockOff | Pc::PublishRxClockOff => Some(C::RxClock(false)),
            Pc::TxClockOff | Pc::PublishTxClockOff => Some(C::TxClock(false)),
            Pc::WorkMode | Pc::PublishWorkMode => Some(C::WorkMode),
            Pc::CalPrepare => Some(C::PrepareCalibration),
            Pc::CalRestore => Some(C::RestoreCalibration),
            Pc::Limits => Some(C::Limits(generated_table(PhyRxGainBank::Wifi).last_index)),
            Pc::IqCorrection => Some(C::IqCorrection),
            _ => None,
        };
        if let Some(command) = control {
            return Ok(RxStep::Control(ControlStep {
                program: self,
                command,
            }));
        }
        if let Some(command) = self.pbus_command() {
            return Ok(RxStep::Pbus(PbusStep {
                program: self,
                command,
            }));
        }
        match self.cursor.pc {
            Pc::SharedRead => Ok(RxStep::ReadPbus(ReadPbusStep {
                program: self,
                selector: 1,
                path: 1,
            })),
            Pc::CalRead => Ok(RxStep::ReadPbus(ReadPbusStep {
                program: self,
                selector: 1,
                path: 2,
            })),
            Pc::CalSettle => Ok(RxStep::Settle(SettleStep {
                program: self,
                micros: 10,
            })),
            Pc::Entry => Ok(RxStep::GainEntry(GainEntryStep { program: self })),
            Pc::Estimate => {
                let Work::Calibration(cal) = &self.work else {
                    return Err(RxError::WrongCompletion);
                };
                let command = cal.estimator.action();
                Ok(RxStep::Estimator(EstimatorStep {
                    program: self,
                    command,
                }))
            }
            Pc::Frequency => {
                let Work::Frequency(model) = &self.work else {
                    return Err(RxError::WrongCompletion);
                };
                let command = model.action();
                Ok(RxStep::Frequency(FrequencyStep {
                    program: self,
                    command,
                }))
            }
            Pc::SharedI2c | Pc::SharedI2cRestore => {
                let Work::Field(model) = &self.work else {
                    return Err(RxError::WrongCompletion);
                };
                if self.bus.is_none() {
                    self.bus = Some(
                        MaskedI2cWriteBinding::new(model.action())
                            .map_err(|_| RxError::WrongCompletion)?,
                    );
                }
                Ok(RxStep::I2c(I2cStep { program: self }))
            }
            Pc::Complete | Pc::Failed => Ok(RxStep::Complete),
            _ => Err(RxError::WrongCompletion),
        }
    }

    fn publish_bank(&self) -> PhyRxGainBank {
        match self.cursor.bank {
            PhyRxGainDcBank::Wifi => PhyRxGainBank::Wifi,
            PhyRxGainDcBank::Shared => PhyRxGainBank::Shared,
        }
    }
    fn baseband(&self) -> [u16; 2] {
        if self.cursor.bank == PhyRxGainDcBank::Wifi && self.cursor.gain != 0 {
            self.memory.wifi_fine_dc[0]
        } else {
            [0x100; 2]
        }
    }
    fn previous(&self) -> [u16; 2] {
        if self.cursor.gain == 0 {
            [0x100; 2]
        } else {
            match self.cursor.bank {
                PhyRxGainDcBank::Wifi => self.memory.wifi_index_dc[0],
                PhyRxGainDcBank::Shared => self.memory.shared_index_dc[0],
            }
        }
    }
    fn pbus_command(&self) -> Option<PhyPbusForceTest> {
        let c = &self.cursor;
        Some(match c.pc {
            Pc::RxOn => dc::rx_on(c.index, self.inputs.pbus_rx_path_value),
            Pc::SharedForce => PhyPbusForceTest::new(1, 1, self.shared_force),
            Pc::RadioI => PhyPbusForceTest::new(2, 1, 0x100),
            Pc::RadioQ => PhyPbusForceTest::new(3, 1, 0x100),
            Pc::BasebandI => PhyPbusForceTest::new(2, 2, self.baseband()[0]),
            Pc::BasebandQ => PhyPbusForceTest::new(3, 2, self.baseband()[1]),
            Pc::Gain => dc::set_rx_gain_transaction(
                u32::from(dc::gain(c.bank, c.gain)) << 12,
                self.inputs.pbus_rx_path_value,
                c.index,
            ),
            Pc::SharedMixer => {
                dc::shared_mixer_dgain_transaction(c.gain, self.inputs.pbus_rx_path_value)
            }
            Pc::FineCode => PhyPbusForceTest::new(1, 2, dc::fine_code(c.fine?)),
            Pc::CalI | Pc::CalQ | Pc::CalCleanupI | Pc::CalCleanupQ | Pc::CalLevel => {
                let Work::Calibration(cal) = &self.work else {
                    return None;
                };
                if c.pc == Pc::CalLevel {
                    PhyPbusForceTest::new(1, 2, if c.high { 0x20 } else { 0 })
                } else {
                    let pair = if matches!(c.pc, Pc::CalCleanupI | Pc::CalCleanupQ) {
                        match cal.terminal {
                            Some(Ok(outcome)) => outcome.configuration,
                            _ => cal.request.initial,
                        }
                    } else {
                        cal.policy.current
                    };
                    let q = matches!(c.pc, Pc::CalQ | Pc::CalCleanupQ);
                    PhyPbusForceTest::new(
                        if q { 3 } else { 2 },
                        dc::calibration_path(cal.request.stage),
                        if q { pair[1] } else { pair[0] },
                    )
                }
            }
            Pc::RxOff => dc::rx_off(c.index),
            Pc::PublishRxOn | Pc::PublishRestoreRxOn => {
                dc::rx_on(c.index, self.memory.parameter_002)
            }
            _ => return None,
        })
    }

    fn hardware_result<T>(&mut self, result: Result<T, RxError>) -> Result<T, RxError> {
        // A fault retains all logical restore obligations and the physical
        // caller's borrow; it cannot produce a successful semantic terminal.
        if let Err(error) = result {
            self.fault = Some(error);
        }
        result
    }
    fn start_frequency(&mut self) {
        let request = RfpllFrequencyRequest {
            crystal_selector: self.inputs.crystal_selector,
            frequency_code: 0x9b4,
            offset: 0,
        };
        #[cfg(feature = "radio-health")]
        {
            self.health_frequency = Some(request.frequency_code);
        }
        self.work = Work::Frequency(RfpllFrequencyTransition::channel(request));
        self.cursor.pc = Pc::Frequency;
    }
    fn frequency_completed(&mut self, completion: RfpllFrequencyCompletion) -> Result<(), RxError> {
        let Work::Frequency(model) = &mut self.work else {
            return Err(RxError::WrongCompletion);
        };
        model
            .advance(completion)
            .map_err(|_| RxError::WrongCompletion)?;
        match model.action() {
            RfpllFrequencyAction::Complete(_) => {
                self.work = Work::Idle;
                self.cursor.pc = Pc::Debug;
            }
            RfpllFrequencyAction::Failed(failure) => {
                self.fail_dc(PhyRxGainDcFailure::Rfpll(failure))
            }
            _ => {}
        }
        Ok(())
    }
    fn start_field(&mut self, restore: bool) {
        self.work = Work::Field(i2c::field_write(
            i2c::fields::BBTOP_SHARED_RX_GAIN_CALIBRATION_ENABLE,
            u8::from(restore),
        ));
        self.cursor.pc = if restore {
            Pc::SharedI2cRestore
        } else {
            Pc::SharedI2c
        };
    }
    fn cleanup_dc(&mut self, after: AfterDc) {
        self.work = Work::Idle;
        self.cursor.after_dc = after;
        self.cursor.pc = Pc::RxClockOff;
    }
    fn fail_dc(&mut self, failure: PhyRxGainDcFailure) {
        self.failure = Some(PhyRxGainInitFailure::Dc(failure));
        self.cleanup_dc(AfterDc::Failed);
    }
    fn next_gain(&mut self) {
        if self.cursor.gain + 1 < dc::bank_count(self.cursor.bank) {
            self.cursor.gain += 1;
            self.cursor.pc = Pc::RadioI;
        } else if self.cursor.bank == PhyRxGainDcBank::Shared {
            self.start_field(true);
        } else {
            self.cleanup_dc(AfterDc::Complete);
        }
    }
    fn start_calibration(&mut self) {
        let fine = self.cursor.fine.is_some();
        let request = PhyRxDcCalibrationRequest {
            shared_radio: self.cursor.bank == PhyRxGainDcBank::Shared,
            stage: if fine {
                PhyRxDcCalibrationStage::Radio
            } else {
                PhyRxDcCalibrationStage::Baseband
            },
            control: 0x800,
            initial: if fine {
                self.fine_current
            } else {
                self.previous()
            },
            gain_index: if fine { 0 } else { self.cursor.gain },
            rx_saturation_detected: self.inputs.rx_saturation_detected,
        };
        let policy = PhyRxDcCalibrationPolicyState::new(request.initial);
        let minimum_request = PhyRxDcMinimumRequest {
            measurement: 0,
            control: request.control,
            mode: u8::from(!fine),
            rx_saturation_detected: request.rx_saturation_detected,
            previous: policy.measurement,
        };
        self.work = Work::Calibration(Calibration {
            request,
            policy,
            population: 0,
            threshold: 0,
            terminal: None,
            minimum_request,
            minimum: PhyRxDcMinimumPolicyState::new(minimum_request.previous),
            estimator: PhyDcIqEstimateTransition::new(estimate_request(minimum_request, 0)),
        });
        self.cursor.high = false;
        self.cursor.pc = Pc::CalPrepare;
    }
    fn start_minimum(&mut self) -> Result<(), RxError> {
        let Work::Calibration(cal) = &mut self.work else {
            return Err(RxError::WrongCompletion);
        };
        let high = self.cursor.high;
        let previous = if cal.request.stage == PhyRxDcCalibrationStage::Baseband && !high {
            cal.policy.low
        } else {
            cal.policy.measurement
        };
        cal.minimum_request = PhyRxDcMinimumRequest {
            measurement: cal
                .policy
                .iteration
                .wrapping_mul(2)
                .wrapping_add(u8::from(high)),
            control: cal.request.control,
            mode: u8::from(cal.request.stage == PhyRxDcCalibrationStage::Baseband),
            rx_saturation_detected: cal.request.rx_saturation_detected,
            previous,
        };
        cal.minimum = PhyRxDcMinimumPolicyState::new(previous);
        cal.estimator = PhyDcIqEstimateTransition::new(estimate_request(cal.minimum_request, 0));
        self.cursor.pc = Pc::Estimate;
        Ok(())
    }
    fn finish_calibration(&mut self) -> Result<(), RxError> {
        let Work::Calibration(cal) = &self.work else {
            return Err(RxError::WrongCompletion);
        };
        let terminal = cal.terminal.ok_or(RxError::WrongCompletion)?;
        let outcome = match terminal {
            Ok(outcome) => outcome,
            Err(failure) => {
                self.fail_dc(PhyRxGainDcFailure::Calibration(failure));
                return Ok(());
            }
        };
        #[cfg(feature = "radio-health")]
        {
            self.health_correction = Some(crate::tracking::observation::health::Correction {
                context: self.measurement_context()?,
                configuration: outcome.configuration,
                iterations: outcome.iterations,
                converged: outcome.converged,
            });
        }
        if let Some(index) = self.cursor.fine {
            let slot = self
                .memory
                .wifi_fine_dc
                .get_mut(usize::from(index))
                .ok_or(RxError::WrongCompletion)?;
            self.quality.record_wifi_fine(index, outcome.converged);
            self.fine_current = outcome.configuration;
            *slot = outcome.configuration;
            self.work = Work::Idle;
            if usize::from(index) + 1 == dc::FINE_CODES {
                self.cursor.fine = None;
                self.next_gain();
            } else {
                self.cursor.fine = Some(index + 1);
                self.cursor.pc = Pc::FineCode;
            }
        } else {
            let index = self.cursor.gain;
            match self.cursor.bank {
                PhyRxGainDcBank::Wifi => {
                    let slot = self
                        .memory
                        .wifi_index_dc
                        .get_mut(usize::from(index))
                        .ok_or(RxError::WrongCompletion)?;
                    self.quality.record_wifi_baseband(index, outcome.converged);
                    *slot = outcome.configuration;
                }
                PhyRxGainDcBank::Shared => {
                    let slot = self
                        .memory
                        .shared_index_dc
                        .get_mut(usize::from(index))
                        .ok_or(RxError::WrongCompletion)?;
                    self.quality.record_shared(index, outcome.converged);
                    *slot = outcome.configuration;
                }
            }
            self.work = Work::Idle;
            if self.cursor.bank == PhyRxGainDcBank::Wifi && index == 0 {
                self.cursor.fine = Some(0);
                self.cursor.pc = Pc::FineCode;
            } else {
                self.next_gain();
            }
        }
        Ok(())
    }
    fn estimator_completed(&mut self, completion: PhyDcIqCompletion) -> Result<(), RxError> {
        let Work::Calibration(cal) = &mut self.work else {
            return Err(RxError::WrongCompletion);
        };
        cal.estimator
            .advance(completion)
            .map_err(|_| RxError::WrongCompletion)?;
        let outcome = match cal.estimator.action() {
            PhyDcIqAction::Complete(outcome) => outcome,
            PhyDcIqAction::Failed(failure) => {
                cal.terminal = Some(Err(PhyRxDcCalibrationFailure::Minimum(
                    PhyRxDcMinimumFailure::DcIq(failure),
                )));
                self.cursor.pc = Pc::CalCleanupI;
                return Ok(());
            }
            _ => return Ok(()),
        };
        match cal.minimum.accept(cal.minimum_request, outcome) {
            PhyRxDcMinimumDecision::Continue => {
                cal.estimator = PhyDcIqEstimateTransition::new(estimate_request(
                    cal.minimum_request,
                    cal.minimum.attempts,
                ))
            }
            PhyRxDcMinimumDecision::Complete(outcome) => {
                let terminal = cal.policy.accept(
                    cal.request,
                    cal.population,
                    cal.threshold,
                    self.cursor.high,
                    outcome.estimate,
                    outcome.readiness_activity_edges,
                );
                if cal.request.stage == PhyRxDcCalibrationStage::Baseband && !self.cursor.high {
                    self.cursor.high = true;
                    self.cursor.pc = Pc::CalLevel;
                } else if let Some(outcome) = terminal {
                    cal.terminal = Some(Ok(outcome));
                    self.cursor.pc = Pc::CalCleanupI;
                } else {
                    self.cursor.pc = Pc::CalI;
                }
            }
        }
        Ok(())
    }

    fn pbus_completed(
        &mut self,
        completed: bool,
        command: PhyPbusForceTest,
    ) -> Result<(), RxError> {
        let pc = self.cursor.pc;
        if !completed {
            match pc {
                Pc::CalI | Pc::CalQ | Pc::CalLevel => {
                    let Work::Calibration(cal) = &mut self.work else {
                        return Err(RxError::WrongCompletion);
                    };
                    cal.terminal = Some(Err(PhyRxDcCalibrationFailure::PbusForceTimedOut(command)));
                    self.cursor.pc = Pc::CalCleanupI;
                    return Ok(());
                }
                Pc::CalCleanupI | Pc::CalCleanupQ | Pc::RxOff => {}
                Pc::PublishRxOn | Pc::PublishRestoreRxOn => {
                    self.failure = Some(PhyRxGainInitFailure::Publish(
                        PhyRxGainPublishFailure::PbusTimedOut {
                            bank: self.publish_bank(),
                            transaction: command,
                        },
                    ));
                    self.cursor.pc = Pc::PublishWorkMode;
                    return Ok(());
                }
                _ => {
                    self.fail_dc(PhyRxGainDcFailure::Pbus {
                        bank: self.cursor.bank,
                        transaction: command,
                    });
                    return Ok(());
                }
            }
        }
        self.cursor.pc = match pc {
            Pc::RxOn => {
                self.cursor.index += 1;
                if self.cursor.index == 7 {
                    Pc::RxClockOn
                } else {
                    Pc::RxOn
                }
            }
            Pc::SharedForce => {
                self.start_field(false);
                return Ok(());
            }
            Pc::RadioI => Pc::RadioQ,
            Pc::RadioQ => Pc::BasebandI,
            Pc::BasebandI => Pc::BasebandQ,
            Pc::BasebandQ => {
                self.cursor.index = 0;
                Pc::Gain
            }
            Pc::Gain => {
                self.cursor.index += 1;
                if self.cursor.index < 3 {
                    Pc::Gain
                } else if self.cursor.bank == PhyRxGainDcBank::Shared {
                    Pc::SharedMixer
                } else {
                    self.start_calibration();
                    return Ok(());
                }
            }
            Pc::SharedMixer | Pc::FineCode => {
                self.start_calibration();
                return Ok(());
            }
            Pc::CalI => Pc::CalQ,
            Pc::CalQ => {
                let Work::Calibration(cal) = &self.work else {
                    return Err(RxError::WrongCompletion);
                };
                self.cursor.high = false;
                if cal.request.stage == PhyRxDcCalibrationStage::Radio {
                    Pc::CalSettle
                } else {
                    Pc::CalLevel
                }
            }
            Pc::CalLevel => Pc::CalSettle,
            Pc::CalCleanupI => Pc::CalCleanupQ,
            Pc::CalCleanupQ => Pc::CalRestore,
            Pc::RxOff => {
                self.cursor.index += 1;
                if self.cursor.index == 3 {
                    Pc::WorkMode
                } else {
                    Pc::RxOff
                }
            }
            Pc::PublishRxOn | Pc::PublishRestoreRxOn => {
                self.cursor.index += 1;
                if self.cursor.index < 7 {
                    pc
                } else if pc == Pc::PublishRestoreRxOn {
                    Pc::PublishWorkMode
                } else {
                    Pc::PublishRxClockOn
                }
            }
            _ => return Err(RxError::WrongCompletion),
        };
        Ok(())
    }
    fn control_completed(&mut self) -> Result<(), RxError> {
        self.cursor.pc = match self.cursor.pc {
            Pc::RootPrepare => {
                // A fresh DC run starts with fresh products; publication later
                // uses these same slots, never the stale measured input bank.
                self.memory.wifi_index_dc = [[0; 2]; 8];
                self.memory.wifi_fine_dc = [[0; 2]; dc::FINE_CODES];
                self.memory.shared_index_dc = [[0; 2]; 11];
                Pc::RegistersOn
            }
            Pc::RegistersOn => {
                self.start_frequency();
                return Ok(());
            }
            Pc::Debug => {
                self.cursor.index = 0;
                Pc::RxOn
            }
            Pc::RxClockOn => Pc::TxClockOn,
            Pc::TxClockOn => {
                if self.cursor.bank == PhyRxGainDcBank::Shared {
                    Pc::SharedRead
                } else {
                    self.cursor.gain = 0;
                    Pc::RadioI
                }
            }
            Pc::CalPrepare => Pc::CalRead,
            Pc::CalRestore => return self.finish_calibration(),
            Pc::RxClockOff => Pc::TxClockOff,
            Pc::TxClockOff => {
                self.cursor.index = 0;
                Pc::RxOff
            }
            Pc::WorkMode => Pc::RegistersOff,
            Pc::RegistersOff => match self.cursor.after_dc {
                AfterDc::Wifi => {
                    self.cursor.bank = PhyRxGainDcBank::Wifi;
                    self.cursor.gain = 0;
                    Pc::RegistersOn
                }
                AfterDc::Complete | AfterDc::Failed => Pc::RootRestore,
            },
            Pc::RootRestore => {
                if self.failure.is_some() {
                    Pc::Failed
                } else {
                    self.dc_complete = self.dc_required;
                    self.cursor.bank = PhyRxGainDcBank::Wifi;
                    self.cursor.index = 0;
                    self.work = Work::Idle;
                    Pc::PublishDebug
                }
            }
            Pc::PublishDebug => {
                self.cursor.index = 0;
                Pc::PublishRxOn
            }
            Pc::PublishRxClockOn => Pc::PublishTxClockOn,
            Pc::PublishTxClockOn => {
                self.cursor.index = 0;
                Pc::Entry
            }
            Pc::PublishRxClockOff => Pc::PublishTxClockOff,
            Pc::PublishTxClockOff => {
                self.cursor.index = 0;
                Pc::PublishRestoreRxOn
            }
            Pc::PublishWorkMode => {
                if self.failure.is_some() {
                    Pc::Failed
                } else if self.cursor.bank == PhyRxGainDcBank::Wifi {
                    self.cursor.bank = PhyRxGainDcBank::Shared;
                    Pc::PublishDebug
                } else {
                    Pc::Limits
                }
            }
            Pc::Limits => Pc::IqCorrection,
            Pc::IqCorrection => Pc::Complete,
            _ => return Err(RxError::WrongCompletion),
        };
        Ok(())
    }
}

fn estimate_request(request: PhyRxDcMinimumRequest, attempt: u8) -> PhyDcIqEstimateRequest {
    PhyDcIqEstimateRequest {
        iteration: attempt,
        chain: 1,
        control: request.control,
        mode: request.mode,
    }
}

impl PbusStep<'_> {
    pub(crate) fn command(&self) -> PhyPbusForceTest {
        self.command
    }
    pub(crate) fn prepare_publication(&mut self) {
        self.program.restore.bus = true;
    }
    pub(crate) fn complete(self, result: Result<bool, RxError>) -> Result<(), RxError> {
        let completed = self.program.hardware_result(result)?;
        self.program.restore.bus = !completed;
        self.program.pbus_completed(completed, self.command)
    }
}
impl ReadPbusStep<'_> {
    pub(crate) fn command(&self) -> (u8, u8) {
        (self.selector, self.path)
    }
    pub(crate) fn complete(self, result: Result<u32, RxError>) -> Result<(), RxError> {
        let value = self.program.hardware_result(result)?;
        match self.program.cursor.pc {
            Pc::SharedRead => {
                self.program.shared_force = value as u16 | 2;
                self.program.cursor.pc = Pc::SharedForce;
            }
            Pc::CalRead => {
                let Work::Calibration(cal) = &mut self.program.work else {
                    return Err(RxError::WrongCompletion);
                };
                cal.population = (value as u8 & 0x3f).count_ones() as u8;
                cal.threshold = dc::calibration_threshold(cal.request, cal.population);
                self.program.cursor.pc = Pc::CalI;
            }
            _ => return Err(RxError::WrongCompletion),
        }
        Ok(())
    }
}
impl FrequencyStep<'_> {
    pub(crate) fn command(&self) -> RfpllFrequencyAction {
        self.command
    }
    pub(crate) fn complete(
        self,
        result: Result<RfpllFrequencyCompletion, RxError>,
    ) -> Result<(), RxError> {
        let result = self.program.hardware_result(result)?;
        self.program.frequency_completed(result)
    }
}
impl EstimatorStep<'_> {
    pub(crate) fn command(&self) -> PhyDcIqAction {
        self.command
    }
    pub(crate) fn complete(
        self,
        result: Result<PhyDcIqCompletion, RxError>,
    ) -> Result<(), RxError> {
        let result = self.program.hardware_result(result)?;
        self.program.estimator_completed(result)
    }
}
impl GainEntryStep<'_> {
    pub(crate) fn command(&self) -> PhyGainMemoryEntry {
        let bank = self.program.publish_bank();
        phy_generated_rx_gain_memory_entry(
            &self.program.memory,
            bank,
            generated_table(bank),
            self.program.cursor.index,
        )
    }
    pub(crate) fn complete(self, result: Result<(), RxError>) -> Result<(), RxError> {
        self.program.hardware_result(result)?;
        if self.program.cursor.index == generated_table(self.program.publish_bank()).last_index {
            self.program.cursor.pc = Pc::PublishRxClockOff;
        } else {
            self.program.cursor.index += 1;
        }
        Ok(())
    }
}
impl ControlStep<'_> {
    pub(crate) fn command(&self) -> ControlCommand {
        self.command
    }
    pub(crate) fn prepare_publication(&mut self) {
        match self.command {
            ControlCommand::PrepareRoot => self.program.restore.root = true,
            ControlCommand::PrepareCalibration => self.program.restore.calibration = true,
            ControlCommand::Registers(true) => self.program.restore.registers = true,
            _ => {}
        }
    }
    pub(crate) fn complete(self, result: Result<(), RxError>) -> Result<(), RxError> {
        self.program.hardware_result(result)?;
        match self.command {
            ControlCommand::RestoreRoot => self.program.restore.root = false,
            ControlCommand::RestoreCalibration => self.program.restore.calibration = false,
            ControlCommand::Registers(false) => self.program.restore.registers = false,
            _ => {}
        }
        self.program.control_completed()
    }
}
impl SettleStep<'_> {
    pub(crate) fn micros(&self) -> u32 {
        self.micros
    }
    pub(crate) fn complete(self, result: Result<(), RxError>) -> Result<(), RxError> {
        self.program.hardware_result(result)?;
        self.program.start_minimum()
    }
}
impl I2cStep<'_> {
    pub(crate) fn transaction_mut(&mut self) -> Result<&mut PhyColdI2cTransaction, RxError> {
        self.program
            .bus
            .as_mut()
            .map(MaskedI2cWriteBinding::transaction_mut)
            .ok_or(RxError::WrongCompletion)
    }
    pub(crate) fn prepare_publication(&mut self) {
        self.program.restore.bus = true;
    }
    pub(crate) fn complete(mut self, result: Result<(), RxError>) -> Result<(), RxError> {
        self.program.hardware_result(result)?;
        if !matches!(
            self.transaction_mut()?.action(),
            PhyColdI2cAction::Complete(_)
        ) {
            return Err(RxError::WrongCompletion);
        }
        let binding = self.program.bus.take().ok_or(RxError::WrongCompletion)?;
        self.program.restore.bus = false;
        let completion = binding
            .into_completion()
            .map_err(|_| RxError::WrongCompletion)?;
        let Work::Field(model) = &mut self.program.work else {
            return Err(RxError::WrongCompletion);
        };
        model
            .advance(completion)
            .map_err(|_| RxError::WrongCompletion)?;
        if model.action() == oer_espressif_analog::Action::Complete(()) {
            if self.program.cursor.pc == Pc::SharedI2cRestore {
                self.program.cleanup_dc(AfterDc::Wifi);
            } else {
                self.program.work = Work::Idle;
                self.program.cursor.gain = 0;
                self.program.cursor.pc = Pc::RadioI;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(feature = "radio-health")]
impl RxProgram {
    fn measurement_context(
        &self,
    ) -> Result<crate::tracking::observation::health::Context, RxError> {
        let Work::Calibration(cal) = &self.work else {
            return Err(RxError::WrongCompletion);
        };
        Ok(crate::tracking::observation::health::Context {
            gain: cal.request.gain_index,
            measurement: cal.minimum_request.measurement,
            shared_radio: cal.request.shared_radio,
            radio_stage: cal.request.stage == PhyRxDcCalibrationStage::Radio,
            fine_code: self.cursor.fine,
            high_level: self.cursor.high,
            frequency_code: self.health_frequency,
        })
    }
    pub(crate) fn take_health_correction(
        &mut self,
    ) -> Option<crate::tracking::observation::health::Correction> {
        self.health_correction.take()
    }
}
#[cfg(feature = "radio-health")]
impl EstimatorStep<'_> {
    pub(crate) fn health_context(
        &self,
    ) -> Result<crate::tracking::observation::health::Context, RxError> {
        self.program.measurement_context()
    }
}
