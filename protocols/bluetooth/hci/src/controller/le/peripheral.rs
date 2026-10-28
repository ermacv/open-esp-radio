//! Owned standard HCI events for an established LE peripheral connection.
//!
//! The Link Layer and chip runtime remain responsible for proving connection
//! establishment and teardown. This module only owns their Host-visible HCI
//! representation so the exact event can survive bounded output backpressure.

use bt_hci::{
    PacketKind,
    cmd::{
        Cmd, Opcode,
        le::{LeLongTermKeyRequestNegativeReply, LeLongTermKeyRequestReply, LeReadRemoteFeatures},
        link_control::{Disconnect, ReadRemoteVersionInformation},
    },
    event::{
        EventKind,
        le::{LeConnectionComplete, LeConnectionUpdateComplete, LeEventKind, LeEventParams},
    },
    param::{AddrKind, BdAddr, ClockAccuracy, ConnHandle, Duration, LeConnRole, Status},
};

use crate::{HciCommandPacket, HciControllerResponse};

/// Complete LE Connection Complete event size without an H4 indicator.
pub const LE_PERIPHERAL_CONNECTION_COMPLETE_EVENT_CAPACITY: usize = 21;
/// Complete Disconnection Complete event size without an H4 indicator.
pub const LE_DISCONNECTION_COMPLETE_EVENT_CAPACITY: usize = 6;
/// Complete Disconnect Command Status event size without an H4 indicator.
pub const LE_DISCONNECT_COMMAND_STATUS_EVENT_CAPACITY: usize = 6;
/// Complete LE Connection Update Complete event size without an H4 indicator.
pub const LE_CONNECTION_UPDATE_COMPLETE_EVENT_CAPACITY: usize = 12;
/// Complete LE Read Remote Features Complete event size without an H4 indicator.
pub const LE_READ_REMOTE_FEATURES_COMPLETE_EVENT_CAPACITY: usize = 14;
/// Complete LE Read Remote Features Command Status event size without an H4 indicator.
pub const LE_READ_REMOTE_FEATURES_COMMAND_STATUS_EVENT_CAPACITY: usize = 6;
/// Complete Read Remote Version Information Complete event size without an H4 indicator.
pub const LE_READ_REMOTE_VERSION_INFORMATION_COMPLETE_EVENT_CAPACITY: usize = 10;
/// Complete Read Remote Version Information Command Status event size without an H4 indicator.
pub const LE_READ_REMOTE_VERSION_INFORMATION_COMMAND_STATUS_EVENT_CAPACITY: usize = 6;
/// Complete LE Long Term Key Request event size without an H4 indicator.
pub const LE_LONG_TERM_KEY_REQUEST_EVENT_CAPACITY: usize = 15;
/// Complete LE Long Term Key Reply/Negative Reply Command Complete event size.
pub const LE_LONG_TERM_KEY_COMMAND_COMPLETE_EVENT_CAPACITY: usize = 8;
/// Complete Encryption Change v1 event size without an H4 indicator.
pub const LE_ENCRYPTION_CHANGE_EVENT_CAPACITY: usize = 6;
/// Complete Encryption Key Refresh Complete event size without an H4 indicator.
pub const LE_ENCRYPTION_KEY_REFRESH_COMPLETE_EVENT_CAPACITY: usize = 5;

/// Host-provided LTK for the Peripheral connection the handle names.
pub struct LeLongTermKeyRequestReplyCommand {
    handle: ConnHandle,
    long_term_key: [u8; 16],
}

impl core::fmt::Debug for LeLongTermKeyRequestReplyCommand {
    fn fmt(&self, output: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        output
            .debug_struct("LeLongTermKeyRequestReplyCommand")
            .field("handle", &self.handle)
            .field("long_term_key", &"<redacted>")
            .finish()
    }
}

impl LeLongTermKeyRequestReplyCommand {
    /// Standard HCI LE Long Term Key Request Reply opcode.
    pub const OPCODE: Opcode = LeLongTermKeyRequestReply::OPCODE;

