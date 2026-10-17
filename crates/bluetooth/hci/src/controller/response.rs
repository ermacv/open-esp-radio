//! Complete Controller responses retained by an operational HCI runner.

use bt_hci::{
    PacketKind,
    cmd::Opcode,
    param::{Error as HciError, Status},
};

use crate::{
    BootstrapCommandCompleteEvent, LeDisconnectCommandStatusEvent, LeDtmCommandCompleteEvent,
    LeHostCompletedPacketsErrorEvent, LeLegacyAdvertisingCommandCompleteEvent,
    LeLegacyScanningCommandCompleteEvent, LeLongTermKeyCommandCompleteEvent,
    LeReadRemoteFeaturesCommandStatusEvent, LeReadRemoteVersionInformationCommandStatusEvent,
};

const UNKNOWN_COMMAND_COMPLETE_EVENT_CAPACITY: usize = 6;

/// Owned `Unknown HCI Command` completion for one unclaimed opcode.
///
/// The portable Controller classifier terminates its closed command table with
/// this response rather than returning a packet which borrows the receive
/// scratch buffer. A runner may therefore clear or reuse that buffer while the
/// response remains retained across Controller-to-Host backpressure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnknownCommandCompleteEvent {
    bytes: [u8; UNKNOWN_COMMAND_COMPLETE_EVENT_CAPACITY],
    opcode: Opcode,
    status: Status,
}

impl UnknownCommandCompleteEvent {
    pub(crate) fn new(opcode: Opcode) -> Self {
        let status = HciError::UNKNOWN_CMD.to_status();
        let opcode_bytes = opcode.to_raw().to_le_bytes();
        Self {
            bytes: [
                0x0e,
                0x04,
                0x01,
                opcode_bytes[0],
                opcode_bytes[1],
                status.into_inner(),
            ],
            opcode,
            status,
        }
    }

    /// Complete HCI Event body without an H4 packet indicator.
    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Opcode rejected by the closed Controller command table.
    pub const fn opcode(&self) -> Opcode {
        self.opcode
    }

    /// Required `Unknown HCI Command` status.
    pub const fn status(&self) -> Status {
        self.status
    }
}

impl HciControllerResponse for UnknownCommandCompleteEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// One complete Controller packet ready for publication toward the Host.
///
/// The trait is intentionally independent of command dispatch. A hardware
/// runner may retain an accepted command through arbitrary affine radio states,
/// build its response only at the proven completion boundary, and retry this
/// immutable packet across bounded output backpressure.
pub trait HciControllerResponse {
    /// HCI packet class published toward the Host.
    fn kind(&self) -> PacketKind;

    /// Complete packet body without an H4 indicator.
    fn as_bytes(&self) -> &[u8];
}

impl HciControllerResponse for BootstrapCommandCompleteEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// Closed ordered command-response set for the initial LE Controller.
///
/// Bootstrap, DTM, Link Layer role, Disconnect status, and terminal responses
/// have different storage types but share the same publication boundary.
/// Keeping the distinction typed avoids copying a response into an unvalidated
/// byte scratch buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeControllerCommandComplete {
    /// Standard LE Rand completion, with redacted Debug output.
    Random(crate::LeRandCommandCompleteEvent),
    /// Immediate status for an admitted or rejected Disconnect command.
    DisconnectStatus(LeDisconnectCommandStatusEvent),
    /// Immediate status for an admitted or rejected remote-feature request.
    ReadRemoteFeaturesStatus(LeReadRemoteFeaturesCommandStatusEvent),
    /// Immediate status for an admitted or rejected remote-version request.
    ReadRemoteVersionInformationStatus(LeReadRemoteVersionInformationCommandStatusEvent),
    /// Completion for a positive, negative, rejected, or malformed LTK reply.
    LongTermKeyReply(LeLongTermKeyCommandCompleteEvent),
    /// Completion of a Data Length Extension command.
    DataLength(crate::LeDataLengthCommandCompleteEvent),
    /// Pure software bootstrap command completion.
    Bootstrap(BootstrapCommandCompleteEvent),
    /// Exceptional invalid-parameters response for Host completed-packet credits.
    HostCompletedPacketsError(LeHostCompletedPacketsErrorEvent),
    /// Completion supplied by the hardware-owned DTM session.
    Dtm(LeDtmCommandCompleteEvent),
    /// Completion for accepted or rejected advertising configuration.
    LegacyAdvertising(LeLegacyAdvertisingCommandCompleteEvent),
    /// Completion for accepted or rejected passive scanning configuration.
    LegacyScanning(LeLegacyScanningCommandCompleteEvent),
    /// Terminal response for an opcode outside the closed command table.
    UnknownCommand(UnknownCommandCompleteEvent),
}

