//! The service's answers from the product station runtime's values
//! (`oer-ieee80211-runtime`), and the numbering of [`StationError::Failed`].
//!
//! A code is four bytes, most significant first: its class ([`class`]), a
//! kind within the class and a 16-bit detail. The image fills the classes
//! whose values only its facade knows.

use oer_ieee80211_runtime::station::{
    ControlError, Status,
    contract::{CommandOutcome, CommandRefusal, DisconnectReason, Notice, SaeFailure},
};
use oer_network_port::LinkState;

use crate::{Joined, Link, StationError};

/// The class byte of a failure code.
pub mod class {
    /// The connect ended without a link; the kind is the
    /// [`DisconnectReason`](super::DisconnectReason) ([`super::connect_failed`]).
    pub const CONNECT_FAILED: u8 = 1;
    /// The station refused the connect command; the kind is the
    /// [`CommandRefusal`](super::CommandRefusal) (0 to 2, 4 for
    /// `NoProfile`), or 3 for a terminal of another command
    /// ([`super::refused`]).
    pub const CONNECT_REFUSED: u8 = 2;
    /// The station's control did not admit the connect command; the kind is
    /// the [`ControlError`](super::ControlError) ([`super::not_admitted`]).
    pub const NOT_ADMITTED: u8 = 3;
    /// The station ended before a link; the image numbers the kind.
    pub const STATION_ENDED: u8 = 4;
    /// The IPv4 stack refused to start; the image numbers the kind.
    pub const NETWORK: u8 = 5;
    /// The credentials could not be prepared; the image numbers the kind.
    pub const CREDENTIALS: u8 = 6;
}

impl StationError {
    /// The failure of `class`, `kind` and `detail`.
    pub const fn failed(class: u8, kind: u8, detail: u16) -> Self {
        let [high, low] = detail.to_be_bytes();
        Self::Failed {
            code: u32::from_be_bytes([class, kind, high, low]),
        }
    }
}

/// The answer to a join from its connect command's terminal: linked once the
/// station is associated and authorized, otherwise refused.
pub fn joined(outcome: &CommandOutcome) -> Joined {
    match outcome {
        CommandOutcome::Connected { bssid, channel } => Joined::Linked {
            bssid: *bssid,
            channel: *channel,
        },
        CommandOutcome::ConnectFailed(reason) | CommandOutcome::Disconnected(reason) => {
            Joined::Refused(connect_failed(reason))
        }
        CommandOutcome::BackgroundScan { .. } => {
            Joined::Refused(StationError::failed(class::CONNECT_REFUSED, 3, 0))
        }
    }
}

/// Why a connect ended without a link. The kind is the reason's position in
/// [`DisconnectReason`]; the detail carries the IEEE 802.11 reason or status
/// code the access point sent, or 1 for a partial scan that found nothing.
pub fn connect_failed(reason: &DisconnectReason) -> StationError {
    let (kind, detail) = match *reason {
        DisconnectReason::LocalRequest => (0, 0),
        DisconnectReason::Peer { reason_code } => (1, reason_code),
        DisconnectReason::AuthenticationFailed => (2, 0),
        DisconnectReason::Sae(failure) => (
            3,
            match failure {
                SaeFailure::PeerDisconnect { reason_code, .. } => reason_code,
                SaeFailure::Rejected { status_code } => status_code,
                _ => 0,
            },
        ),
        DisconnectReason::AssociationFailed => (4, 0),
        DisconnectReason::NotFound { partial } => (5, u16::from(partial)),
        DisconnectReason::HandshakeFailed => (6, 0),
        DisconnectReason::SaQueryTimeout => (7, 0),
        DisconnectReason::TransmitRefused(_) => (8, 0),
        DisconnectReason::Refused(_) => (9, 0),
        DisconnectReason::LeaseExchangeRefused => (10, 0),
        DisconnectReason::Arbiter(_) => (11, 0),
        DisconnectReason::DeadlineOverflow => (12, 0),
        DisconnectReason::Encode => (13, 0),
        DisconnectReason::NoAdmittedAccessCategory => (14, 0),
        DisconnectReason::InvalidEapol(_) => (15, 0),
        DisconnectReason::RetainedSecurityIes(_) => (16, 0),
    };
    StationError::failed(class::CONNECT_FAILED, kind, detail)
}

/// The station refused the connect command.
pub fn refused(reason: CommandRefusal) -> StationError {
    let kind = match reason {
        CommandRefusal::AlreadyConnected => 0,
        CommandRefusal::NotConnected => 1,
        CommandRefusal::Transitioning => 2,
        CommandRefusal::NoProfile => 4,
    };
    StationError::failed(class::CONNECT_REFUSED, kind, 0)
}

/// The station's control did not admit the connect command.
pub fn not_admitted(error: ControlError) -> StationError {
    let kind = match error {
        ControlError::QueueFull => 0,
        ControlError::Stopped => 1,
    };
    StationError::failed(class::NOT_ADMITTED, kind, 0)
}

/// The access point the station is linked to: its link is up and its last
/// notice was the connection.
pub fn link(status: &Status) -> Option<Link> {
    match (status.link.state, status.notice) {
        (LinkState::Up, Some(Notice::Connected { bssid, channel })) => {
            Some(Link { bssid, channel })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