    pub fn decode(command: HciCommandPacket<'_>) -> Result<Self, LeLongTermKeyCommandDecodeError> {
        if command.opcode() != Self::OPCODE {
            return Err(LeLongTermKeyCommandDecodeError::Unsupported);
        }
        let parameters: &[u8; 18] = command
            .parameters()
            .try_into()
            .map_err(|_| LeLongTermKeyCommandDecodeError::Malformed)?;
        let handle = u16::from_le_bytes([parameters[0], parameters[1]]);
        if handle > 0x0eff {
            return Err(LeLongTermKeyCommandDecodeError::Malformed);
        }
        let [_, _, long_term_key @ ..] = *parameters;
        Ok(Self {
            handle: ConnHandle(handle),
            long_term_key,
        })
    }

    pub const fn handle(&self) -> ConnHandle {
        self.handle
    }

    /// Consume the command and transfer the secret to the Link Layer owner.
    pub fn into_long_term_key(self) -> [u8; 16] {
        self.long_term_key
    }

    pub fn into_accepted_complete(self) -> LeLongTermKeyCommandCompleteEvent {
        LeLongTermKeyCommandCompleteEvent::new(Self::OPCODE, Status::SUCCESS, self.handle)
    }

    pub fn into_unknown_connection_complete(self) -> LeLongTermKeyCommandCompleteEvent {
        LeLongTermKeyCommandCompleteEvent::new(
            Self::OPCODE,
            bt_hci::param::Error::UNKNOWN_CONN_IDENTIFIER.to_status(),
            self.handle,
        )
    }

    pub fn into_command_disallowed_complete(self) -> LeLongTermKeyCommandCompleteEvent {
        LeLongTermKeyCommandCompleteEvent::new(
            Self::OPCODE,
            bt_hci::param::Error::CMD_DISALLOWED.to_status(),
            self.handle,
        )
    }
}

/// Host rejection of one pending LTK request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLongTermKeyRequestNegativeReplyCommand {
    handle: ConnHandle,
}

impl LeLongTermKeyRequestNegativeReplyCommand {
    /// Standard HCI LE Long Term Key Request Negative Reply opcode.
    pub const OPCODE: Opcode = LeLongTermKeyRequestNegativeReply::OPCODE;

    pub fn decode(command: HciCommandPacket<'_>) -> Result<Self, LeLongTermKeyCommandDecodeError> {
        if command.opcode() != Self::OPCODE {
            return Err(LeLongTermKeyCommandDecodeError::Unsupported);
        }
        let [handle_low, handle_high] = command.parameters() else {
            return Err(LeLongTermKeyCommandDecodeError::Malformed);
        };
        let handle = u16::from_le_bytes([*handle_low, *handle_high]);
        if handle > 0x0eff {
            return Err(LeLongTermKeyCommandDecodeError::Malformed);
        }
        Ok(Self {
            handle: ConnHandle(handle),
        })
    }

    pub const fn handle(self) -> ConnHandle {
        self.handle
    }

    pub fn into_accepted_complete(self) -> LeLongTermKeyCommandCompleteEvent {
        LeLongTermKeyCommandCompleteEvent::new(Self::OPCODE, Status::SUCCESS, self.handle)
    }

    pub fn into_unknown_connection_complete(self) -> LeLongTermKeyCommandCompleteEvent {
        LeLongTermKeyCommandCompleteEvent::new(
            Self::OPCODE,
            bt_hci::param::Error::UNKNOWN_CONN_IDENTIFIER.to_status(),
            self.handle,
        )
    }

