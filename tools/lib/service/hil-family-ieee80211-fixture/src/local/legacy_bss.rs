//! A laptop-hosted non-ERP (802.11b) BSS on the laboratory AP's channel.
//!
//! Its DSSS beacons carry no ERP or HT element, so a neighbouring ERP/HT AP
//! observes an overlapping legacy BSS and must enable protection. The BSS
//! serves no clients: its passphrase is random and it carries no network.

use std::{
    fmt::Write as _,
    io::{Read as _, Write as _},
    path::Path,
    process::Stdio,
    time::Duration,
};

use oer_process::CommandExt as _;
use zeroize::Zeroizing;

use crate::{
    Result,
    local::wpa_control::{Control, field},
};
use oer_hil_lab::config::LegacyBssConfig;

const SSID: &str = "open-radio-legacy-bss";

/// Restores the laptop radio to managed mode when dropped.
pub struct LegacyBss {
    _owned: (),
}

impl LegacyBss {
    /// Start the BSS on `channel` and verify that hostapd enabled it
    /// without HT.
    pub fn start(config: &LegacyBssConfig, channel: u8, output: &Path) -> Result<Self> {
        crate::local::network_helper::doctor()?;
        let input = profile(config, channel)?;
        // Own cleanup before any helper mutation, including failed setup.
        let owner = Self { _owned: () };
        let mut child = oer_process::command("sudo")
            .args(["-n", crate::local::network_helper::PATH, "ap"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn_owned()?;
        child
            .stdin
            .take()
            .ok_or("helper stdin unavailable")?
            .write_all(input.as_bytes())?;
        let output_status = child.wait_with_output_timeout(Some(Duration::from_secs(30)))?;
        if !output_status.status.success() {
            return Err(crate::Error::new(format!(
                "legacy BSS helper failed with {}: {}",
                output_status.status,
                String::from_utf8_lossy(&output_status.stderr).trim()
            ))
            .into());
        }
        let mut control = Control::connect(Path::new("/run/open-radio-hostapd/wlan0"))?;
        let status = control.wait_enabled()?;
        if field(&status, "ieee80211n") != Some("0")
            || field(&status, "channel") != Some(&channel.to_string())
        {
            return Err(
                format!("hostapd did not enable an 802.11b BSS on channel {channel}").into(),
            );
        }
        oer_durable::atomic_json(
            &output.join("legacy-bss.json"),
            &serde_json::json!({"schema": 1, "ssid": SSID, "channel": channel,
                "phy": "dsss", "rates_mbps": [1, 2, 5.5, 11], "erp_element": false,
                "ht_element": false}),
        )?;
        Ok(owner)
    }
}

impl Drop for LegacyBss {
    fn drop(&mut self) {
        oer_process::cleanup(|| {
            oer_hil_workload::fixture::cleanup::record(
                "restore managed Wi-Fi after legacy BSS",
                || {
                    let status = oer_process::command("sudo")
                        .args(["-n", crate::local::network_helper::PATH, "managed"])
                        .supervised_status()?;
                    if !status.success() {
                        return Err(format!("legacy BSS restore failed with {status}").into());
                    }
                    Ok(())
                },
            );
        });
    }
}

fn profile(config: &LegacyBssConfig, channel: u8) -> Result<Zeroizing<String>> {
    if !(1..=13).contains(&channel) {
        return Err(format!("legacy BSS channel {channel} is outside 1..=13").into());
    }
    let mut random = Zeroizing::new([0_u8; 24]);
    std::fs::File::open("/dev/urandom")?.read_exact(random.as_mut())?;
    let mut passphrase = Zeroizing::new(String::new());
    for byte in random.iter() {
        write!(passphrase, "{byte:02x}")?;
    }
    let mut ssid = String::new();
    for byte in SSID.bytes() {
        write!(ssid, "{byte:02x}")?;
    }
    Ok(Zeroizing::new(format!(
        "{ssid}\n{}\n{}\n{channel}\nHT20\n0\nnone\nnone\n0\ndsss\n",
        passphrase.as_str(),
        config.country
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_selects_an_unaddressed_dsss_bss_with_a_fresh_passphrase() {
        let config = LegacyBssConfig {
            country: "DE".into(),
        };
        let first = profile(&config, 13).unwrap();
        let lines = first.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 10);
        assert_eq!(lines[3..], ["13", "HT20", "0", "none", "none", "0", "dsss"]);
        assert_eq!(lines[1].len(), 48);
        assert_ne!(
            lines[1],
            profile(&config, 13).unwrap().lines().nth(1).unwrap()
        );
        assert!(profile(&config, 14).is_err());
    }
}
