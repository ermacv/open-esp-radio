//! Independent passive 802.11 evidence from the laptop radio.

use crate::capture_process::{self, Capture};
use crate::channel::Geometry;
use oer_process::CommandExt as _;
use std::io::Write as _;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use crate::air::{self, AirFrame, FrameKind, MacAddress};
use crate::{Result, openwrt::evidence::resolve_station_mac, openwrt::tx_monitor::MacFrameKey};
use oer_hil_lab::config::OpenWrtConfig;
use oer_hil_run_bundle_format::run::fixtures::{
    self, AirIntervalSummary, AirMonitor, TargetEgressAirTiming,
};

const MONITOR_INTERFACE: &str = "mon0";
const MAX_CAPTURE_BYTES: u64 = 128 * 1024 * 1024;
const RETRY_GROUP_MICROS: u64 = 100_000;

/// Exercise monitor setup, capture readiness, decoding and restoration without
/// contacting a target. The synthetic MAC is only a parser filter, never TX.
pub fn check_without_device(config: &OpenWrtConfig, output: &Path) -> Result<()> {
    let geometry = resolve_observer_action(config)?;
    let capture = LocalAirMonitorCapture::start_for_target(
        "02:00:00:00:00:01".into(),
        Duration::from_secs(1),
        output,
        Some(geometry),
        true,
    )?;
    let evidence = capture.finish()?;
    fixtures::write(output, &evidence.record())
}

#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct LocalAirMonitorEvidence {
    pub captured_frames: u64,
    pub kernel_dropped: u64,
    pub logical_data_units: u32,
    pub retry_attempts: u32,
    pub missing_mac_metadata: u32,
    #[serde(skip)]
    pub mac_units: BTreeMap<MacFrameKey, u32>,
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

impl LocalAirMonitorEvidence {
    /// The fixture record of this evidence, without the per-frame MAC units
    /// only the live comparison uses.
    pub fn record(&self) -> AirMonitor {
        AirMonitor {
            captured_frames: self.captured_frames,
            kernel_dropped: self.kernel_dropped,
            logical_data_units: self.logical_data_units,
            retry_attempts: self.retry_attempts,
            missing_mac_metadata: self.missing_mac_metadata,
            block_ack_frames: self.block_ack_frames,
            full_block_ack_frames: self.full_block_ack_frames,
            tail_block_ack_frames: self.tail_block_ack_frames,
            hole_block_ack_frames: self.hole_block_ack_frames,
            unique_block_acked_mpdus: self.unique_block_acked_mpdus,
            backward_block_ack_starts: self.backward_block_ack_starts,
            target_egress: self.target_egress.clone(),
        }
    }
}

pub struct LocalAirMonitorCapture {
    output: PathBuf,
    target_mac: String,
    child: Option<Capture>,
    owns_monitor: bool,
}

impl LocalAirMonitorCapture {
    pub fn start(
        config: &OpenWrtConfig,
        target: Ipv4Addr,
        duration: Duration,
        output: &Path,
    ) -> Result<Self> {
        if !config.independent_laptop_monitor {
            return Err(crate::Error::new("scenario requires the independent laptop monitor, but the fixture does not enable it").into());
        }
        let target_mac = resolve_station_mac(config, target)?;
        Self::start_for_target(
            target_mac,
            duration,
            output,
            Some(resolve_observer_action(config).map_err(crate::Error::context)?),
            true,
        )
    }

    /// Observe an AP while the laptop's managed interface remains its traffic
    /// client. The compatibility monitor joins the existing channel instead
    /// of replacing `wlan0` with a standalone monitor wdev.
    pub fn start_associated(target_mac: String, duration: Duration, output: &Path) -> Result<Self> {
        Self::start_for_target(target_mac, duration, output, None, false)
    }

