//! HCI commands and event of the LE Data Length Extension.
//!
//! The Host suggests a default transmit length for new connections, sets one
//! connection's transmit length and reads the Controller's maximum lengths;
//! the Controller reports each change of a connection's effective lengths.
//! Lengths are payload octets and air time in microseconds, validated against
//! the ranges of Core Vol 4 Part E 7.8.33 and 7.8.35; the Link Layer limits
//! them to what it supports.

use bt_hci::{
    PacketKind,
    cmd::{
        Cmd, Opcode,
        le::{
            LeReadMaxDataLength, LeReadSuggestedDefaultDataLength, LeSetDataLength,
            LeWriteSuggestedDefaultDataLength,
        },
    },
    event::EventKind,
    param::{ConnHandle, Error as HciError, Status},
};

use crate::{HciCommandPacket, HciControllerResponse};

/// Longest Data Length Command Complete event without an H4 indicator.
pub const LE_DATA_LENGTH_COMMAND_COMPLETE_EVENT_CAPACITY: usize = 14;
/// LE Data Length Change event size without an H4 indicator.
pub const LE_DATA_LENGTH_CHANGE_EVENT_CAPACITY: usize = 13;

/// One direction's length as HCI carries it: payload octets and air time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeDataLengthParameters {
    pub octets: u16,
    pub time_micros: u16,
}

impl LeDataLengthParameters {
    fn decode(bytes: &[u8]) -> Option<Self> {
        let [a, b, c, d] = *bytes else {
            return None;
        };
        let octets = u16::from_le_bytes([a, b]);
        let time_micros = u16::from_le_bytes([c, d]);
        ((0x1b..=0xfb).contains(&octets) && (0x0148..=0x4290).contains(&time_micros)).then_some(
            Self {
                octets,
                time_micros,
            },
        )
    }

    fn wire(self) -> [u8; 4] {
        let [a, b] = self.octets.to_le_bytes();
        let [c, d] = self.time_micros.to_le_bytes();
        [a, b, c, d]
    }
}

/// A validated Data Length Extension command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeDataLengthCommand {
    /// LE Set Data Length for one connection.
    Set {
        handle: ConnHandle,
        transmit: LeDataLengthParameters,
    },
    /// LE Read Suggested Default Data Length.
    ReadSuggestedDefault,
    /// LE Write Suggested Default Data Length for later connections.
    WriteSuggestedDefault(LeDataLengthParameters),
    /// LE Read Maximum Data Length.
    ReadMaximum,
}

impl LeDataLengthCommand {
    pub const fn opcode(self) -> Opcode {
        match self {
            Self::Set { .. } => LeSetDataLength::OPCODE,
            Self::ReadSuggestedDefault => LeReadSuggestedDefaultDataLength::OPCODE,
            Self::WriteSuggestedDefault(_) => LeWriteSuggestedDefaultDataLength::OPCODE,
            Self::ReadMaximum => LeReadMaxDataLength::OPCODE,
        }
    }

