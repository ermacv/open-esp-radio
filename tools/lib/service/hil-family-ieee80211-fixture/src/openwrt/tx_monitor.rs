//! Opt-in packet evidence from the OpenWrt AP's own TX monitor tap.

use crate::air;
use crate::openwrt::capture::RemoteCapture;
use oer_process::CommandExt as _;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    net::Ipv4Addr,
    path::Path,
    process::Stdio,
    time::Duration,
};

use crate::{Result, openwrt::evidence::resolve_station_mac};
use oer_hil_lab::config::OpenWrtConfig;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MacFrameKey {
    pub tid: u8,
    pub sequence: u16,
    pub fragment: u8,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct OpenWrtTxMonitorEvidence {
    pub captured_frames: u64,
    pub kernel_dropped: u64,
    pub data_units: u32,
    pub unique_units: u32,
    pub duplicates: u32,
    pub gap_events: u32,
    pub forward_missing: u32,
    pub late_recovered: u32,
    pub unrecovered: u32,
    pub out_of_range: u32,
    pub terminal_markers: u32,
    pub mac_retry_publications: u32,
    pub missing_mac_metadata: u32,
    pub first_anomaly: Option<u32>,
    /// Per-frame retry bookkeeping; the counters above summarize it.
    #[serde(skip)]
    pub mac_units: BTreeMap<MacFrameKey, u32>,
}

pub struct OpenWrtTxMonitorCapture {
    capture: RemoteCapture,
    target: Ipv4Addr,
    port: u16,
}

impl OpenWrtTxMonitorCapture {
    pub fn start(
        config: &OpenWrtConfig,
        target: Ipv4Addr,
        port: u16,
        duration: Duration,
        output: &Path,
    ) -> Result<Self> {
        let station_mac = resolve_station_mac(config, target)?;
        let capture = RemoteCapture::start_monitor(
            config,
            output.join("ap-tx-monitor.pcap"),
            &format!("wlan host {station_mac}"),
            false,
            duration,
        )?;
        Ok(Self {
            capture,
            target,
            port,
        })
    }

    pub fn finish(mut self, expected_units: u64) -> Result<OpenWrtTxMonitorEvidence> {
        let (captured_frames, kernel_dropped) = self.capture.finish_capture()?;
        let mut evidence = parse_capture(
            self.capture.output_path(),
            self.target,
            self.port,
            expected_units,
        )
        .map_err(crate::Error::context)?;
        evidence.captured_frames = captured_frames;
        evidence.kernel_dropped = kernel_dropped;
        Ok(evidence)
    }
}

/// AP-epoch capture includes broadcast probes and ACKs, not just addressed data.
pub struct OpenWrtManagementCapture(RemoteCapture);

impl OpenWrtManagementCapture {
    pub fn start(config: &OpenWrtConfig, output: &Path, duration: Duration) -> Result<Self> {
        // Discovery may finish before libpcap's packet-block timeout. Immediate
        // delivery preserves those frames without sleeping before capture Stop.
        Ok(Self(RemoteCapture::start_monitor(
            config,
            output.join("management.pcap"),
            "type mgt or type ctl",
            true,
            duration,
        )?))
    }

    pub fn finish(mut self) -> Result<()> {
        let (captured, dropped) = self.0.finish_capture()?;
        fs::write(
            self.0.output_path().with_extension("json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "captured_frames": captured, "kernel_dropped": dropped,
            }))?,
        )?;
        if captured == 0 {
            return Err(crate::Error::new(
                "discovery capture saw no management frames; evidence is incomplete",
            )
            .into());
        }
        Ok(())
    }
}

