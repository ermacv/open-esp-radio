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
//! A run can also sample where each of the image's harts spends its window
//! ([`Profile`], [`CollectSamples`]); an image samples the harts it has a
//! sampler on.
//!
//! [`header`] is the datagram header both ends write and read. RX and TX
//! are named from the image's side: [`Receive`] is load the image
//! receives, [`Transmit`] load it sends.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

pub mod header;
pub mod tally;

/// The most executors a [`Load`] reports: the library runs three, one
/// spare keeps a [`Report`] with both its loads within one response.
pub const MAX_EXECUTORS: usize = 4;
/// The most harts a [`Load`] reports.
pub const MAX_HARTS: usize = 2;
/// The most named parts one [`Parts`] answer holds, and the longest name.
/// An image with more parts answers them in pages ([`CollectParts::from`]).
pub const MAX_PARTS: usize = 12;
pub const MAX_PART_NAME: usize = 12;
/// The most samples one [`Samples`] answer holds; an image answers its
/// samples in pages ([`CollectSamples::from`]).
pub const MAX_PAGE_SAMPLES: usize = 27;

oer_protocols_hil_protocol::messages! {
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
    endpoint CollectLink = "load/link" => LinkCollected;
    topic LinkCollected = "load/link-report";
    endpoint CollectTracking = "load/tracking" => TrackingCollected;
    topic TrackingCollected = "load/tracking-report";
    endpoint CollectMac = "load/mac" => MacCollected;
    topic MacCollected = "load/mac-report";
    endpoint CollectPositions = "load/positions" => PositionsCollected;
    topic PositionsCollected = "load/positions-report";
    endpoint CollectHart1 = "load/hart1" => Hart1Collected;
    topic Hart1Collected = "load/hart1-report";
    endpoint CollectStall = "load/stall" => StallCollected;
    topic StallCollected = "load/stall-report";
    endpoint CollectStallMac = "load/stall-mac" => StallMacCollected;
    topic StallMacCollected = "load/stall-mac-report";
    endpoint CollectNetwork = "load/network" => NetworkCollected;
    topic NetworkCollected = "load/network-report";
    endpoint CollectAddba = "load/addba" => AddbaCollected;
    topic AddbaCollected = "load/addba-report";
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
    /// The run's values of a trace image's constants
    /// (`oer_portable_trace::tune`), by position.
    pub tune: [u32; TUNE_KNOBS],
}

/// The constants a run names; the image checks them against its own.
pub const TUNE_KNOBS: usize = 4;
/// A constant that keeps its default.
pub const TUNE_UNSET: u32 = u32::MAX;

/// Sample the measured hart's interrupted program counter every `period_us`
/// over the window, from a timer no executor or product timer uses. The
/// period lies within [`MIN_SAMPLE_PERIOD_US`] and [`MAX_SAMPLE_PERIOD_US`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Profile {
    pub period_us: u32,
}

/// Load the image sends within the window to `peer`:`peer_port` from its
/// bound port: datagrams of `payload` bytes (the header included) at
/// `rate_bps` bits per second of UDP payload on average, `burst` of them
/// (at least 1) due together, or as fast as its socket takes them when
/// `rate_bps` is 0, which ignores `burst`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Transmit {
    pub peer: [u8; 4],
    pub peer_port: u16,
    pub payload: u16,
    pub rate_bps: u32,
    pub burst: u16,
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
    /// Lent datagrams whose buffer no longer held what the station showed:
    /// a broken device contract, counted apart from `foreign`.
    pub unreadable: u64,
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
    /// Paced sends the socket had no room for at once; the datagram then
    /// waited for room.
    pub refused: u64,
    /// Sends that failed otherwise.
    pub failed: u64,
    /// Paced datagrams the window held that were never sent: the image
    /// reached their time only after the window ended, or waited for room
    /// past them.
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
    /// Its wakes from the wait for an interrupt since boot, wrapping.
    pub wakes: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ExecutorLoad {
    pub executor: u8,
    pub hart: u8,
    /// The executor's raw CLIC level.
    pub level: u8,
    /// Time between its work boundaries, preemption included.
    pub work_us: u64,
    /// Its rounds (work spans begun) since boot, wrapping.
    pub rounds: u32,
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
#[allow(
    clippy::large_enum_variant,
    reason = "a wire message of a no_std crate without allocation: the counters go by value"
)]
pub enum CountersCollected {
    Report(Counters),
    /// The run's window or drain has not ended yet.
    Running,
    Refused(LoadError),
}

/// Counts within one run's measuring window.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Counters {
    /// Machine cycles of the boot hart, wait-for-interrupt time included.
    pub cycles: u64,
    /// Instructions the boot hart retired.
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
    /// The L1 cache's access counts within the window.
    pub cache: CacheCounts,
    /// The cache geometry at the window's end.
    pub cache_geometry: CacheGeometry,
    /// The receive path's counts within the window: the S31 driver's and
    /// the station service's. Only a trace build counts them; others report
    /// zeros.
    pub rx: RxCounts,
    /// The transmit path's counts within the window.
    pub tx: TxCounts,
}

/// The receive path's counts within one window, from the window's start
/// and end readings.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RxCounts {
    pub driver: DriverRx,
    pub station: StationRx,
}

/// What the S31 receive driver counted (its `WifiRxCounters`): received
/// units, their drops by reason, admission and staging blocks, service
/// passes and the RX-control prefix decode's refusals. It keeps no count
/// of the MAC's hardware buffer-full events.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct DriverRx {
    /// Whether the driver's counters were readable at both ends.
    pub available: bool,
    pub mac_irqs: u32,
    /// Units staged to the station and their bytes.
    pub units: u32,
    pub bytes: u32,
    /// Bulk units the admission dropped, with only its critical reserve
    /// of staging left (after the hardware acknowledged them).
    pub overload_drops: u32,
    /// Units left at the ring head for want of a staging credit.
    pub admission_blocks: u32,
    /// Service passes that ended blocked on staging capacity.
    pub stage_capacity_blocks: u32,
    pub chained_drops: u32,
    pub oversized_drops: u32,
    pub empty_drops: u32,
    pub too_long_drops: u32,
    pub service_passes: u32,
    pub service_errors: u32,
    /// Drains a returned lease's capacity started.
    pub capacity_drains: u32,
    /// Drains continued by an immediate self-wake or after a timer.
    pub continued_immediately: u32,
    pub continued_after_wait: u32,
    /// Prefix refusals: a failed RX state (FCS and other errors), a MIC
    /// failure, a quarantined frame, and every other reason together.
    pub rx_failures: u32,
    pub mic_failures: u32,
    pub quarantined: u32,
    pub prefix_other: u32,
}