    pub(crate) fn decode(command: HciCommandPacket<'_>) -> Result<Self, LeDataLengthDecodeError> {
        let opcode = command.opcode();
        let parameters = command.parameters();
        let malformed = || {
            LeDataLengthDecodeError::Malformed(
                LeDataLengthCommandCompleteEvent::invalid_parameters(opcode),
            )
        };
        if opcode == LeSetDataLength::OPCODE {
            let [low, high, rest @ ..] = parameters else {
                return Err(malformed());
            };
            let handle = u16::from_le_bytes([*low, *high]);
            if handle > 0x0eff {
                return Err(malformed());
            }
            let transmit = LeDataLengthParameters::decode(rest).ok_or_else(malformed)?;
            Ok(Self::Set {
                handle: ConnHandle::new(handle),
                transmit,
            })
        } else if opcode == LeReadSuggestedDefaultDataLength::OPCODE {
            parameters
                .is_empty()
                .then_some(Self::ReadSuggestedDefault)
                .ok_or_else(malformed)
        } else if opcode == LeWriteSuggestedDefaultDataLength::OPCODE {
            LeDataLengthParameters::decode(parameters)
                .map(Self::WriteSuggestedDefault)
                .ok_or_else(malformed)
        } else if opcode == LeReadMaxDataLength::OPCODE {
            parameters
                .is_empty()
                .then_some(Self::ReadMaximum)
                .ok_or_else(malformed)
        } else {
            Err(LeDataLengthDecodeError::Unsupported)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LeDataLengthDecodeError {
    Unsupported,
    Malformed(LeDataLengthCommandCompleteEvent),
}

/// Owned Command Complete of one Data Length Extension command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeDataLengthCommandCompleteEvent {
    bytes: [u8; LE_DATA_LENGTH_COMMAND_COMPLETE_EVENT_CAPACITY],
    length: u8,
    opcode: Opcode,
}

impl LeDataLengthCommandCompleteEvent {
    fn new(opcode: Opcode, status: Status, returned: &[u8]) -> Self {
        let mut bytes = [0; LE_DATA_LENGTH_COMMAND_COMPLETE_EVENT_CAPACITY];
        let [low, high] = opcode.to_raw().to_le_bytes();
        let length = 6 + returned.len();
        bytes[0] = EventKind::CommandComplete.0;
        bytes[1] = (length - 2) as u8;
        bytes[2] = 1;
        bytes[3] = low;
        bytes[4] = high;
        bytes[5] = status.into_inner();
        bytes[6..length].copy_from_slice(returned);
        Self {
            bytes,
            length: length as u8,
            opcode,
        }
    }

    fn invalid_parameters(opcode: Opcode) -> Self {
        let status = HciError::INVALID_HCI_PARAMETERS.to_status();
        if opcode == LeSetDataLength::OPCODE {
            Self::set(status, ConnHandle::new(0))
        } else if opcode == LeReadSuggestedDefaultDataLength::OPCODE {
            Self::new(opcode, status, &[0; 4])
        } else if opcode == LeReadMaxDataLength::OPCODE {
            Self::new(opcode, status, &[0; 8])
        } else {
            Self::new(opcode, status, &[])
        }
    }

    /// LE Set Data Length completed with `status` for `handle`.
    pub fn set(status: Status, handle: ConnHandle) -> Self {
        Self::new(LeSetDataLength::OPCODE, status, &handle.raw().to_le_bytes())
    }

    /// The suggested default transmit length.
    pub fn suggested_default(transmit: LeDataLengthParameters) -> Self {
        Self::new(
            LeReadSuggestedDefaultDataLength::OPCODE,
            Status::SUCCESS,
            &transmit.wire(),
        )
    }

    /// LE Write Suggested Default Data Length completed with `status`.
    pub fn suggested_default_written(status: Status) -> Self {
        Self::new(LeWriteSuggestedDefaultDataLength::OPCODE, status, &[])
    }

    /// The longest supported transmit and receive lengths.
    pub fn maximum(transmit: LeDataLengthParameters, receive: LeDataLengthParameters) -> Self {
        let mut returned = [0; 8];
        returned[..4].copy_from_slice(&transmit.wire());
        returned[4..].copy_from_slice(&receive.wire());
        Self::new(LeReadMaxDataLength::OPCODE, Status::SUCCESS, &returned)
    }

    pub const fn opcode(&self) -> Opcode {
        self.opcode
    }

    /// Complete HCI Event body without an H4 indicator.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }
}

impl HciControllerResponse for LeDataLengthCommandCompleteEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// LE Data Length Change: a connection's new effective lengths.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeDataLengthChangeEvent {
    bytes: [u8; LE_DATA_LENGTH_CHANGE_EVENT_CAPACITY],
}

impl LeDataLengthChangeEvent {
    pub fn new(
        handle: ConnHandle,
        transmit: LeDataLengthParameters,
        receive: LeDataLengthParameters,
    ) -> Self {
        let mut bytes = [0; LE_DATA_LENGTH_CHANGE_EVENT_CAPACITY];
        bytes[0] = EventKind::Le.0;
        bytes[1] = (LE_DATA_LENGTH_CHANGE_EVENT_CAPACITY - 2) as u8;
        bytes[2] = 0x07;
        bytes[3..5].copy_from_slice(&handle.raw().to_le_bytes());
        bytes[5..9].copy_from_slice(&transmit.wire());
        bytes[9..13].copy_from_slice(&receive.wire());
        Self { bytes }
    }

    /// Complete HCI Event body without an H4 indicator.
    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl HciControllerResponse for LeDataLengthChangeEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

#[cfg(test)]
mod tests;
