//! Wire mirrors and command dispatch over the public station contract.
use super::*;
use oer_ieee80211_runtime::station as runtime;
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize, postcard_schema::Schema,
)]
pub enum StationCommandKind {
    Connect,
    Disconnect,
    BackgroundScan,
}

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize, postcard_schema::Schema,
)]
pub enum StationCommandRefusal {
    AlreadyConnected,
    NotConnected,
    Transitioning,
}

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize, postcard_schema::Schema,
)]
pub enum StationAbsenceRefusal {
    IdentityExhausted,
    ScheduleExhausted,
    NotConnected,
    PrimaryEnds { at_us: u64 },
}

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize, postcard_schema::Schema,
)]
pub enum StationBackgroundScanEnd {
    DeadlineOverflow,
    AbsenceRefused(StationAbsenceRefusal),
    CancelledAbsences,
    LinkEnded(StationDisconnectReason),
}

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize, postcard_schema::Schema,
)]
pub enum StationCommandOutcome {
    Connected {
        bssid: [u8; 6],
        channel: u8,
    },
    ConnectFailed(StationDisconnectReason),
    Disconnected(StationDisconnectReason),
    BackgroundScan {
        interrupted: Option<StationBackgroundScanEnd>,
    },
}

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize, postcard_schema::Schema,
)]
pub enum StationNotice {
    Connected { bssid: [u8; 6], channel: u8 },
    Disconnected(StationDisconnectReason),
}

impl From<runtime::contract::DisconnectReason> for StationDisconnectReason {
    fn from(value: runtime::contract::DisconnectReason) -> Self {
        use runtime::contract::DisconnectReason as R;
        match value {
            R::LocalRequest => Self::LocalRequest,
            R::Peer { reason_code } => Self::Peer { reason_code },
            R::AuthenticationFailed => Self::AuthenticationFailed,
            R::Sae(value) => Self::Sae(value.into()),
            R::AssociationFailed => Self::AssociationFailed,
            R::NotFound { partial } => Self::NotFound { partial },
            R::HandshakeFailed => Self::HandshakeFailed,
            R::SaQueryTimeout => Self::SaQueryTimeout,
            R::TransmitRefused(value) => Self::TransmitRefused(value.into()),
            R::Refused(value) => Self::Refused(value.into()),
            R::LeaseExchangeRefused => Self::LeaseExchangeRefused,
            R::Arbiter(value) => Self::Arbiter(value.into()),
            R::DeadlineOverflow => Self::DeadlineOverflow,
            R::Encode => Self::Encode,
            R::InvalidEapol(value) => Self::InvalidEapol(value.into()),
            R::RetainedSecurityIes(value) => Self::RetainedSecurityIes(value.into()),
        }
    }
}

impl From<runtime::contract::SaeFailure> for StationSaeFailure {
    fn from(value: runtime::contract::SaeFailure) -> Self {
        use runtime::contract::SaeFailure as R;
        match value {
            R::Timeout => Self::Timeout,
            R::DeadlineOverflow => Self::DeadlineOverflow,
            R::PeerDisconnect { kind, reason_code } => Self::PeerDisconnect {
                kind: kind.into(),
                reason_code,
            },
            R::Rejected { status_code } => Self::Rejected { status_code },
            R::Protocol(value) => Self::Protocol(value.into()),
            R::TokenTooLong => Self::TokenTooLong,
            R::Crypto { peer, pwe } => Self::Crypto {
                peer,
                pwe: pwe.into(),
            },
        }
    }
}

impl From<runtime::contract::PeerDisconnectKind> for StationPeerDisconnectKind {
    fn from(value: runtime::contract::PeerDisconnectKind) -> Self {
        use runtime::contract::PeerDisconnectKind as R;
        match value {
            R::Disassociation => Self::Disassociation,
            R::Deauthentication => Self::Deauthentication,
        }
    }
}