/// What the station service decided about received frames (its events),
/// by decision. MSDU counts count MSDUs, the others MPDUs or events.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StationRx {
    /// Frames the station dropped, by reason.
    pub dropped_header: u32,
    pub dropped_frame_kind: u32,
    pub dropped_address: u32,
    pub dropped_fragmented: u32,
    pub dropped_protection: u32,
    pub dropped_integrity: u32,
    pub dropped_replay: u32,
    pub dropped_payload: u32,
    pub dropped_not_connected: u32,
    pub dropped_security: u32,
    pub dropped_draining: u32,
    /// MSDUs not delivered: too long, the full queue to the application,
    /// pressure on the station's handoff (no lend past the pressure bound),
    /// no network.
    pub refused_too_long: u32,
    pub refused_queue_full: u32,
    pub refused_pressure: u32,
    pub refused_no_network: u32,
    /// MSDUs copied instead of lent: of a frame with several, or whose
    /// buffer the backend kept.
    pub copied_aggregated: u32,
    pub copied_kept: u32,
    /// MPDUs the receive agreements admitted, by whether they came in an
    /// A-MPDU (unknown without evidence).
    pub mpdus_in_ampdu: u32,
    pub mpdus_single: u32,
    pub mpdus_unknown: u32,
    /// MPDUs the reorder released: on receive, by the gap timeout, by a
    /// BlockAckReq, by an agreement's end or clear.
    pub released_in_order: u32,
    pub released_by_timeout: u32,
    pub released_by_bar: u32,
    pub released_by_stop: u32,
    /// Sequences the reorder skipped without an MPDU: by the gap timeout,
    /// otherwise.
    pub missing_by_timeout: u32,
    pub missing_other: u32,
    /// Buffered duplicates and other rejected input of the reorder.
    pub duplicates: u32,
    pub rejected: u32,
    pub agreements_started: u32,
    pub agreements_stopped: u32,
    /// The window of the latest receive agreement at the window's end;
    /// zero without one.
    pub agreement_window: u16,
    /// Connected RSN inputs the station dropped.
    pub rsn_inputs_dropped: u32,
}

impl DriverRx {
    /// The counts between readings `start` and `end`, wrapping at 2^32.
    pub const fn window(start: &Self, end: &Self) -> Self {
        Self {
            available: start.available && end.available,
            mac_irqs: end.mac_irqs.wrapping_sub(start.mac_irqs),
            units: end.units.wrapping_sub(start.units),
            bytes: end.bytes.wrapping_sub(start.bytes),
            overload_drops: end.overload_drops.wrapping_sub(start.overload_drops),
            admission_blocks: end.admission_blocks.wrapping_sub(start.admission_blocks),
            stage_capacity_blocks: end
                .stage_capacity_blocks
                .wrapping_sub(start.stage_capacity_blocks),
            chained_drops: end.chained_drops.wrapping_sub(start.chained_drops),
            oversized_drops: end.oversized_drops.wrapping_sub(start.oversized_drops),
            empty_drops: end.empty_drops.wrapping_sub(start.empty_drops),
            too_long_drops: end.too_long_drops.wrapping_sub(start.too_long_drops),
            service_passes: end.service_passes.wrapping_sub(start.service_passes),
            service_errors: end.service_errors.wrapping_sub(start.service_errors),
            capacity_drains: end.capacity_drains.wrapping_sub(start.capacity_drains),
            continued_immediately: end
                .continued_immediately
                .wrapping_sub(start.continued_immediately),
            continued_after_wait: end
                .continued_after_wait
                .wrapping_sub(start.continued_after_wait),
            rx_failures: end.rx_failures.wrapping_sub(start.rx_failures),
            mic_failures: end.mic_failures.wrapping_sub(start.mic_failures),
            quarantined: end.quarantined.wrapping_sub(start.quarantined),
            prefix_other: end.prefix_other.wrapping_sub(start.prefix_other),
        }
    }
}

impl StationRx {
    /// The counts between readings `start` and `end`, wrapping at 2^32;
    /// the agreement window is the end's.
    pub const fn window(start: &Self, end: &Self) -> Self {
        Self {
            dropped_header: end.dropped_header.wrapping_sub(start.dropped_header),
            dropped_frame_kind: end
                .dropped_frame_kind
                .wrapping_sub(start.dropped_frame_kind),
            dropped_address: end.dropped_address.wrapping_sub(start.dropped_address),
            dropped_fragmented: end
                .dropped_fragmented
                .wrapping_sub(start.dropped_fragmented),
            dropped_protection: end
                .dropped_protection
                .wrapping_sub(start.dropped_protection),
            dropped_integrity: end.dropped_integrity.wrapping_sub(start.dropped_integrity),
            dropped_replay: end.dropped_replay.wrapping_sub(start.dropped_replay),
            dropped_payload: end.dropped_payload.wrapping_sub(start.dropped_payload),
            dropped_not_connected: end
                .dropped_not_connected
                .wrapping_sub(start.dropped_not_connected),
            dropped_security: end.dropped_security.wrapping_sub(start.dropped_security),
            dropped_draining: end.dropped_draining.wrapping_sub(start.dropped_draining),
            refused_too_long: end.refused_too_long.wrapping_sub(start.refused_too_long),
            refused_queue_full: end
                .refused_queue_full
                .wrapping_sub(start.refused_queue_full),
            refused_pressure: end.refused_pressure.wrapping_sub(start.refused_pressure),
            refused_no_network: end
                .refused_no_network
                .wrapping_sub(start.refused_no_network),
            copied_aggregated: end.copied_aggregated.wrapping_sub(start.copied_aggregated),
            copied_kept: end.copied_kept.wrapping_sub(start.copied_kept),
            mpdus_in_ampdu: end.mpdus_in_ampdu.wrapping_sub(start.mpdus_in_ampdu),
            mpdus_single: end.mpdus_single.wrapping_sub(start.mpdus_single),
            mpdus_unknown: end.mpdus_unknown.wrapping_sub(start.mpdus_unknown),
            released_in_order: end.released_in_order.wrapping_sub(start.released_in_order),
            released_by_timeout: end
                .released_by_timeout
                .wrapping_sub(start.released_by_timeout),
            released_by_bar: end.released_by_bar.wrapping_sub(start.released_by_bar),
            released_by_stop: end.released_by_stop.wrapping_sub(start.released_by_stop),
            missing_by_timeout: end
                .missing_by_timeout
                .wrapping_sub(start.missing_by_timeout),
            missing_other: end.missing_other.wrapping_sub(start.missing_other),
            duplicates: end.duplicates.wrapping_sub(start.duplicates),
            rejected: end.rejected.wrapping_sub(start.rejected),
            agreements_started: end
                .agreements_started
                .wrapping_sub(start.agreements_started),
            agreements_stopped: end
                .agreements_stopped
                .wrapping_sub(start.agreements_stopped),
            agreement_window: end.agreement_window,
            rsn_inputs_dropped: end
                .rsn_inputs_dropped
                .wrapping_sub(start.rsn_inputs_dropped),
        }
    }
}