    pub fn into_command_disallowed_complete(self) -> LeLongTermKeyCommandCompleteEvent {
        LeLongTermKeyCommandCompleteEvent::new(
            Self::OPCODE,
            bt_hci::param::Error::CMD_DISALLOWED.to_status(),
            self.handle,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLongTermKeyCommandDecodeError {
    Unsupported,
    Malformed,
}

/// Owned Command Complete for either LTK reply command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLongTermKeyCommandCompleteEvent {
    bytes: [u8; LE_LONG_TERM_KEY_COMMAND_COMPLETE_EVENT_CAPACITY],
    opcode: Opcode,
    status: Status,
    handle: ConnHandle,
}

impl LeLongTermKeyCommandCompleteEvent {
    /// Command Complete of an LTK reply `opcode` for `handle`.
    pub fn new(opcode: Opcode, status: Status, handle: ConnHandle) -> Self {
        let opcode_bytes = opcode.to_raw().to_le_bytes();
        let handle_bytes = handle.raw().to_le_bytes();
        Self {
            bytes: [
                EventKind::CommandComplete.0,
                (LE_LONG_TERM_KEY_COMMAND_COMPLETE_EVENT_CAPACITY - 2) as u8,
                1,
                opcode_bytes[0],
                opcode_bytes[1],
                status.into_inner(),
                handle_bytes[0],
                handle_bytes[1],
            ],
            opcode,
            status,
            handle,
        }
    }

    pub(crate) fn invalid_parameters(opcode: Opcode) -> Self {
        Self::new(
            opcode,
            bt_hci::param::Error::INVALID_HCI_PARAMETERS.to_status(),
            ConnHandle(0),
        )
    }

    /// Successful completion for `handle`.
    pub fn accepted(opcode: Opcode, handle: ConnHandle) -> Self {
        Self::new(opcode, Status::SUCCESS, handle)
    }

    pub const fn opcode(self) -> Opcode {
        self.opcode
    }

    pub const fn status(self) -> Status {
        self.status
    }

    pub const fn handle(self) -> ConnHandle {
        self.handle
    }

    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl HciControllerResponse for LeLongTermKeyCommandCompleteEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// Host-visible Rand/EDIV request produced by a received `LL_ENC_REQ`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLongTermKeyRequestEvent {
    bytes: [u8; LE_LONG_TERM_KEY_REQUEST_EVENT_CAPACITY],
}

impl LeLongTermKeyRequestEvent {
    pub fn new(handle: ConnHandle, random_number: [u8; 8], encrypted_diversifier: u16) -> Self {
        let [low, high] = handle.raw().to_le_bytes();
        let [ediv_low, ediv_high] = encrypted_diversifier.to_le_bytes();
        let [a, b, c, d, e, f, g, h] = random_number;
        let bytes = [
            EventKind::Le.0,
            (LE_LONG_TERM_KEY_REQUEST_EVENT_CAPACITY - 2) as u8,
            LeEventKind::LeLongTermKeyRequest.0,
            low,
            high,
            a,
            b,
            c,
            d,
            e,
            f,
            g,
            h,
            ediv_low,
            ediv_high,
        ];
        Self { bytes }
    }

    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl HciControllerResponse for LeLongTermKeyRequestEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// Host-visible transition into or out of LE AES-CCM encryption.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeEncryptionChangeEvent {
    bytes: [u8; LE_ENCRYPTION_CHANGE_EVENT_CAPACITY],
}

impl LeEncryptionChangeEvent {
    pub fn new(status: Status, handle: ConnHandle, enabled: bool) -> Self {
        let handle = handle.raw().to_le_bytes();
        Self {
            bytes: [
                EventKind::EncryptionChangeV1.0,
                (LE_ENCRYPTION_CHANGE_EVENT_CAPACITY - 2) as u8,
                status.into_inner(),
                handle[0],
                handle[1],
                u8::from(enabled),
            ],
        }
    }

    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl HciControllerResponse for LeEncryptionChangeEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// Host-visible completion of a pause/restart encryption procedure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeEncryptionKeyRefreshCompleteEvent {
    bytes: [u8; LE_ENCRYPTION_KEY_REFRESH_COMPLETE_EVENT_CAPACITY],
}

impl LeEncryptionKeyRefreshCompleteEvent {
    pub fn new(status: Status, handle: ConnHandle) -> Self {
        let handle = handle.raw().to_le_bytes();
        Self {
            bytes: [
                EventKind::EncryptionKeyRefreshComplete.0,
                (LE_ENCRYPTION_KEY_REFRESH_COMPLETE_EVENT_CAPACITY - 2) as u8,
                status.into_inner(),
                handle[0],
                handle[1],
            ],
        }
    }

    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl HciControllerResponse for LeEncryptionKeyRefreshCompleteEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// The single Link Control command implemented by the peripheral profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeDisconnectCommand {
    handle: ConnHandle,
    reason: u8,
}

/// Owned request to fetch page zero of the connection peer's LE features.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeReadRemoteFeaturesCommand {
    handle: ConnHandle,
}

/// Owned request to fetch the connection peer's Link Layer version.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeReadRemoteVersionInformationCommand {
    handle: ConnHandle,
}

