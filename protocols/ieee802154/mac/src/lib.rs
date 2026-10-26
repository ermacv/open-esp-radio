#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented
)]

//! Hardware-independent IEEE 802.15.4 radio boundary.
//!
//! This crate owns bounded MAC bytes, normalized metadata and portable radio
//! values. It contains no PHR/FCS DMA
//! image, ESP register layout, interrupt owner, allocator or executor. It
//! declares the port's owner shape ([`owner`]) a backend implements, but
//! never waits on it. Platform adapters translate these contracts to their own
//! hardware ownership model; OpenThread- and Zephyr-facing code can use them
//! through the port without inheriting that platform representation.
//!
//! Start with the owner shape ([`owner`]) and its port ([`port`]). The
//! ESP32-S31 lower crates separately implement timing, energy-detection/CCA,
//! DMA, IRQ and affine MAC-operation boundaries. There is no IEEE 802.15.4
//! role, runtime or service assembly, so these types must not be read as an
//! application start API or as hardware readiness.

#[cfg(test)]
extern crate alloc;
#[cfg(test)]
extern crate std;

/// Bounded IEEE 802.15.4 MAC byte representations.
pub mod mac {
    /// Coordinated Sampled Listening IE and phase.
    pub mod csl;
    /// Unslotted CSMA-CA backoff.
    pub mod csma;
    /// Enhanced acknowledgement of IEEE 802.15.4-2015 frames.
    pub mod enhanced_ack;
    /// Owned and borrowed MAC frames without platform DMA framing.
    pub mod frame;
    /// MAC header inspection of `[PHR, PSDU...]` images.
    pub mod header;
    /// Enhanced-ACK based probing of Link Metrics.
    pub mod link_metrics;
    /// Frame-pending table and ACK pending-bit decision.
    pub mod pending;
    /// Frame retransmission after a failed attempt.
    pub mod retransmission;
    /// MAC keys, frame counter and transmit security of the radio.
    pub mod security;
    /// Thread network time in the Time IE of transmitted frames.
    pub mod time_sync;
}

/// Hardware-independent radio values.
pub mod radio;

mod coexistence;
pub use coexistence::RadioOperation;

#[cfg(test)]
pub mod model;
pub mod owner;
pub mod port;

pub use mac::csl::{CSL_IE_ID, CSL_IE_TEMPLATE, CSL_UNIT_MICROS, csl_phase, write_csl_ie};
pub use mac::csma::CsmaCa;
pub use mac::enhanced_ack::{
    EnhancedAck, EnhancedAckError, EnhancedAckSecurity, KeyIdMode, generate_enhanced_ack,
};
pub use mac::frame::{Frame, FrameError, FrameView, MAX_MAC_FRAME_LEN, MIN_MAC_FRAME_LEN};
pub use mac::header::{AddressMode, FrameAddress, FrameType, FrameVersion, PhrFrame};
pub use mac::link_metrics::{
    ENH_ACK_PROBING_DATA_CAPACITY, ENH_ACK_PROBING_IE_CAPACITY, EnhAckProbing, LinkMetrics,
    ProbingData, ProbingError, ProbingInitiator, link_margin,
};
pub use mac::pending::{AckPending, AutoPendingMode, PendingTable, PendingTableFull, ack_pending};
pub use mac::retransmission::{AttemptFailure, FrameRetries, RetryStart};
pub use mac::security::{MacKeys, TransmitSecurity};
pub use mac::time_sync::TimeSync;
pub use oer_radio_port::ClockRelation;
pub use oer_radio_port::{Correlation, CorrelationIds};
pub use port::{
    CancelError, ClockError, ClockInfo, CslCapability, CslReceiver, EnhancedAckGeneration,
    EventsLost, FrameCounterUpdate, Ieee802154Capabilities, Poisoned, PortResult,
};
pub use radio::capabilities::{CapabilityBitsError, RadioCapabilities};
pub use radio::channel::{Channel, ChannelError};
pub use radio::command::{CcaMode, TxMode, TxSecurity};
pub use radio::event::{
    AppliedSecurity, FcsStatus, FramePending, ReceivedFrame, RxMetadata, SecurityStatus,
    SentAcknowledgement, TxStatus,
};
pub use radio::interface::Interface;
pub use radio::power::{
    CcaThresholdCapability, CcaThresholdError, TransmitPowerCapability, TransmitPowerError,
    TransmitPowerLevels, TransmitPowerLevelsError,
};
pub use radio::{Ieee802154Instant, Ieee802154Radio, RequestId};
