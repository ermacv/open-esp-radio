//! The host fixtures the stand file names, as `cargo stand fixtures` and the
//! dashboard show them: the host's Wi-Fi radios and Bluetooth adapter and the managed
//! OpenWrt hosts, each with the resource key a lease claims it by, whether it
//! answers, its interfaces and their channels, and how busy the channel it
//! uses is (clear-channel assessment, from the radio's survey).
//!
//! Probing an OpenWrt host takes an SSH round trip, so the dashboard refreshes
//! these in the background and serves the last result.
#![forbid(unsafe_code)]

use std::{path::Path, time::Duration};

use serde::Serialize;

/// How long one SSH probe may take before the host counts as not answering.
const SSH_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Fixture {
    /// What the lab uses it for, e.g. `station fixture` or `air observer`.
    pub role: String,
    /// The SSH alias, adapter or radio name.
    pub name: String,
    /// The resource a lease claims it by; `None` when it cannot be resolved.
    pub key: Option<String>,
    pub reachable: bool,
    /// Model, firmware and uptime, as the host reports them.
    pub detail: Option<String>,
    pub interfaces: Vec<Interface>,
    pub error: Option<String>,
    /// Its probed radio switch blocks it: it answers no command until
    /// unblocked. Only the Bluetooth adapter's switch is probed.
    pub blocked: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Interface {
    pub name: String,
    /// `AP`, `monitor`, `managed` and so on.
    pub kind: Option<String>,
    pub channel: Option<String>,
    /// Busy time over active time on the frequency in use, in percent.
    pub cca_busy_percent: Option<f64>,
}

/// One line per fixture and one indented line per interface.
pub fn describe(fixtures: &[Fixture]) -> String {
    let mut text = String::new();
    for fixture in fixtures {
        text.push_str(&format!(
            "{} ({}): {}{}\n",
            fixture.name,
            fixture.role,
            if fixture.reachable {
                "answers"
            } else {
                "not answering"
            },
            fixture
                .error
                .as_ref()
                .map(|error| format!(": {error}"))
                .or(fixture.detail.as_ref().map(|detail| format!("; {detail}")))
                .unwrap_or_default()
        ));
        if let Some(key) = &fixture.key {
            text.push_str(&format!("  claimed as {key}\n"));
        }
        for interface in &fixture.interfaces {
            text.push_str(&format!(
                "  {} {}{}{}\n",
                interface.name,
                interface.kind.as_deref().unwrap_or("?"),
                interface
                    .channel
                    .as_ref()
                    .map(|channel| format!(", channel {channel}"))
                    .unwrap_or_default(),
                interface
                    .cca_busy_percent
                    .map(|busy| format!(", CCA busy {busy}%"))
                    .unwrap_or_default()
            ));
        }
    }
    text
}

/// Every fixture of the stand file at `lab` and of this host.
pub fn probe(lab: &Path) -> Vec<Fixture> {
    let config = std::fs::read_to_string(lab)
        .ok()
        .and_then(|text| toml::from_str::<toml::Value>(&text).ok());
    let text = |section: &str, key: &str| {
        config
            .as_ref()
            .and_then(|config| config.get(section)?.get(key)?.as_str())
            .map(str::to_owned)
    };
    let mut hosts = Vec::new();
    if let Some(target) = text("station_fixture", "ssh_target") {
        hosts.push(("station fixture", target));
    }
    if let Some(target) = text("air_observer", "ssh_target") {
        hosts.push(("air observer", target));
    }
    // SSH probes run side by side: one slow host does not delay the others.
    let remote = std::thread::scope(|scope| {
        hosts
            .iter()
            .map(|(role, target)| scope.spawn(move || openwrt(role, target)))
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|probe| probe.join().ok())
            .collect::<Vec<_>>()
    });
    let mut fixtures = local_radios();
    fixtures.extend(bluetooth(text("bluetooth", "adapter")));
    fixtures.extend(remote);
    fixtures
}