impl LeReadRemoteVersionInformationCommand {
    /// Standard HCI Read Remote Version Information opcode.
    pub const OPCODE: Opcode = ReadRemoteVersionInformation::OPCODE;

    pub(crate) fn decode(
        command: HciCommandPacket<'_>,
    ) -> Result<Self, LeReadRemoteVersionInformationDecodeError> {
        if command.opcode() != Self::OPCODE {
            return Err(LeReadRemoteVersionInformationDecodeError::Unsupported);
        }
        let [handle_low, handle_high] = command.parameters() else {
            return Err(LeReadRemoteVersionInformationDecodeError::Malformed);
        };
        let handle = u16::from_le_bytes([*handle_low, *handle_high]);
        if handle > 0x0eff {
            return Err(LeReadRemoteVersionInformationDecodeError::Malformed);
        }
        Ok(Self {
            handle: ConnHandle(handle),
        })
    }

    pub const fn handle(self) -> ConnHandle {
        self.handle
    }

    pub fn into_accepted_status(self) -> LeReadRemoteVersionInformationCommandStatusEvent {
        LeReadRemoteVersionInformationCommandStatusEvent::new(Status::SUCCESS)
    }

    pub fn into_unknown_connection_status(
        self,
    ) -> LeReadRemoteVersionInformationCommandStatusEvent {
        LeReadRemoteVersionInformationCommandStatusEvent::new(
            bt_hci::param::Error::UNKNOWN_CONN_IDENTIFIER.to_status(),
        )
    }