impl From<runtime::contract::SaeError> for StationSaeError {
    fn from(value: runtime::contract::SaeError) -> Self {
        use runtime::contract::SaeError as R;
        match value {
            R::NoPasswordElement => Self::NoPasswordElement,
            R::UnsuitableRandom => Self::UnsuitableRandom,
            R::UnsupportedGroup(value) => Self::UnsupportedGroup(value),
            R::Malformed => Self::Malformed,
            R::InvalidScalar => Self::InvalidScalar,
            R::InvalidElement => Self::InvalidElement,
            R::Reflection => Self::Reflection,
            R::InfiniteSecret => Self::InfiniteSecret,
            R::ConfirmMismatch => Self::ConfirmMismatch,
            R::Crypto => Self::Crypto,
        }
    }
}

impl From<runtime::contract::TransmitRefusal> for StationTransmitRefusal {
    fn from(value: runtime::contract::TransmitRefusal) -> Self {
        use runtime::contract::TransmitRefusal as R;
        match value {
            R::PacketNumberExhausted => Self::PacketNumberExhausted,
            R::LinkChanged => Self::LinkChanged,
            R::NoRate => Self::NoRate,
            R::Preparation(value) => Self::Preparation(value.into()),
            R::Layout(value) => Self::Layout(value.into()),
        }
    }
}

impl From<runtime::contract::PreparationError> for StationPreparationError {
    fn from(value: runtime::contract::PreparationError) -> Self {
        use runtime::contract::PreparationError as R;
        match value {
            R::RetransmissionKey => Self::RetransmissionKey,
            R::RetransmissionKind => Self::RetransmissionKind,
            R::AggregateFull => Self::AggregateFull,
            R::BlockAckWindow => Self::BlockAckWindow,
            R::MissingBodyLoan => Self::MissingBodyLoan,
        }
    }
}

impl From<runtime::contract::LayoutError> for StationLayoutError {
    fn from(value: runtime::contract::LayoutError) -> Self {
        use runtime::contract::LayoutError as R;
        match value {
            R::NumberingProtectionMismatch => Self::NumberingProtectionMismatch,
            R::StorageTooShort => Self::StorageTooShort,
            R::CcmpHeaderTooShort => Self::CcmpHeaderTooShort,
            R::BodyOverlap => Self::BodyOverlap,
        }
    }
}

impl From<runtime::contract::ChannelRefusal> for StationChannelRefusal {
    fn from(value: runtime::contract::ChannelRefusal) -> Self {
        use runtime::contract::ChannelRefusal as R;
        match value {
            R::IdentityExhausted => Self::IdentityExhausted,
            R::ScheduleExhausted => Self::ScheduleExhausted,
            R::Unservable => Self::Unservable,
            R::TooSoon { retry_at_us } => Self::TooSoon { retry_at_us },
        }
    }
}

impl From<runtime::contract::LeaveReason> for StationLeaveReason {
    fn from(value: runtime::contract::LeaveReason) -> Self {
        use runtime::contract::LeaveReason as R;
        match value {
            R::Stopped => Self::Stopped,
            R::Unservable => Self::Unservable,
            R::PortFailed => Self::PortFailed,
        }
    }
}

impl From<runtime::contract::EapolParseError> for StationEapolParseError {
    fn from(value: runtime::contract::EapolParseError) -> Self {
        use runtime::contract::EapolParseError as R;
        match value {
            R::Truncated => Self::Truncated,
            R::LengthMismatch => Self::LengthMismatch,
            R::NotKeyPacket => Self::NotKeyPacket,
            R::NotRsnKeyDescriptor => Self::NotRsnKeyDescriptor,
        }
    }
}

