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
//!
//! Runtime DCODE advances through the P1 engine at complete byte commands.
//! A masked RMW retains its binding between the read and write; each byte
//! keeps the direct observation settle independently of executor suspension.
//! RF and bus ownership remain held across checkpoints, and the parent
//! commits only after accepting the terminal child completion.

use crate::analog::{
    i2c::{Field, analog_i2c},
    rfpll::{
        RfpllFrequencyAction, RfpllFrequencyCompletion, RfpllFrequencyFailure,
        RfpllFrequencyRequest, RfpllFrequencyTransition,
    },
};
use crate::ordinal::Ordinal;

const CHANNELS: [u8; 4] = [1, 5, 10, 14];

type Channel = Ordinal<{ CHANNELS.len() }>;

const CKGEN_WRITES: [(Field, u8); 4] = [
    (
        analog_i2c::rfpll::dcode_0_source_select::DCODE_0_SOURCE_SELECT,
        0,
    ),
    (
        analog_i2c::rfpll::dcode_1_source_select::DCODE_1_SOURCE_SELECT,
        0,
    ),
    (analog_i2c::rfpll::dcode_ckgen_reset::DCODE_CKGEN_RESET, 0),
    (analog_i2c::rfpll::dcode_ckgen_reset::DCODE_CKGEN_RESET, 1),
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
    WriteMasked { field: Field, value: u8 },
    ReadMasked { field: Field },
    Complete(PhyDcodeOutcome),
    Failed(PhyDcodeFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcodeCompletion {
    Rfpll(RfpllFrequencyCompletion),
    MaskedWrite { field: Field, value: u8 },
    MaskedRead { field: Field, value: u8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcodeTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyDcodeStep {
    Rfpll {
        calibration_index: Channel,
        transition: RfpllFrequencyTransition,
    },
    ResetCkgen {
        calibration_index: Channel,
        write_index: Ordinal<{ CKGEN_WRITES.len() }>,
    },
    ReadLow {
        calibration_index: Channel,
    },
    ReadHigh {
        calibration_index: Channel,
    },
    Complete(PhyDcodeOutcome),
    Failed(PhyDcodeFailure),
}

const fn rfpll_transition(
    calibration_index: Channel,
    crystal_selector: u8,
) -> RfpllFrequencyTransition {
    RfpllFrequencyTransition::channel(RfpllFrequencyRequest {
        crystal_selector,
        frequency_code: calibration_index.at(&CHANNELS) as u16,
        offset: 0,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyDcodeTransition {
    parameters: PhyDcodeParameters,
    /// The low and high code of each channel.
    codes: [[u8; 2]; CHANNELS.len()],
    step: PhyDcodeStep,
}

impl PhyDcodeTransition {
    pub const fn new(parameters: PhyDcodeParameters) -> Self {
        Self {
            parameters,
            codes: [[0; 2]; CHANNELS.len()],
            step: PhyDcodeStep::Rfpll {
                calibration_index: Ordinal::FIRST,
                transition: rfpll_transition(Ordinal::FIRST, parameters.crystal_selector),
            },
        }
    }

    pub const fn action(&self) -> PhyDcodeAction {
        match &self.step {
            PhyDcodeStep::Rfpll { transition, .. } => PhyDcodeAction::Rfpll(transition.action()),
            PhyDcodeStep::ResetCkgen { write_index, .. } => {
                let (field, value) = write_index.at(&CKGEN_WRITES);
                PhyDcodeAction::WriteMasked { field, value }
            }
            PhyDcodeStep::ReadLow { .. } => PhyDcodeAction::ReadMasked {
                field: analog_i2c::rfpll::internal_dcode_0::INTERNAL_DCODE_0,
            },
            PhyDcodeStep::ReadHigh { .. } => PhyDcodeAction::ReadMasked {
                field: analog_i2c::rfpll::internal_dcode_1::INTERNAL_DCODE_1,
            },
            PhyDcodeStep::Complete(outcome) => PhyDcodeAction::Complete(*outcome),
            PhyDcodeStep::Failed(failure) => PhyDcodeAction::Failed(*failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyDcodeCompletion,
    ) -> Result<(), PhyDcodeTransitionError> {
        // A rejected RFPLL completion leaves its child unchanged. Borrow the
        // live child rather than copying the largest step into this frame.
        match (&mut self.step, completion) {
            (
                PhyDcodeStep::Rfpll {
                    calibration_index,
                    transition,
                },
                PhyDcodeCompletion::Rfpll(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyDcodeTransitionError::WrongCompletion)?;
                match transition.action() {
                    RfpllFrequencyAction::Complete(_) => {
                        self.step = PhyDcodeStep::ResetCkgen {
                            calibration_index: *calibration_index,
                            write_index: Ordinal::FIRST,
                        };
                    }
                    RfpllFrequencyAction::Failed(failure) => {
                        self.step = PhyDcodeStep::Failed(PhyDcodeFailure::Rfpll {
                            calibration_index: calibration_index.get(),
                            failure,
                        });
                    }
                    _ => {}
                }
            }
            (
                PhyDcodeStep::ResetCkgen {
                    calibration_index,
                    write_index,
                },
                PhyDcodeCompletion::MaskedWrite { field, value },
            ) => {
                let (expected_field, expected_value) = write_index.at(&CKGEN_WRITES);
                if field != expected_field || value != expected_value {
                    return Err(PhyDcodeTransitionError::WrongCompletion);
                }
                match write_index.next() {
                    Some(next) => *write_index = next,
                    None => {
                        self.step = PhyDcodeStep::ReadLow {
                            calibration_index: *calibration_index,
                        };
                    }
                }
            }
            (
                PhyDcodeStep::ReadLow { calibration_index },
                PhyDcodeCompletion::MaskedRead {
                    field: analog_i2c::rfpll::internal_dcode_0::INTERNAL_DCODE_0,
                    value,
                },
            ) if value <= 0x3f => {
                let [low, _] = calibration_index.slot(&mut self.codes);
                *low = value;
                self.step = PhyDcodeStep::ReadHigh {
                    calibration_index: *calibration_index,
                };
            }
            (
                PhyDcodeStep::ReadHigh { calibration_index },
                PhyDcodeCompletion::MaskedRead {
                    field: analog_i2c::rfpll::internal_dcode_1::INTERNAL_DCODE_1,
                    value,
                },
            ) if value <= 0x3f => {
                let [_, high] = calibration_index.slot(&mut self.codes);
                *high = value;
                let Some(next) = calibration_index.next() else {
                    let [[a, b], [c, d], [e, f], [g, h]] = self.codes;
                    self.step = PhyDcodeStep::Complete(PhyDcodeOutcome {
                        codes: [a, b, c, d, e, f, g, h],
                    });
                    return Ok(());
                };
                self.step = PhyDcodeStep::Rfpll {
                    calibration_index: next,
                    transition: rfpll_transition(next, self.parameters.crystal_selector),
                };
            }
            (PhyDcodeStep::Complete(_), _) | (PhyDcodeStep::Failed(_), _) => {
                return Err(PhyDcodeTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyDcodeTransitionError::WrongCompletion),
        }
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
    pub(crate) fn transaction_mut(
        &mut self,
    ) -> &mut crate::calibration::cold::PhyColdI2cTransaction {
        &mut self.transaction
    }
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
        result: Result<u8, crate::serial::PhyI2cError>,
    ) -> Result<
        crate::calibration::cold::PhyColdI2cObservation,
        crate::calibration::cold::PhyColdI2cError,
    > {
        self.transaction.observe_read_result(result)
    }

    pub fn observe_write_result(
        &mut self,
        result: Result<(), crate::serial::PhyI2cError>,
    ) -> Result<
        crate::calibration::cold::PhyColdI2cObservation,
        crate::calibration::cold::PhyColdI2cError,
    > {
        self.transaction.observe_write_result(result)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target<P: oer_chips_esp32s31_hal::owner::SharedPhyAccess>(
        &mut self,
        platform: &mut P,
    ) -> Result<(), crate::calibration::cold::PhyColdI2cError> {
        self.transaction.start_target(platform)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge<P: oer_chips_esp32s31_hal::owner::SharedPhyAccess>(
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