fn openwrt(role: &str, target: &str) -> Fixture {
    let mut fixture = Fixture {
        role: role.to_owned(),
        name: target.to_owned(),
        ..Fixture::default()
    };
    let script = r#"cat /proc/sys/kernel/random/boot_id
cat /tmp/sysinfo/model 2>/dev/null
. /etc/openwrt_release 2>/dev/null; echo "$DISTRIB_DESCRIPTION"
cut -d. -f1 /proc/uptime
for i in $(iw dev | awk '$1 == "Interface" {print $2}'); do
  echo "@interface $i"
  iw dev "$i" info | awk '$1 == "type" || $1 == "channel"'
  iw dev "$i" survey dump
done"#;
    let output = oer_process::output(
        &mut oer_stand_ssh::command(target, script),
        Some(SSH_TIMEOUT),
    );
    match output {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout);
            let mut lines = text.lines();
            fixture.key = oer_stand_ssh::host_key(lines.next().unwrap_or_default()).ok();
            let model = lines.next().unwrap_or_default().trim();
            let release = lines.next().unwrap_or_default().trim();
            let uptime = lines
                .next()
                .and_then(|uptime| uptime.trim().parse::<u64>().ok())
                .map(|seconds| format!("up {}d {}h", seconds / 86400, seconds % 86400 / 3600))
                .unwrap_or_default();
            fixture.detail = Some(
                [model, release, &uptime]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · "),
            );
            fixture.interfaces = interfaces(&lines.collect::<Vec<_>>().join("\n"));
            fixture.reachable = true;
        }
        Ok(output) => {
            fixture.error = Some(
                String::from_utf8_lossy(&output.stderr)
                    .lines()
                    .last()
                    .unwrap_or("ssh failed")
                    .to_owned(),
            );
        }
        Err(error) => fixture.error = Some(error.to_string()),
    }
    fixture
}

/// Interfaces from `@interface NAME` blocks of `iw dev NAME info` type and
/// channel lines followed by `iw dev NAME survey dump`.
fn interfaces(text: &str) -> Vec<Interface> {
    let mut interfaces: Vec<Interface> = Vec::new();
    let mut in_use = false;
    let mut active = None;
    let mut busy = None;
    let finish = |interface: Option<&mut Interface>, active: Option<u64>, busy: Option<u64>| {
        if let (Some(interface), Some(active), Some(busy)) = (interface, active, busy)
            && active > 0
        {
            interface.cca_busy_percent =
                Some((busy as f64 * 1000.0 / active as f64).round() / 10.0);
        }
    };
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix("@interface ") {
            finish(interfaces.last_mut(), active.take(), busy.take());
            in_use = false;
            interfaces.push(Interface {
                name: name.to_owned(),
                ..Interface::default()
            });
        } else if let Some(kind) = line.strip_prefix("type ") {
            if let Some(interface) = interfaces.last_mut() {
                interface.kind = Some(kind.to_owned());
            }
        } else if let Some(channel) = line
            .strip_prefix("channel ")
            .filter(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
        {
            // `iw dev info`'s channel, not a survey's `channel ... time:`.
            if let Some(interface) = interfaces.last_mut() {
                interface.channel = Some(short_channel(channel));
            }
        } else if line.starts_with("frequency:") {
            if in_use {
                finish(interfaces.last_mut(), active.take(), busy.take());
            }
            in_use = line.contains("[in use]");
        } else if in_use {
            let number = |prefix: &str| {
                line.strip_prefix(prefix)?
                    .trim()
                    .trim_end_matches("ms")
                    .trim()
                    .parse::<u64>()
                    .ok()
            };
            active = number("channel active time:").or(active);
            busy = number("channel busy time:").or(busy);
        }
    }
    finish(interfaces.last_mut(), active, busy);
    interfaces
}