    fn start_for_target(
        target_mac: String,
        duration: Duration,
        output: &Path,
        geometry: Option<Geometry>,
        restore_managed: bool,
    ) -> Result<Self> {
        let mut owner = Self {
            output: output.join("independent-air.pcapng"),
            target_mac,
            child: None,
            owns_monitor: restore_managed,
        };
        if let Some(geometry) = geometry {
            let mut command = oer_process::command("sudo");
            command
                .args(["-n", crate::local::network_helper::PATH, "observer"])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped());
            let mut child = command.spawn_owned()?;
            writeln!(
                child.stdin.take().ok_or("observer stdin unavailable")?,
                "{} {}",
                geometry.frequency,
                geometry.iw_width()?
            )?;
            let result = child.wait_with_output_timeout(Some(Duration::from_secs(20)))?;
            if !result.status.success() {
                return Err(crate::Error::new(format!(
                    "cannot prepare observer: {}",
                    String::from_utf8_lossy(&result.stderr)
                ))
                .into());
            }
            let observed = oer_process::command("iw")
                .args(["dev", MONITOR_INTERFACE, "info"])
                .supervised_output()?;
            if !observed.status.success()
                || Geometry::parse(&String::from_utf8(observed.stdout)?)? != geometry
            {
                return Err(crate::Error::new(
                    "observer channel differs from the active OpenWrt channel",
                )
                .into());
            }
        } else {
            helper_action("monitor")?;
        }
        owner.child = Some(capture_process::dumpcap(
            MONITOR_INTERFACE,
            None,
            512,
            &owner.output,
            duration,
        )?);
        Ok(owner)
    }

    pub fn finish(mut self) -> Result<LocalAirMonitorEvidence> {
        let output = self
            .child
            .take()
            .expect("independent monitor capture owns its child")
            .finish();
        let restore = self.restore_managed();
        let output = output?;
        restore?;
        if !output.status.success() {
            return Err(crate::Error::new(format!(
                "independent laptop capture failed with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ))
            .into());
        }
        let summary = String::from_utf8(output.stderr)?;
        let captured_frames = dumpcap_captured(&summary).map_err(crate::Error::context)?;
        let kernel_dropped = dumpcap_dropped(&summary).map_err(crate::Error::context)?;
        if kernel_dropped != 0 {
            return Err(crate::Error::new(format!(
                "independent laptop capture dropped {kernel_dropped} packets in its capture socket"
            ))
            .into());
        }
        let size = fs::metadata(&self.output)?.len();
        if size == 0 || size > MAX_CAPTURE_BYTES {
            return Err(crate::Error::new(format!(
                "independent laptop capture size is outside 1..={MAX_CAPTURE_BYTES} bytes: {size}"
            ))
            .into());
        }
        let mut evidence =
            parse_capture(&self.output, &self.target_mac).map_err(crate::Error::context)?;
        evidence.captured_frames = captured_frames;
        evidence.kernel_dropped = kernel_dropped;
        Ok(evidence)
    }

    fn restore_managed(&mut self) -> Result<()> {
        if self.owns_monitor {
            helper_action("managed")?;
            self.owns_monitor = false;
        }
        Ok(())
    }
}

pub(crate) fn resolve_observer_action(config: &OpenWrtConfig) -> Result<Geometry> {
    let output = oer_stand_ssh::command(
        &config.ssh_target,
        &format!("iw dev {} info", config.wireless_interface),
    )
    .supervised_output()?;
    if !output.status.success() {
        return Err(crate::Error::new(format!(
            "cannot query OpenWrt channel for independent monitor: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .into());
    }
    resolve_observer_action_from_iw(&String::from_utf8(output.stdout)?)
}

fn resolve_observer_action_from_iw(info: &str) -> Result<Geometry> {
    Geometry::parse(info).map_err(crate::Error::context)
}

impl Drop for LocalAirMonitorCapture {
    fn drop(&mut self) {
        oer_process::cleanup(|| {
            drop(self.child.take());
            oer_hil_workload::fixture::cleanup::record("restore monitor interface", || {
                self.restore_managed()
            });
        });
    }
}

pub fn doctor() -> Result<()> {
    crate::local::network_helper::doctor()?;
    for tool in ["dumpcap", "tshark"] {
        let status = oer_process::command(tool)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .supervised_status()?;
        if !status.success() {
            return Err(crate::Error::new(format!(
                "`{tool}` is required for independent air evidence"
            ))
            .into());
        }
    }
    Ok(())
}

pub(crate) fn parse_capture(path: &Path, target_mac: &str) -> Result<LocalAirMonitorEvidence> {
    let frames = air::decode(
        path,
        &format!(
            "(wlan.fc.type == 2 && (wlan.da == {target_mac} || wlan.ta == {target_mac})) || \
             (wlan.fc.type_subtype == 0x0019 && (wlan.ta == {target_mac} || wlan.ra == {target_mac}))"
        ),
        air::Payload::Omit,
    )?;
    Ok(analyze(&frames, target_mac.parse()?))
}

fn analyze(frames: &[AirFrame], target: MacAddress) -> LocalAirMonitorEvidence {
    let mut evidence = LocalAirMonitorEvidence::default();
    let mut last_observed = BTreeMap::<MacFrameKey, u64>::new();
    for frame in frames
        .iter()
        .filter(|frame| frame.kind.is_data() && frame.destination == Some(target))
    {
        let retry = frame.retry == Some(true);
        let Some(key) = frame
            .sequence
            .zip(frame.fragment)
            .map(|(sequence, fragment)| MacFrameKey {
                tid: frame.tid.unwrap_or(u8::MAX),
                sequence,
                fragment,
            })
        else {
            evidence.missing_mac_metadata = evidence.missing_mac_metadata.saturating_add(1);
            continue;
        };
        if retry {
            evidence.retry_attempts = evidence.retry_attempts.saturating_add(1);
        }
        let is_new = !retry
            || last_observed
                .get(&key)
                .is_none_or(|previous| frame.time_micros - previous > RETRY_GROUP_MICROS);
        last_observed.insert(key, frame.time_micros);
        if is_new {
            evidence.logical_data_units = evidence.logical_data_units.saturating_add(1);
            let count = evidence.mac_units.entry(key).or_default();
            *count = count.saturating_add(1);
        }
    }
    let mut tracker = BlockAckTracker::default();
    for block_ack in frames
        .iter()
        .filter(|frame| frame.kind == FrameKind::BLOCK_ACK && frame.transmitter == Some(target))
        .filter_map(|frame| frame.block_ack)
    {
        tracker.observe(block_ack.start_sequence, block_ack.bitmap);
    }
    evidence.block_ack_frames = tracker.frames;
    evidence.full_block_ack_frames = tracker.full_frames;
    evidence.tail_block_ack_frames = tracker.tail_frames;
    evidence.hole_block_ack_frames = tracker.hole_frames;
    evidence.unique_block_acked_mpdus =
        u32::try_from(tracker.acknowledged.len()).unwrap_or(u32::MAX);
    evidence.backward_block_ack_starts = tracker.backward_starts;
    evidence.target_egress = target_egress_timing(frames, target);
    evidence
}

fn target_egress_timing(frames: &[AirFrame], target: MacAddress) -> TargetEgressAirTiming {
    let mut target_data_frames = 0_u32;
    let mut peer_block_ack_frames = 0_u32;
    let mut last_target_data = None;
    let mut previous_block_ack = None;
    let mut pending_block_ack = None;
    let mut peer_block_ack_interarrival = Vec::new();
    let mut data_to_block_ack = Vec::new();
    let mut block_ack_to_next_data = Vec::new();

    for frame in frames {
        let timestamp = frame.time_micros;
        if frame.kind.is_data() && frame.transmitter == Some(target) {
            target_data_frames = target_data_frames.saturating_add(1);
            if let Some(block_ack) = pending_block_ack.take()
                && let Some(interval) = timestamp.checked_sub(block_ack)
            {
                block_ack_to_next_data.push(interval);
            }
            // An A-MPDU can be exposed as multiple MPDU records. Keeping the
            // final data timestamp before the BlockAck measures from the
            // observed end of the PPDU, not from its first subframe.
            last_target_data = Some(timestamp);
        } else if frame.kind == FrameKind::BLOCK_ACK && frame.receiver == Some(target) {
            peer_block_ack_frames = peer_block_ack_frames.saturating_add(1);
            if let Some(previous) = previous_block_ack
                && let Some(interval) = timestamp.checked_sub(previous)
            {
                peer_block_ack_interarrival.push(interval);
            }
            previous_block_ack = Some(timestamp);
            if let Some(target_data) = last_target_data.take()
                && let Some(interval) = timestamp.checked_sub(target_data)
            {
                data_to_block_ack.push(interval);
                pending_block_ack = Some(timestamp);
            }
        }
    }

    let target_data_pairing_available = peer_block_ack_frames != 0
        && usize::try_from(peer_block_ack_frames).ok() == Some(data_to_block_ack.len());
    TargetEgressAirTiming {
        target_data_frames,
        peer_block_ack_frames,
        target_data_pairing_available,
        peer_block_ack_interarrival: summarize_intervals(peer_block_ack_interarrival),
        data_to_block_ack: target_data_pairing_available
            .then(|| summarize_intervals(data_to_block_ack))
            .flatten(),
        block_ack_to_next_data: target_data_pairing_available
            .then(|| summarize_intervals(block_ack_to_next_data))
            .flatten(),
    }
}

fn summarize_intervals(mut intervals: Vec<u64>) -> Option<AirIntervalSummary> {
    if intervals.is_empty() {
        return None;
    }
    intervals.sort_unstable();
    let samples = u32::try_from(intervals.len()).unwrap_or(u32::MAX);
    Some(AirIntervalSummary {
        samples,
        total_micros: intervals.iter().copied().fold(0_u64, u64::saturating_add),
        minimum_micros: intervals[0],
        p50_micros: oer_hil_net_traffic::icmp::nearest_rank(&intervals, 50),
        p95_micros: oer_hil_net_traffic::icmp::nearest_rank(&intervals, 95),
        p99_micros: oer_hil_net_traffic::icmp::nearest_rank(&intervals, 99),
        maximum_micros: *intervals.last().expect("nonempty interval sample"),
    })
}

#[derive(Default)]
struct BlockAckTracker {
    frames: u32,
    full_frames: u32,
    tail_frames: u32,
    hole_frames: u32,
    backward_starts: u32,
    previous_start: Option<u16>,
    unwrapped_start: u64,
    acknowledged: BTreeSet<u64>,
}

impl BlockAckTracker {
    fn observe(&mut self, start: u16, bitmap: [u8; 8]) {
        let start = start & 0x0fff;
        if let Some(previous) = self.previous_start {
            let advance = start.wrapping_sub(previous) & 0x0fff;
            if advance > 0x0800 {
                self.backward_starts = self.backward_starts.saturating_add(1);
                return;
            }
            self.unwrapped_start = self.unwrapped_start.saturating_add(u64::from(advance));
        } else {
            self.unwrapped_start = u64::from(start);
        }
        self.previous_start = Some(start);
        self.frames = self.frames.saturating_add(1);
        if bitmap == [u8::MAX; 8] {
            self.full_frames = self.full_frames.saturating_add(1);
        } else if block_ack_bitmap_has_internal_hole(bitmap) {
            self.hole_frames = self.hole_frames.saturating_add(1);
        } else {
            self.tail_frames = self.tail_frames.saturating_add(1);
        }
        for (byte_index, byte) in bitmap.into_iter().enumerate() {
            for bit in 0..8_u8 {
                if byte & (1_u8 << bit) != 0 {
                    self.acknowledged.insert(
                        self.unwrapped_start
                            .saturating_add((byte_index * 8 + usize::from(bit)) as u64),
                    );
                }
            }
        }
    }
}

/// Return true when a later MPDU is acknowledged after an earlier zero bit.
///
/// A non-full compressed BlockAck bitmap is not itself loss evidence. During
/// window growth or at the tail of a transfer it normally contains a prefix
/// of set bits followed by zeroes. Only a set bit after the first zero proves
/// that the receiver observed a hole inside the represented sequence range.
fn block_ack_bitmap_has_internal_hole(bitmap: [u8; 8]) -> bool {
    let mut observed_zero = false;
    for byte in bitmap {
        for bit in 0..8_u8 {
            if byte & (1_u8 << bit) == 0 {
                observed_zero = true;
            } else if observed_zero {
                return true;
            }
        }
    }
    false
}

fn helper_action(action: &str) -> Result<()> {
    let status = oer_process::command("sudo")
        .args(["-n", crate::local::network_helper::PATH, action])
        .supervised_status()?;
    if !status.success() {
        return Err(crate::Error::new(format!(
            "laptop radio helper `{action}` failed with {status}"
        ))
        .into());
    }
    Ok(())
}

pub(crate) fn dumpcap_captured(summary: &str) -> Result<u64> {
    summary
        .lines()
        .find_map(|line| line.trim().strip_prefix("Packets captured:"))
        .ok_or("independent capture omitted its packet count")?
        .trim()
        .parse()
        .map_err(|error| format!("invalid independent captured packet count: {error}").into())
}

pub(crate) fn dumpcap_dropped(summary: &str) -> Result<u64> {
    let counts = summary
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("Packets received/dropped on interface")?
                .split_once(':')
                .map(|(_, counts)| counts.trim())
        })
        .ok_or("independent capture omitted its drop count")?;
    counts
        .split_once('/')
        .and_then(|(_, dropped)| dropped.split_whitespace().next())
        .ok_or("independent capture reported a malformed drop count")?
        .parse()
        .map_err(|error| format!("invalid independent dropped packet count: {error}").into())
}

#[cfg(test)]
mod tests;
