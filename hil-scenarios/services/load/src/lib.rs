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
pub mod tally;

/// The most executors a [`Load`] reports.
pub const MAX_EXECUTORS: usize = 8;
/// The most harts a [`Load`] reports.
pub const MAX_HARTS: usize = 2;
/// The most named parts a [`Parts`] report holds, and the longest name.
pub const MAX_PARTS: usize = 12;
pub const MAX_PART_NAME: usize = 12;
/// The most interrupted program counters one [`Samples`] answer holds.
pub const MAX_SAMPLES: usize = 96;

oer_hil_protocol::messages! {
    endpoint Start = "load/start" => Started;
    topic Started = "load/started";
    endpoint Collect = "load/collect" => Collected;
    topic Collected = "load/collected";
    endpoint GetCpu = "load/cpu" => Load;
    topic Load = "load/cpu-report";
    endpoint CollectParts = "load/parts" => PartsCollected;
    topic PartsCollected = "load/parts-report";
    endpoint CollectSamples = "load/samples" => Samples;
    topic Samples = "load/samples-report";
}

/// The program counters the last window's Wi-Fi interrupts interrupted,
/// from the sample numbered `from`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CollectSamples {
    pub from: u16,
}

/// The answer to [`CollectSamples`]: the samples kept, the interrupts seen
/// in the window, and up to [`MAX_SAMPLES`] program counters.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Samples {
    pub kept: u16,
    pub interrupts: u32,
    pub pcs: heapless::Vec<u32, MAX_SAMPLES>,
}

/// The longest measuring window, start delay and drain a run may ask for.
pub const MAX_DURATION_MS: u32 = 60_000;
pub const MAX_DELAY_MS: u32 = 10_000;
pub const MAX_DRAIN_MS: u32 = 5_000;

/// Start a run. The image binds `port` on its IPv4 address and receives
/// from the answer on. The run has one measuring window: it opens
/// `delay_ms` after [`Start`] and lasts `duration_ms`; the image sends
/// `transmit` within it and reads its CPU at its two ends. It receives and
/// echoes until `drain_ms` after the window, then closes its socket and
/// keeps the finished [`Report`]: a run ends by itself, and [`Collect`]
/// only fetches its report.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Start {
    pub run: u32,
    pub port: u16,
    pub delay_ms: u32,
    pub duration_ms: u32,
    pub drain_ms: u32,
    pub transmit: Option<Transmit>,
}

/// Load the image sends within the window to `peer`:`peer_port` from its
/// bound port: datagrams of `payload` bytes (the header included) at
/// `rate_bps` bits per second of UDP payload, or as fast as its socket
/// takes them when `rate_bps` is 0.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Transmit {
    pub peer: [u8; 4],
    pub peer_port: u16,
    pub payload: u16,
    pub rate_bps: u32,
}

/// The answer to [`Start`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Started {
    Running,
    Refused(LoadError),
}

/// The report of run `run`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Collect {
    pub run: u32,
}

/// The answer to [`Collect`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[allow(
    clippy::large_enum_variant,
    reason = "a wire message of a no_std crate without allocation: the report goes by value"
)]
pub enum Collected {
    Report(Report),
    /// The run's window or drain has not ended yet.
    Running,
    Refused(LoadError),
}

/// What the image counted of one run. Times are the image's monotonic
/// microseconds.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Report {
    /// The measuring window as the image kept it.
    pub window_start_us: u64,
    pub window_end_us: u64,
    /// The image's CPU at the window's two ends.
    pub cpu_start: Load,
    pub cpu_end: Load,
    pub received: Received,
    pub sent: Sent,
}

/// The load datagrams of the run the image received on its port, counted
/// by a [`tally::Tally`]. A datagram with a valid header and
/// `echo_request` set, of any run, is echoed back to its source and counted
/// only in `echoed` / `echo_refused`, never as load.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Received {
    /// Distinct datagrams of the run, and their UDP payload bytes.
    pub datagrams: u64,
    pub bytes: u64,
    /// Of those, the ones that arrived within the measuring window.
    pub window_datagrams: u64,
    pub window_bytes: u64,
    /// `Tally::lost`, `reordered`, `duplicates` and `untracked`.
    pub lost: u64,
    pub reordered: u64,
    pub duplicates: u64,
    pub untracked: u64,
    pub highest: Option<u64>,
    /// Datagrams on the port that are not the run's load: another run, no
    /// header, or too short.
    pub foreign: u64,
    /// Echo replies the image sent, and those its socket refused.
    pub echoed: u64,
    pub echo_refused: u64,
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
    /// Paced datagrams the window held that were never sent: the image
    /// reached their time only after the window ended.
    pub missed: u64,
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

/// The polls of the image's named parts within run `run`'s window, from
/// the part numbered `from`: at most [`MAX_PARTS`] per answer, fewer at the
/// end.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CollectParts {
    pub run: u32,
    pub from: u8,
}

/// The answer to [`CollectParts`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[allow(
    clippy::large_enum_variant,
    reason = "a wire message of a no_std crate without allocation: the parts go by value"
)]
pub enum PartsCollected {
    Report(Parts),
    /// The run's window or drain has not ended yet.
    Running,
    Refused(LoadError),
}

/// What the image's named parts did within one run's measuring window. A
/// part is a task or a future inside one; its poll time runs from the start
/// to the end of each poll, preemption by higher executors and interrupts
/// included. Nested parts and the parts of different executors overlap, so
/// their times do not add up to a hart's busy time.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Parts {
    pub parts: heapless::Vec<PartWindow, MAX_PARTS>,
}

/// One part within the window.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct PartWindow {
    pub name: heapless::String<MAX_PART_NAME>,
    /// Polls that ended within the window.
    pub polls: u32,
    /// Time inside its polls within the window, an open poll's share
    /// included.
    pub poll_us: u32,
    /// The whole time of the polls that ended within the window, the
    /// counterpart of `polls` for a mean.
    pub ended_poll_us: u32,
    /// The longest poll that ended within the window; `None` without one.
    pub longest_poll_us: Option<u32>,
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
    /// or longer than the image's datagram buffer, a zero port, a window,
    /// delay or drain past its maximum).
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
        CollectParts(CollectParts) => parts,
        CollectSamples(CollectSamples) => samples,
    }
}

#[cfg(test)]
mod tests;
