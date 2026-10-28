//! Bounded Command Complete encoding for the closed bootstrap subset.

use super::*;
use crate::encoding::Length;

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
    length: Length<usize, BOOTSTRAP_COMMAND_COMPLETE_EVENT_CAPACITY>,
    opcode: Opcode,
    status: Status,
}

impl BootstrapCommandCompleteEvent {
    /// Complete HCI Event bytes, without an H4 packet indicator.
    pub fn as_bytes(&self) -> &[u8] {
        self.length.borrow(&self.bytes)
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

    fn new<const N: usize>(opcode: Opcode, status: Status, return_parameters: &[u8; N]) -> Self {
        let length =
            Length::<usize, BOOTSTRAP_COMMAND_COMPLETE_EVENT_CAPACITY>::with_header::<6, N>();
        let mut bytes = [0; BOOTSTRAP_COMMAND_COMPLETE_EVENT_CAPACITY];
        bytes[0] = 0x0e;
        bytes[1] = const { (4 + N) as u8 };
        bytes[2] = 1;
        let [low, high] = opcode.to_raw().to_le_bytes();
        bytes[3] = low;
        bytes[4] = high;
        bytes[5] = status.into_inner();
        // The const length proof admits every returned octet into this tail.
        for (destination, source) in bytes.iter_mut().skip(6).zip(return_parameters) {
            *destination = *source;
        }
        Self {
            bytes,
            length,
            opcode,
            status,
        }
    }
}

pub(super) fn command_success<const N: usize>(
    opcode: Opcode,
    parameters: &[u8; N],
) -> BootstrapCommandCompleteEvent {
    BootstrapCommandCompleteEvent::new(opcode, Status::SUCCESS, parameters)
}

pub(super) fn command_error(opcode: Opcode, error: HciError) -> BootstrapCommandCompleteEvent {
    BootstrapCommandCompleteEvent::new(opcode, error.to_status(), &[])
}

pub(crate) fn invalid_parameters(opcode: Opcode) -> BootstrapCommandCompleteEvent {
    command_error(opcode, HciError::INVALID_HCI_PARAMETERS)
}
