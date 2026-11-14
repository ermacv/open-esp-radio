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
        clippy::arithmetic_side_effects,
        clippy::disallowed_macros,
        clippy::disallowed_methods
    )
)]
//! The HIL Wi-Fi access-point service: the host has the image's access
//! point serve a WPA2-Personal BSS, asks what it and its IPv4 interface
//! know, and has it stop.
//!
//! The messages are shared by the image, which serves them through
//! [`Request`] and its [`Handler`], and the host scenario, which sends them
//! and checks the answers against what its own station sees.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

/// The longest SSID (IEEE 802.11-2020 9.4.2.2).
pub const MAX_SSID: usize = 32;
/// The longest WPA2 passphrase (IEEE 802.11-2020 J.4.1).
pub const MAX_PASSPHRASE: usize = 63;
/// The longest end of the access point as its line reads.
pub const MAX_END: usize = 96;

/// How the access point ended, as its line reads: `wifi-ap-end` and the
/// variant path of the product's exit (`Ended/Stopped` once it stopped and
/// its radio was released; `ReleaseRefused/…` with what the port still
/// holds; another end with its cause).
pub type End = heapless::String<MAX_END>;

oer_protocols_hil_protocol::messages! {
    endpoint Serve = "wifi-access-point/serve" => Served;
    topic Served = "wifi-access-point/served";
    endpoint GetStatus = "wifi-access-point/status" => Status;
    topic Status = "wifi-access-point/status-report";
    endpoint Stop = "wifi-access-point/stop" => Stopped;
    topic Stopped = "wifi-access-point/stopped";
}

/// Serve the WPA2-Personal BSS `ssid` with `passphrase` on `channel`,
/// admitting `clients` stations. The answer comes once the access point
/// beacons or the start failed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Serve {
    pub ssid: heapless::String<MAX_SSID>,
    pub passphrase: heapless::String<MAX_PASSPHRASE>,
    pub channel: u8,
    pub clients: u8,
}

/// The answer to [`Serve`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Served {
    /// The access point beacons at `address`, its BSSID.
    Beaconing {
        address: [u8; 6],
    },
    Refused(AccessPointError),
}

/// Ask what the access point and its IPv4 interface know now.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetStatus;

/// The answer to [`GetStatus`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Status {
    /// The access point's own address, its BSSID.
    pub address: [u8; 6],
    /// Whether it beacons and carries data.
    pub running: bool,
    /// The stations associated now.
    pub associated: u8,
    /// The stations whose 4-way handshake completed.
    pub authorized: u8,
    /// The beacons that went on the air.
    pub beacons: u32,
    /// The beacons that missed their TBTT and were not sent.
    pub beacons_expired: u32,
    /// The steps the engine refused for a peer's input or a peer that
    /// left.
    pub refused: u32,
    /// The received MSDUs the network had no room for.
    pub rx_dropped: u32,
    /// Its static IPv4 configuration.
    pub ipv4: Option<Ipv4>,
    /// How it ended, once its task ended ([`End`]); none before.
    pub ended: Option<End>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ipv4 {
    pub address: [u8; 4],
    pub prefix: u8,
}

/// Stop the access point: it tears its stations down and stops the radio.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Stop;

/// The answer to [`Stop`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Stopped {
    /// The access point stopped and its radio was released.
    Down,
    /// It ended otherwise ([`End`]): a refused release, with what the port
    /// still holds, or an end that was not the stop.
    Ended(End),
    Refused(AccessPointError),
}

/// Why the image refused or failed a request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum AccessPointError {
    /// The SSID, passphrase, channel or client count is not valid for the
    /// access point.
    InvalidSettings,
    /// It served already: the image serves once per boot.
    AlreadyServing,
    /// The radio cannot serve an access point: its port declares no beacon
    /// Timestamp rule the access point writes. Refused before the radio
    /// started.
    Unsupported,
    /// It does not serve.
    NotServing,
    /// It did not reach the state in the image's bound.
    Timeout,
}

oer_protocols_hil_agent::service! {
    /// The Wi-Fi access-point service's requests.
    pub enum Request (sessions = false, handler = Handler) {
        Serve(Serve) => serve,
        GetStatus(GetStatus) => status,
        Stop(Stop) => stop,
    }
}

#[cfg(test)]
mod tests;
