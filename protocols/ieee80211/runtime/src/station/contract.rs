//! Product vocabulary of station Control. Core conversions are exhaustive and live here.
use oer_ieee80211_arbiter::{LeaveReason as CoreLeaveReason, Refusal};
use oer_ieee80211_port::{
    LayoutError as CoreLayoutError, PreparationError as CorePreparationError,
};
use oer_ieee80211_rsn::{EapolParseError as CoreEapolParseError, sae::SaeError as CoreSaeError};
use oer_ieee80211_sta::join::sae::{
    StaPeerDisconnectKind as CorePeerDisconnectKind, StaSaePwe as CoreSaePwe,
};
use oer_ieee80211_sta::{
    join::sae::StaSaeFailure,
    machine::{StaDisconnectReason, TxRefusal},
};
/// Why a connection ended, preserving every core cause.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisconnectReason {
    /// The application asked the station to leave.
    LocalRequest,
    /// The access point deauthenticated or disassociated the station.
    Peer {
        reason_code: u16,
    },
    /// The access point refused or never answered the authentication.
    AuthenticationFailed,
    /// The SAE exchange ended with this cause.
    Sae(SaeFailure),
    /// The access point refused or never answered the association.
    AssociationFailed,
    /// No access point of the network answered the scan; `partial` when the
    /// scan ended early after cancelled absences.
    NotFound {
        partial: bool,
    },
    /// The four-way or a group key handshake failed or timed out.
    HandshakeFailed,
    /// The peer did not confirm its protected association within the SA
    /// Query timeout.
    SaQueryTimeout,
    /// A frame the station had to send was refused before admission.
    TransmitRefused(TransmitRefusal),
    /// The channel arbiter did not let the station on a channel.
    Refused(ChannelRefusal),
    /// A move's primary lease exchange was refused.
    LeaseExchangeRefused,
    /// The channel arbiter told the station to leave its network.
    Arbiter(LeaveReason),
    /// An operation's monotonic deadline cannot be represented.
    DeadlineOverflow,
    /// A frame the station had to send did not encode.
    Encode,
    /// An owned EAPOL frame failed its retained parse invariant.
    InvalidEapol(EapolParseError),
    RetainedSecurityIes(RsnFrameError),
}

/// The cause an SAE exchange ended with.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaeFailure {
    Timeout,
    DeadlineOverflow,
    PeerDisconnect {
        kind: PeerDisconnectKind,
        reason_code: u16,
    },
    /// The peer's IEEE 802.11 status code.
    Rejected {
        status_code: u16,
    },
    Protocol(SaeError),
    TokenTooLong,
    /// Local curve invariant, with the derivation inputs identifying its owner.
    Crypto {
        peer: [u8; 6],
        pwe: SaePwe,
    },
}

/// The management frame by which the access point ended the relationship.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerDisconnectKind {
    Disassociation,
    Deauthentication,
}

/// Why an SAE computation or received message was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaeError {
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
    /// An SAE field or curve conversion failed its mathematical invariant.
    Crypto,
}

/// Why a required frame was refused before admission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransmitRefusal {
    PacketNumberExhausted,
    LinkChanged,
    NoRate,
    Preparation(PreparationError),
    Layout(LayoutError),
}

/// Why the TX pool refused a preparation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreparationError {
    RetransmissionKey,
    RetransmissionKind,
    AggregateFull,
    BlockAckWindow,
    /// The backend source lacks its body resource loan.
    MissingBodyLoan,
}

/// Why a frame's portable placement failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutError {
    NumberingProtectionMismatch,
    StorageTooShort,
    CcmpHeaderTooShort,
    BodyOverlap,
}

/// Why the channel arbiter refused the station a channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelRefusal {
    IdentityExhausted,
    ScheduleExhausted,
    Unservable,
    /// The station may ask again at this device time, in microseconds.
    TooSoon {
        retry_at_us: u64,
    },
}

/// Why the channel arbiter told the station to leave.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeaveReason {
    Stopped,
    Unservable,
    /// The port could not return to a channel, or it is poisoned.
    PortFailed,
}

/// The fixed EAPOL invariant refused by the RSN parser.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EapolParseError {
    Truncated,
    LengthMismatch,
    NotKeyPacket,
    NotRsnKeyDescriptor,
}

