//! Typed records of a repetition's fixtures and host traffic: what the
//! laboratory's fixtures applied and observed, written beside the
//! repetition's result.
//!
//! A record is a [`FixtureRecord`]: its type is its schema and fixes its
//! file name. Fixture code writes one through [`write()`] (or
//! [`write_labelled`], for a kind a directory holds several of) into the
//! repetition's artifact directory, and readers (the analyses) take it back
//! through [`crate::format::RunBundle::fixture_record`]; no fixture writes a report
//! file of its own.
//!
//! - [`Applied`] (`fixture-applied.json`): the station fixture access
//!   point's applied settings, of an OpenWrt router or a local Linux
//!   `hostapd`;
//! - [`Protection`] (`fixture-protection.json`): the beacon protection an
//!   induced non-HT member or legacy BSS made the fixture access point
//!   advertise;
//! - [`AirMonitor`] (`fixture-monitor.json`): the independent laptop
//!   monitor's passive 802.11 evidence of a fixture check;
//! - [`Reception`] (`<label>-reception.json`): one host UDP reception of a
//!   target's transmission;
//! - [`Helper`] (`helper.json`): a privileged fixture helper's report, of
//!   the helper's own runner contract (`oer_hil_fixture`).

use std::{net::Ipv4Addr, path::Path};

use oer_hil_model::catalog::link::{
    AccessPointBeacon, AccessPointSecurity, ManagementFrameProtection, PhyExpectation,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::format::Result;

/// A typed fixture record and its file name.
pub trait FixtureRecord: Serialize + DeserializeOwned {
    /// The file the record is kept in, in the directory its fixture records
    /// into.
    const FILE: &'static str;
}

fn file_name<R: FixtureRecord>(label: Option<&str>) -> String {
    match label {
        Some(label) => format!("{label}-{}", R::FILE),
        None => R::FILE.to_owned(),
    }
}

/// Write `record` into `directory`, atomically.
pub fn write<R: FixtureRecord>(directory: &Path, record: &R) -> Result<()> {
    oer_os_durable::atomic_json(&directory.join(file_name::<R>(None)), record)
}

/// Write `record` as the record `label` of its kind (`<label>-<FILE>`).
pub fn write_labelled<R: FixtureRecord>(directory: &Path, label: &str, record: &R) -> Result<()> {
    oer_os_durable::atomic_json(&directory.join(file_name::<R>(Some(label))), record)
}

/// The record of type `R` (labelled `label`) in `directory`, or `None`
/// when the fixture recorded none. Bundle readers use
/// [`crate::format::RunBundle::fixture_record`].
pub(crate) fn read<R: FixtureRecord>(directory: &Path, label: Option<&str>) -> Result<Option<R>> {
    super::validation::read_optional_json(&directory.join(file_name::<R>(label)))
}

/// The station fixture access point's applied settings.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Applied {
    OpenWrt(OpenWrtApplied),
    Local(LocalApplied),
}

impl FixtureRecord for Applied {
    const FILE: &'static str = "fixture-applied.json";
}

/// An OpenWrt router's access point: the profile the scenario requested,
/// the router's settings before it, and what the radio runs.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OpenWrtApplied {
    pub schema: u16,
    pub requested: OpenWrtProfile,
    /// The router's own profile was verified, never changed.
    pub read_only: bool,
    pub before: OpenWrtBefore,
    pub applied: OpenWrtRadio,
}

/// The access point profile a scenario requests of an OpenWrt router.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct OpenWrtProfile {
    pub ht40_above: bool,
    pub phy: PhyExpectation,
    pub channel: u8,
    pub management_frame_protection: ManagementFrameProtection,
    pub access_point_security: AccessPointSecurity,
    /// The beacon schedule the scenario sets; `None` keeps the router's.
    pub beacon: Option<AccessPointBeacon>,
}

/// The router's UCI settings before the scenario, as `ubus` reports them
/// (a value of any JSON type, `null` when unset).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OpenWrtBefore {
    pub up: serde_json::Value,
    pub channel: serde_json::Value,
    pub htmode: serde_json::Value,
    pub beacon_int: serde_json::Value,
    pub dtim_period: serde_json::Value,
}

/// What an OpenWrt router's radio runs.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct OpenWrtRadio {
    pub enabled: bool,
    pub channel: u8,
    pub geometry: String,
    pub htmode: String,
    pub ht: bool,
    pub he: bool,
    /// The beacon interval and DTIM period hostapd runs, from its generated
    /// configuration; `None` when it states none.
    #[serde(default)]
    pub beacon_interval_tu: Option<u16>,
    #[serde(default)]
    pub dtim_period: Option<u8>,
}

/// A local Linux `hostapd` access point, verified.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LocalApplied {
    pub schema: u16,
    /// `local-linux`.
    pub backend: String,
    pub verified: bool,
    pub phy: PhyExpectation,
    pub channel: u8,
    pub country: String,
    pub frequency_mhz: u16,
    pub width_mhz: u16,
    pub center1_mhz: u16,
    pub address: Ipv4Addr,
    pub prefix_length: u8,
    /// The laboratory's 20/40 MHz coexistence policy (`respect`,
    /// `force-ht40`).
    pub coexistence: String,
}

