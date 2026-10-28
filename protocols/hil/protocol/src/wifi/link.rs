use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

use super::NetworkCredentials;

#[derive(Debug, Serialize, Deserialize, Schema)]
pub struct StationStart {
    pub credentials: NetworkCredentials,
    pub power_save: StationPowerSave,
}

/// W4 keeps the station awake; no modem power-save mode is advertised.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationPowerSave {
    #[default]
    Off,
}

/// A station attempt within the envelope's boot. The owner advances `epoch`
/// before a new attempt so delayed events cannot authorize another attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StationLink {
    pub epoch: u32,
    pub state: StationLinkState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationLinkState {
    Associated,
    /// RSN keys have been installed and the controlled port is open.
    Authorized,
    Disconnected {
        reason: StationDisconnectReason,
    },
}

/// Why a station link ended: one variant for each reason the portable
/// station gives, and `RadioFailure` when the image's radio front end refused
/// the station port before any link.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationDisconnectReason {
    /// The application asked the station to leave.
    LocalRequest,
    /// The access point deauthenticated or disassociated the station.
    Peer { reason_code: u16 },
    /// The access point refused or never answered the authentication.
    AuthenticationFailed,
    /// The SAE exchange ended with this cause.
    Sae(StationSaeFailure),
    /// The access point refused or never answered the association.
    AssociationFailed,
    /// No access point of the network answered the scan; `partial` when the
    /// scan ended early after cancelled absences.
    NotFound { partial: bool },
    /// The four-way or a group key handshake failed or timed out.
    HandshakeFailed,
    /// The peer did not confirm its protected association within the SA
    /// Query timeout.
    SaQueryTimeout,
    /// A frame the station had to send was refused before admission.
    TransmitRefused(StationTransmitRefusal),
    /// The channel arbiter did not let the station on a channel.
    Refused(StationChannelRefusal),
    /// A move's primary lease exchange was refused.
    LeaseExchangeRefused,
    /// The channel arbiter told the station to leave its network.
    Arbiter(StationLeaveReason),
    /// An operation's monotonic deadline cannot be represented.
    DeadlineOverflow,
    /// A frame the station had to send did not encode.
    Encode,
    /// The radio front end refused the station port.
    RadioFailure,
}

/// The cause an SAE exchange ended with.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationSaeFailure {
    Timeout,
    DeadlineOverflow,
    PeerDisconnect {
        kind: StationPeerDisconnectKind,
        reason_code: u16,
    },
    /// The peer's IEEE 802.11 status code.
    Rejected {
        status_code: u16,
    },
    Protocol(StationSaeError),
    TokenTooLong,
}

/// The management frame by which the access point ended the relationship.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationPeerDisconnectKind {
    Disassociation,
    Deauthentication,
}

/// Why an SAE computation or received message was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationSaeError {
    NoPasswordElement,
    UnsuitableRandom,
    /// A commit of this finite cyclic group.
    UnsupportedGroup(u16),
    Malformed,
    InvalidScalar,
    InvalidElement,
    Reflection,
    InfiniteSecret,
    ConfirmMismatch,
}

/// Why a required frame was refused before admission.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationTransmitRefusal {
    PacketNumberExhausted,
    LinkChanged,
    NoRate,
    Preparation(StationPreparationError),
    Layout(StationLayoutError),
}

/// Why the TX pool refused a preparation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationPreparationError {
    RetransmissionKey,
    RetransmissionKind,
    AggregateFull,
    BlockAckWindow,
    /// The backend source lacks its body resource loan.
    MissingBodyLoan,
}

/// Why a frame's portable placement failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationLayoutError {
    NumberingProtectionMismatch,
    StorageTooShort,
    CcmpHeaderTooShort,
    BodyOverlap,
}

/// Why the channel arbiter refused the station a channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationChannelRefusal {
    IdentityExhausted,
    ScheduleExhausted,
    Unservable,
    /// The station may ask again at this device time, in microseconds.
    TooSoon {
        retry_at_us: u64,
    },
}

/// Why the channel arbiter told the station to leave.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationLeaveReason {
    Stopped,
    Unservable,
    /// The port could not return to a channel, or it is poisoned.
    PortFailed,
}
