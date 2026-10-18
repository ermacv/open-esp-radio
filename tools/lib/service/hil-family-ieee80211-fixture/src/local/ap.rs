//! Scenario-owned Linux AP configuration. Secrets travel only on helper stdin.

use crate::Result;
use crate::{
    channel::Geometry,
    local::wpa_control::{Control, field},
};
use oer_hil_lab::config::{Coexistence, LocalLinuxConfig, StationConfig};
use oer_hil_run_bundle_format::run::fixtures::LocalApplied;
use oer_hil_scenario_catalog::link::PhyExpectation;
use oer_process::CommandExt as _;
use std::{
    io::{Read, Seek, SeekFrom, Write},
    net::Ipv4Addr,
    path::Path,
    process::Stdio,
    time::Duration,
};
use zeroize::Zeroizing;

pub struct AccessPoint {
    config: LocalLinuxConfig,
    phy: PhyExpectation,
    input: Zeroizing<String>,
}

impl AccessPoint {
    pub fn start(
        config: &LocalLinuxConfig,
        station: &StationConfig,
        phy: PhyExpectation,
    ) -> Result<Self> {
        if phy == PhyExpectation::Legacy {
            return Err("the local Linux AP fixture serves HT and HE links only".into());
        }
        let input = profile(config, station, phy)?;
        probe(config, phy)?;
        // Own cleanup before any helper mutation, including failed setup.
        let owner = Self {
            config: config.clone(),
            phy,
            input,
        };
        owner.restart()?;
        Ok(owner)
    }

    pub fn stop(&self) -> Result<()> {
        helper("stop", None).map(drop)
    }

    /// The fixture record of the verified local access point.
    pub fn report(&self) -> LocalApplied {
        let observed = geometry(&self.config, self.phy);
        LocalApplied {
            schema: 1,
            backend: String::from("local-linux"),
            verified: true,
            phy: self.phy,
            channel: self.config.channel,
            country: self.config.country.clone(),
            frequency_mhz: observed.frequency,
            width_mhz: observed.width,
            center1_mhz: observed.center,
            address: self.config.address,
            prefix_length: self.config.prefix_length,
            coexistence: String::from(match self.config.coexistence {
                Coexistence::Respect => "respect",
                Coexistence::ForceHt40 => "force-ht40",
            }),
        }
    }

    pub fn restart(&self) -> Result<()> {
        self.start_and_verify().map_err(|error| {
            let log = startup_log(
                Path::new("/run/open-radio-hostapd/hostapd.log"),
                &self.input,
            );
            format!("{error}; hostapd log: {}", log.as_str()).into()
        })
    }

    fn start_and_verify(&self) -> Result<()> {
        let startup = helper("ap", Some(&self.input))?;
        let mut control = Control::connect(Path::new("/run/open-radio-hostapd/wlan0"))?;
        let status = control.wait_enabled()?;
        // Detailed scan diagnostics are needed only during preparation, not
        // during the measured data-plane workload.
        if control.request("LOG_LEVEL INFO")?.trim() != "OK" {
            return Err("hostapd rejected restoring normal logging after startup".into());
        }
        if field(&status, "ieee80211n") != Some("1")
            || (field(&status, "ieee80211ax") == Some("1")) != (self.phy == PhyExpectation::He20)
        {
            return Err("hostapd did not enable the requested HT/HE mode".into());
        }
        let configured = Zeroizing::new(control.request("GET_CONFIG")?);
        if field(&configured, "wpa") != Some("2")
            || !field(&configured, "key_mgmt")
                .is_some_and(|value| value.split_whitespace().any(|key| key == "WPA-PSK"))
        {
            return Err("hostapd did not apply WPA2-PSK".into());
        }
        let info = oer_process::command("iw")
            .args(["dev", &self.config.interface, "info"])
            .supervised_output()?;
        if !info.status.success() {
            return Err("cannot read Linux AP channel geometry with iw".into());
        }
        verify_geometry(
            geometry(&self.config, self.phy),
            Geometry::parse(std::str::from_utf8(&info.stdout)?)?,
        )
        .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> {
            format!(
                "{error}; hostapd secondary_channel={}; startup: {}",
                field(&status, "secondary_channel").unwrap_or("unknown"),
                startup.trim()
            )
            .into()
        })?;
        let addresses = oer_process::command("ip")
            .args(["-j", "-4", "address", "show", "dev", &self.config.interface])
            .supervised_output()?;
        if !addresses.status.success() {
            return Err("cannot verify the Linux AP address".into());
        }
        let addresses: serde_json::Value = serde_json::from_slice(&addresses.stdout)?;
        let expected = self.config.address.to_string();
        if !addresses[0]["addr_info"]
            .as_array()
            .is_some_and(|addresses| {
                addresses.iter().any(|address| {
                    address["local"].as_str() == Some(&expected)
                        && address["prefixlen"].as_u64()
                            == Some(u64::from(self.config.prefix_length))
                })
            })
        {
            return Err("Linux AP did not acquire the configured IPv4 address".into());
        }
        Ok(())
    }
}

