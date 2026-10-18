//! Session-bounded evidence from the laptop-owned nl80211 AP.

use crate::capture_process::{self, Capture};
use oer_process::CommandExt as _;
use std::{net::Ipv4Addr, time::Duration};

use crate::Result;
use oer_hil_lab::config::LocalLinuxConfig;
use oer_hil_scenario_catalog::link::PhyExpectation;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct LocalLinuxRxEvidence {
    pub udp_packets: u64,
    pub station_tx_packets: u64,
    pub station_tx_retries: u64,
    pub station_tx_failed: u64,
    pub interface_tx_packets: u64,
    pub station_tx_duration_micros: u64,
    pub channel_width_mhz: u8,
    pub tx_bitrate: String,
    pub rx_bitrate: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct LocalLinuxTxEvidence {
    pub udp_packets: u64,
    pub channel_width_mhz: u8,
    pub tx_bitrate: String,
    pub rx_bitrate: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Snapshot {
    station_tx_packets: u64,
    station_tx_retries: u64,
    station_tx_failed: u64,
    interface_tx_packets: u64,
    station_tx_duration_micros: u64,
    channel_width_mhz: u8,
    tx_bitrate: String,
    rx_bitrate: String,
}

pub struct LocalLinuxRxCapture {
    config: LocalLinuxConfig,
    target: Ipv4Addr,
    expected_phy: PhyExpectation,
    before: Snapshot,
    capture: LocalPacketCapture,
}

pub struct LocalLinuxTxCapture {
    config: LocalLinuxConfig,
    target: Ipv4Addr,
    expected_phy: PhyExpectation,
    capture: LocalPacketCapture,
}

struct LocalPacketCapture {
    child: Option<Capture>,
}

impl LocalLinuxRxCapture {
    pub fn start(
        config: &LocalLinuxConfig,
        target: Ipv4Addr,
        port: u16,
        traffic_duration: Duration,
        expected_phy: PhyExpectation,
    ) -> Result<Self> {
        let before = snapshot(config, target).map_err(crate::Error::context)?;
        require_width(expected_phy, before.channel_width_mhz)?;
        let filter = format!("udp and dst host {target} and dst port {port}");
        let capture = LocalPacketCapture::start(&config.interface, &filter, traffic_duration)?;
        Ok(Self {
            config: config.clone(),
            target,
            expected_phy,
            before,
            capture,
        })
    }

    pub fn finish(self) -> Result<LocalLinuxRxEvidence> {
        let udp_packets = self.capture.finish()?;
        let after = snapshot(&self.config, self.target).map_err(crate::Error::context)?;
        require_width(self.expected_phy, after.channel_width_mhz)?;
        Ok(LocalLinuxRxEvidence {
            udp_packets,
            station_tx_packets: delta(
                "station TX packets",
                self.before.station_tx_packets,
                after.station_tx_packets,
            )?,
            station_tx_retries: delta(
                "station TX retries",
                self.before.station_tx_retries,
                after.station_tx_retries,
            )?,
            station_tx_failed: delta(
                "station TX failed",
                self.before.station_tx_failed,
                after.station_tx_failed,
            )?,
            interface_tx_packets: delta(
                "interface TX packets",
                self.before.interface_tx_packets,
                after.interface_tx_packets,
            )?,
            station_tx_duration_micros: delta(
                "station TX duration",
                self.before.station_tx_duration_micros,
                after.station_tx_duration_micros,
            )?,
            channel_width_mhz: after.channel_width_mhz,
            tx_bitrate: after.tx_bitrate,
            rx_bitrate: after.rx_bitrate,
        })
    }
}

impl LocalLinuxTxCapture {
    pub fn start(
        config: &LocalLinuxConfig,
        target: Ipv4Addr,
        source_port: u16,
        destination_port: u16,
        traffic_duration: Duration,
        expected_phy: PhyExpectation,
    ) -> Result<Self> {
        let before = snapshot(config, target).map_err(crate::Error::context)?;
        require_width(expected_phy, before.channel_width_mhz)?;
        let filter = format!(
            "udp and src host {target} and src port {source_port} and dst port {destination_port}"
        );
        Ok(Self {
            config: config.clone(),
            target,
            expected_phy,
            capture: LocalPacketCapture::start(&config.interface, &filter, traffic_duration)?,
        })
    }

    pub fn finish(self) -> Result<LocalLinuxTxEvidence> {
        let udp_packets = self.capture.finish()?;
        let after = snapshot(&self.config, self.target).map_err(crate::Error::context)?;
        require_width(self.expected_phy, after.channel_width_mhz)?;
        Ok(LocalLinuxTxEvidence {
            udp_packets,
            channel_width_mhz: after.channel_width_mhz,
            tx_bitrate: after.tx_bitrate,
            rx_bitrate: after.rx_bitrate,
        })
    }
}

impl LocalPacketCapture {
    fn start(interface: &str, filter: &str, traffic_duration: Duration) -> Result<Self> {
        let child = capture_process::dumpcap(
            interface,
            Some(filter),
            128,
            std::path::Path::new("/dev/null"),
            traffic_duration,
        )?;
        Ok(Self { child: Some(child) })
    }

    fn finish(mut self) -> Result<u64> {
        let output = self
            .child
            .take()
            .expect("local fixture owns packet capture")
            .finish()?;
        if !output.status.success() {
            return Err(crate::Error::new(format!(
                "local AP packet capture exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ))
            .into());
        }
        let summary = String::from_utf8(output.stderr)?;
        let udp_packets = dumpcap_captured(&summary).map_err(crate::Error::context)?;
        let dropped = dumpcap_dropped(&summary).map_err(crate::Error::context)?;
        if dropped != 0 {
            return Err(crate::Error::new(format!(
                "local AP packet capture dropped {dropped} packets"
            ))
            .into());
        }
        Ok(udp_packets)
    }
}

fn dumpcap_captured(summary: &str) -> Result<u64> {
    summary
        .lines()
        .find_map(|line| line.trim().strip_prefix("Packets captured:"))
        .ok_or("local AP packet capture omitted its packet count")?
        .trim()
        .parse()
        .map_err(|error| format!("invalid local AP captured packet count: {error}").into())
}

fn dumpcap_dropped(summary: &str) -> Result<u64> {
    let counts = summary
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("Packets received/dropped on interface")?
                .split_once(':')
                .map(|(_, counts)| counts.trim())
        })
        .ok_or("local AP packet capture omitted its drop count")?;
    counts
        .split_once('/')
        .and_then(|(_, dropped)| dropped.split_whitespace().next())
        .ok_or("local AP packet capture reported a malformed drop count")?
        .parse()
        .map_err(|error| format!("invalid local AP dropped packet count: {error}").into())
}

fn snapshot(config: &LocalLinuxConfig, target: Ipv4Addr) -> Result<Snapshot> {
    let neighbor = oer_process::command("ip")
        .args([
            "neigh",
            "show",
            &target.to_string(),
            "dev",
            &config.interface,
        ])
        .supervised_output()?;
    if !neighbor.status.success() {
        return Err(crate::Error::new("cannot resolve the local-AP station neighbor").into());
    }
    let neighbor = String::from_utf8(neighbor.stdout)?;
    let mac = neighbor
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .find_map(|pair| (pair[0] == "lladdr").then_some(pair[1]))
        .ok_or("local AP has no resolved station MAC")?;
    let output = oer_process::command("iw")
        .args(["dev", &config.interface, "station", "get", mac])
        .supervised_output()?;
    if !output.status.success() {
        return Err(crate::Error::new(format!(
            "cannot snapshot local AP station counters: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .into());
    }
    let station = String::from_utf8(output.stdout)?;
    let interface = oer_process::command("iw")
        .args(["dev", &config.interface, "info"])
        .supervised_output()?;
    if !interface.status.success() {
        return Err(crate::Error::new("cannot read the local AP channel width").into());
    }
    let interface = String::from_utf8(interface.stdout)?;
    let interface_tx_packets = std::fs::read_to_string(format!(
        "/sys/class/net/{}/statistics/tx_packets",
        config.interface
    ))?
    .trim()
    .parse()?;
    Ok(Snapshot {
        station_tx_packets: tagged_u64(&station, "tx packets:")?,
        station_tx_retries: tagged_u64(&station, "tx retries:")?,
        station_tx_failed: tagged_u64(&station, "tx failed:")?,
        interface_tx_packets,
        station_tx_duration_micros: tagged_u64(&station, "tx duration:")?,
        channel_width_mhz: channel_width(&interface)?,
        tx_bitrate: tagged_text(&station, "tx bitrate:")?,
        rx_bitrate: tagged_text(&station, "rx bitrate:")?,
    })
}

fn channel_width(output: &str) -> Result<u8> {
    output
        .lines()
        .find_map(|line| line.split_once("width:").map(|(_, value)| value.trim()))
        .and_then(|value| value.split_whitespace().next())
        .ok_or("local AP interface snapshot omitted channel width")?
        .parse()
        .map_err(|error| format!("invalid local AP channel width: {error}").into())
}

fn require_width(phy: PhyExpectation, observed: u8) -> Result<()> {
    let expected = match phy {
        PhyExpectation::Legacy | PhyExpectation::He20 | PhyExpectation::Ht20 => 20,
        PhyExpectation::Ht40 => 40,
    };
    if observed != expected {
        return Err(crate::Error::new(format!(
            "local AP link width changed during the HIL session: expected={expected} observed={observed} MHz"
        )).into());
    }
    Ok(())
}

/// The first number after `key` in an `iw` station listing.
pub(crate) fn tagged_u64(output: &str, key: &str) -> Result<u64> {
    tagged_text(output, key)?
        .split_whitespace()
        .next()
        .ok_or_else(|| format!("station counter `{key}` is empty"))?
        .parse()
        .map_err(|error| format!("invalid station counter `{key}`: {error}").into())
}

/// The text after `key` in an `iw` station listing.
pub(crate) fn tagged_text(output: &str, key: &str) -> Result<String> {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix(key).map(str::trim))
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("station snapshot omitted `{key}`").into())
}

fn delta(name: &str, before: u64, after: u64) -> Result<u64> {
    after.checked_sub(before).ok_or_else(|| {
        format!("local AP `{name}` counter reset during the HIL session: {before} -> {after}")
            .into()
    })
}

#[cfg(test)]
mod tests;