impl HciControllerResponse for LeControllerCommandComplete {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Random(response) => response.as_bytes(),
            Self::DisconnectStatus(response) => response.as_bytes(),
            Self::ReadRemoteFeaturesStatus(response) => response.as_bytes(),
            Self::ReadRemoteVersionInformationStatus(response) => response.as_bytes(),
            Self::LongTermKeyReply(response) => response.as_bytes(),
            Self::DataLength(response) => response.as_bytes(),
            Self::Bootstrap(response) => response.as_bytes(),
            Self::HostCompletedPacketsError(response) => response.as_bytes(),
            Self::Dtm(response) => response.as_bytes(),
            Self::LegacyAdvertising(response) => response.as_bytes(),
            Self::LegacyScanning(response) => response.as_bytes(),
            Self::UnknownCommand(response) => response.as_bytes(),
        }
    }
}

impl From<BootstrapCommandCompleteEvent> for LeControllerCommandComplete {
    fn from(response: BootstrapCommandCompleteEvent) -> Self {
        Self::Bootstrap(response)
    }
}

impl From<crate::LeRandCommandCompleteEvent> for LeControllerCommandComplete {
    fn from(response: crate::LeRandCommandCompleteEvent) -> Self {
        Self::Random(response)
    }
}

impl From<LeDisconnectCommandStatusEvent> for LeControllerCommandComplete {
    fn from(response: LeDisconnectCommandStatusEvent) -> Self {
        Self::DisconnectStatus(response)
    }
}

impl From<LeReadRemoteFeaturesCommandStatusEvent> for LeControllerCommandComplete {
    fn from(response: LeReadRemoteFeaturesCommandStatusEvent) -> Self {
        Self::ReadRemoteFeaturesStatus(response)
    }
}

impl From<LeReadRemoteVersionInformationCommandStatusEvent> for LeControllerCommandComplete {
    fn from(response: LeReadRemoteVersionInformationCommandStatusEvent) -> Self {
        Self::ReadRemoteVersionInformationStatus(response)
    }
}

impl From<LeLongTermKeyCommandCompleteEvent> for LeControllerCommandComplete {
    fn from(response: LeLongTermKeyCommandCompleteEvent) -> Self {
        Self::LongTermKeyReply(response)
    }
}

impl From<crate::LeDataLengthCommandCompleteEvent> for LeControllerCommandComplete {
    fn from(response: crate::LeDataLengthCommandCompleteEvent) -> Self {
        Self::DataLength(response)
    }
}

impl From<LeHostCompletedPacketsErrorEvent> for LeControllerCommandComplete {
    fn from(response: LeHostCompletedPacketsErrorEvent) -> Self {
        Self::HostCompletedPacketsError(response)
    }
}

impl From<LeDtmCommandCompleteEvent> for LeControllerCommandComplete {
    fn from(response: LeDtmCommandCompleteEvent) -> Self {
        Self::Dtm(response)
    }
}

impl From<LeLegacyAdvertisingCommandCompleteEvent> for LeControllerCommandComplete {
    fn from(response: LeLegacyAdvertisingCommandCompleteEvent) -> Self {
        Self::LegacyAdvertising(response)
    }
}

impl From<LeLegacyScanningCommandCompleteEvent> for LeControllerCommandComplete {
    fn from(response: LeLegacyScanningCommandCompleteEvent) -> Self {
        Self::LegacyScanning(response)
    }
}

impl From<UnknownCommandCompleteEvent> for LeControllerCommandComplete {
    fn from(response: UnknownCommandCompleteEvent) -> Self {
        Self::UnknownCommand(response)
    }
}