fn verify_geometry(expected: Geometry, observed: Geometry) -> Result<()> {
    if observed != expected {
        return Err(format!(
            "Linux AP channel/width differs from the scenario profile: expected primary={} MHz width={} MHz center={} MHz; observed primary={} MHz width={} MHz center={} MHz",
            expected.frequency, expected.width, expected.center,
            observed.frequency, observed.width, observed.center,
        ).into());
    }
    Ok(())
}

impl Drop for AccessPoint {
    fn drop(&mut self) {
        oer_hil_workload::fixture::cleanup::record("restore managed Wi-Fi", || {
            helper("managed", None).map(drop)
        });
    }
}

pub fn check(
    config: &LocalLinuxConfig,
    station: &StationConfig,
    phy: PhyExpectation,
) -> Result<()> {
    let _request = profile(config, station, phy)?;
    probe(config, phy)
}

fn probe(config: &LocalLinuxConfig, phy: PhyExpectation) -> Result<()> {
    crate::local::network_helper::doctor()?;
    let identity = oer_process::command("sudo")
        .args(["-n", crate::local::network_helper::PATH, "identity"])
        .supervised_output()?;
    if !identity.status.success() {
        return Err("cannot discover local AP radio".into());
    }
    let name = std::str::from_utf8(&identity.stdout)?.trim();
    let info = oer_process::command("iw")
        .args(["phy", name, "info"])
        .supervised_output()?;
    if !info.status.success() {
        return Err("cannot discover local AP capabilities".into());
    }
    crate::channel::verify_ap_capabilities(
        config.channel,
        (phy == PhyExpectation::Ht40).then_some(config.ht40_above),
        phy == PhyExpectation::He20,
        std::str::from_utf8(&info.stdout)?,
    )
}

fn geometry(config: &LocalLinuxConfig, phy: PhyExpectation) -> Geometry {
    let frequency = 2407 + u16::from(config.channel) * 5;
    let width = if phy == PhyExpectation::Ht40 { 40 } else { 20 };
    let center = if width == 20 {
        frequency
    } else if config.ht40_above {
        frequency + 10
    } else {
        frequency - 10
    };
    Geometry {
        frequency,
        width,
        center,
    }
}

