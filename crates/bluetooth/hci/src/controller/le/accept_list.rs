//! HCI commands of the LE filter accept list.
//!
//! The Host clears the list, adds a device and removes a device (Core Vol 4
//! Part E 7.8.15 to 7.8.17). An entry is a public (0) or random (1) device or
//! anonymous advertisements (0xFF); any other address type is an invalid
//! parameter.

use bt_hci::{
    cmd::{
        Cmd, Opcode,
        le::{
            LeAddDeviceToFilterAcceptList, LeClearFilterAcceptList,
            LeRemoveDeviceFromFilterAcceptList,
        },
    },
    event::EventKind,
    param::{Error as HciError, Status},
};

use crate::HciCommandPacket;

/// Command Complete of a list command without an H4 indicator.
pub const LE_ACCEPT_LIST_COMMAND_COMPLETE_EVENT_CAPACITY: usize = 6;

/// One device of a list command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeAcceptListDevice {
    /// The address is random rather than public.
    pub random: bool,
    /// The address, least significant octet first.
    pub address: [u8; 6],
}

/// One entry of an Add or Remove command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeAcceptListEntry {
    /// A device by its address type and address.
    Device(LeAcceptListDevice),
    /// Advertisements without an advertiser address.
    Anonymous,
}

/// A validated filter accept list command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeAcceptListCommand {
    /// LE Clear Filter Accept List.
    Clear,
    /// LE Add Device To Filter Accept List.
    Add(LeAcceptListEntry),
    /// LE Remove Device From Filter Accept List.
    Remove(LeAcceptListEntry),
}

impl LeAcceptListCommand {
    pub const fn opcode(self) -> Opcode {
        match self {
            Self::Clear => LeClearFilterAcceptList::OPCODE,
            Self::Add(_) => LeAddDeviceToFilterAcceptList::OPCODE,
            Self::Remove(_) => LeRemoveDeviceFromFilterAcceptList::OPCODE,
        }
    }

    pub(crate) fn decode(command: HciCommandPacket<'_>) -> Result<Self, LeAcceptListDecodeError> {
        let opcode = command.opcode();
        let parameters = command.parameters();
        let respond = |error: HciError| {
            LeAcceptListDecodeError::Malformed(LeAcceptListCommandCompleteEvent::new(
                opcode,
                error.to_status(),
            ))
        };
        let device = || {
            let [kind, address @ ..] = parameters else {
                return Err(respond(HciError::INVALID_HCI_PARAMETERS));
            };
            let address: [u8; 6] = address
                .try_into()
                .map_err(|_| respond(HciError::INVALID_HCI_PARAMETERS))?;
            match kind {
                0 | 1 => Ok(LeAcceptListEntry::Device(LeAcceptListDevice {
                    random: *kind == 1,
                    address,
                })),
                0xff => Ok(LeAcceptListEntry::Anonymous),
                _ => Err(respond(HciError::INVALID_HCI_PARAMETERS)),
            }
        };
        if opcode == LeClearFilterAcceptList::OPCODE {
            if parameters.is_empty() {
                Ok(Self::Clear)
            } else {
                Err(respond(HciError::INVALID_HCI_PARAMETERS))
            }
        } else if opcode == LeAddDeviceToFilterAcceptList::OPCODE {
            device().map(Self::Add)
        } else if opcode == LeRemoveDeviceFromFilterAcceptList::OPCODE {
            device().map(Self::Remove)
        } else {
            Err(LeAcceptListDecodeError::Unsupported)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LeAcceptListDecodeError {
    Unsupported,
    Malformed(LeAcceptListCommandCompleteEvent),
}

/// Owned Command Complete of one filter accept list command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeAcceptListCommandCompleteEvent {
    bytes: [u8; LE_ACCEPT_LIST_COMMAND_COMPLETE_EVENT_CAPACITY],
    opcode: Opcode,
}

impl LeAcceptListCommandCompleteEvent {
    /// The command of `opcode` completed with `status`.
    pub fn new(opcode: Opcode, status: Status) -> Self {
        let [low, high] = opcode.to_raw().to_le_bytes();
        Self {
            bytes: [
                EventKind::CommandComplete.0,
                4,
                1,
                low,
                high,
                status.into_inner(),
            ],
            opcode,
        }
    }

    /// Opcode of the completed command.
    pub const fn opcode(&self) -> Opcode {
        self.opcode
    }

    /// Complete HCI event body without an H4 packet indicator.
    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[cfg(test)]
mod tests;