impl From<runtime::contract::RsnFrameError> for StationRsnFrameError {
    fn from(value: runtime::contract::RsnFrameError) -> Self {
        use runtime::contract::RsnFrameError as R;
        match value {
            R::CapacityExceeded => Self::CapacityExceeded,
            R::InvalidRsnIe => Self::InvalidRsnIe,
            R::InvalidRsnxe => Self::InvalidRsnxe,
            R::InvalidKeyId => Self::InvalidKeyId,
            R::ZeroNonce => Self::ZeroNonce,
            R::EmptyKeyData => Self::EmptyKeyData,
            R::MalformedKeyData => Self::MalformedKeyData,
            R::UnsupportedKeyData => Self::UnsupportedKeyData,
            R::DuplicateRsnIe => Self::DuplicateRsnIe,
            R::DuplicateRsnxe => Self::DuplicateRsnxe,
            R::DuplicateGtk => Self::DuplicateGtk,
            R::DuplicateIgtk => Self::DuplicateIgtk,
            R::MissingRsnIe => Self::MissingRsnIe,
            R::MissingRsnxe => Self::MissingRsnxe,
            R::UnexpectedRsnxe => Self::UnexpectedRsnxe,
            R::MissingGtk => Self::MissingGtk,
            R::MissingIgtk => Self::MissingIgtk,
            R::UnexpectedIgtk => Self::UnexpectedIgtk,
            R::RsnIeMismatch => Self::RsnIeMismatch,
            R::RsnxeMismatch => Self::RsnxeMismatch,
            R::UnexpectedTransmitAction => Self::UnexpectedTransmitAction,
        }
    }
}

impl From<runtime::contract::SaePwe> for StationSaePwe {
    fn from(value: runtime::contract::SaePwe) -> Self {
        use runtime::contract::SaePwe as R;
        match value {
            R::HuntingAndPecking => Self::HuntingAndPecking,
            R::HashToElement => Self::HashToElement,
        }
    }
}

impl From<runtime::contract::Command> for StationCommandKind {
    fn from(value: runtime::contract::Command) -> Self {
        use runtime::contract::Command as R;
        match value {
            R::Connect => Self::Connect,
            R::Disconnect => Self::Disconnect,
            R::BackgroundScan => Self::BackgroundScan,
        }
    }
}

impl From<runtime::contract::CommandRefusal> for StationCommandRefusal {
    fn from(value: runtime::contract::CommandRefusal) -> Self {
        use runtime::contract::CommandRefusal as R;
        match value {
            R::AlreadyConnected => Self::AlreadyConnected,
            R::NotConnected => Self::NotConnected,
            R::Transitioning => Self::Transitioning,
        }
    }
}

impl From<runtime::contract::AbsenceRefusal> for StationAbsenceRefusal {
    fn from(value: runtime::contract::AbsenceRefusal) -> Self {
        use runtime::contract::AbsenceRefusal as R;
        match value {
            R::IdentityExhausted => Self::IdentityExhausted,
            R::ScheduleExhausted => Self::ScheduleExhausted,
            R::NotConnected => Self::NotConnected,
            R::PrimaryEnds { at_us } => Self::PrimaryEnds { at_us },
        }
    }
}

impl From<runtime::contract::BackgroundScanEnd> for StationBackgroundScanEnd {
    fn from(value: runtime::contract::BackgroundScanEnd) -> Self {
        use runtime::contract::BackgroundScanEnd as R;
        match value {
            R::DeadlineOverflow => Self::DeadlineOverflow,
            R::AbsenceRefused(value) => Self::AbsenceRefused(value.into()),
            R::CancelledAbsences => Self::CancelledAbsences,
            R::LinkEnded(value) => Self::LinkEnded(value.into()),
        }
    }
}

impl From<runtime::contract::CommandOutcome> for StationCommandOutcome {
    fn from(value: runtime::contract::CommandOutcome) -> Self {
        use runtime::contract::CommandOutcome as R;
        match value {
            R::Connected { bssid, channel } => Self::Connected { bssid, channel },
            R::ConnectFailed(value) => Self::ConnectFailed(value.into()),
            R::Disconnected(value) => Self::Disconnected(value.into()),
            R::BackgroundScan { interrupted } => Self::BackgroundScan {
                interrupted: interrupted.map(Into::into),
            },
        }
    }
}