/// The beacon protection an induced protection peer made the fixture
/// access point advertise, window by window.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Protection {
    pub schema: u16,
    pub bssid: String,
    /// `false` when no window satisfied the scenario; absent when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub established: Option<bool>,
    pub non_ht_member: bool,
    pub legacy_bss: bool,
    pub windows: Vec<BeaconProtection>,
}

impl FixtureRecord for Protection {
    const FILE: &'static str = "fixture-protection.json";
}

/// The protection one window of beacons advertised.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BeaconProtection {
    pub beacons: usize,
    /// Every beacon carried ERP Use_Protection.
    pub erp_use_protection: bool,
    /// The HT Protection field when every beacon agreed on it.
    pub ht_protection: Option<u8>,
}

/// The independent laptop monitor's passive 802.11 evidence.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AirMonitor {
    pub captured_frames: u64,
    pub kernel_dropped: u64,
    pub logical_data_units: u32,
    pub retry_attempts: u32,
    pub missing_mac_metadata: u32,
    pub block_ack_frames: u32,
    pub full_block_ack_frames: u32,
    pub tail_block_ack_frames: u32,
    pub hole_block_ack_frames: u32,
    pub unique_block_acked_mpdus: u32,
    pub backward_block_ack_starts: u32,
    /// Target-oriented egress timing. This is deliberately independent of
    /// whether the target is a station or an access point.
    pub target_egress: TargetEgressAirTiming,
}

impl FixtureRecord for AirMonitor {
    const FILE: &'static str = "fixture-monitor.json";
}

/// The air timing of a target's transmissions and its peer's BlockAcks.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TargetEgressAirTiming {
    pub target_data_frames: u32,
    pub peer_block_ack_frames: u32,
    /// Whether the observer decoded enough target data records to pair every
    /// peer BlockAck with a target transmission. Pair-derived intervals stay
    /// absent when this is false; sparse target decoding must not manufacture
    /// apparently valid multi-millisecond gaps.
    pub target_data_pairing_available: bool,
    /// Direction-neutral cadence of peer BlockAck responses to the target.
    /// This remains useful when the observer cannot decode the target's HT40
    /// A-MPDU records, but it cannot separate peer response time from the
    /// target's post-BlockAck scheduling delay.
    pub peer_block_ack_interarrival: Option<AirIntervalSummary>,
    pub data_to_block_ack: Option<AirIntervalSummary>,
    pub block_ack_to_next_data: Option<AirIntervalSummary>,
}

/// The distribution of one kind of air interval.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AirIntervalSummary {
    pub samples: u32,
    pub total_micros: u64,
    pub minimum_micros: u64,
    pub p50_micros: u64,
    pub p95_micros: u64,
    pub p99_micros: u64,
    pub maximum_micros: u64,
}

/// One host UDP reception of a target's transmission, up to its end.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct Reception {
    pub schema: u16,
    /// Datagrams the host socket dropped in the kernel during the session.
    pub host_kernel_drops: Option<u32>,
    pub completion: ReceptionEnd,
    /// The count the target's terminal marker announced.
    pub expected_datagrams: Option<u64>,
    pub received_unique_datagrams: u64,
    pub undelivered_datagrams: Option<u64>,
    pub elapsed_micros: u64,
    pub bursts: Vec<Burst>,
    pub error: Option<String>,
}

impl FixtureRecord for Reception {
    const FILE: &'static str = "reception.json";
}

/// How a reception ended.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReceptionEnd {
    Delivered,
    DeliveryDeadline,
    TargetUnavailable,
    SessionDeadline,
    Cancelled,
    ReceiveError,
    HostOverflow,
    Aborted,
}

/// One burst of datagrams a reception counted.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Burst {
    pub bytes: u64,
    pub datagrams: u64,
    pub missing: u64,
    pub reordered: u64,
    pub first_reordered_after: Option<u32>,
    pub first_reordered_sequence: Option<u32>,
    pub maximum_reorder_distance: u32,
    pub duplicates: u64,
    pub elapsed_us: u64,
    pub started_at_zero: bool,
    pub lowest_sequence: u32,
    pub highest_sequence: u32,
    pub maximum_interarrival_us: u64,
    pub sequence_after_maximum_interarrival: Option<u32>,
    pub missing_runs: u64,
    pub maximum_missing_run: u64,
    pub maximum_missing_run_start: Option<u32>,
    pub maximum_missing_run_end: Option<u32>,
}

impl Burst {
    /// Payload throughput over the burst's elapsed time.
    pub fn throughput_kbps(self) -> u64 {
        self.bytes
            .saturating_mul(8)
            .saturating_mul(1_000)
            .checked_div(self.elapsed_us.max(1))
            .unwrap_or(0)
    }
}

/// A privileged fixture helper's report, `R` of the helper's own runner
/// contract, kept as the helper printed it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Helper<R>(pub R);

impl<R: Serialize + DeserializeOwned> FixtureRecord for Helper<R> {
    const FILE: &'static str = "helper.json";
}
