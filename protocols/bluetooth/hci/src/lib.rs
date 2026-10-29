#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
#![cfg_attr(
    test,
    allow(
        clippy::arithmetic_side_effects,
        clippy::disallowed_macros,
        clippy::disallowed_methods,
        clippy::unreachable
    )
)]

//! Sans-IO HCI packet values and codecs for an LE Controller.
//!
//! HCI packet bodies travel with a separate typed packet kind inside the
//! process; the [`h4`] module is the one UART framing codec for the transports
//! that carry a packet kind as an indicator octet. [`HostToControllerFrame`] and
//! [`HciCommandPacket`] are the borrowed Host packets a Controller consumes;
//! [`validate_controller_to_host_packet`] checks that a Controller packet body
//! is one complete packet.
//!
//! The codecs decode the supported commands into owned semantic values and
//! build complete responses and events:
//!
//! - [`classify_le_controller_command`] maps one command packet to bootstrap,
//!   LE Rand, Direct Test Mode, legacy advertising, legacy scanning,
//!   Disconnect, remote feature and version discovery or LTK reply values; a
//!   malformed known command becomes its exact error response and any other
//!   opcode an Unknown Command completion;
//! - [`BootstrapCommandCompleteEvent`] builds the bootstrap responses,
//!   [`le_controller_supported_commands`] reports the production command
//!   inventory and [`default_event_mask`] and [`default_le_event_mask`] are
//!   the masks a Reset restores; the reset-scoped bootstrap state belongs to
//!   the LE Controller;
//! - LE event and ACL codecs build advertising reports, connection, update,
//!   disconnection, feature, version and encryption events, Number Of
//!   Completed Packets and fragmented Controller ACL packets.
//!
//! The crate owns no command ordering, role state, Link Layer, radio, MMIO,
//! interrupt, executor or allocator, and never waits.

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod test_support;

mod controller;

pub use controller::random::{LeRandCommand, LeRandCommandCompleteEvent};
mod encoding;
mod events;
pub mod h4;
pub use events::{CommandCompleteStatusEvent, HardwareErrorEvent, VendorShutdownObservationEvent};
mod transport;
mod wire;

// HCI vocabulary used by the Controller, runtime and transports. The async
// bt-hci controller implementation is outside this sans-IO boundary.
pub use bt_hci::{
    ControllerToHostPacket, FromHciBytes, PacketKind,
    cmd::{
        Cmd, Opcode, OpcodeGroup,
        controller_baseband::{
            HostBufferSize, Reset, SetControllerToHostFlowControl, SetEventMask, SetEventMaskPage2,
        },
        info::{ReadBdAddr, ReadLocalSupportedCmds},
        le::{
            LeRand, LeReadBufferSize, LeReadFilterAcceptListSize, LeReadLocalSupportedFeatures,
            LeSetAdvEnable, LeSetEventMask, LeSetRandomAddr, LeSetScanEnable, LeSetScanParams,
        },
    },
    event::{CommandComplete, CommandCompleteWithStatus, EventKind},
    param::{
        BdAddr, CmdMask, ControllerToHostFlowControl, Error, EventMask, EventMaskPage2,
        LeEventMask, Status,
    },
    transport::PacketToController,
};

