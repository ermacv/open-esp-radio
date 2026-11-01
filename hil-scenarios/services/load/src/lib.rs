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
//! A run can also sample where the image's measured hart (its README names
//! it) spends its window
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
    endpoint CollectLink = "load/link" => LinkCollected;
    topic LinkCollected = "load/link-report";
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

/// Sample the measured hart's interrupted program counter every `period_us`
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
    /// Machine cycles of the image's measured hart, wait-for-interrupt
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
    /// The L1 cache's access counts within the window.
    pub cache: CacheCounts,
    /// The cache geometry at the window's end.
    pub cache_geometry: CacheGeometry,
    /// The receive path's counts within the window: the S31 driver's and
    /// the station service's. Only a trace build counts them; others report
    /// zeros.
    pub rx: RxCounts,
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
    /// The port reported lost events.
    pub port_events_lost: u32,
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
    /// no network.
    pub refused_too_long: u32,
    pub refused_queue_full: u32,
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
            port_events_lost: end.port_events_lost.wrapping_sub(start.port_events_lost),
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

/// The station's single data MPDU transmissions that ended: by the rate of
/// each one's last transmission, the transmissions they took, and how they
/// ended. Aggregates are not counted here.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct TxLink {
    /// Ended MPDUs by their last transmission's rate; one that reports no
    /// transmission counts as unknown.
    pub mpdus: Rates,
    /// Their transmissions, retries included.
    pub transmissions: u32,
    pub delivered: u32,
    pub ack_timeouts: u32,
    /// A protection timeout, the contention limit or a coexistence denial.
    pub other_ends: u32,
    /// Expired, withdrawn, or without a known outcome.
    pub unsent: u32,
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
            },
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

/// One kept tick: where the measured hart was when the timer interrupted it.
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
        CollectLink(CollectLink) => link,
    }
}

#[cfg(test)]
mod tests;