/// The transmit path's counts within one window, from the window's start
/// and end readings. Only a trace build counts them; others report zeros.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct TxCounts {
    pub ampdu: TxAmpdu,
}

/// What the station's transmit agreements settled: data aggregates by
/// their subframes, their members by BlockAck outcome, and the
/// BlockAckReqs the station sent.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct TxAmpdu {
    /// Aggregates settled: by a terminal, or refused before the port.
    pub aggregates: u32,
    /// Of them, refused before the port.
    pub refused: u32,
    /// Their subframes: every member of every aggregate.
    pub subframes: u32,
    /// Aggregates by subframes: 1, 2, 3 to 4, 5 to 8, 9 to 16, 17 to 32.
    /// These wrap at 2^16, so the counters fit one response.
    pub subframes_1: u16,
    pub subframes_2: u16,
    pub subframes_3_4: u16,
    pub subframes_5_8: u16,
    pub subframes_9_16: u16,
    pub subframes_17_32: u16,
    /// Members the BlockAck acknowledged, back in their queue for a later
    /// aggregate, and dropped.
    pub acknowledged: u32,
    pub retried: u32,
    pub dropped: u32,
    /// BlockAckReqs the station sent.
    pub block_ack_requests: u32,
}

impl TxAmpdu {
    /// The counts between readings `start` and `end`, wrapping at 2^32
    /// (the histogram at 2^16).
    pub const fn window(start: &Self, end: &Self) -> Self {
        Self {
            aggregates: end.aggregates.wrapping_sub(start.aggregates),
            refused: end.refused.wrapping_sub(start.refused),
            subframes: end.subframes.wrapping_sub(start.subframes),
            subframes_1: end.subframes_1.wrapping_sub(start.subframes_1),
            subframes_2: end.subframes_2.wrapping_sub(start.subframes_2),
            subframes_3_4: end.subframes_3_4.wrapping_sub(start.subframes_3_4),
            subframes_5_8: end.subframes_5_8.wrapping_sub(start.subframes_5_8),
            subframes_9_16: end.subframes_9_16.wrapping_sub(start.subframes_9_16),
            subframes_17_32: end.subframes_17_32.wrapping_sub(start.subframes_17_32),
            acknowledged: end.acknowledged.wrapping_sub(start.acknowledged),
            retried: end.retried.wrapping_sub(start.retried),
            dropped: end.dropped.wrapping_sub(start.dropped),
            block_ack_requests: end
                .block_ack_requests
                .wrapping_sub(start.block_ack_requests),
        }
    }
}

/// What the station's link ran within run `run`'s window.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CollectLink {
    pub run: u32,
}

/// The answer to [`CollectLink`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[allow(
    clippy::large_enum_variant,
    reason = "a wire message of a no_std crate without allocation: the counts go by value"
)]
pub enum LinkCollected {
    Report(Link),
    /// The run's window or drain has not ended yet.
    Running,
    Refused(LoadError),
}

/// The rates the station's link ran at within one window, from the window's
/// start and end readings, each direction from the image's side. Only a
/// trace build counts them; others report zeros.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Link {
    pub rx: RxLink,
    pub tx: TxLink,
}

/// Counts by a PPDU's rate: HT MCS 0 to 7 by channel width and guard
/// interval, the other formats each in one count.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Rates {
    /// HT 20 MHz with the long and the short guard interval, by MCS.
    pub ht20_long_gi: [u32; 8],
    pub ht20_short_gi: [u32; 8],
    /// HT 40 MHz with the long and the short guard interval, by MCS.
    pub ht40_long_gi: [u32; 8],
    pub ht40_short_gi: [u32; 8],
    /// HT MCS 8 to 31: more than one spatial stream.
    pub ht_other: u32,
    /// DSSS, CCK and OFDM.
    pub non_ht: u32,
    pub he: u32,
    /// Without a rate.
    pub unknown: u32,
}

/// The data MPDUs addressed to the station's own address that reached its
/// port, by their PPDU's rate and by whether their metadata says they came
/// in an A-MPDU. The metadata carries no PPDU boundary: how many A-MPDUs
/// carried them is not counted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RxLink {
    pub mpdus: Rates,
    pub in_ampdu: u32,
    pub single: u32,
    pub containment_unknown: u32,
}

/// The station's data MPDUs sent, by rate, and its single data MPDU
/// transmissions that ended: the transmissions they took and how they
/// ended. The aggregates themselves count in [`TxAmpdu`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct TxLink {
    /// A single MPDU by its last transmission's rate, one that reports no
    /// transmission as unknown; an aggregate's subframes by the rate of
    /// the aggregate's last transmission.
    pub mpdus: Rates,
    /// The single MPDUs' transmissions, retries included.
    pub transmissions: u32,
    pub delivered: u32,
    pub ack_timeouts: u32,
    /// A protection timeout, the contention limit or a coexistence denial.
    pub other_ends: u32,
    /// Expired, withdrawn, or without a known outcome.
    pub unsent: u32,
    /// Aggregates that went, by the members their BlockAck left missing:
    /// none, 1, 2, 3 to 4, 5 to 8, 9 and more with some acknowledged, and
    /// all (a lost aggregate or BlockAck).
    pub missing: [u32; 7],
}

