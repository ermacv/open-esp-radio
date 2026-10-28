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
//! Station admission and link state, separate from network transport sessions.

mod credentials;
mod link;
mod observation;
mod service;
pub use credentials::*;
pub use link::*;
pub use observation::*;
pub use service::*;

oer_hil_protocol::messages! {
    property Station = "wifi/station";
    /// Accept a station configuration; authorization is a later link event.
        endpoint StartStation = "wifi/station/start" => oer_hil_protocol::base::Accepted;
        topic StationLink = "wifi/station/link";
        endpoint ProbeStation = "wifi/station/probe" => oer_hil_protocol::base::Accepted;
        topic StationProbeFinished = "wifi/station/probe/finished";
        topic StationProbeReady = "wifi/station/probe/ready";
        endpoint GetStationCounters = "wifi/station/counters/get" => StationCounters;
        topic StationCounters = "wifi/station/counters";
        topic StationGroupKey = "wifi/station/group-key";
        topic StationReceiveDrop = "wifi/station/receive-drop";
        topic StationArpFrame = "wifi/station/arp-frame";
        endpoint StopStation = "wifi/station/stop" => oer_hil_protocol::base::Accepted;
        topic StationStopped = "wifi/station/stopped";
        topic StationServiceEnded = "wifi/station/service-ended";
}

#[derive(Debug, serde::Serialize, serde::Deserialize, postcard_schema::Schema)]
pub struct StartStation(pub StationStart);

#[cfg(test)]
mod tests;

/// Reviewed paths, schemas (through keys) and message kinds.
#[cfg(feature = "registry")]
pub const MESSAGES_LOCK: &str = include_str!("../messages.lock");
