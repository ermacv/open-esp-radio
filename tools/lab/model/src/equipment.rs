//! What the stand's equipment reports of itself, as `cargo stand fixtures`
//! shows it: each fixture or SSH host with the resource key a lease claims
//! it by, whether it answers, its interfaces and their channels, and how
//! busy the channel it uses is (clear-channel assessment, from the radio's
//! survey). The LAB service probes the equipment it reaches over SSH
//! (`oer-lab-router`); the host's own radios are the fixtures' (`oer-lab-fixture`).
#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
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

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
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

/// Interfaces from `@interface NAME` blocks of `iw dev NAME info` type and
/// channel lines followed by `iw dev NAME survey dump`.
pub fn interfaces(text: &str) -> Vec<Interface> {
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

/// `iw`'s channel line in short form, `13 (2472 MHz), width: 40 MHz,
/// center1: 2462 MHz` as `13 · 2472 MHz · 40 MHz wide`; a line of another
/// shape stays whole.
pub fn short_channel(line: &str) -> String {
    let short = || {
        let (number, rest) = line.split_once(" (")?;
        let (frequency, rest) = rest.split_once(')')?;
        let width = rest.split_once("width: ")?.1.split(',').next()?.trim();
        Some(format!("{number} · {frequency} · {width} wide"))
    };
    short().unwrap_or_else(|| line.to_owned())
}

/// The interfaces `text` (see [`interfaces`]) names, else just `names`.
pub fn interfaces_or_names(text: &str, names: &[String]) -> Vec<Interface> {
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
pub fn radio_switch(hard: bool, soft: bool) -> (&'static str, bool) {
    match (hard, soft) {
        (true, _) => ("radio hard-blocked", true),
        (false, true) => ("radio soft-blocked", true),
        (false, false) => ("radio unblocked", false),
    }
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
