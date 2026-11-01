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
//! A run can also sample where the serving hart spends its window
//! ([`Profile`], [`CollectSamples`]).
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
/// The most named parts one [`Parts`] answer holds, and the longest name.
/// An image with more parts answers them in pages ([`CollectParts::from`]).
pub const MAX_PARTS: usize = 12;
pub const MAX_PART_NAME: usize = 12;
/// The most samples one [`Samples`] answer holds; an image answers its
/// samples in pages ([`CollectSamples::from`]).
pub const MAX_PAGE_SAMPLES: usize = 27;

oer_hil_protocol::messages! {
    endpoint Start = "load/start" => Started;
    topic Started = "load/started";
    endpoint Collect = "load/collect" => Collected;
    topic Collected = "load/collected";
    endpoint GetCpu = "load/cpu" => Load;
    topic Load = "load/cpu-report";
    endpoint CollectParts = "load/parts" => PartsCollected;
    topic PartsCollected = "load/parts-report";
    endpoint CollectCounters = "load/counters" => CountersCollected;
    topic CountersCollected = "load/counters-report";
    endpoint CollectSamples = "load/samples" => SamplesCollected;
    topic SamplesCollected = "load/samples-report";
}

/// The longest measuring window, start delay and drain a run may ask for.
pub const MAX_DURATION_MS: u32 = 60_000;
pub const MAX_DELAY_MS: u32 = 10_000;
pub const MAX_DRAIN_MS: u32 = 5_000;
/// The shortest and longest sampling period a run may ask for.
pub const MIN_SAMPLE_PERIOD_US: u32 = 250;
pub const MAX_SAMPLE_PERIOD_US: u32 = 10_000;

/// Start a run. The image binds `port` on its IPv4 address and receives
/// from the answer on. The run has one measuring window: it opens
/// `delay_ms` after [`Start`] and lasts `duration_ms`; the image sends
/// `transmit` within it and reads its CPU at its two ends. It receives and
/// echoes until `drain_ms` after the window, then closes its socket and
/// keeps the finished [`Report`]: a run ends by itself, and [`Collect`]
/// only fetches its report. With `profile` the image samples its serving
/// hart over the window; an ordinary run samples nothing.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Start {
    pub run: u32,
    pub port: u16,
    pub delay_ms: u32,
    pub duration_ms: u32,
    pub drain_ms: u32,
    pub transmit: Option<Transmit>,
    pub profile: Option<Profile>,
}

/// Sample the serving hart's interrupted program counter every `period_us`
/// over the window, from a timer no executor or product timer uses. The
/// period lies within [`MIN_SAMPLE_PERIOD_US`] and [`MAX_SAMPLE_PERIOD_US`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Profile {
    pub period_us: u32,
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

/// The polls of the image's named parts within run `run`'s window, from the
/// part numbered `from`: at most [`MAX_PARTS`] an answer, fewer at the end.
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

/// The hardware and frame counters within run `run`'s window.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CollectCounters {
    pub run: u32,
}

/// The answer to [`CollectCounters`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum CountersCollected {
    Report(Counters),
    /// The run's window or drain has not ended yet.
    Running,
    Refused(LoadError),
}

/// Counts within one run's measuring window.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Counters {
    /// Machine cycles of the hart that serves the load, wait-for-interrupt
    /// time included.
    pub cycles: u64,
    /// Instructions that hart retired.
    pub instructions: u64,
    /// Frames the image's network bridge took from the station, those it
    /// then had refused included.
    pub bridge_received: u32,
    /// Bridge frames the IP stack's receive pool had no packet for.
    pub bridge_refused_pool: u32,
    /// Bridge frames the IP stack's receive queue had no room for.
    pub bridge_refused_queue: u32,
    /// Bridge frames refused for their length or a down link.
    pub bridge_refused_other: u32,
    /// Bridge frames the station lent in place that the bridge copied.
    pub bridge_copied: u32,
    /// Frames the IP stack took from its device.
    pub stack_received: u32,
}

/// The samples of run `run`'s window, from the sample numbered `from`: at
/// most [`MAX_PAGE_SAMPLES`] an answer, fewer at the end.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CollectSamples {
    pub run: u32,
    pub from: u16,
}

/// The answer to [`CollectSamples`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[allow(
    clippy::large_enum_variant,
    reason = "a wire message of a no_std crate without allocation: the page goes by value"
)]
pub enum SamplesCollected {
    Page(Samples),
    /// The run's window or drain has not ended yet.
    Running,
    Refused(LoadError),
}

/// A page of one run's samples. The sampler's ticks are due every
/// `period_us` on the timer's own schedule from the window's start; a tick
/// due while the hart's interrupts were masked fires, and samples, where
/// they open. The image keeps every `stride`-th tick, which cover the
/// window evenly, one each `stride * period_us`. Samples come in buffer
/// order, not tick order.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Samples {
    pub period_us: u32,
    /// The ticks the window held.
    pub ticks: u32,
    /// The keep-stride at the window's end.
    pub stride: u32,
    /// The samples kept in all; this page holds those numbered from the
    /// request's `from`.
    pub kept: u16,
    /// Ticks that found no sampling run: a stale timer event. Zero when
    /// the sampler works as intended.
    pub stray: u32,
    /// The timer refused a tick's time: the samples end before the window.
    pub cut: bool,
    pub samples: heapless::Vec<Sample, MAX_PAGE_SAMPLES>,
}

/// One kept tick: where the serving hart was when the timer interrupted it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Sample {
    /// The tick's number from the window's first, 0.
    pub tick: u32,
    /// The interrupted program counter.
    pub pc: u32,
    /// The interrupted code's raw CLIC level: 0 for a task in thread mode,
    /// otherwise the level of the interrupt handler or executor it ran in.
    pub level: u8,
    /// The interrupted return address register, where the image records
    /// it: in a leaf function such as a memory copy, the caller's return
    /// address.
    pub ra: Option<u32>,
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
    /// The image has no sampler: it cannot serve a run's [`Profile`].
    NoSampler,
    /// The run was not sampled: its [`Start`] had no [`Profile`].
    NotSampled,
}

oer_hil_agent::service! {
    /// The load service's requests.
    pub enum Request (sessions = false, handler = Handler) {
        Start(Start) => start,
        Collect(Collect) => collect,
        GetCpu(GetCpu) => cpu,
        CollectParts(CollectParts) => parts,
        CollectCounters(CollectCounters) => counters,
        CollectSamples(CollectSamples) => samples,
    }
}

#[cfg(test)]
mod tests;