impl Rates {
    /// The counts between readings `start` and `end`, wrapping at 2^32.
    pub fn window(start: &Self, end: &Self) -> Self {
        let each = |start: &[u32; 8], end: &[u32; 8]| {
            core::array::from_fn(|mcs| {
                let at = |counts: &[u32; 8]| counts.get(mcs).copied().unwrap_or(0);
                at(end).wrapping_sub(at(start))
            })
        };
        Self {
            ht20_long_gi: each(&start.ht20_long_gi, &end.ht20_long_gi),
            ht20_short_gi: each(&start.ht20_short_gi, &end.ht20_short_gi),
            ht40_long_gi: each(&start.ht40_long_gi, &end.ht40_long_gi),
            ht40_short_gi: each(&start.ht40_short_gi, &end.ht40_short_gi),
            ht_other: end.ht_other.wrapping_sub(start.ht_other),
            non_ht: end.non_ht.wrapping_sub(start.non_ht),
            he: end.he.wrapping_sub(start.he),
            unknown: end.unknown.wrapping_sub(start.unknown),
        }
    }
}

impl Link {
    /// The counts between readings `start` and `end`, wrapping at 2^32.
    pub fn window(start: &Self, end: &Self) -> Self {
        let (rx0, rx1, tx0, tx1) = (&start.rx, &end.rx, &start.tx, &end.tx);
        Self {
            rx: RxLink {
                mpdus: Rates::window(&rx0.mpdus, &rx1.mpdus),
                in_ampdu: rx1.in_ampdu.wrapping_sub(rx0.in_ampdu),
                single: rx1.single.wrapping_sub(rx0.single),
                containment_unknown: rx1
                    .containment_unknown
                    .wrapping_sub(rx0.containment_unknown),
            },
            tx: TxLink {
                mpdus: Rates::window(&tx0.mpdus, &tx1.mpdus),
                transmissions: tx1.transmissions.wrapping_sub(tx0.transmissions),
                delivered: tx1.delivered.wrapping_sub(tx0.delivered),
                ack_timeouts: tx1.ack_timeouts.wrapping_sub(tx0.ack_timeouts),
                other_ends: tx1.other_ends.wrapping_sub(tx0.other_ends),
                unsent: tx1.unsent.wrapping_sub(tx0.unsent),
                missing: core::array::from_fn(|bin| {
                    let at = |counts: &[u32; 7]| counts.get(bin).copied().unwrap_or(0);
                    at(&tx1.missing).wrapping_sub(at(&tx0.missing))
                }),
            },
        }
    }
}

/// The counts of [`Mac`].
pub const MAC_COUNTS: usize = 45;

/// Each of [`Mac`]'s counts: its name, as the vendor's decoders print it
/// or the register model names it, and the bits it wraps at.
pub const MAC_COUNT_NAMES: [(&str, u8); MAC_COUNTS] = [
    ("mpdu", 16),
    ("fcs_error", 16),
    ("abort", 16),
    ("abort_fcs_pass", 16),
    ("power_drop_error", 16),
    ("he_sig_b_error", 16),
    ("same_bm_error", 16),
    ("signal_field", 16),
    ("end", 16),
    ("data_success", 16),
    ("other_unicast", 16),
    ("buffer_full", 16),
    ("fifo_overflow", 16),
    ("tkip_error", 16),
    ("bt_block_error", 16),
    ("frequency_hop_error", 16),
    ("last_unmatched_error", 16),
    ("ack_interrupt", 16),
    ("rts_interrupt", 16),
    ("brx_agc", 10),
    ("brx", 10),
    ("nrx", 10),
    ("nrx_abort", 10),
    ("nrx_agc_exit", 10),
    ("nrx_baseband_off", 10),
    ("nrx_fdm_watchdog", 10),
    ("nrx_restart", 10),
    ("nrx_service", 10),
    ("nrx_tx_over", 10),
    ("nrx_unsupported", 10),
    ("nrx_he_format", 10),
    ("nrx_ht_sig", 10),
    ("nrx_he_unsupported", 10),
    ("nrx_he_sig_a_crc", 10),
    ("hang_rx", 8),
    ("hang_tx", 8),
    ("rx_tx_hang", 32),
    ("rx_tx_panic", 32),
    ("tx_rts", 32),
    ("tx_cts", 32),
    ("track", 32),
    ("trcts", 32),
    // The HE Trigger frames received, the HE TB PPDUs sent and the QoS
    // Nulls among them.
    ("he_rx_trigger", 16),
    ("he_tb", 16),
    ("he_tb_qos_null", 16),
];

/// What the MAC itself counted within run `run`'s window.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CollectMac {
    pub run: u32,
}

/// The answer to [`CollectMac`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[allow(
    clippy::large_enum_variant,
    reason = "a wire message of a no_std crate without allocation: the counts go by value"
)]
pub enum MacCollected {
    Report(Mac),
    /// The run's window or drain has not ended yet.
    Running,
    Refused(LoadError),
}

/// The MAC's own statistics, by [`MAC_COUNT_NAMES`], and its frequency
/// offset at their sample: a trace build samples the bank every 50 ms from
/// the port's turn, so a window's ends lag by up to that much. Others
/// report zeros.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Mac {
    /// The counts in two arrays: the wire's traits stop at 32.
    head: [u32; 32],
    tail: [u32; MAC_COUNTS - 32],
    pub frequency_offset: i32,
}

/// The aggregates a BlockAck answered at their first transmission within
/// run `run`'s window, by position on air.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CollectPositions {
    pub run: u32,
}

/// The answer to [`CollectPositions`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[allow(
    clippy::large_enum_variant,
    reason = "a wire message of a no_std crate without allocation: the counts go by value"
)]
pub enum PositionsCollected {
    Report(Positions),
    /// The run's window or drain has not ended yet.
    Running,
    Refused(LoadError),
}

