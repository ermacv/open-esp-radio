//! Bounded Command Complete encoding for the closed bootstrap subset.

use super::*;

/// Maximum complete HCI Event body emitted by the bootstrap state machine.
///
/// This includes the two-byte Event header. The largest supported response is
/// Read Local Supported Commands: six Command Complete bytes plus the standard
/// 64-octet command mask.
pub const BOOTSTRAP_COMMAND_COMPLETE_EVENT_CAPACITY: usize = 70;

/// Complete, validated Command Complete HCI Event emitted by bootstrap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootstrapCommandCompleteEvent {
    bytes: [u8; BOOTSTRAP_COMMAND_COMPLETE_EVENT_CAPACITY],
    length: usize,
    opcode: Opcode,
    status: Status,
}

impl BootstrapCommandCompleteEvent {
    /// Complete HCI Event bytes, without an H4 packet indicator.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }

    /// Opcode copied into the Command Complete event.
    pub const fn opcode(&self) -> Opcode {
        self.opcode
    }

    /// HCI status returned for the command.
    pub const fn status(&self) -> Status {
        self.status
    }

    /// Command Complete refusing a bootstrap command with `error`, as the
    /// Controller does when its role state forbids the command.
    pub fn error(opcode: Opcode, error: HciError) -> Self {
        command_error(opcode, error)
    }

    fn new(opcode: Opcode, status: Status, return_parameters: &[u8]) -> Self {
        let length = 6 + return_parameters.len();
        assert!(
            length <= BOOTSTRAP_COMMAND_COMPLETE_EVENT_CAPACITY,
            "bootstrap Command Complete exceeded its closed response profile"
        );
        let mut bytes = [0; BOOTSTRAP_COMMAND_COMPLETE_EVENT_CAPACITY];
        bytes[0] = 0x0e;
        bytes[1] = (4 + return_parameters.len()) as u8;
        bytes[2] = 1;
        bytes[3..5].copy_from_slice(&opcode.to_raw().to_le_bytes());
        bytes[5] = status.into_inner();
        bytes[6..length].copy_from_slice(return_parameters);
        Self {
            bytes,
            length,
            opcode,
            status,
        }
    }
}

pub(super) fn command_success(opcode: Opcode, parameters: &[u8]) -> BootstrapCommandCompleteEvent {
    BootstrapCommandCompleteEvent::new(opcode, Status::SUCCESS, parameters)
}

pub(super) fn command_error(opcode: Opcode, error: HciError) -> BootstrapCommandCompleteEvent {
    BootstrapCommandCompleteEvent::new(opcode, error.to_status(), &[])
}

pub(crate) fn invalid_parameters(opcode: Opcode) -> BootstrapCommandCompleteEvent {
    command_error(opcode, HciError::INVALID_HCI_PARAMETERS)
}
