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
//! The HIL load service: the host has the image receive, send and echo UDP
//! load over the test network, then collects what the image counted and
//! how busy its harts were. The load itself never crosses the HIL link:
//! the link carries the parameters and the counters.
//!
//! [`header`] is the datagram header both ends write and read. RX and TX
//! are named from the image's side: [`Receive`] is load the image
//! receives, [`Transmit`] load it sends.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

pub mod header;

/// The most executors a [`Load`] reports.
pub const MAX_EXECUTORS: usize = 8;
/// The most harts a [`Load`] reports.
pub const MAX_HARTS: usize = 2;

oer_hil_protocol::messages! {
    endpoint Start = "load/start" => Started;
    topic Started = "load/started";
    endpoint Collect = "load/collect" => Collected;
    topic Collected = "load/collected";
    endpoint GetCpu = "load/cpu" => Load;
    topic Load = "load/cpu-report";
}

/// Start a run: bind `port` on the image's IPv4 address, count the load
/// datagrams of run `run` it receives, echo those that ask for it, and send
/// `transmit` when given. The answer comes at once; the load runs until
/// [`Collect`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Start {
    pub run: u32,
    pub port: u16,
    pub transmit: Option<Transmit>,
}

/// Load the image sends to `peer`:`peer_port` from its bound port: datagrams
/// of `payload` bytes (the header included) at `rate_bps` bits per second
/// of UDP payload, or as fast as its socket takes them when `rate_bps` is 0,
/// starting `delay_ms` after [`Start`] and sending for `duration_ms`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Transmit {
    pub peer: [u8; 4],
    pub peer_port: u16,
    pub payload: u16,
    pub rate_bps: u32,
    pub delay_ms: u32,
    pub duration_ms: u32,
}

/// The answer to [`Start`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Started {
    Running,
    Refused(LoadError),
}

/// End run `run` and report what the image counted.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Collect {
    pub run: u32,
}

/// The answer to [`Collect`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Collected {
    Report(Report),
    Refused(LoadError),
}

/// What the image counted of one run. Times are the image's monotonic
/// microseconds.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Report {
    pub received: Received,
    pub sent: Sent,
}

/// The load datagrams of the run the image received on its port.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Received {
    /// Distinct datagrams of the run, and their UDP payload bytes.
    pub datagrams: u64,
    pub bytes: u64,
    /// Sequence numbers below the highest one seen that never arrived.
    pub lost: u64,
    /// Datagrams that arrived after a higher sequence number.
    pub reordered: u64,
    pub duplicates: u64,
    /// Datagrams on the port that are not the run's: another run, no
    /// header, or too short.
    pub foreign: u64,
    /// Echo replies the image sent, and those its socket refused.
    pub echoed: u64,
    pub echo_refused: u64,
    /// The highest sequence number seen; none before the first datagram.
    pub highest: Option<u64>,
    pub first_us: Option<u64>,
    pub last_us: Option<u64>,
}

/// The load the image sent.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Sent {
    /// Datagrams the socket took, and their UDP payload bytes.
    pub datagrams: u64,
    pub bytes: u64,
    /// Sends the socket refused for want of room; the datagram was dropped.
    pub refused: u64,
    /// Sends that failed otherwise.
    pub failed: u64,
    pub first_us: Option<u64>,
    pub last_us: Option<u64>,
}

/// The image's CPU since boot, cumulative: the host takes the difference of
/// two reports. A hart's busy time over a window is the window less its
/// idle time; executors' work spans include their preemption and overlap,
/// so they do not add up to a hart's busy time.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetCpu;

/// The answer to [`GetCpu`].
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Load {
    /// The image's monotonic microseconds now.
    pub now_us: u64,
    pub harts: heapless::Vec<HartLoad, MAX_HARTS>,
    pub executors: heapless::Vec<ExecutorLoad, MAX_EXECUTORS>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct HartLoad {
    pub hart: u8,
    /// Time spent idle (in WFI, between an executor's idle boundaries).
    pub idle_us: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ExecutorLoad {
    pub executor: u8,
    pub hart: u8,
    /// The executor's raw CLIC level.
    pub level: u8,
    /// Time between its work boundaries, preemption included.
    pub work_us: u64,
}

/// Why the image refused a request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum LoadError {
    /// The station has no IPv4 address yet.
    NoAddress,
    /// A run is already running.
    Busy,
    /// No run with this number runs.
    UnknownRun,
    /// The parameters are out of range (a payload shorter than the header
    /// or longer than the image's datagram buffer, a zero port).
    InvalidParameters,
    /// The image could not bind its port.
    Bind,
}

oer_hil_agent::service! {
    /// The load service's requests.
    pub enum Request (sessions = false, handler = Handler) {
        Start(Start) => start,
        Collect(Collect) => collect,
        GetCpu(GetCpu) => cpu,
    }
}

#[cfg(test)]
mod tests;
