//! Scoped ownership of the laptop Wi-Fi interface as one AP test client.

use oer_process::CommandExt as _;
use std::{io::Write as _, path::Path, process::Stdio};

use zeroize::Zeroizing;

use crate::Result;
use oer_hil_lab::config::{AccessPointConfig, StationConfig};

/// The PHY capabilities the laptop station advertises.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientPhy {
    /// HT with the adapter's full capabilities.
    Ht,
    /// A Clause 18 ERP-OFDM station without HT, VHT or HE capability.
    NonHt,
}

/// The BSS the laptop joins and the station it presents there.
pub struct ClientNetwork<'a> {
    ssid: &'a str,
    passphrase: &'a str,
    /// The laptop's IPv4 address; a membership-only peer carries none.
    address: Option<String>,
    frequency_mhz: u16,
    phy: ClientPhy,
}

impl<'a> ClientNetwork<'a> {
    /// A traffic client of the target's AP.
    pub fn target_access_point(config: &'a AccessPointConfig, phy: ClientPhy) -> Self {
        let (ssid, passphrase) = config.credentials();
        Self {
            ssid,
            passphrase,
            address: Some(config.client_cidr()),
            frequency_mhz: config.frequency_mhz(),
            phy,
        }
    }

    /// A membership-only station of the laboratory AP on `channel`.
    pub fn station_fixture(station: &'a StationConfig, channel: u8, phy: ClientPhy) -> Self {
        let (ssid, passphrase) = station.credentials();
        Self {
            ssid,
            passphrase,
            address: None,
            frequency_mhz: 2407 + u16::from(channel) * 5,
            phy,
        }
    }

    fn helper_input(&self) -> Zeroizing<Vec<u8>> {
        let mut input = Zeroizing::new(Vec::new());
        for line in [
            self.ssid,
            self.passphrase,
            self.address.as_deref().unwrap_or("none"),
            &self.frequency_mhz.to_string(),
            match self.phy {
                ClientPhy::Ht => "ht",
                ClientPhy::NonHt => "non-ht",
            },
        ] {
            input.extend_from_slice(line.as_bytes());
            input.push(b'\n');
        }
        input
    }
}

pub fn doctor() -> Result<()> {
    crate::local::network_helper::doctor()
}

pub struct ControlledClient {
    restored: bool,
}

impl ControlledClient {
    pub fn connect(network: &ClientNetwork<'_>, output: &Path) -> Result<Self> {
        std::fs::create_dir_all(output)?;
        let log = std::fs::File::create(output.join("helper.log"))?;
        let input = network.helper_input();
        let owner = Self { restored: false };
        let mut child = oer_process::command("sudo")
            .args(["-n", crate::local::network_helper::PATH, "client"])
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .stdin(Stdio::piped())
            .spawn_owned()?;
        child
            .stdin
            .take()
            .ok_or("controlled-client helper has no stdin")?
            .write_all(&input)?;
        let status = child.wait_timeout(Some(std::time::Duration::from_secs(120)))?;
        if !status.success() {
            return Err(crate::Error::new(format!(
                "controlled-client helper failed with {status}"
            ))
            .into());
        }
        let mut control = crate::local::wpa_control::Control::connect(Path::new(
            "/run/open-radio-wpa-control/wlan0",
        ))?;
        control.record_to(std::fs::File::create(output.join("control.jsonl"))?);
        let result = control.wait_connected();
        let state = crate::local::wpa_control::field(&control.last_status, "wpa_state");
        // The address the station associated with; the interface may carry a
        // different, randomized address while it is not associated.
        let station_address = crate::local::wpa_control::field(&control.last_status, "address");
        let stage = connection_stage(state);
        std::fs::write(
            output.join("connection.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": 2, "connected": result.is_ok(), "last_state": state,
                "station_address": station_address,
                "last_observed_stage": stage, "last_event": control.last_event,
                "last_failure_event": control.last_failure_event,
                "error": result.as_ref().err().map(|error| error.to_string()),
            }))?,
        )?;
        result.map_err(|error| -> Box<dyn std::error::Error + Send + Sync> {
            format!("controlled client connection failed: {error}; last observed stage={stage}, wpa_state={state:?}, last_failure={:?}; diagnostics={}", control.last_failure_event, output.display()).into()
        })?;
        Ok(owner)
    }

    /// Return the BSSID owned by the AP under test without changing the
    /// controlled client's managed-mode lifetime.
    pub fn bssid(&self) -> Result<String> {
        let output = oer_process::command("iw")
            .args(["dev", "wlan0", "link"])
            .supervised_output()?;
        if !output.status.success() {
            return Err(crate::Error::new(format!(
                "cannot query controlled-client BSSID: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
            .into());
        }
        parse_bssid(&String::from_utf8(output.stdout)?)
    }

    pub fn restore(mut self) -> Result<()> {
        restore_managed()?;
        self.restored = true;
        Ok(())
    }

    /// Begin observing the client's link to the AP under test: its station
    /// counters now, compared with them at [`LaptopClientLinkObservation::finish`].
    pub fn begin_link_observation(&self) -> Result<LaptopClientLinkObservation> {
        Ok(LaptopClientLinkObservation {
            before: LaptopLinkSnapshot::take()?,
        })
    }
}

/// What the laptop counted on its link to the AP under test during one
/// workload. Its driver's station counters (`iw dev wlan0 station dump`)
/// tell the frames it gave up on in the air (`tx_failed`); the interface's
/// drops and mac80211's AQM drops of TID 0 tell the packets it dropped
/// before they became frames, which leave no gap in the AP's sequence. The
/// root queueing discipline is `noqueue` on a mac80211 TXQ driver, so the
/// AQM counters are where such a drop shows.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct LaptopClientLinkEvidence {
    pub tx_packets: u64,
    pub tx_retries: u64,
    pub tx_failed: u64,
    /// Packets `wlan0` dropped on transmit (its `tx_dropped` statistic).
    pub interface_tx_dropped: u64,
    /// Packets mac80211's AQM dropped from the AP's TID 0 queue, and
    /// those it refused over its limit.
    pub tid0_aqm_drops: u64,
    pub tid0_aqm_overlimit: u64,
    pub rx_packets: u64,
    /// Frames from the AP that mac80211 dropped after reception.
    pub rx_drop_misc: u64,
    /// The bitrate of the client's frames after the workload.
    pub tx_bitrate: String,
}

