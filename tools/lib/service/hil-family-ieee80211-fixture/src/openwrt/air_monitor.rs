//! Independent passive capture on a dedicated, initially idle OpenWrt PHY.
//! Uses the same event-driven capture owner and air decoder as other fixtures.
use crate::Result;
use crate::air::{self, AirFrame, FrameKind, MacAddress};
use crate::{
    local,
    openwrt::capture::{RemoteCapture, SnapshotLength},
};
use oer_hil_lab::config::{LabConfig, StationFixtureConfig};
use oer_process::CommandExt as _;
use std::{fs, net::Ipv4Addr, path::Path, time::Duration};

pub struct Capture {
    remote: RemoteCapture,
    target_mac: String,
    setup_only: bool,
}

impl Capture {
    pub fn start(
        lab: &LabConfig,
        target: Option<Ipv4Addr>,
        duration: Duration,
        output: &Path,
    ) -> Result<Option<Self>> {
        let Some(observer) = &lab.air_observer else {
            return Ok(None);
        };
        let StationFixtureConfig::OpenWrt(ap) = &lab.station_fixture else {
            return Err("independent OpenWrt observer requires a managed AP".into());
        };
        let boot = |target: &str| -> Result<String> {
            let result =
                oer_stand_ssh::command(target, oer_stand_ssh::BOOT_ID).supervised_output()?;
            if !result.status.success() {
                return Err("cannot identify independent observer hardware".into());
            }
            let id = String::from_utf8(result.stdout)?.trim().to_owned();
            if id.is_empty() {
                return Err("empty hardware boot identity".into());
            }
            Ok(id)
        };
        let ap_boot = boot(&ap.ssh_target)?;
        let observer_boot = boot(&observer.ssh_target)?;
        if ap_boot == observer_boot {
            return Err(
                "independent observer must be a different host from the transmitting AP".into(),
            );
        }
        let geometry = local::air_monitor::resolve_observer_action(ap)?;
        let target_mac = target
            .map(|address| crate::openwrt::evidence::resolve_station_mac(ap, address))
            .transpose()?
            .unwrap_or_else(|| "02:00:00:00:00:01".into());
        let filter = if target.is_some() {
            format!("wlan host {target_mac} or type ctl")
        } else {
            "type mgt".into()
        };
        let remote = RemoteCapture::start_independent(
            observer,
            geometry,
            &filter,
            SnapshotLength::Headers,
            output.join("independent-openwrt-air.pcap"),
            duration,
        )?;
        fs::write(
            output.join("independent-openwrt-fixture.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": 1, "ap_boot": ap_boot, "observer_boot": observer_boot,
                "phy": observer.phy, "interface": observer.interface, "target_mac": target_mac,
                "frequency": geometry.frequency, "width_mhz": geometry.width, "center_frequency": geometry.center,
                "readiness": "tcpdump listening and verified channel", "initial_phy_state": "no interfaces",
            }))?,
        )?;
        Ok(Some(Self {
            remote,
            target_mac,
            setup_only: target.is_none(),
        }))
    }

    pub fn finish(mut self) -> Result<()> {
        let result = self.remote.finish_capture();
        let path = self.remote.output_path().with_extension("json");
        match result {
            Ok((captured, dropped)) => {
                let mut evidence =
                    local::air_monitor::parse_capture(self.remote.output_path(), &self.target_mac)?;
                evidence.captured_frames = captured;
                evidence.kernel_dropped = dropped;
                fs::write(path, serde_json::to_vec_pretty(&evidence)?)?;
                if captured == 0 && !self.setup_only {
                    return Err("independent OpenWrt observer captured no frames".into());
                }
                Ok(())
            }
            Err(error) => {
                fs::write(
                    path,
                    serde_json::to_vec_pretty(
                        &serde_json::json!({"failure": error.to_string(), "complete": false}),
                    )?,
                )?;
                Err(error)
            }
        }
    }
}

/// The BSSID of the station fixture AP.
pub fn fixture_bssid(ap: &oer_hil_lab::config::OpenWrtConfig) -> Result<MacAddress> {
    let output = oer_stand_ssh::command(
        &ap.ssh_target,
        &format!("cat /sys/class/net/{}/address", ap.wireless_interface),
    )
    .supervised_output()?;
    if !output.status.success() {
        return Err("cannot read the station fixture BSSID".into());
    }
    Ok(std::str::from_utf8(&output.stdout)?.trim().parse()?)
}