impl From<runtime::contract::Notice> for StationNotice {
    fn from(value: runtime::contract::Notice) -> Self {
        use runtime::contract::Notice as R;
        match value {
            R::Connected { bssid, channel } => Self::Connected { bssid, channel },
            R::Disconnected(value) => Self::Disconnected(value.into()),
        }
    }
}

use runtime::{BackendContractViolation, ControlTerminal, PendingTerminal};
pub fn contract_violation(violation: BackendContractViolation) -> StationContractViolation {
    use BackendContractViolation as V;
    use StationContractViolation as W;
    match violation {
        V::UnexpectedAggregate => W::UnexpectedAggregate,
        V::UnexpectedTerminal(terminal) => W::UnexpectedTerminal(pending_terminal(terminal)),
        V::TerminalAfterEventEnd => W::TerminalAfterEventEnd,
        V::UnexpectedAccessPointTerminal => W::UnexpectedAccessPointTerminal,
        V::UnexpectedTsfSample => W::UnexpectedTsfSample,
        V::GroupTerminalWithoutRequest => W::GroupTerminalWithoutRequest,
        V::GroupSlotMismatch => W::GroupSlotMismatch,
        V::GroupTerminalKindMismatch => W::GroupTerminalKindMismatch,
        V::TxContextBufferMismatch => W::TxContextBufferMismatch,
        V::StationKeyBank(violation) => W::StationKeyBank(match violation {
            runtime::contract::KeyContractViolation::IncompleteBank => {
                StationKeyContractViolation::IncompleteBank
            }
            runtime::contract::KeyContractViolation::MissingAssociation => {
                StationKeyContractViolation::MissingAssociation
            }
        }),
        V::StationKeyUnwrap => W::StationKeyUnwrap,
    }
}

pub fn pending_terminal(terminal: PendingTerminal) -> StationPendingTerminal {
    match terminal {
        PendingTerminal::Transmit(index) => StationPendingTerminal::Transmit(index),
        PendingTerminal::Control(control) => StationPendingTerminal::Control(match control {
            ControlTerminal::Configure => StationControlTerminal::Configure,
            ControlTerminal::PairwiseRemoval => StationControlTerminal::PairwiseRemoval,
            ControlTerminal::GroupChange => StationControlTerminal::GroupChange,
            ControlTerminal::GateClosed => StationControlTerminal::GateClosed,
        }),
    }
}

impl From<runtime::contract::Notice> for StationLinkState {
    fn from(value: runtime::contract::Notice) -> Self {
        match value {
            runtime::contract::Notice::Connected { bssid, channel } => {
                Self::Authorized { bssid, channel }
            }
            runtime::contract::Notice::Disconnected(reason) => Self::Disconnected {
                reason: reason.into(),
            },
        }
    }
}

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize, postcard_schema::Schema,
)]
pub enum StationTxOperation {
    Station,
    Data,
}
impl From<runtime::TxOperation> for StationTxOperation {
    fn from(value: runtime::TxOperation) -> Self {
        match value {
            runtime::TxOperation::Station => Self::Station,
            runtime::TxOperation::Data => Self::Data,
        }
    }
}
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize, postcard_schema::Schema,
)]
pub struct StationTransmitRefused {
    pub operation: StationTxOperation,
    pub reason: StationTransmitRefusal,
}
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize, postcard_schema::Schema,
)]
pub enum StationMsduRefusal {
    NoTokens,
    TooLong,
}
impl From<runtime::contract::MsduRefusal> for StationMsduRefusal {
    fn from(value: runtime::contract::MsduRefusal) -> Self {
        match value {
            runtime::contract::MsduRefusal::NoTokens => Self::NoTokens,
            runtime::contract::MsduRefusal::TooLong => Self::TooLong,
        }
    }
}