/// See [`CollectPositions`], the first position at index 0.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Positions {
    /// Subframes that went at each position.
    pub sent: [u32; 32],
    /// Those left unacknowledged.
    pub holes: [u32; 32],
    /// Aggregates whose last subframes went missing together, by the
    /// length of that run, a run of one at index 0.
    pub tail_runs: [u32; 32],
    /// Aggregates with a hole whose last subframe was acknowledged.
    pub interior: u32,
}

impl Positions {
    /// The counts between readings `start` and `end`, wrapping at 2^32.
    pub fn window(start: &Self, end: &Self) -> Self {
        let between = |at: fn(&Self) -> &[u32; 32]| -> [u32; 32] {
            core::array::from_fn(|position| {
                let read = |reading: &Self| at(reading).get(position).copied().unwrap_or(0);
                read(end).wrapping_sub(read(start))
            })
        };
        Self {
            sent: between(|reading| &reading.sent),
            holes: between(|reading| &reading.holes),
            tail_runs: between(|reading| &reading.tail_runs),
            interior: end.interior.wrapping_sub(start.interior),
        }
    }
}

impl Mac {
    /// The reading of `counts` and `frequency_offset`.
    pub fn new(counts: [u32; MAC_COUNTS], frequency_offset: i32) -> Self {
        let at = |index: usize| counts.get(index).copied().unwrap_or(0);
        Self {
            head: core::array::from_fn(at),
            tail: core::array::from_fn(|index| at(index.saturating_add(32))),
            frequency_offset,
        }
    }

    /// Count `index`, by [`MAC_COUNT_NAMES`].
    pub fn count(&self, index: usize) -> u32 {
        self.head
            .iter()
            .chain(&self.tail)
            .nth(index)
            .copied()
            .unwrap_or(0)
    }

    /// The counts between readings `start` and `end`, each wrapping at its
    /// own width; the frequency offset is `end`'s.
    pub fn window(start: &Self, end: &Self) -> Self {
        let counts = core::array::from_fn(|index| {
            let bits = MAC_COUNT_NAMES.get(index).map_or(32, |(_, bits)| *bits);
            let moved = end.count(index).wrapping_sub(start.count(index));
            moved
                & u32::MAX
                    .checked_shr(32u32.saturating_sub(u32::from(bits)))
                    .unwrap_or(u32::MAX)
        });
        Self::new(counts, end.frequency_offset)
    }
}

/// What the shared RF actor's periodic PHY tracking did within run `run`'s
/// window.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CollectTracking {
    pub run: u32,
}

/// The answer to [`CollectTracking`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum TrackingCollected {
    Report(Tracking),
    /// The run's window or drain has not ended yet.
    Running,
    Refused(LoadError),
}

/// The shared RF actor's periodic PHY tracking ticks within one window:
/// the ticks by what they did, the tracked ticks that committed each
/// branch, the PHY's temperature after them and the ticks' durations. Only
/// a trace build counts them; others report `available: false` and zeros.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Tracking {
    /// Whether the image counts the ticks.
    pub available: bool,
    /// Ticks that ran the tracking graph.
    pub ticks: u32,
    /// Ticks whose deadline fired before any client's tracking fell due.
    pub not_due: u32,
    /// Ticks on a domain that could not track (not registered, RF closed).
    pub unavailable: u32,
    /// Tracked ticks the registered policy inhibited: no child ran.
    pub inhibited: u32,
    /// Tracked ticks that committed each branch: the RFPLL capacitance
    /// correction, a Wi-Fi PHY-I2C band change, a Wi-Fi or a Bluetooth LE /
    /// IEEE 802.15.4 TX power gain change, the common (RX) and the transmit
    /// recalibration.
    pub rfpll_corrected: u32,
    pub wifi_i2c_changed: u32,
    pub wifi_tx_power: u32,
    pub bluetooth_ieee802154_tx_power: u32,
    pub calibration_common: u32,
    pub calibration_transmit: u32,
    /// The PHY's temperature after the window's last tracked tick and its
    /// extremes over the window's tracked ticks, in PHY sensor units (no
    /// Celsius conversion is established); `None` without a tracked tick
    /// that read one.
    pub temperature_last: Option<i16>,
    pub temperature_min: Option<i16>,
    pub temperature_max: Option<i16>,
    /// The ticks' whole time and the longest tick that ended within the
    /// window; `None` without one.
    pub tick_us: u32,
    pub longest_tick_us: Option<u32>,
}

/// What the network core's turns did within run `run`'s window.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CollectNetwork {
    pub run: u32,
}

/// The answer to [`CollectNetwork`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum NetworkCollected {
    Report(NetworkTurns),
    /// The run's window or drain has not ended yet.
    Running,
    Refused(LoadError),
}

/// The bins of [`NetworkTurns::accepted_by_turn`]: turns whose port took 0,
/// 1, 2, 3, 4, 5 to 8, and 9 or more frames.
pub const ACCEPTED_BINS: usize = 7;

/// The bins of [`NetworkTurns::reserve_by_size`]: reservations asking for
/// 1, 2, 3 to 4, 5 to 8, and 9 to 16 blocks.
pub const RESERVE_BINS: usize = 5;

/// The network core's drop reasons, in the order of
/// [`NetworkTurns::drops`].
pub const DROP_NAMES: [&str; 18] = [
    "malformed",
    "checksum",
    "no_socket",
    "receive_full",
    "no_block",
    "group_limit",
    "too_long",
    "fragment",
    "bad_source",
    "reply_dropped",
    "send_only",
    "no_credit",
    "not_lent_stopping",
    "not_lent_socket",
    "pressure_copy",
    "lent_lane_full",
    "dhcp",
    "icmp",
];
/// The words of [`NetworkTurns::drops`].
pub const DROP_KINDS: usize = DROP_NAMES.len();

/// The bins of [`NetworkTurns::loans_at_turn_end`]: turns that ended with
/// 0, 1, 2, 3, 4, and 5 or more receive loans out.
pub const LOAN_BINS: usize = 6;