    pub fn into_command_disallowed_status(
        self,
    ) -> LeReadRemoteVersionInformationCommandStatusEvent {
        LeReadRemoteVersionInformationCommandStatusEvent::new(
            bt_hci::param::Error::CMD_DISALLOWED.to_status(),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LeReadRemoteVersionInformationDecodeError {
    Unsupported,
    Malformed,
}

/// Owned Command Status for Read Remote Version Information.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeReadRemoteVersionInformationCommandStatusEvent {
    bytes: [u8; LE_READ_REMOTE_VERSION_INFORMATION_COMMAND_STATUS_EVENT_CAPACITY],
    status: Status,
}

impl LeReadRemoteVersionInformationCommandStatusEvent {
    pub(crate) fn new(status: Status) -> Self {
        let opcode = LeReadRemoteVersionInformationCommand::OPCODE
            .to_raw()
            .to_le_bytes();
        Self {
            bytes: [
                EventKind::CommandStatus.0,
                (LE_READ_REMOTE_VERSION_INFORMATION_COMMAND_STATUS_EVENT_CAPACITY - 2) as u8,
                status.into_inner(),
                1,
                opcode[0],
                opcode[1],
            ],
            status,
        }
    }

    pub(crate) fn invalid_parameters() -> Self {
        Self::new(bt_hci::param::Error::INVALID_HCI_PARAMETERS.to_status())
    }

    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub const fn status(self) -> Status {
        self.status
    }
}

impl HciControllerResponse for LeReadRemoteVersionInformationCommandStatusEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// Owned Read Remote Version Information Complete event retained across backpressure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeReadRemoteVersionInformationCompleteEvent {
    bytes: [u8; LE_READ_REMOTE_VERSION_INFORMATION_COMPLETE_EVENT_CAPACITY],
}

impl LeReadRemoteVersionInformationCompleteEvent {
    pub fn new(
        status: Status,
        handle: ConnHandle,
        version: u8,
        company_identifier: u16,
        subversion: u16,
    ) -> Self {
        let handle = handle.raw().to_le_bytes();
        let company_identifier = company_identifier.to_le_bytes();
        let subversion = subversion.to_le_bytes();
        Self {
            bytes: [
                EventKind::ReadRemoteVersionInformationComplete.0,
                (LE_READ_REMOTE_VERSION_INFORMATION_COMPLETE_EVENT_CAPACITY - 2) as u8,
                status.into_inner(),
                handle[0],
                handle[1],
                version,
                company_identifier[0],
                company_identifier[1],
                subversion[0],
                subversion[1],
            ],
        }
    }

    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl HciControllerResponse for LeReadRemoteVersionInformationCompleteEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl LeReadRemoteFeaturesCommand {
    /// Standard HCI LE Read Remote Features opcode.
    pub const OPCODE: Opcode = LeReadRemoteFeatures::OPCODE;

    pub(crate) fn decode(
        command: HciCommandPacket<'_>,
    ) -> Result<Self, LeReadRemoteFeaturesDecodeError> {
        if command.opcode() != Self::OPCODE {
            return Err(LeReadRemoteFeaturesDecodeError::Unsupported);
        }
        let [handle_low, handle_high] = command.parameters() else {
            return Err(LeReadRemoteFeaturesDecodeError::Malformed);
        };
        let handle = u16::from_le_bytes([*handle_low, *handle_high]);
        if handle > 0x0eff {
            return Err(LeReadRemoteFeaturesDecodeError::Malformed);
        }
        Ok(Self {
            handle: ConnHandle(handle),
        })
    }

    /// Connection handle selected by the Host.
    pub const fn handle(self) -> ConnHandle {
        self.handle
    }

    /// Accept the asynchronous Link Layer procedure.
    pub fn into_accepted_status(self) -> LeReadRemoteFeaturesCommandStatusEvent {
        LeReadRemoteFeaturesCommandStatusEvent::new(Status::SUCCESS)
    }

    /// Reject a request for a handle outside the live connection epoch.
    pub fn into_unknown_connection_status(self) -> LeReadRemoteFeaturesCommandStatusEvent {
        LeReadRemoteFeaturesCommandStatusEvent::new(
            bt_hci::param::Error::UNKNOWN_CONN_IDENTIFIER.to_status(),
        )
    }

    /// Reject a second request while the first procedure remains active.
    pub fn into_command_disallowed_status(self) -> LeReadRemoteFeaturesCommandStatusEvent {
        LeReadRemoteFeaturesCommandStatusEvent::new(
            bt_hci::param::Error::CMD_DISALLOWED.to_status(),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LeReadRemoteFeaturesDecodeError {
    Unsupported,
    Malformed,
}

/// Owned Command Status for LE Read Remote Features.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeReadRemoteFeaturesCommandStatusEvent {
    bytes: [u8; LE_READ_REMOTE_FEATURES_COMMAND_STATUS_EVENT_CAPACITY],
    status: Status,
}

impl LeReadRemoteFeaturesCommandStatusEvent {
    pub(crate) fn new(status: Status) -> Self {
        let opcode = LeReadRemoteFeaturesCommand::OPCODE.to_raw().to_le_bytes();
        Self {
            bytes: [
                EventKind::CommandStatus.0,
                (LE_READ_REMOTE_FEATURES_COMMAND_STATUS_EVENT_CAPACITY - 2) as u8,
                status.into_inner(),
                1,
                opcode[0],
                opcode[1],
            ],
            status,
        }
    }

    pub(crate) fn invalid_parameters() -> Self {
        Self::new(bt_hci::param::Error::INVALID_HCI_PARAMETERS.to_status())
    }

    /// Complete HCI Event body without an H4 indicator.
    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Status reported for the retained command opcode.
    pub const fn status(self) -> Status {
        self.status
    }
}

impl HciControllerResponse for LeReadRemoteFeaturesCommandStatusEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// Owned LE Read Remote Features Complete event retained across backpressure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeReadRemoteFeaturesCompleteEvent {
    bytes: [u8; LE_READ_REMOTE_FEATURES_COMPLETE_EVENT_CAPACITY],
}

impl LeReadRemoteFeaturesCompleteEvent {
    /// Build a completion for the sole connection handle.
    pub fn new(status: Status, handle: ConnHandle, features: [u8; 8]) -> Self {
        let handle = handle.raw().to_le_bytes();
        let [handle_low, handle_high] = handle;
        let [a, b, c, d, e, f, g, h] = features;
        let bytes = [
            EventKind::Le.0,
            (LE_READ_REMOTE_FEATURES_COMPLETE_EVENT_CAPACITY - 2) as u8,
            0x04,
            status.into_inner(),
            handle_low,
            handle_high,
            a,
            b,
            c,
            d,
            e,
            f,
            g,
            h,
        ];
        Self { bytes }
    }

    /// Complete HCI Event body without an H4 indicator.
    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl HciControllerResponse for LeReadRemoteFeaturesCompleteEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl LeDisconnectCommand {
    /// Standard HCI Disconnect opcode.
    pub const OPCODE: Opcode = Disconnect::OPCODE;

    /// Decode a complete Disconnect parameter body.
    pub(crate) fn decode(command: HciCommandPacket<'_>) -> Result<Self, LeDisconnectDecodeError> {
        if command.opcode() != Self::OPCODE {
            return Err(LeDisconnectDecodeError::Unsupported);
        }
        let [handle_low, handle_high, reason] = command.parameters() else {
            return Err(LeDisconnectDecodeError::Malformed);
        };
        let handle = u16::from_le_bytes([*handle_low, *handle_high]);
        if handle > 0x0eff || !is_disconnect_reason(*reason) {
            return Err(LeDisconnectDecodeError::Malformed);
        }
        Ok(Self {
            handle: ConnHandle(handle),
            reason: *reason,
        })
    }

    /// Connection handle selected by the Host.
    pub const fn handle(&self) -> ConnHandle {
        self.handle
    }

    /// Standard reason byte to place in `LL_TERMINATE_IND`.
    pub const fn reason(&self) -> u8 {
        self.reason
    }

    /// Build the immediate successful Command Status after lifecycle admission.
    pub fn into_accepted_status(self) -> LeDisconnectCommandStatusEvent {
        LeDisconnectCommandStatusEvent::new(Status::SUCCESS)
    }

    /// Reject a well-formed command whose handle is not live in this epoch.
    pub fn into_unknown_connection_status(self) -> LeDisconnectCommandStatusEvent {
        LeDisconnectCommandStatusEvent::new(
            bt_hci::param::Error::UNKNOWN_CONN_IDENTIFIER.to_status(),
        )
    }
}

const fn is_disconnect_reason(reason: u8) -> bool {
    matches!(reason, 0x05 | 0x13 | 0x14 | 0x15 | 0x1a | 0x29 | 0x3b)
}

/// Decode result distinguishing this command family from malformed input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LeDisconnectDecodeError {
    Unsupported,
    Malformed,
}

/// Owned Command Status for one HCI Disconnect command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeDisconnectCommandStatusEvent {
    bytes: [u8; LE_DISCONNECT_COMMAND_STATUS_EVENT_CAPACITY],
    status: Status,
}

impl LeDisconnectCommandStatusEvent {
    pub(crate) fn new(status: Status) -> Self {
        let opcode = LeDisconnectCommand::OPCODE.to_raw().to_le_bytes();
        Self {
            bytes: [
                EventKind::CommandStatus.0,
                (LE_DISCONNECT_COMMAND_STATUS_EVENT_CAPACITY - 2) as u8,
                status.into_inner(),
                1,
                opcode[0],
                opcode[1],
            ],
            status,
        }
    }

