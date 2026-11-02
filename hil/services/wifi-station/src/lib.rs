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
//! The HIL Wi-Fi station service: the host has the image's station join a
//! WPA2-Personal network, asks what the station and its IPv4 interface
//! know, and has it leave.
//!
//! The messages are shared by the image, which serves them through
//! [`Request`] and its [`Handler`], and the host scenario, which sends them
//! and checks the answers against what the lab router sees. [`runtime`]
//! builds the answers from the product station runtime's values, and
//! [`phase`] says which requests an image answers by itself.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

/// The longest SSID (IEEE 802.11-2020 9.4.2.2).
pub const MAX_SSID: usize = 32;
/// The longest WPA2 passphrase (IEEE 802.11-2020 J.4.1).
pub const MAX_PASSPHRASE: usize = 63;

oer_hil_protocol::messages! {
    endpoint Join = "wifi-station/join" => Joined;
    topic Joined = "wifi-station/joined";
    endpoint GetStatus = "wifi-station/status" => Status;
    topic Status = "wifi-station/status-report";
    endpoint Leave = "wifi-station/leave" => Left;
    topic Left = "wifi-station/left";
}

/// Join the WPA2-Personal network `ssid` with `passphrase` on `channel`,
/// with IPv4 by DHCP. The answer comes once the station's link is up or
/// the attempt failed; the address comes later ([`GetStatus`]).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Join {
    pub ssid: heapless::String<MAX_SSID>,
    pub passphrase: heapless::String<MAX_PASSPHRASE>,
    pub channel: u8,
}

/// The answer to [`Join`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Joined {
    /// Associated and authorized: the 4-way handshake completed.
    Linked {
        bssid: [u8; 6],
        channel: u8,
    },
    Refused(StationError),
}

/// Ask what the station and its IPv4 interface know now.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetStatus;

/// The answer to [`GetStatus`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Status {
    /// The station's own address.
    pub address: [u8; 6],
    /// The access point it is linked to and its channel; none while down.
    pub link: Option<Link>,
    /// The IPv4 configuration DHCP gave it; none before a lease.
    pub ipv4: Option<Ipv4>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Link {
    pub bssid: [u8; 6],
    pub channel: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ipv4 {
    pub address: [u8; 4],
    pub prefix: u8,
    pub gateway: Option<[u8; 4]>,
}

/// Leave the network: disconnect and stop the station.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Leave;

/// The answer to [`Leave`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Left {
    /// The link is down and the station stopped.
    Down,
    Refused(StationError),
}

/// Why the image refused or failed a request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationError {
    /// The SSID, passphrase or channel is not valid for the station.
    InvalidSettings,
    /// A join was already made: the image joins once per boot.
    AlreadyJoined,
    /// No join was made.
    NotJoined,
    /// The attempt failed; `code` is the facade's failure, as the image
    /// numbers it in its README.
    Failed { code: u32 },
    /// The station stopped and cannot take requests.
    Unavailable,
}

oer_hil_agent::service! {
    /// The Wi-Fi station service's requests.
    pub enum Request (sessions = false, handler = Handler) {
        Join(Join) => join,
        GetStatus(GetStatus) => status,
        Leave(Leave) => leave,
    }
}

pub mod phase;
pub mod runtime;

#[cfg(test)]
mod tests;