fn profile(
    config: &LocalLinuxConfig,
    station: &StationConfig,
    phy: PhyExpectation,
) -> Result<Zeroizing<String>> {
    use std::fmt::Write as _;
    let (ssid, passphrase) = station.credentials();
    let (start, end, mask) = dhcp_range(config.address, config.prefix_length)?;
    if let oer_hil_lab::wifi::NetworkIpv4Configuration::Static {
        address,
        prefix_length,
        gateway,
    } = station.ipv4()
    {
        let station_address = Ipv4Addr::from(address);
        let mask = u32::MAX << (32 - config.prefix_length);
        if prefix_length != config.prefix_length
            || station_address == config.address
            || u32::from(station_address) & mask != u32::from(config.address) & mask
            || gateway.is_some_and(|gateway| Ipv4Addr::from(gateway) != config.address)
        {
            return Err(
                "static station IPv4 settings do not match the local AP subnet/gateway".into(),
            );
        }
    }
    if ssid.is_empty()
        || ssid.len() > 32
        || !(8..=63).contains(&passphrase.len())
        || passphrase.contains(['\n', '\r', '\0'])
    {
        return Err("invalid local AP credentials".into());
    }
    let mut hex = String::new();
    for byte in ssid.bytes() {
        write!(hex, "{byte:02x}")?;
    }
    Ok(Zeroizing::new(format!(
        "{hex}\n{passphrase}\n{}\n{}\n{}\n{}\n{}/{}\n{start},{end},{mask}\n{}\nht\n",
        config.country,
        config.channel,
        geometry(config, phy).iw_width()?,
        u8::from(phy == PhyExpectation::He20),
        config.address,
        config.prefix_length,
        u8::from(
            phy == PhyExpectation::Ht40
                && config.coexistence == oer_hil_lab::config::Coexistence::ForceHt40
        )
    )))
}

fn dhcp_range(address: Ipv4Addr, prefix: u8) -> Result<(Ipv4Addr, Ipv4Addr, Ipv4Addr)> {
    if !(1..=30).contains(&prefix) {
        return Err("AP prefix has no DHCP host range".into());
    }
    let mask = u32::MAX << (32 - prefix);
    let address = u32::from(address);
    let first = (address & mask) + 1;
    let last = (address | !mask) - 1;
    if address < first || address > last {
        return Err("AP address is a network or broadcast address".into());
    }
    let (start, end) = if address - first > last - address {
        (first, address - 1)
    } else {
        (address + 1, last)
    };
    Ok((start.into(), end.into(), mask.into()))
}

fn helper(action: &str, input: Option<&str>) -> Result<Zeroizing<String>> {
    let mut command = oer_process::command("sudo");
    command
        .args(["-n", crate::local::network_helper::PATH, action])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn_owned()?;
    if let Some(input) = input {
        child
            .stdin
            .take()
            .ok_or("helper stdin unavailable")?
            .write_all(input.as_bytes())?;
    }
    let output = child.wait_with_output_timeout(Some(Duration::from_secs(30)))?;
    if !output.status.success() {
        let diagnostic = diagnostic(&output, input);
        return Err(crate::Error::new(format!(
            "Linux AP helper {action} failed with {}: {}",
            output.status,
            diagnostic.trim()
        ))
        .into());
    }
    Ok(diagnostic(&output, input))
}

fn diagnostic(output: &std::process::Output, input: Option<&str>) -> Zeroizing<String> {
    redact(
        Zeroizing::new(format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )),
        input,
    )
}

fn startup_log(path: &Path, input: &str) -> Zeroizing<String> {
    let read = || -> std::io::Result<Vec<u8>> {
        let mut file = std::fs::File::open(path)?;
        let length = file.metadata()?.len();
        file.seek(SeekFrom::Start(length.saturating_sub(65536)))?;
        let mut bytes = Vec::new();
        file.take(65536).read_to_end(&mut bytes)?;
        Ok(bytes)
    };
    match read() {
        Ok(bytes) => redact(
            Zeroizing::new(String::from_utf8_lossy(&bytes).into_owned()),
            Some(input),
        ),
        Err(error) => Zeroizing::new(format!("unavailable ({error})")),
    }
}

fn redact(mut text: Zeroizing<String>, input: Option<&str>) -> Zeroizing<String> {
    if let Some(input) = input {
        let mut lines = input.lines();
        let hex = lines.next().unwrap_or_default();
        let password = lines.next().unwrap_or_default();
        let ssid = hex
            .as_bytes()
            .chunks_exact(2)
            .filter_map(|pair| {
                std::str::from_utf8(pair)
                    .ok()
                    .and_then(|text| u8::from_str_radix(text, 16).ok())
            })
            .collect::<Vec<_>>();
        let ssid = String::from_utf8_lossy(&ssid);
        for secret in [password, hex, ssid.as_ref()] {
            if !secret.is_empty() {
                *text = text.replace(secret, "[redacted]");
            }
        }
    }
    text
}

#[cfg(test)]
mod tests;
