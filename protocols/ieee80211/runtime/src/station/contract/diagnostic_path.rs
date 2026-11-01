//! The station's causes as diagnostic paths: each pushes its variant's name,
//! then its payload's path or numbers, so a line or a note names a nested
//! cause without formatting it (`Failed(Sae(Protocol(Malformed)))`).

use oer_diagnostics::{DiagPath, DiagnosticPath};

use super::{
    ChannelRefusal, CommandRefusal, DisconnectReason, EapolParseError, LayoutError, LeaveReason,
    PeerDisconnectKind, PreparationError, RsnFrameError, SaeError, SaeFailure, SaePwe,
    TransmitRefusal,
};

/// A path for an enum whose variants carry nothing: the variant's name.
macro_rules! unit_paths {
    ($($type:ident { $($variant:ident),* $(,)? })*) => {$(
        impl DiagnosticPath for $type {
            fn push_path(&self, path: &mut DiagPath) {
                path.name(match self {
                    $(Self::$variant => stringify!($variant),)*
                });
            }
        }
    )*};
}

unit_paths! {
    PeerDisconnectKind { Disassociation, Deauthentication }
    PreparationError {
        RetransmissionKey, RetransmissionKind, AggregateFull, BlockAckWindow, MissingBodyLoan,
    }
    LayoutError {
        NumberingProtectionMismatch, StorageTooShort, CcmpHeaderTooShort, BodyOverlap,
        BufferUnavailable,
    }
    LeaveReason { Stopped, Unservable, PortFailed }
    EapolParseError { Truncated, LengthMismatch, NotKeyPacket, NotRsnKeyDescriptor }
    RsnFrameError {
        CapacityExceeded, InvalidRsnIe, InvalidRsnxe, InvalidKeyId, ZeroNonce, EmptyKeyData,
        MalformedKeyData, UnsupportedKeyData, DuplicateRsnIe, DuplicateRsnxe, DuplicateGtk,
        DuplicateIgtk, MissingRsnIe, MissingRsnxe, UnexpectedRsnxe, MissingGtk, MissingIgtk,
        UnexpectedIgtk, RsnIeMismatch, RsnxeMismatch, UnexpectedTransmitAction,
    }
    SaePwe { HuntingAndPecking, HashToElement }
    CommandRefusal { AlreadyConnected, NotConnected, Transitioning, NoProfile }
}

impl DiagnosticPath for DisconnectReason {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::LocalRequest => path.name("LocalRequest"),
            Self::Peer { reason_code } => {
                path.name("Peer");
                path.word(u32::from(*reason_code));
            }
            Self::AuthenticationFailed => path.name("AuthenticationFailed"),
            Self::Sae(failure) => {
                path.name("Sae");
                failure.push_path(path);
            }
            Self::AssociationFailed => path.name("AssociationFailed"),
            Self::NotFound { partial } => {
                path.name("NotFound");
                path.word(u32::from(*partial));
            }
            Self::HandshakeFailed => path.name("HandshakeFailed"),
            Self::SaQueryTimeout => path.name("SaQueryTimeout"),
            Self::TransmitRefused(refusal) => {
                path.name("TransmitRefused");
                refusal.push_path(path);
            }
            Self::Refused(refusal) => {
                path.name("Refused");
                refusal.push_path(path);
            }
            Self::LeaseExchangeRefused => path.name("LeaseExchangeRefused"),
            Self::Arbiter(reason) => {
                path.name("Arbiter");
                reason.push_path(path);
            }
            Self::DeadlineOverflow => path.name("DeadlineOverflow"),
            Self::Encode => path.name("Encode"),
            Self::NoAdmittedAccessCategory => path.name("NoAdmittedAccessCategory"),
            Self::InvalidEapol(error) => {
                path.name("InvalidEapol");
                error.push_path(path);
            }
            Self::RetainedSecurityIes(error) => {
                path.name("RetainedSecurityIes");
                error.push_path(path);
            }
        }
    }
}

impl DiagnosticPath for SaeFailure {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::Timeout => path.name("Timeout"),
            Self::DeadlineOverflow => path.name("DeadlineOverflow"),
            Self::PeerDisconnect { kind, reason_code } => {
                path.name("PeerDisconnect");
                kind.push_path(path);
                path.word(u32::from(*reason_code));
            }
            Self::Rejected { status_code } => {
                path.name("Rejected");
                path.word(u32::from(*status_code));
            }
            Self::Protocol(error) => {
                path.name("Protocol");
                error.push_path(path);
            }
            Self::TokenTooLong => path.name("TokenTooLong"),
            Self::Crypto { peer, pwe } => {
                path.name("Crypto");
                pwe.push_path(path);
                let [a, b, c, d, e, f] = *peer;
                path.word(u32::from_be_bytes([a, b, c, d]));
                path.word(u32::from(u16::from_be_bytes([e, f])));
            }
        }
    }
}

impl DiagnosticPath for SaeError {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::NoPasswordElement => path.name("NoPasswordElement"),
            Self::UnsuitableRandom => path.name("UnsuitableRandom"),
            Self::UnsupportedGroup(group) => {
                path.name("UnsupportedGroup");
                path.word(u32::from(*group));
            }
            Self::Malformed => path.name("Malformed"),
            Self::InvalidScalar => path.name("InvalidScalar"),
            Self::InvalidElement => path.name("InvalidElement"),
            Self::Reflection => path.name("Reflection"),
            Self::InfiniteSecret => path.name("InfiniteSecret"),
            Self::ConfirmMismatch => path.name("ConfirmMismatch"),
            Self::Crypto => path.name("Crypto"),
        }
    }
}

impl DiagnosticPath for TransmitRefusal {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::PacketNumberExhausted => path.name("PacketNumberExhausted"),
            Self::LinkChanged => path.name("LinkChanged"),
            Self::NoRate => path.name("NoRate"),
            Self::Preparation(error) => {
                path.name("Preparation");
                error.push_path(path);
            }
            Self::Layout(error) => {
                path.name("Layout");
                error.push_path(path);
            }
        }
    }
}

impl DiagnosticPath for ChannelRefusal {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::IdentityExhausted => path.name("IdentityExhausted"),
            Self::ScheduleExhausted => path.name("ScheduleExhausted"),
            Self::Unservable => path.name("Unservable"),
            Self::TooSoon { retry_at_us } => {
                path.name("TooSoon");
                path.wide(*retry_at_us);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nested_cause_names_every_level_and_its_numbers() {
        let reason = DisconnectReason::Sae(SaeFailure::PeerDisconnect {
            kind: PeerDisconnectKind::Deauthentication,
            reason_code: 15,
        });
        let path = DiagPath::of(&reason);
        assert_eq!(path.names(), ["Sae", "PeerDisconnect", "Deauthentication"]);
        assert_eq!(path.words(), [15]);
        assert!(!path.overflowed());

        let refused = DisconnectReason::TransmitRefused(TransmitRefusal::Layout(
            LayoutError::BufferUnavailable,
        ));
        assert_eq!(
            DiagPath::of(&refused).names(),
            ["TransmitRefused", "Layout", "BufferUnavailable"]
        );
        let soon = DisconnectReason::Refused(ChannelRefusal::TooSoon {
            retry_at_us: 1 << 40,
        });
        assert_eq!(DiagPath::of(&soon).words(), [0, 1 << 8]);
        assert_eq!(
            DiagPath::of(&CommandRefusal::NoProfile).names(),
            ["NoProfile"]
        );
    }
}
