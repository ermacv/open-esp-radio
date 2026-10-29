#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
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
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects,
        clippy::disallowed_macros,
        clippy::disallowed_methods
    )
)]
//! The network module: measured transport sessions over a network interface,
//! their lifecycle and evidence, with the properties network images advertise.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

mod datagram;
pub use datagram::*;
mod state;
pub use state::*;
mod session;
pub use session::*;
mod traffic;
pub use traffic::*;
mod stream_pattern;
pub use stream_pattern::*;
mod udp_probe;
pub use udp_probe::*;

oer_hil_protocol::messages! {
    property Udp = "network/udp";
        endpoint StartDatagrams = "network/datagrams/start" => oer_hil_protocol::base::Accepted;
        endpoint StopDatagrams = "network/datagrams/stop" => oer_hil_protocol::base::Accepted;
        topic DatagramsFinished = "network/datagrams/finished";
    property Tcp = "network/tcp";
    property Rx = "network/rx";
    property Tx = "network/tx";
    property Bidirectional = "network/bidirectional";
    /// Session configuration at run time.
    property RuntimeConfiguration = "network/runtime-configuration";
    /// Sessions end with typed evidence records.
    property StructuredEvidence = "network/structured-evidence";
    /// One UDP session executes and accounts more than one peer flow.
    property UdpMultiFlow = "network/udp-multi-flow";
    /// UDP RX sessions return typed evidence for every delivery frontier
    /// from post-reorder publication through the application socket.
    property RxDeliveryReport = "network/rx-delivery-evidence";
    /// Aggregate cooperative network scheduler evidence.
    property SchedulerEvidence = "network/scheduler-evidence";
    /// Startup provisioning selects the data-plane executor topology.
    property DataPlanePlacement = "network/data-plane-placement";
        endpoint Configure = "network/session/configure" => oer_hil_protocol::base::Accepted;
        endpoint Arm = "network/session/arm" => oer_hil_protocol::base::Accepted;
        endpoint Start = "network/session/start" => oer_hil_protocol::base::Accepted;
        endpoint Cancel = "network/session/cancel" => oer_hil_protocol::base::Accepted;
        endpoint Recover = "network/session/recover" => oer_hil_protocol::base::Accepted;
        endpoint ReplayResult = "network/session/result/replay" => crate::Finished;
        endpoint AcknowledgeResult = "network/session/result/acknowledge" => oer_hil_protocol::base::Accepted;
        endpoint GetStatus = "network/session/status/get" => crate::Status;
        topic Status = "network/session/status";
        topic StateChanged = "network/session/state";
        topic Ready = "network/ready";
        topic ServiceReady = "network/service/ready";
        topic SessionReady = "network/session/ready";
        topic UdpRxStarted = "network/session/udp-rx-started";
        topic Evidence = "network/session/evidence";
        topic Finished = "network/session/finished";
        topic Failed = "network/session/failed";
}

/// `network/session/configure`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Configure(pub crate::SessionConfig);

/// `network/session/arm`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Arm;

/// `network/session/start`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Start;

/// Cancel a configured or armed session before any workload starts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Cancel;

/// Explicitly discard a terminal result and return to idle ownership.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Recover;

/// Replay the retained evidence for a completed session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ReplayResult;

/// `network/session/result/acknowledge`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct AcknowledgeResult;

/// Return the current operation state and retained session identities.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetStatus;

/// Correlated response to its request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Status(pub crate::OperationStatus);

/// `network/session/state`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StateChanged(pub crate::StateChange);

/// `network/ready`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ready(pub crate::NetworkInfo);

/// `network/service/ready`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ServiceReady(pub crate::ServiceInfo);

/// Single-flow UDP consumer reached its first 256 valid data packets.
/// Session-correlated delivery, distinct from socket readiness or host send.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct UdpRxStarted {
    pub datagrams: u64,
}

/// `network/session/evidence`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Evidence(pub crate::EvidenceRecord);

/// `network/session/failed`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Failed(pub crate::FailureCode);

#[cfg(test)]
mod tests;

#[cfg(test)]
mod framing_tests;

/// Reviewed paths, schemas (through keys) and message kinds.
#[cfg(feature = "descriptors")]
pub const MESSAGES_LOCK: &str = include_str!("../messages.lock");

#[cfg(all(test, feature = "descriptors"))]
#[test]
fn the_reviewed_lock_lists_exactly_this_crates_descriptors() {
    assert_eq!(
        oer_hil_protocol::check_lock(MESSAGES_LOCK, &[MESSAGES]),
        Ok(())
    );
}