/// The laptop client's link counters at the start of a workload.
pub struct LaptopClientLinkObservation {
    before: LaptopLinkSnapshot,
}

impl LaptopClientLinkObservation {
    /// The counters' growth since the observation began.
    pub fn finish(self) -> Result<LaptopClientLinkEvidence> {
        let after = LaptopLinkSnapshot::take()?;
        let delta = |name: &str, before: u64, after: u64| -> Result<u64> {
            after.checked_sub(before).ok_or_else(|| {
                format!(
                    "laptop client `{name}` counter reset during the workload: {before} -> {after}"
                )
                .into()
            })
        };
        Ok(LaptopClientLinkEvidence {
            tx_packets: delta("tx packets", self.before.tx_packets, after.tx_packets)?,
            tx_retries: delta("tx retries", self.before.tx_retries, after.tx_retries)?,
            tx_failed: delta("tx failed", self.before.tx_failed, after.tx_failed)?,
            interface_tx_dropped: delta(
                "interface tx dropped",
                self.before.interface_tx_dropped,
                after.interface_tx_dropped,
            )?,
            tid0_aqm_drops: delta(
                "TID 0 AQM drops",
                self.before.tid0_aqm_drops,
                after.tid0_aqm_drops,
            )?,
            tid0_aqm_overlimit: delta(
                "TID 0 AQM overlimit",
                self.before.tid0_aqm_overlimit,
                after.tid0_aqm_overlimit,
            )?,
            rx_packets: delta("rx packets", self.before.rx_packets, after.rx_packets)?,
            rx_drop_misc: delta("rx drop misc", self.before.rx_drop_misc, after.rx_drop_misc)?,
            tx_bitrate: after.tx_bitrate,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LaptopLinkSnapshot {
    tx_packets: u64,
    tx_retries: u64,
    tx_failed: u64,
    interface_tx_dropped: u64,
    tid0_aqm_drops: u64,
    tid0_aqm_overlimit: u64,
    rx_packets: u64,
    rx_drop_misc: u64,
    tx_bitrate: String,
}

impl LaptopLinkSnapshot {
    fn take() -> Result<Self> {
        let output = oer_process::command("iw")
            .args(["dev", "wlan0", "station", "dump"])
            .supervised_output()?;
        if !output.status.success() {
            return Err(crate::Error::new(format!(
                "cannot snapshot the laptop client's link counters: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
            .into());
        }
        let aqm = oer_process::command("sudo")
            .args(["-n", crate::local::network_helper::PATH, "client-aqm"])
            .supervised_output()?;
        if !aqm.status.success() {
            return Err(crate::Error::new(format!(
                "cannot read the laptop client's AQM counters: {}",
                String::from_utf8_lossy(&aqm.stderr).trim()
            ))
            .into());
        }
        let interface_tx_dropped =
            std::fs::read_to_string("/sys/class/net/wlan0/statistics/tx_dropped")?
                .trim()
                .parse()?;
        Self::parse(
            &String::from_utf8(output.stdout)?,
            &String::from_utf8(aqm.stdout)?,
            interface_tx_dropped,
        )
    }

    /// The one station a managed client lists, its AP, and the helper's
    /// `client-aqm` counters in `aqm`.
    fn parse(dump: &str, aqm: &str, interface_tx_dropped: u64) -> Result<Self> {
        if dump.matches("Station ").count() != 1 {
            return Err(crate::Error::new(
                "the laptop client lists other than exactly one station",
            )
            .into());
        }
        use crate::local::evidence::{tagged_text, tagged_u64};
        Ok(Self {
            tx_packets: tagged_u64(dump, "tx packets:")?,
            tx_retries: tagged_u64(dump, "tx retries:")?,
            tx_failed: tagged_u64(dump, "tx failed:")?,
            interface_tx_dropped,
            tid0_aqm_drops: helper_value(aqm, "tid0_aqm_drops")?,
            tid0_aqm_overlimit: helper_value(aqm, "tid0_aqm_overlimit")?,
            rx_packets: tagged_u64(dump, "rx packets:")?,
            rx_drop_misc: tagged_u64(dump, "rx drop misc:")?,
            tx_bitrate: tagged_text(dump, "tx bitrate:")?,
        })
    }
}

/// The station address of every controlled-client connection recorded under
/// `output`, as the `linux-client` directories of its cycles hold them.
pub fn connected_station_addresses(output: &Path) -> Result<Vec<String>> {
    let mut addresses = Vec::new();
    let mut pending = vec![output.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let path = entry?.path();
            if !path.is_dir() {
                continue;
            }
            let connection = path.join("connection.json");
            if path.file_name().is_some_and(|name| name == "linux-client") && connection.is_file() {
                let record: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&connection)?)?;
                if let Some(address) = record["station_address"].as_str() {
                    addresses.push(address.to_ascii_lowercase());
                }
            } else {
                pending.push(path);
            }
        }
    }
    addresses.sort_unstable();
    addresses.dedup();
    Ok(addresses)
}

fn parse_bssid(link: &str) -> Result<String> {
    let Some(bssid) = link.lines().find_map(|line| {
        line.trim()
            .strip_prefix("Connected to ")?
            .split_whitespace()
            .next()
    }) else {
        return Err("controlled client is not associated with an AP".into());
    };
    let valid = bssid.len() == 17
        && bssid.bytes().enumerate().all(|(index, byte)| {
            index % 3 == 2 && byte == b':' || index % 3 != 2 && byte.is_ascii_hexdigit()
        });
    if !valid {
        return Err(crate::Error::new(format!(
            "controlled client reported invalid BSSID `{bssid}`"
        ))
        .into());
    }
    Ok(bssid.to_ascii_lowercase())
}

impl Drop for ControlledClient {
    fn drop(&mut self) {
        oer_process::cleanup(|| {
            if !self.restored {
                oer_hil_workload::fixture::cleanup::record(
                    "restore managed Wi-Fi",
                    restore_managed,
                );
            }
        });
    }
}

fn restore_managed() -> Result<()> {
    let status = oer_process::command("sudo")
        .args(["-n", crate::local::network_helper::PATH, "managed"])
        .supervised_status()?;
    if !status.success() {
        return Err(
            crate::Error::new(format!("controlled-client restore failed with {status}")).into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;

fn connection_stage(state: Option<&str>) -> &'static str {
    match state {
        Some("SCANNING") => "discovery",
        Some("AUTHENTICATING") => "authentication",
        Some("ASSOCIATING" | "ASSOCIATED") => "association",
        Some("4WAY_HANDSHAKE" | "GROUP_HANDSHAKE") => "key-negotiation",
        Some("COMPLETED") => "connected",
        _ => "unknown",
    }
}

/// The value of `key=` in the helper's output.
fn helper_value(output: &str, key: &str) -> Result<u64> {
    output
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        .ok_or_else(|| format!("the laptop client's AQM counters omit `{key}`"))?
        .trim()
        .parse()
        .map_err(|error| format!("invalid laptop client AQM counter `{key}`: {error}").into())
}