pub fn doctor(config: &OpenWrtConfig) -> Result<()> {
    let Some(monitor) = &config.monitor_interface else {
        return Err(
            crate::Error::new("scenario requires an OpenWrt monitor_interface name").into(),
        );
    };
    let output = oer_stand_ssh::command(
        &config.ssh_target,
        &format!(
            "set -eu; ! iw dev {monitor} info >/dev/null 2>&1; \
             wiphy=$(ubus call iwinfo phyname '{{\"section\":\"{}\"}}' | jsonfilter -e '@.phyname'); \
             iw phy \"$wiphy\" info | grep -q '^[[:space:]]*\\* monitor$'",
            config.radio
        ),
    )
    .supervised_output()?;
    if !output.status.success() {
        return Err(crate::Error::new(format!(
            "OpenWrt monitor evidence is unavailable or `{monitor}` already exists: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .into());
    }
    let status = oer_process::command("tshark")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .supervised_status()?;
    if !status.success() {
        return Err(
            crate::Error::new("local tshark is required for OpenWrt TX-monitor evidence").into(),
        );
    }
    Ok(())
}

fn parse_capture(
    path: &Path,
    target: Ipv4Addr,
    port: u16,
    expected_units: u64,
) -> Result<OpenWrtTxMonitorEvidence> {
    let frames = air::decode(path, "wlan.fc.type == 2", air::Payload::Include)?;
    let expected_units = u32::try_from(expected_units)?;
    let mut tracker = SequenceTracker::default();
    for frame in frames {
        let Some(sequence) = frame
            .payload
            .as_deref()
            .and_then(|raw| udp_sequence(raw, target, port))
        else {
            continue;
        };
        let mac = frame
            .sequence
            .zip(frame.fragment)
            .map(|(sequence, fragment)| MacFrameKey {
                tid: frame.tid.unwrap_or(u8::MAX),
                sequence,
                fragment,
            });
        tracker.observe(
            sequence,
            mac,
            frame.transmit_retries.unwrap_or(0),
            expected_units,
        );
    }
    Ok(tracker.finish(expected_units))
}

#[derive(Default)]
struct SequenceTracker {
    seen: BTreeSet<u32>,
    evidence: OpenWrtTxMonitorEvidence,
    highest: Option<u32>,
}

impl SequenceTracker {
    fn observe(&mut self, sequence: i32, mac: Option<MacFrameKey>, retries: u32, limit: u32) {
        self.evidence.mac_retry_publications =
            self.evidence.mac_retry_publications.saturating_add(retries);
        if sequence < 0 {
            self.evidence.terminal_markers = self.evidence.terminal_markers.saturating_add(1);
            return;
        }
        let sequence = sequence as u32;
        self.evidence.data_units = self.evidence.data_units.saturating_add(1);
        if let Some(mac) = mac {
            let count = self.evidence.mac_units.entry(mac).or_default();
            *count = count.saturating_add(1);
        } else {
            self.evidence.missing_mac_metadata =
                self.evidence.missing_mac_metadata.saturating_add(1);
        }
        if sequence >= limit {
            self.evidence.out_of_range = self.evidence.out_of_range.saturating_add(1);
            self.evidence.first_anomaly.get_or_insert(sequence);
            return;
        }
        if !self.seen.insert(sequence) {
            self.evidence.duplicates = self.evidence.duplicates.saturating_add(1);
            self.evidence.first_anomaly.get_or_insert(sequence);
            return;
        }
        self.evidence.unique_units = self.evidence.unique_units.saturating_add(1);
        match self.highest {
            None => self.highest = Some(sequence),
            Some(highest) if sequence > highest => {
                let missing = sequence.saturating_sub(highest).saturating_sub(1);
                if missing != 0 {
                    self.evidence.gap_events = self.evidence.gap_events.saturating_add(1);
                    self.evidence.forward_missing =
                        self.evidence.forward_missing.saturating_add(missing);
                    self.evidence.first_anomaly.get_or_insert(sequence);
                }
                self.highest = Some(sequence);
            }
            Some(_) => {
                self.evidence.late_recovered = self.evidence.late_recovered.saturating_add(1);
                self.evidence.first_anomaly.get_or_insert(sequence);
            }
        }
    }

    fn finish(mut self, expected: u32) -> OpenWrtTxMonitorEvidence {
        self.evidence.unrecovered = expected.saturating_sub(self.evidence.unique_units);
        self.evidence
    }
}

fn udp_sequence(raw: &[u8], target: Ipv4Addr, port: u16) -> Option<i32> {
    for offset in 0..raw.len().min(32) {
        let version_ihl = *raw.get(offset)?;
        if version_ihl >> 4 != 4 || version_ihl & 0x0f < 5 {
            continue;
        }
        let header_len = usize::from(version_ihl & 0x0f) * 4;
        if *raw.get(offset + 9)? != 17 || raw.get(offset + 16..offset + 20)? != target.octets() {
            continue;
        }
        let udp = offset + header_len;
        if u16::from_be_bytes(raw.get(udp + 2..udp + 4)?.try_into().ok()?) != port {
            continue;
        }
        return Some(i32::from_be_bytes(
            raw.get(udp + 8..udp + 12)?.try_into().ok()?,
        ));
    }
    None
}

#[cfg(test)]
mod tests;