/// The network core's turns within one window: the requests they took from
/// the sockets' lanes, the sockets they left with requests when their
/// budget ran out, the frames the port accepted, how each turn's publishing
/// ended and what had run out when it ended, and how each pass after a turn
/// went on. Only a trace image on the network core counts them; others
/// report `available: false` and zeros.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct NetworkTurns {
    /// Whether the image counts the turns.
    pub available: bool,
    pub turns: u32,
    /// Requests the turns took, by kind.
    pub reserves: u32,
    pub releases: u32,
    pub sends: u32,
    pub closes: u32,
    /// Sockets whose lane still held requests when the turn had taken its
    /// budget of them, summed over the turns, and the turns with any.
    pub budget_spent: u32,
    pub budget_turns: u32,
    /// Turns that took in their whole budget of received frames.
    pub intake_full: u32,
    /// Frames the port accepted, and the turns by how many they had it take
    /// ([`ACCEPTED_BINS`]).
    pub accepted: u32,
    pub accepted_by_turn: [u32; ACCEPTED_BINS],
    /// Turns by how their publishing ended: the policy chose nothing, the
    /// port refused for room or link, publishing took its whole budget, it
    /// waited for untold notices, or a fault.
    pub publish_chose: u32,
    pub publish_refused: u32,
    pub publish_budget: u32,
    pub publish_deferred: u32,
    pub publish_fault: u32,
    /// Turns that ended with no free block, and with an open socket out of
    /// credits.
    pub blocks_out: u32,
    pub credits_out: u32,
    /// Passes after a turn that moved, yielded to the executor: all, and
    /// those after a turn with signs of possible work left (a spent budget,
    /// a full intake, publishing at its budget or deferred); a spent frame
    /// budget does not prove that a next frame exists.
    pub yields: u32,
    pub yields_with_work: u32,
    /// Waits after a turn that moved nothing: those that found nothing as
    /// they registered and so end their poll (unless a deadline was due),
    /// and those that found something and turn again in the same poll.
    pub waits_pending: u32,
    pub waits_ready: u32,
    /// The blocks the reservations asked for, and the reservations by how
    /// many ([`RESERVE_BINS`]): with `reserves`, the mean request size.
    pub reserve_blocks: u32,
    pub reserve_by_size: [u32; RESERVE_BINS],
    /// The local producer's steps: its offers, the core's final refusals
    /// and holds of them, declining fills, withdrawals of stale works, and
    /// the waits that registered it for new offers or for changes only.
    pub local_offered: u32,
    pub local_refused: u32,
    pub local_held: u32,
    pub local_declined: u32,
    pub local_withdrawn: u32,
    pub local_waits_offers: u32,
    pub local_waits_changes: u32,
    /// The producer's own counts: the wakes it gave the network's task, and
    /// its datagrams whose end is not known.
    pub producer_wakes: u32,
    pub producer_unknown: u32,
    /// The core's drops, by reason ([`DROP_NAMES`]).
    pub drops: [u32; DROP_KINDS],
    /// Turns by the receive loans out when they ended ([`LOAN_BINS`]), old
    /// installations' included: with a lending socket, how near its credit
    /// it was.
    pub loans_at_turn_end: [u32; LOAN_BINS],
}

impl NetworkTurns {
    /// The counts between readings `start` and `end`, wrapping at 2^32;
    /// available when `end` is.
    pub fn window(start: &Self, end: &Self) -> Self {
        let count = |at: fn(&Self) -> u32| at(end).wrapping_sub(at(start));
        Self {
            available: end.available,
            turns: count(|turns| turns.turns),
            reserves: count(|turns| turns.reserves),
            releases: count(|turns| turns.releases),
            sends: count(|turns| turns.sends),
            closes: count(|turns| turns.closes),
            budget_spent: count(|turns| turns.budget_spent),
            budget_turns: count(|turns| turns.budget_turns),
            intake_full: count(|turns| turns.intake_full),
            accepted: count(|turns| turns.accepted),
            accepted_by_turn: core::array::from_fn(|bin| {
                let at = |turns: &Self| turns.accepted_by_turn.get(bin).copied().unwrap_or(0);
                at(end).wrapping_sub(at(start))
            }),
            publish_chose: count(|turns| turns.publish_chose),
            publish_refused: count(|turns| turns.publish_refused),
            publish_budget: count(|turns| turns.publish_budget),
            publish_deferred: count(|turns| turns.publish_deferred),
            publish_fault: count(|turns| turns.publish_fault),
            blocks_out: count(|turns| turns.blocks_out),
            credits_out: count(|turns| turns.credits_out),
            yields: count(|turns| turns.yields),
            yields_with_work: count(|turns| turns.yields_with_work),
            waits_pending: count(|turns| turns.waits_pending),
            waits_ready: count(|turns| turns.waits_ready),
            reserve_blocks: count(|turns| turns.reserve_blocks),
            reserve_by_size: core::array::from_fn(|bin| {
                let at = |turns: &Self| turns.reserve_by_size.get(bin).copied().unwrap_or(0);
                at(end).wrapping_sub(at(start))
            }),
            local_offered: count(|turns| turns.local_offered),
            local_refused: count(|turns| turns.local_refused),
            local_held: count(|turns| turns.local_held),
            local_declined: count(|turns| turns.local_declined),
            local_withdrawn: count(|turns| turns.local_withdrawn),
            local_waits_offers: count(|turns| turns.local_waits_offers),
            local_waits_changes: count(|turns| turns.local_waits_changes),
            producer_wakes: count(|turns| turns.producer_wakes),
            producer_unknown: count(|turns| turns.producer_unknown),
            drops: core::array::from_fn(|kind| {
                let at = |turns: &Self| turns.drops.get(kind).copied().unwrap_or(0);
                at(end).wrapping_sub(at(start))
            }),
            loans_at_turn_end: core::array::from_fn(|bin| {
                let at = |turns: &Self| turns.loans_at_turn_end.get(bin).copied().unwrap_or(0);
                at(end).wrapping_sub(at(start))
            }),
        }
    }
}