    /// Build the required invalid-parameters status for malformed Disconnect.
    pub(crate) fn invalid_parameters() -> Self {
        Self::new(bt_hci::param::Error::INVALID_HCI_PARAMETERS.to_status())
    }

    /// Complete HCI Event body without an H4 indicator.
    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Status reported for the retained Disconnect opcode.
    pub const fn status(&self) -> Status {
        self.status
    }
}

impl HciControllerResponse for LeDisconnectCommandStatusEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// Why an established legacy connection cannot be represented by this event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LePeripheralConnectionCompleteEventError {
    /// Legacy CONNECT_IND carries only a public or random peer address kind.
    UnsupportedPeerAddressKind(AddrKind),
    /// A failed-completion constructor was given the success status.
    SuccessfulFailureStatus,
}

/// One owned successful LE Connection Complete event for the peripheral role.
///
/// `bt-hci` 0.10 exposes parsing but not construction for Controller events.
/// This owner accepts its field-domain types and is regression-decoded through
/// the standard event model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LePeripheralConnectionCompleteEvent {
    bytes: [u8; LE_PERIPHERAL_CONNECTION_COMPLETE_EVENT_CAPACITY],
}

struct LeConnectionCompleteFields {
    handle: ConnHandle,
    peer_address_kind: AddrKind,
    peer_address: BdAddr,
    connection_interval: Duration<1_250>,
    peripheral_latency: u16,
    supervision_timeout: Duration<10_000>,
    central_clock_accuracy: ClockAccuracy,
}

