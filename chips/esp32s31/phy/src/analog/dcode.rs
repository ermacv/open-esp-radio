//! Rust-owned ESP32-S31 PHY D-code calibration.
//!
//! The required root is ROM `phy_dcode_cal_init` at `0x2f82_b8da`, size
//! 128 bytes. It visits Wi-Fi channels 1, 5, 10 and 14 through the frequency-table
//! switch path (including its NRX update), resets four CKGEN
//! fields through PHY-I2C, reads two six-bit D-code values, and stores the
//! resulting eight bytes through the global `phy_param` pointer.
//!
//! The nested RFPLL work reuses [`crate::analog::rfpll`]. Every remaining I2C and
//! MMIO operation is an identity-bound action. The four-byte ROM table and
//! eight output bytes are Rust-owned values; no ROM RAM or callback table is
//! required.

use crate::analog::{
    i2c::{PhyI2cField, analog_registers},
    rfpll::{
        RfpllFrequencyAction, RfpllFrequencyCompletion, RfpllFrequencyFailure,
        RfpllFrequencyRequest, RfpllFrequencyTransition,
    },
};

const CHANNELS: [u8; 4] = [1, 5, 10, 14];

const CKGEN_WRITES: [(PhyI2cField, u8); 4] = [
    (analog_registers::RFPLL_DCODE_0_SOURCE_SELECT, 0),
    (analog_registers::RFPLL_DCODE_1_SOURCE_SELECT, 0),
    (analog_registers::RFPLL_DCODE_CKGEN_RESET, 0),
    (analog_registers::RFPLL_DCODE_CKGEN_RESET, 1),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyDcodeParameters {
    pub crystal_selector: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyDcodeOutcome {
    pub codes: [u8; 8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcodeFailure {
    Rfpll {
        calibration_index: u8,
        failure: RfpllFrequencyFailure,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcodeAction {
    Rfpll(RfpllFrequencyAction),
    WriteMasked { field: PhyI2cField, value: u8 },
    ReadMasked { field: PhyI2cField },
    Complete(PhyDcodeOutcome),
    Failed(PhyDcodeFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcodeCompletion {
    Rfpll(RfpllFrequencyCompletion),
    MaskedWrite { field: PhyI2cField, value: u8 },
    MaskedRead { field: PhyI2cField, value: u8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcodeTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyDcodeStep {
    Rfpll {
        calibration_index: u8,
        transition: RfpllFrequencyTransition,
    },
    ResetCkgen {
        calibration_index: u8,
        write_index: u8,
    },
    ReadLow {
        calibration_index: u8,
    },
    ReadHigh {
        calibration_index: u8,
    },
    Complete(PhyDcodeOutcome),
    Failed(PhyDcodeFailure),
}

const fn rfpll_transition(calibration_index: u8, crystal_selector: u8) -> RfpllFrequencyTransition {
    RfpllFrequencyTransition::channel(RfpllFrequencyRequest {
        crystal_selector,
        frequency_code: CHANNELS[calibration_index as usize] as u16,
        offset: 0,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyDcodeTransition {
    parameters: PhyDcodeParameters,
    codes: [u8; 8],
    step: PhyDcodeStep,
}

impl PhyDcodeTransition {
    pub const fn new(parameters: PhyDcodeParameters) -> Self {
        Self {
            parameters,
            codes: [0; 8],
            step: PhyDcodeStep::Rfpll {
                calibration_index: 0,
                transition: rfpll_transition(0, parameters.crystal_selector),
            },
        }
    }

    pub const fn action(self) -> PhyDcodeAction {
        match self.step {
            PhyDcodeStep::Rfpll { transition, .. } => PhyDcodeAction::Rfpll(transition.action()),
            PhyDcodeStep::ResetCkgen { write_index, .. } => {
                let (field, value) = CKGEN_WRITES[write_index as usize];
                PhyDcodeAction::WriteMasked { field, value }
            }
            PhyDcodeStep::ReadLow { .. } => PhyDcodeAction::ReadMasked {
                field: analog_registers::RFPLL_INTERNAL_DCODE_0,
            },
            PhyDcodeStep::ReadHigh { .. } => PhyDcodeAction::ReadMasked {
                field: analog_registers::RFPLL_INTERNAL_DCODE_1,
            },
            PhyDcodeStep::Complete(outcome) => PhyDcodeAction::Complete(outcome),
            PhyDcodeStep::Failed(failure) => PhyDcodeAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyDcodeCompletion,
    ) -> Result<(), PhyDcodeTransitionError> {
        self.step = match (self.step, completion) {
            (
                PhyDcodeStep::Rfpll {
                    calibration_index,
                    mut transition,
                },
                PhyDcodeCompletion::Rfpll(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyDcodeTransitionError::WrongCompletion)?;
                match transition.action() {
                    RfpllFrequencyAction::Complete(_) => PhyDcodeStep::ResetCkgen {
                        calibration_index,
                        write_index: 0,
                    },
                    RfpllFrequencyAction::Failed(failure) => {
                        PhyDcodeStep::Failed(PhyDcodeFailure::Rfpll {
                            calibration_index,
                            failure,
                        })
                    }
                    _ => PhyDcodeStep::Rfpll {
                        calibration_index,
                        transition,
                    },
                }
            }
            (
                PhyDcodeStep::ResetCkgen {
                    calibration_index,
                    write_index,
                },
                PhyDcodeCompletion::MaskedWrite { field, value },
            ) => {
                let (expected_field, expected_value) = CKGEN_WRITES[write_index as usize];
                if field != expected_field || value != expected_value {
                    return Err(PhyDcodeTransitionError::WrongCompletion);
                }
                if write_index + 1 == CKGEN_WRITES.len() as u8 {
                    PhyDcodeStep::ReadLow { calibration_index }
                } else {
                    PhyDcodeStep::ResetCkgen {
                        calibration_index,
                        write_index: write_index + 1,
                    }
                }
            }
            (
                PhyDcodeStep::ReadLow { calibration_index },
                PhyDcodeCompletion::MaskedRead {
                    field: analog_registers::RFPLL_INTERNAL_DCODE_0,
                    value,
                },
            ) if value <= 0x3f => {
                self.codes[calibration_index as usize * 2] = value;
                PhyDcodeStep::ReadHigh { calibration_index }
            }
            (
                PhyDcodeStep::ReadHigh { calibration_index },
                PhyDcodeCompletion::MaskedRead {
                    field: analog_registers::RFPLL_INTERNAL_DCODE_1,
                    value,
                },
            ) if value <= 0x3f => {
                self.codes[calibration_index as usize * 2 + 1] = value;
                if calibration_index + 1 == CHANNELS.len() as u8 {
                    PhyDcodeStep::Complete(PhyDcodeOutcome { codes: self.codes })
                } else {
                    let next = calibration_index + 1;
                    PhyDcodeStep::Rfpll {
                        calibration_index: next,
                        transition: rfpll_transition(next, self.parameters.crystal_selector),
                    }
                }
            }
            (PhyDcodeStep::Complete(_), _) | (PhyDcodeStep::Failed(_), _) => {
                return Err(PhyDcodeTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyDcodeTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcodeBindingError {
    UnsupportedAction,
    IncompleteTransaction,
    UnexpectedOutcome,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyDcodeI2cBinding {
    outer_action: PhyDcodeAction,
    transaction: crate::calibration::cold::PhyColdI2cTransaction,
}

impl PhyDcodeI2cBinding {
    pub fn new(action: PhyDcodeAction) -> Result<Self, PhyDcodeBindingError> {
        let request = match action {
            PhyDcodeAction::WriteMasked { field, value } => {
                crate::calibration::cold::PhyColdI2cRequest::write_field(field, value)
            }
            PhyDcodeAction::ReadMasked { field } => {
                crate::calibration::cold::PhyColdI2cRequest::read_field(field)
            }
            _ => return Err(PhyDcodeBindingError::UnsupportedAction),
        };
        Ok(Self {
            outer_action: action,
            transaction: crate::calibration::cold::PhyColdI2cTransaction::new(request),
        })
    }

    pub const fn action(&self) -> crate::calibration::cold::PhyColdI2cAction {
        self.transaction.action()
    }

    pub fn read_started(&mut self) -> Result<(), crate::calibration::cold::PhyColdI2cError> {
        self.transaction.read_started()
    }

    pub fn write_started(&mut self) -> Result<(), crate::calibration::cold::PhyColdI2cError> {
        self.transaction.write_started()
    }

    pub fn observe_read_result(
        &mut self,
        result: Result<u8, crate::analog::i2c::PhyI2cError>,
    ) -> Result<
        crate::calibration::cold::PhyColdI2cObservation,
        crate::calibration::cold::PhyColdI2cError,
    > {
        self.transaction.observe_read_result(result)
    }

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

    pub fn into_completion(self) -> Result<PhyDcodeCompletion, PhyDcodeBindingError> {
        let crate::calibration::cold::PhyColdI2cAction::Complete(outcome) =
            self.transaction.action()
        else {
            return Err(PhyDcodeBindingError::IncompleteTransaction);
        };
        match (self.outer_action, outcome) {
            (
                PhyDcodeAction::WriteMasked { field, value },
                crate::calibration::cold::PhyColdI2cOutcome::Written { address: completed },
            ) if completed == field.address() => {
                Ok(PhyDcodeCompletion::MaskedWrite { field, value })
            }
            (
                PhyDcodeAction::ReadMasked { field },
                crate::calibration::cold::PhyColdI2cOutcome::Read {
                    address: completed,
                    value,
                },
            ) if completed == field.address() => {
                Ok(PhyDcodeCompletion::MaskedRead { field, value })
            }
            _ => Err(PhyDcodeBindingError::UnexpectedOutcome),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyDcodeExternalBinding {
    Rfpll(crate::analog::rfpll::RfpllFrequencyExternalBinding),
    I2c(PhyDcodeI2cBinding),
}

impl PhyDcodeExternalBinding {
    pub fn lower(action: PhyDcodeAction) -> Result<Self, PhyDcodeBindingError> {
        match action {
            PhyDcodeAction::Rfpll(action) => {
                crate::analog::rfpll::RfpllFrequencyExternalBinding::lower(action)
                    .map(Self::Rfpll)
                    .map_err(|_| PhyDcodeBindingError::UnsupportedAction)
            }
            PhyDcodeAction::WriteMasked { .. } | PhyDcodeAction::ReadMasked { .. } => {
                PhyDcodeI2cBinding::new(action).map(Self::I2c)
            }
            PhyDcodeAction::Complete(_) | PhyDcodeAction::Failed(_) => {
                Err(PhyDcodeBindingError::UnsupportedAction)
            }
        }
    }
}

#[cfg(test)]
mod tests;