/// Why an API value cannot form the requested HCI packet or ACL transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HciBuildError {
    /// An admitted ACL fragment still awaits acknowledgement.
    AclFragmentInFlight { end: u8 },
    /// A fragment must be nonempty and fit the unacknowledged packet suffix.
    InvalidAclFragmentLength {
        length: usize,
        acknowledged: u16,
        packet_length: u16,
    },
    /// Number Of Completed Packets must return at least one Host credit.
    ZeroCompletedPackets,
    /// The standard codec rejected a reset-default event mask.
    InvalidResetEventMask(bt_hci::FromHciBytesError),
}
pub use controller::bootstrap::{
    BOOTSTRAP_COMMAND_COMPLETE_EVENT_CAPACITY, BluetoothPublicDeviceAddress, BootstrapCommand,
    BootstrapCommandCompleteEvent, BootstrapConfigError, LeControllerBootstrapConfig,
    OwnedBootstrapCommand, default_event_mask, default_le_event_mask,
    le_controller_supported_commands,
};
pub use controller::classification::{
    LeControllerCommandClassification, classify_le_controller_command,
};
pub use controller::le::accept_list::{
    LE_ACCEPT_LIST_COMMAND_COMPLETE_EVENT_CAPACITY, LeAcceptListCommand,
    LeAcceptListCommandCompleteEvent, LeAcceptListDevice, LeAcceptListEntry,
};
pub use controller::le::acl::{
    LE_ACL_DATA_PACKET_CAPACITY, LE_CONTROLLER_ACL_PACKET_CAPACITY,
    LE_HOST_COMPLETED_PACKETS_ERROR_EVENT_CAPACITY, LE_NUMBER_OF_COMPLETED_PACKETS_EVENT_CAPACITY,
    LeControllerAclPacket, LeControllerToHostAclProfile, LeHostAclFragment, LeHostAclPacket,
    LeHostAclPacketRejection, LeHostCompletedPacketsCommand, LeHostCompletedPacketsDecodeError,
    LeHostCompletedPacketsErrorEvent, LeNumberOfCompletedPacketsEvent,
};
pub use controller::le::advertising::{
    LE_LEGACY_ADVERTISING_COMMAND_COMPLETE_EVENT_CAPACITY, LE_LEGACY_ADVERTISING_DATA_CAPACITY,
    LeLegacyAdvertisingAddress, LeLegacyAdvertisingCommand,
    LeLegacyAdvertisingCommandCompleteEvent, LeLegacyAdvertisingCommandKind,
    LeLegacyAdvertisingConfiguration, LeLegacyAdvertisingConfigurationCommand,
    LeLegacyAdvertisingData, LeLegacyAdvertisingDecodeError, LeLegacyAdvertisingEnableCommand,
    LeLegacyAdvertisingEnableRequest, LeLegacyAdvertisingIntervalRange,
    LeLegacyAdvertisingOwnAddressKind, LeLegacyAdvertisingParameters,
    LeLegacyAdvertisingPrimaryChannels, LeLegacyAdvertisingRandomAddressMissing,
    LeLegacyAdvertisingRole, LeLegacyNonconnectableAdvertisingEnableRequest,
    LeLegacyResponseCapableAdvertisingEnableRequest, LeLegacyScanResponseData,
};
pub use controller::le::data_length::{
    LE_DATA_LENGTH_CHANGE_EVENT_CAPACITY, LE_DATA_LENGTH_COMMAND_COMPLETE_EVENT_CAPACITY,
    LeDataLengthChangeEvent, LeDataLengthCommand, LeDataLengthCommandCompleteEvent,
    LeDataLengthParameters,
};
pub use controller::le::dtm::{
    LE_DTM_COMMAND_COMPLETE_EVENT_CAPACITY, LE_RECEIVER_TEST_V1_OPCODE, LE_RECEIVER_TEST_V2_OPCODE,
    LE_TEST_END_OPCODE, LE_TRANSMITTER_TEST_V1_OPCODE, LE_TRANSMITTER_TEST_V2_OPCODE,
    LeDtmActiveSessionDisposition, LeDtmChannel, LeDtmCommand, LeDtmCommandCompleteEvent,
    LeDtmCommandDecodeError, LeDtmCommandKind, LeDtmIdleSessionDisposition, LeDtmModulationIndex,
    LeDtmPayloadPattern, LeDtmPhy, LeReceiverTestCommand, LeTestEndCommand,
    LeTransmitterTestCommand,
};
pub use controller::le::peripheral::{
    LE_CONNECTION_UPDATE_COMPLETE_EVENT_CAPACITY, LE_DISCONNECT_COMMAND_STATUS_EVENT_CAPACITY,
    LE_DISCONNECTION_COMPLETE_EVENT_CAPACITY, LE_ENCRYPTION_CHANGE_EVENT_CAPACITY,
    LE_ENCRYPTION_KEY_REFRESH_COMPLETE_EVENT_CAPACITY,
    LE_LONG_TERM_KEY_COMMAND_COMPLETE_EVENT_CAPACITY, LE_LONG_TERM_KEY_REQUEST_EVENT_CAPACITY,
    LE_PERIPHERAL_CONNECTION_COMPLETE_EVENT_CAPACITY,
    LE_READ_REMOTE_FEATURES_COMMAND_STATUS_EVENT_CAPACITY,
    LE_READ_REMOTE_FEATURES_COMPLETE_EVENT_CAPACITY,
    LE_READ_REMOTE_VERSION_INFORMATION_COMMAND_STATUS_EVENT_CAPACITY,
    LE_READ_REMOTE_VERSION_INFORMATION_COMPLETE_EVENT_CAPACITY, LeConnectionUpdateCompleteEvent,
    LeDisconnectCommand, LeDisconnectCommandStatusEvent, LeDisconnectionCompleteEvent,
    LeEncryptionChangeEvent, LeEncryptionKeyRefreshCompleteEvent,
    LeLongTermKeyCommandCompleteEvent, LeLongTermKeyCommandDecodeError, LeLongTermKeyRequestEvent,
    LeLongTermKeyRequestNegativeReplyCommand, LeLongTermKeyRequestReplyCommand,
    LePeripheralConnectionCompleteEvent, LePeripheralConnectionCompleteEventError,
    LeReadRemoteFeaturesCommand, LeReadRemoteFeaturesCommandStatusEvent,
    LeReadRemoteFeaturesCompleteEvent, LeReadRemoteVersionInformationCommand,
    LeReadRemoteVersionInformationCommandStatusEvent, LeReadRemoteVersionInformationCompleteEvent,
};
pub use controller::le::scanning::{
    LE_LEGACY_ADVERTISING_REPORT_EVENT_CAPACITY,
    LE_LEGACY_SCANNING_COMMAND_COMPLETE_EVENT_CAPACITY, LeLegacyAdvertisingReportEvent,
    LeLegacyAdvertisingReportEventError, LeLegacyScanParameters, LeLegacyScanningCommand,
    LeLegacyScanningCommandCompleteEvent, LeLegacyScanningCommandKind,
    LeLegacyScanningConfiguration, LeLegacyScanningConfigurationCommand,
    LeLegacyScanningDecodeError, LeLegacyScanningDuplicatePolicy, LeLegacyScanningEnableCommand,
    LeLegacyScanningEnableRequest, LeLegacyScanningParametersMissing,
};
pub use controller::response::{
    HciControllerResponse, LeControllerCommandComplete, UnknownCommandCompleteEvent,
};
pub use transport::{ControllerToHostPacketError, validate_controller_to_host_packet};

pub use wire::{HciCommandPacket, HostToControllerFrame};