impl LePeripheralConnectionCompleteEvent {
    /// Build a successful standard LE Connection Complete event.
    pub fn new(
        handle: ConnHandle,
        peer_address_kind: AddrKind,
        peer_address: BdAddr,
        connection_interval: Duration<1_250>,
        peripheral_latency: u16,
        supervision_timeout: Duration<10_000>,
        central_clock_accuracy: ClockAccuracy,
    ) -> Result<Self, LePeripheralConnectionCompleteEventError> {
        if peer_address_kind != AddrKind::PUBLIC && peer_address_kind != AddrKind::RANDOM {
            return Err(
                LePeripheralConnectionCompleteEventError::UnsupportedPeerAddressKind(
                    peer_address_kind,
                ),
            );
        }

        Ok(Self::from_fields(
            Status::SUCCESS,
            LeConnectionCompleteFields {
                handle,
                peer_address_kind,
                peer_address,
                connection_interval,
                peripheral_latency,
                supervision_timeout,
                central_clock_accuracy,
            },
        ))
    }

    /// Build a failed LE Connection Complete event with no allocated handle.
    pub fn failed(status: Status) -> Result<Self, LePeripheralConnectionCompleteEventError> {
        if status == Status::SUCCESS {
            return Err(LePeripheralConnectionCompleteEventError::SuccessfulFailureStatus);
        }
        let mut bytes = [0; LE_PERIPHERAL_CONNECTION_COMPLETE_EVENT_CAPACITY];
        bytes[0] = EventKind::Le.0;
        bytes[1] = (LE_PERIPHERAL_CONNECTION_COMPLETE_EVENT_CAPACITY - 2) as u8;
        bytes[2] = LeConnectionComplete::SUBEVENT_CODE;
        bytes[3] = status.into_inner();
        Ok(Self { bytes })
    }

