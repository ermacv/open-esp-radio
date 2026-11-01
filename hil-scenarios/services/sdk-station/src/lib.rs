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
//! The HIL SDK station service: the host has an image's `oer-espressif`
//! Wi-Fi station connect to a WPA2-Personal network, wait for its DHCP
//! address or its link loss, exchange a line over TCP with a peer, and
//! report its status. Every wait is bounded by the request.
//!
//! The messages are shared by the image, which serves them through
//! [`Request`], and the host scenario, which sends them and checks the
//! answers against the lab router and its own TCP peer.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

/// The longest SSID (IEEE 802.11-2020 9.4.2.2).
pub const MAX_SSID: usize = 32;
/// The longest WPA2 passphrase (IEEE 802.11-2020 J.4.1).
pub const MAX_PASSPHRASE: usize = 63;
/// The longest line an exchange sends.
pub const MAX_LINE: usize = 64;

oer_hil_protocol::messages! {
    endpoint Connect = "sdk-station/connect" => Connection;
    topic Connection = "sdk-station/connection";
    endpoint WaitIpv4 = "sdk-station/wait-ipv4" => Address;
    topic Address = "sdk-station/address";
    endpoint Exchange = "sdk-station/exchange" => Exchanged;
    topic Exchanged = "sdk-station/exchanged";
    endpoint WaitLinkDown = "sdk-station/wait-link-down" => LinkLoss;
    topic LinkLoss = "sdk-station/link-loss";
    endpoint GetStatus = "sdk-station/status" => Status;
    topic Status = "sdk-station/status-report";
}

/// Connect to the WPA2-Personal network `ssid` with `passphrase`, on any
/// channel the image's domain permits. The answer is that attempt's result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Connect {
    pub ssid: heapless::String<MAX_SSID>,
    pub passphrase: heapless::String<MAX_PASSPHRASE>,
}

/// The answer to [`Connect`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Connection {
    /// Associated and authorized: the 4-way handshake completed.
    Linked { bssid: [u8; 6], channel: u8 },
    /// The station refused the connect or the attempt failed; `code` is
    /// numbered as `oer_services_wifi_station::runtime` numbers its classes
    /// 1 to 3 (connect failed with its disconnect reason, refused, not
    /// admitted).
    Failed { code: u32 },
    /// The SSID or the passphrase has the wrong length.
    InvalidProfile,
    /// The station stopped.
    Stopped,
}

/// Wait up to `timeout_ms` for an IPv4 configuration over an up link.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WaitIpv4 {
    pub timeout_ms: u32,
}

/// The answer to [`WaitIpv4`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Address {
    Leased(Ipv4),
    TimedOut,
    /// The IP service stopped or failed.
    Stopped,
}

/// Open a TCP connection to `peer`:`port`, send `line`, read the same
/// number of bytes back and close, all within `timeout_ms`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Exchange {
    pub peer: [u8; 4],
    pub port: u16,
    pub line: heapless::Vec<u8, MAX_LINE>,
    pub timeout_ms: u32,
}

/// The answer to [`Exchange`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Exchanged {
    /// The peer sent the line back unchanged.
    Echoed,
    /// The peer sent other bytes back.
    Mismatch,
    /// The connection was refused or reset, or the peer closed it early.
    ConnectionFailed,
    TimedOut,
    /// The station holds no IPv4 address.
    NoAddress,
}

/// Wait up to `timeout_ms` until the link that is up now goes down or is
/// replaced.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WaitLinkDown {
    pub timeout_ms: u32,
}

/// The answer to [`WaitLinkDown`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum LinkLoss {
    Down,
    TimedOut,
    /// The IP service stopped or failed.
    Stopped,
}

/// Ask what the station and its IP service know now.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetStatus;

/// The answer to [`GetStatus`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Status {
    /// The station's own address.
    pub address: [u8; 6],
    /// Whether the station's link is up.
    pub link_up: bool,
    /// The IPv4 configuration over the link; none without one.
    pub ipv4: Option<Ipv4>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ipv4 {
    pub address: [u8; 4],
    pub prefix: u8,
    pub gateway: Option<[u8; 4]>,
}

oer_hil_agent::service! {
    /// The SDK station service's requests.
    pub enum Request (sessions = false, handler = Handler) {
        Connect(Connect) => connect,
        WaitIpv4(WaitIpv4) => wait_ipv4,
        Exchange(Exchange) => exchange,
        WaitLinkDown(WaitLinkDown) => wait_link_down,
        GetStatus(GetStatus) => status,
    }
}

#[cfg(test)]
mod tests;