/// Beacons of `bssid` observed by the independent observer on the station
/// fixture channel during `duration`.
pub fn observe_beacons(
    lab: &LabConfig,
    bssid: MacAddress,
    duration: Duration,
    output: &Path,
) -> Result<Vec<AirFrame>> {
    let observer = lab
        .air_observer
        .as_ref()
        .ok_or("beacon observation requires the independent air observer")?;
    let StationFixtureConfig::OpenWrt(ap) = &lab.station_fixture else {
        return Err("beacon observation requires the OpenWrt station fixture".into());
    };
    let geometry = local::air_monitor::resolve_observer_action(ap)?;
    fs::create_dir_all(output)?;
    let mut remote = RemoteCapture::start_independent(
        observer,
        geometry,
        &format!("type mgt subtype beacon and wlan addr2 {bssid}"),
        SnapshotLength::Complete,
        output.join("beacons.pcap"),
        duration,
    )?;
    std::thread::sleep(duration);
    remote.finish_capture()?;
    Ok(air::decode(
        remote.output_path(),
        "wlan.fc.type_subtype == 0x0008",
        air::Payload::Omit,
    )?
    .into_iter()
    .filter(|frame| frame.kind == FrameKind::BEACON && frame.transmitter == Some(bssid))
    .collect())
}

/// Control and data frames on the laboratory channel, captured by the
/// independent observer for the protection analysis.
pub struct ProtectionCapture {
    remote: RemoteCapture,
    scope: ProtectionScope,
}

/// Which data the observer retains besides control frames.
#[derive(Clone, Copy, Debug)]
pub enum ProtectionScope {
    /// Data sent to the station fixture AP: the target is its station.
    StationFixture(MacAddress),
    /// All data: the target is the access point on the channel.
    Channel,
}

impl ProtectionCapture {
    /// Capture on the station fixture's channel. `bound` limits the capture
    /// if the workload never finishes it.
    pub fn start(
        lab: &LabConfig,
        station_target: bool,
        bound: Duration,
        output: &Path,
    ) -> Result<Self> {
        let observer = lab
            .air_observer
            .as_ref()
            .ok_or("protection evidence requires the independent air observer")?;
        let StationFixtureConfig::OpenWrt(ap) = &lab.station_fixture else {
            return Err("protection evidence requires the OpenWrt station fixture".into());
        };
        let scope = if station_target {
            ProtectionScope::StationFixture(fixture_bssid(ap)?)
        } else {
            ProtectionScope::Channel
        };
        let filter = match scope {
            ProtectionScope::StationFixture(bssid) => format!(
                "type ctl or (type data and wlan addr1 {bssid}) or \
                 (type mgt subtype beacon and wlan addr2 {bssid})"
            ),
            ProtectionScope::Channel => {
                "type ctl or type data or (type mgt subtype beacon)".to_owned()
            }
        };
        let geometry = local::air_monitor::resolve_observer_action(ap)?;
        fs::create_dir_all(output)?;
        let remote = RemoteCapture::start_independent(
            observer,
            geometry,
            &filter,
            SnapshotLength::Headers,
            output.join("protection-air.pcap"),
            bound,
        )?;
        Ok(Self { remote, scope })
    }

    /// Stop the capture and analyze the target flow. `peer` is the laptop:
    /// a station target's co-member, or an access point target's other client.
    pub fn finish(
        mut self,
        peer: Option<MacAddress>,
    ) -> Result<crate::protection::ProtectionEvidence> {
        use crate::protection::{Flow, analyze};
        let (captured, dropped) = self.remote.finish_capture()?;
        if dropped != 0 {
            return Err(
                format!("protection observer dropped {dropped} of {captured} frames").into(),
            );
        }
        let frames = air::decode(
            self.remote.output_path(),
            "wlan.fc.type == 1 || wlan.fc.type == 2",
            air::Payload::Omit,
        )?;
        let flow = match self.scope {
            ProtectionScope::StationFixture(bssid) => Flow::station(&frames, bssid, peer)?,
            ProtectionScope::Channel => Flow::access_point(
                &frames,
                peer.ok_or("an access point flow needs the laptop client")?,
            )?,
        };
        // The BSS whose basic rates and ERP protection govern the exchange.
        let bss = match self.scope {
            ProtectionScope::StationFixture(bssid) => bssid,
            ProtectionScope::Channel => flow.target,
        };
        let rates = air::beacon_rates(self.remote.output_path(), bss)?;
        let evidence = analyze(&frames, flow, &rates)?;
        fs::write(
            self.remote.output_path().with_extension("json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": 1, "captured_frames": captured,
                "bss": bss.to_string(), "bss_rates": rates, "evidence": evidence,
            }))?,
        )?;
        Ok(evidence)
    }
}