    fn from_fields(status: Status, fields: LeConnectionCompleteFields) -> Self {
        let LeConnectionCompleteFields {
            handle,
            peer_address_kind,
            peer_address,
            connection_interval,
            peripheral_latency,
            supervision_timeout,
            central_clock_accuracy,
        } = fields;
        let handle = handle.raw().to_le_bytes();
        let interval = connection_interval.as_u16().to_le_bytes();
        let latency = peripheral_latency.to_le_bytes();
        let timeout = supervision_timeout.as_u16().to_le_bytes();
        let [handle_low, handle_high] = handle;
        let [a, b, c, d, e, f] = peer_address.into_inner();
        let [interval_low, interval_high] = interval;
        let [latency_low, latency_high] = latency;
        let [timeout_low, timeout_high] = timeout;
        let bytes = [
            EventKind::Le.0,
            (LE_PERIPHERAL_CONNECTION_COMPLETE_EVENT_CAPACITY - 2) as u8,
            LeConnectionComplete::SUBEVENT_CODE,
            status.into_inner(),
            handle_low,
            handle_high,
            LeConnRole::Peripheral as u8,
            peer_address_kind.as_raw(),
            a,
            b,
            c,
            d,
            e,
            f,
            interval_low,
            interval_high,
            latency_low,
            latency_high,
            timeout_low,
            timeout_high,
            central_clock_accuracy as u8,
        ];
        Self { bytes }
    }

    /// Complete HCI Event body without an H4 packet indicator.
    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl HciControllerResponse for LePeripheralConnectionCompleteEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// One owned successful LE Connection Update Complete event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeConnectionUpdateCompleteEvent {
    bytes: [u8; LE_CONNECTION_UPDATE_COMPLETE_EVENT_CAPACITY],
}

impl LeConnectionUpdateCompleteEvent {
    pub fn new(
        handle: ConnHandle,
        connection_interval: Duration<1_250>,
        peripheral_latency: u16,
        supervision_timeout: Duration<10_000>,
    ) -> Self {
        let handle = handle.raw().to_le_bytes();
        let interval = connection_interval.as_u16().to_le_bytes();
        let latency = peripheral_latency.to_le_bytes();
        let timeout = supervision_timeout.as_u16().to_le_bytes();
        let [handle_low, handle_high] = handle;
        let [interval_low, interval_high] = interval;
        let [latency_low, latency_high] = latency;
        let [timeout_low, timeout_high] = timeout;
        let bytes = [
            EventKind::Le.0,
            (LE_CONNECTION_UPDATE_COMPLETE_EVENT_CAPACITY - 2) as u8,
            LeConnectionUpdateComplete::SUBEVENT_CODE,
            Status::SUCCESS.into_inner(),
            handle_low,
            handle_high,
            interval_low,
            interval_high,
            latency_low,
            latency_high,
            timeout_low,
            timeout_high,
        ];
        Self { bytes }
    }

    /// Complete HCI Event body without an H4 packet indicator.
    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl HciControllerResponse for LeConnectionUpdateCompleteEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// One owned successful Disconnection Complete event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeDisconnectionCompleteEvent {
    bytes: [u8; LE_DISCONNECTION_COMPLETE_EVENT_CAPACITY],
}

impl LeDisconnectionCompleteEvent {
    /// Build a standard Disconnection Complete event for a terminated link.
    pub fn new(handle: ConnHandle, reason: Status) -> Self {
        let handle = handle.raw().to_le_bytes();
        Self {
            bytes: [
                EventKind::DisconnectionComplete.0,
                (LE_DISCONNECTION_COMPLETE_EVENT_CAPACITY - 2) as u8,
                Status::SUCCESS.into_inner(),
                handle[0],
                handle[1],
                reason.into_inner(),
            ],
        }
    }

    /// Complete HCI Event body without an H4 packet indicator.
    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl HciControllerResponse for LeDisconnectionCompleteEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

#[cfg(test)]
mod tests;
