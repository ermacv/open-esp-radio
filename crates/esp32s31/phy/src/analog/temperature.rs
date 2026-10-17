//! Explicit ESP32-S31 PHY temperature-sensor transition.
//!
//! The required reference graph is ROM `phy_tsens_temp_read` at
//! `0x2f82_5eec`, its callback target `phy_tsens_temp_read_local` at
//! `0x2f82_5f1e`, and the finite conversion helpers at
//! `0x2f82_5de4..0x2f82_5ecc`. The outer function's indirect call and stores
//! through `g_phyFuns` and `phy_param` are ABI plumbing, not part of the radio
//! algorithm.
//!
//! Rust exposes the PHY-I2C read, one temperature-code sample, and the
//! conditional PHY-I2C range write as identity-bound actions. An out-of-range
//! sample requires a completed range write even if it reselects the current DAC.
//! The DAC register and field are supplied by the reviewed semantic PAC.
//! Invalid DAC codes fail closed instead of reproducing the ROM's out-of-bounds default
//! table index. The reset value zero is handled separately: a cold start on
//! hardware observed it before the first baseband temperature pass, and the existing
//! vendor-oracle handoff primes the same field to the first ROM range (DAC 5)
//! before entering the open channel graph.

use crate::analog::i2c::{PhyI2cField, analog_registers};

const RESET_DAC: u8 = 0;
const DEFAULT_DAC: u8 = 5;
const DEFAULT_SENSOR_INDEX: u8 = 0;

#[derive(Clone, Copy)]
struct TemperatureAttribute {
    calibration: i8,
    dac: u8,
    low: i16,
    high: i16,
}

