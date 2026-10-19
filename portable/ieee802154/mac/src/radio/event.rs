//! Backend-to-caller observations, completion categories and receive metadata.

use super::{Ieee802154Instant, channel::Channel, interface::Interface};
use crate::mac::frame::FrameView;

/// How a backend validated the received frame check sequence.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FcsStatus {
    /// Hardware or software reported a valid FCS.
    Valid,
    /// Hardware or software reported an invalid FCS.
    Invalid,
    /// The adapter cannot report FCS validation for this frame.
    Unavailable,
}

/// Security processing already performed before frame publication.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SecurityStatus {
    /// No security offload was applied.
    Unprocessed,
    /// Authentication/decryption completed successfully.
    Processed,
    /// Authentication/decryption failed; promiscuous policy retained the
    /// frame for diagnostics.
    Failed,
}

/// Tri-state frame-pending observation retained from an acknowledgement.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FramePending {
    /// The acknowledgement carried a clear frame-pending bit.
    Clear,
    /// The acknowledgement carried a set frame-pending bit.
    Set,
    /// The backend did not expose this observation.
    Unavailable,
}

/// Auxiliary security header fields a radio wrote into a frame it secured:
/// what OpenThread reads back from a transmitted frame
/// (`SubMac::SignalFrameCounterUsedOnTxDone`) or from the receive
/// information of a frame acknowledged with a secured enhanced ACK
/// (`mAckFrameCounter`, `mAckKeyId`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AppliedSecurity {
    /// The frame counter the frame carries.
    pub frame_counter: u32,
    /// The key index of a key identifier mode 1 frame.
    pub key_id: Option<u8>,
}

/// The acknowledgement the radio sent for a received frame, as ESP-IDF's
/// OpenThread port reports it in the receive information
/// (`mAckedWithFramePending`, `mAckedWithSecEnhAck`).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct SentAcknowledgement {
    /// The acknowledgement set the frame-pending bit.
    pub frame_pending: bool,
    /// The security of a secured enhanced acknowledgement.
    pub security: Option<AppliedSecurity>,
}

impl SentAcknowledgement {
    /// No acknowledgement information: frame pending clear, unsecured.
    pub const NONE: Self = Self {
        frame_pending: false,
        security: None,
    };
}

/// Backend-neutral receive metadata.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RxMetadata {
    /// Channel on which the complete frame was received.
    pub channel: Channel,
    /// Signed RSSI normalized to dBm.
    pub rssi_dbm: i8,
    /// Link quality in the portable zero-through-255 domain.
    pub link_quality: u8,
    /// Optional start-of-frame timestamp in the controller monotonic epoch.
    pub timestamp: Option<Ieee802154Instant>,
    /// FCS validation result.
    pub fcs: FcsStatus,
    /// Security processing already applied to the bytes.
    pub security: SecurityStatus,
    /// Frame-pending observation when the frame is an acknowledgement.
    pub frame_pending: FramePending,
    /// The acknowledgement the radio sent for the frame.
    pub sent_acknowledgement: SentAcknowledgement,
    /// The interface whose PAN ID and address the frame's destination
    /// matched (ESP-IDF's `mpf_index`), which acknowledged it and whose
    /// security [`Self::sent_acknowledgement`] reports. `None` for a frame
    /// no single interface claims - a broadcast, or one received in
    /// promiscuous mode - which ESP-IDF's multi-instance OpenThread port
    /// hands to every interface. A radio without multi-PAN reports
    /// [`Interface::PRIMARY`] for every frame.
    pub interface: Option<Interface>,
}

/// One borrowed received MAC frame and its normalized metadata.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ReceivedFrame<'frame> {
    /// MAC bytes without PHR, FCS storage or DMA metadata.
    pub frame: FrameView<'frame>,
    /// Backend-neutral receive observations.
    pub metadata: RxMetadata,
}

/// Terminal result of one accepted transmit request.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TxStatus {
    /// The on-air operation completed under the requested acknowledgement
    /// policy.
    Success,
    /// Channel access did not find an idle window.
    ChannelBusy,
    /// An acknowledgement was required but not received.
    NoAcknowledgement,
    /// A later command or platform shutdown cancelled the request.
    Aborted,
    /// The backend rejected bytes after accepting the portable request.
    InvalidFrame,
    /// Hardware could not complete the operation.
    HardwareFailure,
    /// Radio coexistence refused the transmission.
    CoexistenceRejected,
    /// The transmit security configuration was invalid for the frame.
    SecurityFailure,
    /// An acknowledgement arrived but was not a valid ACK for the frame.
    InvalidAcknowledgement,
}