/// The L1 cache's access counts within one window, by bus: the image
/// clears each bus's counts at the window's start and reads them at its
/// end. Instruction bus 0 carries hart 0's fetches and instruction bus 1
/// hart 1's. ESP-IDF requests data bus 0 for every hart's data and never
/// data bus 1; which hart's data accesses each data bus carries is not
/// established. Only accesses that reach the L1 cache count.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CacheCounts {
    pub ibus0: InstructionBus,
    pub ibus1: InstructionBus,
    pub dbus0: DataBus,
    pub dbus1: DataBus,
}

/// One instruction bus's accesses to its L1 instruction cache.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct InstructionBus {
    pub hits: u32,
    pub misses: u32,
    /// The accesses' conflicts, as the cache counts them.
    pub conflicts: u32,
    /// The L1 cache's reads of the L2 cache these accesses caused.
    pub next_level_reads: u32,
    /// A count of the bus passed its 32 bits within the window: the bus's
    /// counts are not exact.
    pub overflowed: bool,
}

/// One data bus's accesses to the L1 data cache.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct DataBus {
    pub hits: u32,
    pub misses: u32,
    /// The accesses' conflicts, as the cache counts them.
    pub conflicts: u32,
    /// The L1 cache's reads of the L2 cache these accesses caused.
    pub next_level_reads: u32,
    /// The write-backs to the L2 cache these accesses caused.
    pub next_level_writes: u32,
    /// A count of the bus passed its 32 bits within the window: the bus's
    /// counts are not exact.
    pub overflowed: bool,
}

/// The caches as configured: sizes and line sizes from their one-hot
/// configuration words (`None` when a word selects no single size), the
/// fixed L1 associativity and whether the L2 cache is bypassed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CacheGeometry {
    pub l1_icache: CacheLevel,
    pub l1_dcache: CacheLevel,
    pub l2: CacheLevel,
    pub l2_bypassed: bool,
}

/// One cache's configured geometry; `ways` is `None` where no source
/// states it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CacheLevel {
    pub size_bytes: Option<u32>,
    pub line_bytes: Option<u32>,
    pub ways: Option<u32>,
}

/// Hart `hart`'s samples of run `run`'s window, from the sample numbered
/// `from`: at most [`MAX_PAGE_SAMPLES`] an answer, fewer at the end. An
/// image without a sampler on that hart refuses with
/// [`LoadError::NoSampler`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CollectSamples {
    pub run: u32,
    pub from: u16,
    pub hart: u8,
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

/// One kept tick: where the sampled hart was when the timer interrupted it.
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

/// Hart 1's own counters at run `run`'s window's two ends.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CollectHart1 {
    pub run: u32,
}

/// The answer to [`CollectHart1`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Hart1Collected {
    Report(Hart1Window),
    /// The run's window or drain has not ended yet.
    Running,
    Refused(LoadError),
}

/// A hart's machine counters at one instant, from its reset, read on that
/// hart.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct HartCount {
    /// Machine cycles, wait-for-interrupt time included.
    pub cycles: u64,
    /// Instructions retired.
    pub instructions: u64,
}

/// Hart 1's counters at a window's very ends. Both travel, not their
/// difference, so that the host sees them advance within a window and from
/// one window to the next.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Hart1Window {
    pub start: HartCount,
    pub end: HartCount,
}

/// How long the image waits, after it asked, for its radio owner's stall
/// snapshot and for the turn after it ([`Stall`]).
pub const STALL_WAIT_MS: u32 = 200;

/// The receive path's stall snapshot of run `run`: the image asks its radio
/// owner for one at the window's end, after the window's own readings.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CollectStall {
    pub run: u32,
}

/// The answer to [`CollectStall`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[allow(
    clippy::large_enum_variant,
    reason = "a wire message of a no_std crate without allocation: the snapshot goes by value"
)]
pub enum StallCollected {
    Report(Stall),
    /// The run's window or drain has not ended yet.
    Running,
    Refused(LoadError),
}

/// Where the receive path last moved, by boundary, and what its owner read
/// of it at the window's end. Times are the image's monotonic microseconds,
/// the clock of [`Report::window_start_us`]. Only a trace build takes the
/// snapshot; the images that serve this service are trace builds.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Stall {
    /// When the image asked its radio owner.
    pub requested_us: u64,
    /// The socket boundary, which the load server stamps itself: the run's
    /// latest load datagram before the request, and its first one at or
    /// after the snapshot, until the drain's end.
    pub socket_last_us: Option<u64>,
    pub socket_after_us: Option<u64>,
    /// The owner's snapshot; `None` when it took none within
    /// [`STALL_WAIT_MS`] of the request: its task did not turn.
    pub snapshot: Option<RxSnapshot>,
}

/// The radio owner's snapshot, taken at the turn the request woke, before
/// that turn's receive service.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RxSnapshot {
    pub at_us: u64,
    /// The DMA boundary (descriptors of completed units a service pass
    /// observed) and the driver boundary (units the radio device handed to
    /// the station), until the snapshot.
    pub dma: StallBoundary,
    pub driver: StallBoundary,
    pub device: StallDevice,
    /// What the owner read with its registers; `None` while it held none
    /// (a retune in flight) or its receive session failed.
    pub hardware: Option<StallHardware>,
    /// The same boundaries at the owner's facts after the turn's receive
    /// pull; `None` when the owner recorded none within [`STALL_WAIT_MS`]
    /// of the request.
    pub resume: Option<StallResume>,
}

/// How far one boundary went since the station's receive path was bound
/// (wrapping), and when it last moved; `None` when it never moved.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StallBoundary {
    pub count: u32,
    pub last_us: Option<u64>,
}

/// The boundaries when the owner checked them after the snapshot: a
/// boundary resumed when its count moved past the snapshot's. `serviced`
/// says whether the turn's receive pull ran before the check; without it,
/// the check is the latest the owner made within [`STALL_WAIT_MS`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StallResume {
    pub checked_us: u64,
    pub dma: StallBoundary,
    pub driver: StallBoundary,
    pub serviced: bool,
}

/// The radio device's own state: ring work it knows of, a continuation it
/// waits for, a receive cut, a stop under way, a paused ring, and the units
/// it staged and holds for the station.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StallDevice {
    pub rx_pending: bool,
    pub continuation: bool,
    pub cut: bool,
    pub pausing: bool,
    pub paused: bool,
    pub queued: u32,
}