// Exact 30-byte `phy_tsens_attribute` object at ROM address 0x2f84_d9ec.
const ATTRIBUTES: [TemperatureAttribute; 5] = [
    TemperatureAttribute {
        calibration: -2,
        dac: 5,
        low: 50,
        high: 125,
    },
    TemperatureAttribute {
        calibration: -1,
        dac: 7,
        low: 20,
        high: 100,
    },
    TemperatureAttribute {
        calibration: 0,
        dac: 15,
        low: -10,
        high: 80,
    },
    TemperatureAttribute {
        calibration: 1,
        dac: 11,
        low: -30,
        high: 50,
    },
    TemperatureAttribute {
        calibration: 2,
        dac: 10,
        low: -40,
        high: 20,
    },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTemperatureOutcome {
    pub temperature: i16,
    pub sensor_index: u8,
    pub next_dac: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTemperatureFailure {
    InvalidDac(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTemperatureAction {
    ReadMasked { field: PhyI2cField },
    SampleCode,
    WriteMasked { field: PhyI2cField, value: u8 },
    Complete(PhyTemperatureOutcome),
    Failed(PhyTemperatureFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTemperatureCompletion {
    MaskedRead { field: PhyI2cField, value: u8 },
    CodeSampled { value: u8 },
    MaskedWrite { field: PhyI2cField, value: u8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTemperatureTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyTemperatureStep {
    ReadDac,
    PrimeDefaultDac,
    SampleCode { sensor_index: u8 },
    WriteDac { outcome: PhyTemperatureOutcome },
    Complete(PhyTemperatureOutcome),
    Failed(PhyTemperatureFailure),
}

const fn sensor_index(dac: u8) -> Option<u8> {
    match dac {
        5 => Some(0),
        7 => Some(1),
        15 => Some(2),
        11 => Some(3),
        10 => Some(4),
        _ => None,
    }
}

const fn attribute(index: u8) -> TemperatureAttribute {
    match index {
        0 => ATTRIBUTES[0],
        1 => ATTRIBUTES[1],
        2 => ATTRIBUTES[2],
        3 => ATTRIBUTES[3],
        _ => ATTRIBUTES[4],
    }
}

/// Exact arithmetic of ROM `phy_code_to_temp`.
pub const fn temperature_from_code(code: u8, calibration: i8) -> i16 {
    let scaled = (code as i32) * 44 + (calibration as i32) * -2_788;
    if scaled > 27_151 {
        250
    } else {
        let temperature = (scaled - 2_052) / 100;
        if temperature < -200 {
            -200
        } else {
            temperature as i16
        }
    }
}

const fn selected_dac(temperature: i16, current: TemperatureAttribute) -> u8 {
    if temperature >= current.low && temperature <= current.high {
        current.dac
    } else if temperature > 99 {
        5
    } else if temperature > 79 {
        7
    } else if temperature >= -9 {
        15
    } else if temperature < -29 {
        10
    } else {
        11
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTemperatureTransition {
    step: PhyTemperatureStep,
}

impl PhyTemperatureTransition {
    pub const fn new() -> Self {
        Self {
            step: PhyTemperatureStep::ReadDac,
        }
    }

    pub const fn action(self) -> PhyTemperatureAction {
        match self.step {
            PhyTemperatureStep::ReadDac => PhyTemperatureAction::ReadMasked {
                field: analog_registers::TEMPERATURE_SENSOR_DAC_STATUS,
            },
            PhyTemperatureStep::PrimeDefaultDac => PhyTemperatureAction::WriteMasked {
                field: analog_registers::TEMPERATURE_SENSOR_DAC,
                value: DEFAULT_DAC,
            },
            PhyTemperatureStep::SampleCode { .. } => PhyTemperatureAction::SampleCode,
            PhyTemperatureStep::WriteDac { outcome } => PhyTemperatureAction::WriteMasked {
                field: analog_registers::TEMPERATURE_SENSOR_DAC,
                value: outcome.next_dac,
            },
            PhyTemperatureStep::Complete(outcome) => PhyTemperatureAction::Complete(outcome),
            PhyTemperatureStep::Failed(failure) => PhyTemperatureAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyTemperatureCompletion,
    ) -> Result<(), PhyTemperatureTransitionError> {
        self.step = match (self.step, completion) {
            (
                PhyTemperatureStep::ReadDac,
                PhyTemperatureCompletion::MaskedRead {
                    field: analog_registers::TEMPERATURE_SENSOR_DAC_STATUS,
                    value,
                },
            ) => {
                let dac = value & 0x0f;
                if dac == RESET_DAC {
                    // SOURCE[HIL_OPEN_PHY_COLD_2026_07_28]: rev0 cold start
                    // reaches the first baseband temperature pass with DAC 0.
                    // SOURCE[ROM_REV0_PHY_TSENS]: `phy_tsens_dac_to_index(0)`
                    // returns index 5 for the five-entry table at 0x2f84_d9ec,
                    // so following the ROM literally would read out of bounds.
                    // SOURCE[VENDOR_ORACLE_OPEN_TEMPERATURE_POWER]: the
                    // vendor-to-open handoff already writes DAC 5 before its
                    // first open temperature sample.
                    PhyTemperatureStep::PrimeDefaultDac
                } else {
                    match sensor_index(dac) {
                        Some(sensor_index) => PhyTemperatureStep::SampleCode { sensor_index },
                        None => PhyTemperatureStep::Failed(PhyTemperatureFailure::InvalidDac(dac)),
                    }
                }
            }
            (
                PhyTemperatureStep::PrimeDefaultDac,
                PhyTemperatureCompletion::MaskedWrite {
                    field: analog_registers::TEMPERATURE_SENSOR_DAC,
                    value: DEFAULT_DAC,
                },
            ) => PhyTemperatureStep::SampleCode {
                sensor_index: DEFAULT_SENSOR_INDEX,
            },
            (
                PhyTemperatureStep::SampleCode { sensor_index },
                PhyTemperatureCompletion::CodeSampled { value },
            ) => {
                let current = attribute(sensor_index);
                let temperature = temperature_from_code(value, current.calibration);
                let next_dac = selected_dac(temperature, current);
                let outcome = PhyTemperatureOutcome {
                    temperature,
                    sensor_index,
                    next_dac,
                };
                // ROM phy_tsens_dac_cal writes on every out-of-range sample,
                // including saturation at the current end range. Equality of
                // DAC values alone does not prove that the write can be omitted.
                if temperature >= current.low && temperature <= current.high {
                    PhyTemperatureStep::Complete(outcome)
                } else {
                    PhyTemperatureStep::WriteDac { outcome }
                }
            }
            (
                PhyTemperatureStep::WriteDac { outcome },
                PhyTemperatureCompletion::MaskedWrite {
                    field: analog_registers::TEMPERATURE_SENSOR_DAC,
                    value,
                },
            ) if value == outcome.next_dac => PhyTemperatureStep::Complete(outcome),
            (PhyTemperatureStep::Complete(_), _) | (PhyTemperatureStep::Failed(_), _) => {
                return Err(PhyTemperatureTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTemperatureTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

impl Default for PhyTemperatureTransition {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTemperatureBindingError {
    UnsupportedAction,
    IncompleteTransaction,
    UnexpectedOutcome,
}

/// Non-cloneable owner of one temperature-sensor PHY-I2C transaction.
///
/// The original high-level action remains attached to the low-level
/// transaction until completion, so an observation for another field at the
/// same I2C address cannot be accepted accidentally.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyTemperatureI2cBinding {
    outer_action: PhyTemperatureAction,
    transaction: crate::calibration::cold::PhyColdI2cTransaction,
}

impl PhyTemperatureI2cBinding {
    pub fn new(action: PhyTemperatureAction) -> Result<Self, PhyTemperatureBindingError> {
        let request = match action {
            PhyTemperatureAction::ReadMasked { field } => Some(
                crate::calibration::cold::PhyColdI2cRequest::read_field(field),
            ),
            PhyTemperatureAction::WriteMasked { field, value } => Some(
                crate::calibration::cold::PhyColdI2cRequest::write_field(field, value),
            ),
            _ => None,
        }
        .ok_or(PhyTemperatureBindingError::UnsupportedAction)?;
        Ok(Self {
            outer_action: action,
            transaction: crate::calibration::cold::PhyColdI2cTransaction::new(request),
        })
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn outer_action(&self) -> PhyTemperatureAction {
        self.outer_action
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

    pub fn into_completion(self) -> Result<PhyTemperatureCompletion, PhyTemperatureBindingError> {
        let crate::calibration::cold::PhyColdI2cAction::Complete(outcome) =
            self.transaction.action()
        else {
            return Err(PhyTemperatureBindingError::IncompleteTransaction);
        };
        match (self.outer_action, outcome) {
            (
                PhyTemperatureAction::ReadMasked { field },
                crate::calibration::cold::PhyColdI2cOutcome::Read {
                    address: completed_address,
                    value,
                },
            ) if completed_address == field.address() => {
                Ok(PhyTemperatureCompletion::MaskedRead { field, value })
            }
            (
                PhyTemperatureAction::WriteMasked { field, value },
                crate::calibration::cold::PhyColdI2cOutcome::Written {
                    address: completed_address,
                },
            ) if completed_address == field.address() => {
                Ok(PhyTemperatureCompletion::MaskedWrite { field, value })
            }
            _ => Err(PhyTemperatureBindingError::UnexpectedOutcome),
        }
    }
}

/// Non-cloneable token for the single MMIO temperature-code sample.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyTemperatureSampleBinding {
    _private: (),
}

impl PhyTemperatureSampleBinding {
    pub fn new(action: PhyTemperatureAction) -> Result<Self, PhyTemperatureBindingError> {
        match action {
            PhyTemperatureAction::SampleCode => Ok(Self { _private: () }),
            _ => Err(PhyTemperatureBindingError::UnsupportedAction),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target<P: oer_esp32s31_hal::phy::temperature::PhyTemperatureSystemControl>(
        self,
        platform: &P,
    ) -> PhyTemperatureCompletion {
        PhyTemperatureCompletion::CodeSampled {
            value: oer_esp32s31_hal::phy::temperature::read_code(platform),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyTemperatureExternalBinding {
    I2c(PhyTemperatureI2cBinding),
    Sample(PhyTemperatureSampleBinding),
}

impl PhyTemperatureExternalBinding {
    pub fn lower(action: PhyTemperatureAction) -> Result<Self, PhyTemperatureBindingError> {
        if let Ok(binding) = PhyTemperatureI2cBinding::new(action) {
            return Ok(Self::I2c(binding));
        }
        if let Ok(binding) = PhyTemperatureSampleBinding::new(action) {
            return Ok(Self::Sample(binding));
        }
        Err(PhyTemperatureBindingError::UnsupportedAction)
    }
}

#[cfg(test)]
mod binding_tests;

#[cfg(test)]
mod tests;
