#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_macros,
        clippy::disallowed_methods,
        clippy::arithmetic_side_effects,
        clippy::unreachable
    )
)]

//! IEEE 802.15.4 backend contract and shared execution vocabulary.
//! Owner resources, configuration and futures describe backend admission;
//! pending and probing helpers execute below the port. CSMA-CA backoff over
//! single clear-channel assessments, protocol keys, retry policy and
//! network-time calculations belong to the MAC core.

#[cfg(test)]
extern crate std;

mod coexistence;
/// Backend capabilities and configuration values.
pub mod config;
/// Enhanced-ACK probing configuration and lookup table.
pub mod link_metrics;
/// Owner, resource and backend futures contracts.
pub mod owner;
/// Source-match storage and pending-bit selection below the port.
pub mod pending;
/// Portable radio values and observations.
pub mod radio;
/// Security offload snapshots and header application.
pub mod security;
/// Time IE request configuration.
pub mod time_sync;

pub use coexistence::RadioOperation;
pub use config::{
    CancelError, ClockError, ClockInfo, CslCapability, CslReceiver, EnhancedAckGeneration,
    EventsLost, FrameCounterUpdate, Ieee802154Capabilities, Poisoned, PortResult,
};
pub use link_metrics::{EnhAckProbing, LinkMetrics, ProbingError, ProbingInitiator, link_margin};
pub use pending::{AckPending, AutoPendingMode, PendingTable, PendingTableFull, ack_pending};
pub use radio::capabilities::{CapabilityBitsError, RadioCapabilities};
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
pub use security::TransmitSecurity;
pub use time_sync::TimeSync;
