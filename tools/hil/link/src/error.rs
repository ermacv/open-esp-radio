//! Why the link or one exchange on it failed.

use oer_hil_protocol::base::RejectReason;

/// The link cannot carry the protocol any more: its serial line failed or
/// closed, or the device broke the wire contract. The first cause is kept.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LinkError {
    /// The serial line failed, closed or made no progress.
    Transport(String),
    /// The device broke the wire contract: another framing, a reserved boot
    /// identity or a payload that does not decode as its key.
    Protocol(String),
    /// The host process is cancelled.
    Cancelled,
    /// No Hello of the running boot arrived in time.
    NoHello,
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(cause) => write!(formatter, "HIL link transport: {cause}"),
            Self::Protocol(cause) => write!(formatter, "HIL link protocol: {cause}"),
            Self::Cancelled => formatter.write_str("HIL link cancelled"),
            Self::NoHello => formatter.write_str("HIL link: no Hello of the running boot"),
        }
    }
}

impl std::error::Error for LinkError {}

/// Why one request got no response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CallError {
    /// The device refused the request.
    Rejected(RejectReason),
    /// No response within the timeout. The request may still be running on
    /// the device: its outcome is unconfirmed until a reset ends its boot.
    Unconfirmed { request_id: u32 },
    /// The device runs another boot: every operation of the request's boot
    /// ended with it.
    Rebooted { boot_id: u64 },
    /// The response's payload does not decode as its type.
    Payload { path: &'static str },
    /// The request did not leave the host: sending it again is safe.
    NotSent(NotSent),
    /// The link failed after the request was queued: its outcome is
    /// unknown.
    Link(LinkError),
}

/// Why a request did not leave the host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotSent {
    /// The request does not fit a frame.
    Encode,
    /// [`crate::OUTBOUND_CAPACITY`] frames wait for the serial line.
    QueueFull,
    /// [`crate::MAX_PENDING`] requests wait for their responses.
    PendingFull,
    /// The link's worker ended.
    LinkEnded,
}

impl std::fmt::Display for CallError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(reason) => {
                write!(formatter, "the device rejected the request: {reason:?}")
            }
            Self::Unconfirmed { request_id } => {
                write!(
                    formatter,
                    "request {request_id} got no response; its outcome is unconfirmed"
                )
            }
            Self::Rebooted { boot_id } => {
                write!(formatter, "the device rebooted into boot {boot_id:016x}")
            }
            Self::Payload { path } => write!(
                formatter,
                "{path}: the response does not decode as its type"
            ),
            Self::NotSent(reason) => write!(formatter, "the request was not sent: {reason:?}"),
            Self::Link(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for CallError {}

impl From<LinkError> for CallError {
    fn from(error: LinkError) -> Self {
        Self::Link(error)
    }
}