/// What the owner read with its registers.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StallHardware {
    pub walker: StallWalker,
    /// `None` without a live ring.
    pub ring: Option<StallRing>,
    pub session: StallSession,
}

/// The RX descriptor walker's registers and the MAC's own state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StallWalker {
    /// `RX_CONTROL`: the walker enable and the reload doorbell.
    pub enabled: bool,
    pub reload_doorbell: bool,
    /// `RX_NEXT_DESCRIPTOR`: the low address and whether its unassigned
    /// upper field read nonzero.
    pub next_low: u32,
    pub next_upper: bool,
    /// `RX_LAST_DESCRIPTOR`, low address.
    pub last_low: u32,
    /// `RX_DESCRIPTOR_BASE`, where the platform reads it.
    pub base: Option<u32>,
    /// The MAC's hardware starvation counter, where it has one.
    pub buffer_full: Option<u16>,
    /// The MAC runtime's active state; zero is idle.
    pub runtime_state: u8,
}

/// The live ring's own view: its descriptor base, recycle frontier and
/// accepted tail (indices), the descriptors returned to software in this
/// generation, the newly completed ones at the frontier, the accepted list
/// left from NEXT (`None` when NEXT lies outside the ring) and a reload it
/// requested that is still pending.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StallRing {
    pub base: u32,
    pub recycle_start: u32,
    pub accepted_tail: u32,
    pub observed: u32,
    pub completed: u32,
    pub remaining: Option<u32>,
    pub reload_pending: bool,
}

/// The receive session's owners and the software side of its interrupt
/// line. The MAC's interrupt registers belong to the radio handler and are
/// not read.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StallSession {
    /// The ring's descriptors by their first word: all of them, `OWNER`
    /// without `RX_DONE` (armed for the walker, or a filled descriptor of a
    /// unit not done), `OWNER` with `RX_DONE` (a completed unit's last
    /// descriptor), `OWNER` clear (its buffer with software); and the
    /// reserved descriptor the walker starts an epoch from, by the same bits.
    pub descriptors: u32,
    pub armed: u32,
    pub done: u32,
    pub software: u32,
    pub reserved_owner: bool,
    pub reserved_done: bool,
    /// Buffers released for rearm, buffers detached as staged leases (those
    /// the station and the network hold included) and claimed staging slots.
    pub released: u32,
    pub detached: u32,
    pub staged: u32,
    /// The session masked both banks and froze ingress (a cut or a stop).
    pub masked: bool,
    pub frozen: bool,
    /// A request to the interrupt line its handler has not applied.
    pub line_request_pending: bool,
    /// A receive fact the line posted that no poll took yet; `None` when
    /// the owner could not look.
    pub receive_fact_pending: Option<bool>,
    /// Radio handler entries, RX unmasks it applied and ignored, and
    /// service passes, since the receive path was bound (wrapping).
    pub mac_irqs: u32,
    pub doorbell_unmasks: u32,
    pub ignored_unmasks: u32,
    pub service_passes: u32,
}

/// The MAC statistics bank at run `run`'s stall snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CollectStallMac {
    pub run: u32,
}

/// The answer to [`CollectStallMac`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[allow(
    clippy::large_enum_variant,
    reason = "a wire message of a no_std crate without allocation: the banks go by value"
)]
pub enum StallMacCollected {
    Report(StallMac),
    /// The run's window or drain has not ended yet.
    Running,
    Refused(LoadError),
}

/// The MAC statistics bank, whole counts by [`MAC_COUNT_NAMES`], as the
/// owner read it at the stall snapshot, beside the periodic sample of it its
/// turn keeps: their difference is what the MAC counted since that sample.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StallMac {
    /// `None` when the owner took no snapshot or has no bank.
    pub fresh: Option<Mac>,
    /// The periodic sample and when it was taken; `None` before the first.
    pub periodic: Option<Mac>,
    pub periodic_us: Option<u64>,
}

/// The ADDBA Requests the station took since boot, at the end of run
/// `run`'s window.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CollectAddba {
    pub run: u32,
}

/// The answer to [`CollectAddba`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum AddbaCollected {
    Report(Addba),
    /// The run's window or drain has not ended yet.
    Running,
    Refused(LoadError),
}

/// The TIDs an ADDBA Request can name: its four-bit field.
pub const ADDBA_TIDS: usize = 16;

/// The ADDBA Requests the station's receive agreements took since boot:
/// the last of each TID and the requests by outcome. Only a trace build
/// counts them; others report none.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Addba {
    /// The last request of each TID, `None` for a TID without one.
    pub last: [Option<AddbaRequest>; ADDBA_TIDS],
    pub accepted: u32,
    pub repeated: u32,
    pub refused: u32,
    pub unanswered: u32,
}

/// One ADDBA Request: the Buffer Size the access point asked for as
/// received (zero asks the station to choose) and what became of it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct AddbaRequest {
    pub requested: u16,
    pub outcome: AddbaOutcome,
}

/// What became of one ADDBA Request, as the station's recipient names it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum AddbaOutcome {
    /// A new agreement started with `window`.
    Accepted { window: u16 },
    /// The running agreement's own request again, answered with its
    /// `window`.
    Repeated { window: u16 },
    /// Refused with this IEEE 802.11 status code.
    Refused { status: u16 },
    /// No reply place or bank remained: it went unanswered.
    Unanswered,
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

oer_protocols_hil_agent::service! {
    /// The load service's requests.
    pub enum Request (sessions = false, handler = Handler) {
        Start(Start) => start,
        Collect(Collect) => collect,
        GetCpu(GetCpu) => cpu,
        CollectParts(CollectParts) => parts,
        CollectCounters(CollectCounters) => counters,
        CollectSamples(CollectSamples) => samples,
        CollectLink(CollectLink) => link,
        CollectTracking(CollectTracking) => tracking,
        CollectMac(CollectMac) => mac,
        CollectPositions(CollectPositions) => positions,
        CollectHart1(CollectHart1) => hart1,
        CollectStall(CollectStall) => stall,
        CollectStallMac(CollectStallMac) => stall_mac,
        CollectNetwork(CollectNetwork) => network,
        CollectAddba(CollectAddba) => addba,
    }
}

#[cfg(test)]
mod tests;