/// This host's Wi-Fi radios, claimed by their sysfs path.
fn local_radios() -> Vec<Fixture> {
    let Ok(entries) = std::fs::read_dir("/sys/class/ieee80211") else {
        return Vec::new();
    };
    let mut radios = entries
        .flatten()
        .map(|entry| {
            let phy = entry.file_name().to_string_lossy().into_owned();
            let key = entry
                .path()
                .canonicalize()
                .ok()
                .map(|path| format!("local-radio:{}", path.display()));
            let interfaces = std::fs::read_dir(entry.path().join("device/net"))
                .into_iter()
                .flatten()
                .flatten()
                .map(|interface| interface.file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let script = interfaces
                .iter()
                .map(|interface| {
                    format!(
                        "echo '@interface {interface}'; iw dev {interface} info | awk '$1 == \"type\" || $1 == \"channel\"'; iw dev {interface} survey dump"
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            let probed = oer_process::output(
                oer_process::command("sh").args(["-c", &script]),
                Some(Duration::from_secs(5)),
            )
            .ok()
            .map(|output| interfaces_or_names(&String::from_utf8_lossy(&output.stdout), &interfaces))
            .unwrap_or_default();
            Fixture {
                role: String::from("host Wi-Fi"),
                name: phy,
                key,
                reachable: true,
                detail: std::fs::read_to_string(entry.path().join("device/driver/module/drivers"))
                    .ok()
                    .or_else(|| {
                        std::fs::read_link(entry.path().join("device/driver"))
                            .ok()
                            .and_then(|driver| Some(format!("driver {}", driver.file_name()?.to_string_lossy())))
                    }),
                interfaces: probed,
                error: None,
                blocked: false,
            }
        })
        .collect::<Vec<_>>();
    radios.sort_by(|a, b| a.name.cmp(&b.name));
    radios
}

/// `iw`'s channel line in short form, `13 (2472 MHz), width: 40 MHz,
/// center1: 2462 MHz` as `13 · 2472 MHz · 40 MHz wide`; a line of another
/// shape stays whole.
fn short_channel(line: &str) -> String {
    let short = || {
        let (number, rest) = line.split_once(" (")?;
        let (frequency, rest) = rest.split_once(')')?;
        let width = rest.split_once("width: ")?.1.split(',').next()?.trim();
        Some(format!("{number} · {frequency} · {width} wide"))
    };
    short().unwrap_or_else(|| line.to_owned())
}

fn interfaces_or_names(text: &str, names: &[String]) -> Vec<Interface> {
    let parsed = interfaces(text);
    if parsed.is_empty() {
        names
            .iter()
            .map(|name| Interface {
                name: name.clone(),
                ..Interface::default()
            })
            .collect()
    } else {
        parsed
    }
}

/// A radio switch's state as the fixture's detail, and whether it blocks.
fn radio_switch(hard: bool, soft: bool) -> (&'static str, bool) {
    match (hard, soft) {
        (true, _) => ("radio hard-blocked", true),
        (false, true) => ("radio soft-blocked", true),
        (false, false) => ("radio unblocked", false),
    }
}

/// The Bluetooth adapter the lab configuration names, `hci0` by default.
fn bluetooth(adapter: Option<String>) -> Option<Fixture> {
    let adapter = adapter.unwrap_or_else(|| String::from("hci0"));
    let path = Path::new("/sys/class/bluetooth").join(&adapter);
    let key = path
        .canonicalize()
        .ok()
        .map(|path| format!("bluetooth:{}", path.display()));
    // The adapter's radio switch: a blocked adapter answers no HCI command.
    let rfkill = std::fs::read_dir(&path)
        .into_iter()
        .flatten()
        .flatten()
        .find(|entry| entry.file_name().to_string_lossy().starts_with("rfkill"))
        .map(|entry| {
            let blocked = |kind: &str| {
                std::fs::read_to_string(entry.path().join(kind))
                    .is_ok_and(|state| state.trim() == "1")
            };
            radio_switch(blocked("hard"), blocked("soft"))
        });
    Some(Fixture {
        role: String::from("host Bluetooth"),
        blocked: rfkill.is_some_and(|(_, blocked)| blocked),
        detail: rfkill.map(|(state, _)| state.to_owned()),
        reachable: key.is_some(),
        error: key
            .is_none()
            .then(|| format!("{} is absent", path.display())),
        name: adapter,
        key,
        ..Fixture::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_survey_gives_the_busy_share_of_the_frequency_in_use() {
        let text = "@interface phy0-ap0
type AP
channel 13 (2472 MHz), width: 40 MHz, center1: 2462 MHz
Survey data from phy0-ap0
	frequency:			2467 MHz
	channel active time:		100 ms
	channel busy time:		99 ms
Survey data from phy0-ap0
	frequency:			2472 MHz [in use]
	noise:				-91 dBm
	channel active time:		2000 ms
	channel busy time:		500 ms
@interface mon0
type monitor";
        let interfaces = interfaces(text);
        assert_eq!(interfaces.len(), 2);
        assert_eq!(interfaces[0].kind.as_deref(), Some("AP"));
        assert_eq!(
            interfaces[0].channel.as_deref(),
            Some("13 · 2472 MHz · 40 MHz wide")
        );
        assert_eq!(interfaces[0].cca_busy_percent, Some(25.0));
        assert_eq!(interfaces[1].kind.as_deref(), Some("monitor"));
        assert_eq!(interfaces[1].cca_busy_percent, None);
        assert_eq!(short_channel("36 (5180 MHz)"), "36 (5180 MHz)");
    }

    #[test]
    fn a_soft_or_hard_blocked_radio_blocks_its_fixture() {
        assert_eq!(radio_switch(false, true), ("radio soft-blocked", true));
        assert_eq!(radio_switch(true, false), ("radio hard-blocked", true));
        assert_eq!(radio_switch(false, false), ("radio unblocked", false));
    }
}