/// Source of a retained RSN security-IE invariant failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnFrameError {
    CapacityExceeded,
    InvalidRsnIe,
    InvalidRsnxe,
    InvalidKeyId,
    ZeroNonce,
    EmptyKeyData,
    MalformedKeyData,
    UnsupportedKeyData,
    DuplicateRsnIe,
    DuplicateRsnxe,
    DuplicateGtk,
    DuplicateIgtk,
    MissingRsnIe,
    MissingRsnxe,
    UnexpectedRsnxe,
    MissingGtk,
    /// Management frame protection is negotiated but no IGTK KDE came.
    MissingIgtk,
    /// An IGTK KDE came although management frame protection is not
    /// negotiated.
    UnexpectedIgtk,
    RsnIeMismatch,
    RsnxeMismatch,
    UnexpectedTransmitAction,
}

/// Password-element derivation used by the local SAE owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaePwe {
    HuntingAndPecking,
    HashToElement,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    Connect,
    Disconnect,
    BackgroundScan,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandRefusal {
    AlreadyConnected,
    NotConnected,
    Transitioning,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AbsenceRefusal {
    IdentityExhausted,
    ScheduleExhausted,
    NotConnected,
    PrimaryEnds { at_us: u64 },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackgroundScanEnd {
    DeadlineOverflow,
    AbsenceRefused(AbsenceRefusal),
    CancelledAbsences,
    LinkEnded(DisconnectReason),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandOutcome {
    Connected {
        bssid: [u8; 6],
        channel: u8,
    },
    ConnectFailed(DisconnectReason),
    Disconnected(DisconnectReason),
    BackgroundScan {
        interrupted: Option<BackgroundScanEnd>,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Notice {
    Connected { bssid: [u8; 6], channel: u8 },
    Disconnected(DisconnectReason),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MsduRefusal {
    NoTokens,
    TooLong,
}
/// Original refused Ethernet owner. Its private implementation is not a station-core API.
#[derive(Debug)]
pub struct TransmitPacket(pub(crate) oer_ieee80211_sta::data::TxMsdu);
impl oer_network_port::EthernetFrame for TransmitPacket {
    fn view(
        &self,
    ) -> Result<oer_network_port::EthernetFrameView<'_>, oer_network_port::FrameLengthError> {
        self.0.frame.view()
    }
}
fn disconnect(reason: StaDisconnectReason) -> DisconnectReason {
    use DisconnectReason as W;
    use StaDisconnectReason as R;
    match reason {
        R::InvalidEapol(error) => W::InvalidEapol(eapol(error)),
        R::RetainedSecurityIes(error) => W::RetainedSecurityIes(retained_security_ies(error)),
        R::TransmitRefused(refusal) => W::TransmitRefused(transmit(refusal)),
        R::Requested => W::LocalRequest,
        R::NotFound { partial } => W::NotFound { partial },
        R::Refused(refusal) => W::Refused(channel(refusal)),
        R::LeaseExchangeRefused => W::LeaseExchangeRefused,
        R::Arbiter(leave) => W::Arbiter(match leave {
            CoreLeaveReason::Stopped => LeaveReason::Stopped,
            CoreLeaveReason::Unservable => LeaveReason::Unservable,
            CoreLeaveReason::PortFailed => LeaveReason::PortFailed,
        }),
        R::AuthenticationFailed => W::AuthenticationFailed,
        R::Sae(failure) => W::Sae(sae(failure)),
        R::AssociationFailed => W::AssociationFailed,
        R::DeadlineOverflow => W::DeadlineOverflow,
        R::Peer { reason_code } => W::Peer { reason_code },
        R::Encode => W::Encode,
        R::HandshakeFailed => W::HandshakeFailed,
        R::SaQueryTimeout => W::SaQueryTimeout,
    }
}

fn transmit(refusal: TxRefusal) -> TransmitRefusal {
    match refusal {
        TxRefusal::PacketNumberExhausted => TransmitRefusal::PacketNumberExhausted,
        TxRefusal::LinkChanged => TransmitRefusal::LinkChanged,
        TxRefusal::NoRate => TransmitRefusal::NoRate,
        TxRefusal::Preparation(error) => TransmitRefusal::Preparation(match error {
            CorePreparationError::MissingBodyLoan => PreparationError::MissingBodyLoan,
            CorePreparationError::RetransmissionKey => PreparationError::RetransmissionKey,
            CorePreparationError::RetransmissionKind => PreparationError::RetransmissionKind,
            CorePreparationError::AggregateFull => PreparationError::AggregateFull,
            CorePreparationError::BlockAckWindow => PreparationError::BlockAckWindow,
        }),
        TxRefusal::Layout(error) => TransmitRefusal::Layout(match error {
            CoreLayoutError::NumberingProtectionMismatch => {
                LayoutError::NumberingProtectionMismatch
            }
            CoreLayoutError::StorageTooShort => LayoutError::StorageTooShort,
            CoreLayoutError::CcmpHeaderTooShort => LayoutError::CcmpHeaderTooShort,
            CoreLayoutError::BodyOverlap => LayoutError::BodyOverlap,
        }),
    }
}

fn channel(refusal: Refusal) -> ChannelRefusal {
    match refusal {
        Refusal::IdentityExhausted => ChannelRefusal::IdentityExhausted,
        Refusal::ScheduleExhausted => ChannelRefusal::ScheduleExhausted,
        Refusal::Unservable => ChannelRefusal::Unservable,
        Refusal::TooSoon { retry_at } => ChannelRefusal::TooSoon {
            retry_at_us: retry_at.as_micros(),
        },
    }
}

fn sae(failure: StaSaeFailure) -> SaeFailure {
    match failure {
        StaSaeFailure::Crypto { peer, pwe } => SaeFailure::Crypto {
            peer,
            pwe: sae_pwe(pwe),
        },
        StaSaeFailure::Timeout => SaeFailure::Timeout,
        StaSaeFailure::DeadlineOverflow => SaeFailure::DeadlineOverflow,
        StaSaeFailure::PeerDisconnect { kind, reason_code } => SaeFailure::PeerDisconnect {
            kind: match kind {
                CorePeerDisconnectKind::Disassociation => PeerDisconnectKind::Disassociation,
                CorePeerDisconnectKind::Deauthentication => PeerDisconnectKind::Deauthentication,
            },
            reason_code,
        },
        StaSaeFailure::Rejected { status_code } => SaeFailure::Rejected { status_code },
        StaSaeFailure::Protocol(error) => SaeFailure::Protocol(match error {
            CoreSaeError::Crypto => SaeError::Crypto,
            CoreSaeError::NoPasswordElement => SaeError::NoPasswordElement,
            CoreSaeError::UnsuitableRandom => SaeError::UnsuitableRandom,
            CoreSaeError::UnsupportedGroup(group) => SaeError::UnsupportedGroup(group),
            CoreSaeError::Malformed => SaeError::Malformed,
            CoreSaeError::InvalidScalar => SaeError::InvalidScalar,
            CoreSaeError::InvalidElement => SaeError::InvalidElement,
            CoreSaeError::Reflection => SaeError::Reflection,
            CoreSaeError::InfiniteSecret => SaeError::InfiniteSecret,
            CoreSaeError::ConfirmMismatch => SaeError::ConfirmMismatch,
        }),
        StaSaeFailure::TokenTooLong => SaeFailure::TokenTooLong,
    }
}

fn eapol(error: CoreEapolParseError) -> EapolParseError {
    match error {
        CoreEapolParseError::Truncated => EapolParseError::Truncated,
        CoreEapolParseError::LengthMismatch => EapolParseError::LengthMismatch,
        CoreEapolParseError::NotKeyPacket => EapolParseError::NotKeyPacket,
        CoreEapolParseError::NotRsnKeyDescriptor => EapolParseError::NotRsnKeyDescriptor,
    }
}

fn sae_pwe(pwe: CoreSaePwe) -> SaePwe {
    match pwe {
        CoreSaePwe::HuntingAndPecking => SaePwe::HuntingAndPecking,
        CoreSaePwe::HashToElement => SaePwe::HashToElement,
    }
}

fn retained_security_ies(error: oer_ieee80211_rsn::frames::RsnFrameError) -> RsnFrameError {
    use oer_ieee80211_rsn::frames::RsnFrameError as R;
    match error {
        R::CapacityExceeded => RsnFrameError::CapacityExceeded,
        R::InvalidRsnIe => RsnFrameError::InvalidRsnIe,
        R::InvalidRsnxe => RsnFrameError::InvalidRsnxe,
        R::InvalidKeyId => RsnFrameError::InvalidKeyId,
        R::ZeroNonce => RsnFrameError::ZeroNonce,
        R::EmptyKeyData => RsnFrameError::EmptyKeyData,
        R::MalformedKeyData => RsnFrameError::MalformedKeyData,
        R::UnsupportedKeyData => RsnFrameError::UnsupportedKeyData,
        R::DuplicateRsnIe => RsnFrameError::DuplicateRsnIe,
        R::DuplicateRsnxe => RsnFrameError::DuplicateRsnxe,
        R::DuplicateGtk => RsnFrameError::DuplicateGtk,
        R::DuplicateIgtk => RsnFrameError::DuplicateIgtk,
        R::MissingRsnIe => RsnFrameError::MissingRsnIe,
        R::MissingRsnxe => RsnFrameError::MissingRsnxe,
        R::UnexpectedRsnxe => RsnFrameError::UnexpectedRsnxe,
        R::MissingGtk => RsnFrameError::MissingGtk,
        R::MissingIgtk => RsnFrameError::MissingIgtk,
        R::UnexpectedIgtk => RsnFrameError::UnexpectedIgtk,
        R::RsnIeMismatch => RsnFrameError::RsnIeMismatch,
        R::RsnxeMismatch => RsnFrameError::RsnxeMismatch,
        R::UnexpectedTransmitAction => RsnFrameError::UnexpectedTransmitAction,
    }
}

impl From<StaDisconnectReason> for DisconnectReason {
    fn from(value: StaDisconnectReason) -> Self {
        disconnect(value)
    }
}
use oer_ieee80211_sta::machine as core_station;
impl From<core_station::StaCommand> for Command {
    fn from(value: core_station::StaCommand) -> Self {
        match value {
            core_station::StaCommand::Connect => Self::Connect,
            core_station::StaCommand::Disconnect => Self::Disconnect,
            core_station::StaCommand::BackgroundScan => Self::BackgroundScan,
        }
    }
}
impl From<Command> for core_station::StaCommand {
    fn from(value: Command) -> Self {
        match value {
            Command::Connect => Self::Connect,
            Command::Disconnect => Self::Disconnect,
            Command::BackgroundScan => Self::BackgroundScan,
        }
    }
}
impl From<core_station::CommandRefusal> for CommandRefusal {
    fn from(value: core_station::CommandRefusal) -> Self {
        match value {
            core_station::CommandRefusal::AlreadyConnected => Self::AlreadyConnected,
            core_station::CommandRefusal::NotConnected => Self::NotConnected,
            core_station::CommandRefusal::Transitioning => Self::Transitioning,
        }
    }
}
impl From<oer_ieee80211_arbiter::AbsenceRefusal> for AbsenceRefusal {
    fn from(value: oer_ieee80211_arbiter::AbsenceRefusal) -> Self {
        use oer_ieee80211_arbiter::AbsenceRefusal as C;
        match value {
            C::IdentityExhausted => Self::IdentityExhausted,
            C::ScheduleExhausted => Self::ScheduleExhausted,
            C::NotConnected => Self::NotConnected,
            C::PrimaryEnds { at } => Self::PrimaryEnds {
                at_us: at.as_micros(),
            },
        }
    }
}
impl From<core_station::BackgroundScanEnd> for BackgroundScanEnd {
    fn from(value: core_station::BackgroundScanEnd) -> Self {
        use core_station::BackgroundScanEnd as C;
        match value {
            C::DeadlineOverflow => Self::DeadlineOverflow,
            C::AbsenceRefused(r) => Self::AbsenceRefused(r.into()),
            C::CancelledAbsences => Self::CancelledAbsences,
            C::LinkEnded(r) => Self::LinkEnded(r.into()),
        }
    }
}
impl From<core_station::StaCommandOutcome> for CommandOutcome {
    fn from(value: core_station::StaCommandOutcome) -> Self {
        use core_station::StaCommandOutcome as C;
        match value {
            C::Connected { bssid, channel } => Self::Connected {
                bssid: bssid.get(),
                channel,
            },
            C::ConnectFailed(r) => Self::ConnectFailed(r.into()),
            C::Disconnected(r) => Self::Disconnected(r.into()),
            C::BackgroundScan { interrupted } => Self::BackgroundScan {
                interrupted: interrupted.map(Into::into),
            },
        }
    }
}
impl From<core_station::StaNotice> for Notice {
    fn from(value: core_station::StaNotice) -> Self {
        match value {
            core_station::StaNotice::Connected { bssid, channel } => Self::Connected {
                bssid: bssid.get(),
                channel,
            },
            core_station::StaNotice::Disconnected(r) => Self::Disconnected(r.into()),
        }
    }
}
impl From<oer_ieee80211_sta::data::DataRefusal> for MsduRefusal {
    fn from(value: oer_ieee80211_sta::data::DataRefusal) -> Self {
        match value {
            oer_ieee80211_sta::data::DataRefusal::NoTokens => Self::NoTokens,
            oer_ieee80211_sta::data::DataRefusal::TooLong => Self::TooLong,
        }
    }
}
impl From<TxRefusal> for TransmitRefusal {
    fn from(value: TxRefusal) -> Self {
        transmit(value)
    }
}

/// Validated AP timing selected by an application; the coordinator representation is private.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeaconInterval(oer_ieee80211_arbiter::coordinator::BeaconInterval);
impl BeaconInterval {
    pub fn new(duration: oer_time::Duration) -> Option<Self> {
        oer_ieee80211_arbiter::coordinator::BeaconInterval::new(duration).map(Self)
    }
    pub fn as_micros(self) -> u64 {
        self.0.as_micros()
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointCommand<K> {
    AnnounceSwitch {
        target: oer_ieee80211_port::SupportedChannel<K>,
        mode: SwitchMode,
        count: u8,
    },
    Start(oer_ieee80211_port::SupportedChannel<K>),
    Stop,
}
impl<K> From<oer_ieee80211_arbiter::AccessPointCommand<K>> for AccessPointCommand<K> {
    fn from(value: oer_ieee80211_arbiter::AccessPointCommand<K>) -> Self {
        use oer_ieee80211_arbiter::AccessPointCommand as C;
        match value {
            C::AnnounceSwitch {
                target,
                mode,
                count,
            } => Self::AnnounceSwitch {
                target,
                mode: mode.into(),
                count,
            },
            C::Start(channel) => Self::Start(channel),
            C::Stop => Self::Stop,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointFact<K> {
    Running {
        channel: oer_ieee80211_port::SupportedChannel<K>,
        next_tbtt: oer_time::Instant,
        beacon_interval: BeaconInterval,
    },
    Stopped,
    SwitchDue(oer_ieee80211_port::SupportedChannel<K>),
}
impl<K> From<AccessPointFact<K>> for oer_ieee80211_arbiter::AccessPointFact<K> {
    fn from(value: AccessPointFact<K>) -> Self {
        match value {
            AccessPointFact::Running {
                channel,
                next_tbtt,
                beacon_interval,
            } => Self::Running {
                channel,
                next_tbtt,
                beacon_interval: beacon_interval.0,
            },
            AccessPointFact::Stopped => Self::Stopped,
            AccessPointFact::SwitchDue(channel) => Self::SwitchDue(channel),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyContractViolation {
    IncompleteBank,
    MissingAssociation,
}
impl From<core_station::StaKeyContractViolation> for KeyContractViolation {
    fn from(value: core_station::StaKeyContractViolation) -> Self {
        match value {
            core_station::StaKeyContractViolation::IncompleteBank => Self::IncompleteBank,
            core_station::StaKeyContractViolation::MissingAssociation => Self::MissingAssociation,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SwitchMode {
    Continue,
    StopTransmitting,
}
impl From<oer_ieee80211_arbiter::SwitchMode> for SwitchMode {
    fn from(value: oer_ieee80211_arbiter::SwitchMode) -> Self {
        use oer_ieee80211_arbiter::SwitchMode as C;
        match value {
            C::Continue => Self::Continue,
            C::StopTransmitting => Self::StopTransmitting,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_security_and_scan_causes_cross_only_the_runtime_boundary() {
        let reason: DisconnectReason = StaDisconnectReason::Sae(StaSaeFailure::Crypto {
            peer: [3; 6],
            pwe: CoreSaePwe::HashToElement,
        })
        .into();
        assert_eq!(
            reason,
            DisconnectReason::Sae(SaeFailure::Crypto {
                peer: [3; 6],
                pwe: SaePwe::HashToElement
            })
        );
        for (error, expected) in [
            (
                oer_ieee80211_rsn::frames::RsnFrameError::InvalidRsnIe,
                RsnFrameError::InvalidRsnIe,
            ),
            (
                oer_ieee80211_rsn::frames::RsnFrameError::InvalidRsnxe,
                RsnFrameError::InvalidRsnxe,
            ),
        ] {
            let value: DisconnectReason = StaDisconnectReason::RetainedSecurityIes(error).into();
            assert_eq!(value, DisconnectReason::RetainedSecurityIes(expected));
        }
        assert_eq!(
            BackgroundScanEnd::from(core_station::BackgroundScanEnd::AbsenceRefused(
                oer_ieee80211_arbiter::AbsenceRefusal::PrimaryEnds {
                    at: oer_time::Instant::from_micros(u64::MAX)
                }
            )),
            BackgroundScanEnd::AbsenceRefused(AbsenceRefusal::PrimaryEnds { at_us: u64::MAX })
        );
    }
}
