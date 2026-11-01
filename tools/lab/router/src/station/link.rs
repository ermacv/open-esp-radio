//! The AP's view of one station's link, as its Linux wireless stack reports
//! it: the station's counters and the rates of its last frames each way
//! (`iw dev <ap> station get`), the AP's channel and width (`iw dev <ap>
//! info`), the survey of the channel in use (`iw dev <ap> survey dump`) and
//! the station's TID 0 queue drops (mac80211's per-station `aqm` debugfs
//! file). Directions are the AP's: `tx` is what the AP sent the station.
//!
//! The router filters before anything leaves it: the station line that
//! names the peer, every `info` line but the channel's and every survey
//! block but the one in use stay there. The host parses the rest; an `iw`
//! line this parser does not know is ignored, a known one it cannot read
//! refuses the observation.

use serde::{Deserialize, Serialize};

use super::mac_text;
use crate::{
    mac::Mac,
    router::quote,
    ssh::{Remote, Result},
};

/// What the AP reports of one station's link at one moment. Counters are
/// the station's since its association (the survey's since the radio
/// started); a counter or a value the driver does not report is absent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StationLink {
    /// The station's frames and bytes the AP received and sent.
    pub rx_bytes: u64,
    pub rx_packets: u64,
    pub tx_bytes: u64,
    pub tx_packets: u64,
    /// The AP's retransmissions to the station and the frames it gave up.
    pub tx_retries: u64,
    pub tx_failed: u64,
    /// The station's frames the AP dropped after receiving them.
    pub rx_drop_misc: Option<u64>,
    /// Air time of the station's frames each way, in microseconds.
    pub rx_duration_us: Option<u64>,
    pub tx_duration_us: Option<u64>,
    /// The averaged signal of the station's frames, in dBm.
    pub signal_avg_dbm: Option<i32>,
    /// The rate of the last frame from the station and of the last one to it.
    pub rx_bitrate: Option<Bitrate>,
    pub tx_bitrate: Option<Bitrate>,
    /// The AP's channel, its frequency and its width.
    pub channel: Option<u8>,
    pub frequency_mhz: Option<u32>,
    pub width_mhz: Option<u32>,
    /// The channel in use's survey.
    pub survey: Option<Survey>,
    /// The packets the AP's queue for the station's TID 0 dropped; absent
    /// without a readable debugfs file.
    pub tid0_aqm_drops: Option<u64>,
}

/// The survey of the channel in use: the radio's time on it, busy, receiving
/// (and receiving its own BSS) and transmitting, in milliseconds, and its
/// noise floor.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Survey {
    pub frequency_mhz: u32,
    pub noise_dbm: Option<i32>,
    pub active_ms: Option<u64>,
    pub busy_ms: Option<u64>,
    pub receive_ms: Option<u64>,
    pub bss_receive_ms: Option<u64>,
    pub transmit_ms: Option<u64>,
}

/// The PHY format of a reported rate.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RateFormat {
    /// DSSS, CCK or OFDM: no MCS.
    Legacy,
    Ht,
    Vht,
    He,
    Eht,
}

/// One rate as `iw` prints it, and what it names.
///
/// SOURCE: iw 6.9 `station.c::parse_bitrate`: the rate in units of
/// 100 kbit/s as `"%d.%d MBit/s"` (`"(unknown)"` without one), then
/// ` MCS %d` (HT), ` VHT-MCS %d`, ` HE-MCS %d` or ` EHT-MCS %d`, a width
/// token other than 20 MHz (` 40MHz`, ` 80MHz`, ` 80P80MHz`, ` 160MHz`,
/// ` 320MHz`), ` short GI`, and the stream count (` VHT-NSS %d`,
/// ` HE-NSS %d`, ` EHT-NSS %d`).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Bitrate {
    /// The line as `iw` printed it.
    pub text: String,
    pub kbit_per_s: Option<u64>,
    pub format: RateFormat,
    pub mcs: Option<u8>,
    /// The spatial streams a VHT, HE or EHT rate names; an HT MCS implies
    /// its own.
    pub streams: Option<u8>,
    pub width_mhz: u32,
    pub short_gi: bool,
}

/// Station `target`'s link on the AP `interface`.
pub fn link(remote: &mut dyn Remote, interface: &str, target: Mac) -> Result<StationLink> {
    parse_link(&remote.run(&script(interface, target)?)?)
}

/// The router's script: four sections, each after its `[name]` line.
fn script(interface: &str, target: Mac) -> Result<String> {
    Ok(format!(
        "set -e\niface={}\nmac={}\n\
         stats=$(iw dev \"$iface\" station get \"$mac\")\n\
         info=$(iw dev \"$iface\" info)\n\
         survey=$(iw dev \"$iface\" survey dump)\n\
         echo '[station]'\n\
         printf '%s\\n' \"$stats\" | grep -v '^Station ' || true\n\
         echo '[channel]'\n\
         printf '%s\\n' \"$info\" | grep '^[[:space:]]*channel ' || true\n\
         echo '[survey]'\n\
         printf '%s\\n' \"$survey\" | awk '/frequency:/ {{ used = /in use/ }} used'\n\
         echo '[aqm]'\n\
         for aqm in /sys/kernel/debug/ieee80211/*/netdev:\"$iface\"/stations/\"$mac\"/aqm; do\n\
         if test -r \"$aqm\"; then awk '$1 == \"0\" {{ print \"tid0 drops:\", $6 }}' \"$aqm\"; fi\n\
         done\n",
        quote(interface)?,
        quote(&mac_text(target))?
    ))
}

/// The sections of the script's output, by name.
#[derive(Default)]
struct Sections<'a> {
    station: Vec<&'a str>,
    channel: Vec<&'a str>,
    survey: Vec<&'a str>,
    aqm: Vec<&'a str>,
}

fn sections(text: &str) -> Result<Sections<'_>> {
    let mut sections = Sections::default();
    let mut current: Option<&mut Vec<&str>> = None;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        match line.trim() {
            "[station]" => current = Some(&mut sections.station),
            "[channel]" => current = Some(&mut sections.channel),
            "[survey]" => current = Some(&mut sections.survey),
            "[aqm]" => current = Some(&mut sections.aqm),
            _ => match current.as_mut() {
                Some(section) => section.push(line),
                None => {
                    return Err("the router's link observation starts without a section".to_owned());
                }
            },
        }
    }
    Ok(sections)
}

/// The `key: value` pairs of `iw`'s indented lines.
fn pairs<'a>(lines: &[&'a str]) -> impl Iterator<Item = (&'a str, &'a str)> {
    lines
        .iter()
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim(), value.trim()))
}

fn number<T: std::str::FromStr>(key: &str, text: &str) -> Result<T> {
    text.parse()
        .map_err(|_| format!("the router's `{key}` is not a number: `{text}`"))
}

/// The number before `unit` in `value`, as `123 us` or `-92 dBm` print it.
fn with_unit<T: std::str::FromStr>(key: &str, value: &str, unit: &str) -> Result<T> {
    let mut words = value.split_whitespace();
    match (words.next(), words.next()) {
        (Some(number_text), Some(found)) if found == unit => number(key, number_text),
        _ => Err(format!("the router's `{key}` is not in {unit}: `{value}`")),
    }
}

/// Parse the script's output.
pub fn parse_link(text: &str) -> Result<StationLink> {
    let sections = sections(text)?;
    let mut link = StationLink {
        rx_bytes: 0,
        rx_packets: 0,
        tx_bytes: 0,
        tx_packets: 0,
        tx_retries: 0,
        tx_failed: 0,
        rx_drop_misc: None,
        rx_duration_us: None,
        tx_duration_us: None,
        signal_avg_dbm: None,
        rx_bitrate: None,
        tx_bitrate: None,
        channel: None,
        frequency_mhz: None,
        width_mhz: None,
        survey: None,
        tid0_aqm_drops: None,
    };
    let mut required = [false; 6];
    for (key, value) in pairs(&sections.station) {
        let counter = match key {
            "rx bytes" => Some((0, &mut link.rx_bytes)),
            "rx packets" => Some((1, &mut link.rx_packets)),
            "tx bytes" => Some((2, &mut link.tx_bytes)),
            "tx packets" => Some((3, &mut link.tx_packets)),
            "tx retries" => Some((4, &mut link.tx_retries)),
            "tx failed" => Some((5, &mut link.tx_failed)),
            _ => None,
        };
        if let Some((index, counter)) = counter {
            *counter = number(key, value)?;
            if let Some(seen) = required.get_mut(index) {
                *seen = true;
            }
            continue;
        }
        match key {
            "rx drop misc" => link.rx_drop_misc = Some(number(key, value)?),
            "rx duration" => link.rx_duration_us = Some(with_unit(key, value, "us")?),
            "tx duration" => link.tx_duration_us = Some(with_unit(key, value, "us")?),
            "signal avg" => {
                let first = value.split_whitespace().next().unwrap_or_default();
                link.signal_avg_dbm = Some(number(key, first)?);
            }
            "rx bitrate" => link.rx_bitrate = Some(parse_bitrate(value)?),
            "tx bitrate" => link.tx_bitrate = Some(parse_bitrate(value)?),
            _ => {}
        }
    }
    if required.contains(&false) {
        return Err(
            "the router's station report lacks its byte, packet, retry or failure counters"
                .to_owned(),
        );
    }
    if let Some(line) = sections.channel.first() {
        let (channel, frequency, width) = parse_channel(line)?;
        link.channel = Some(channel);
        link.frequency_mhz = Some(frequency);
        link.width_mhz = width;
    }
    if !sections.survey.is_empty() {
        link.survey = Some(parse_survey(&sections.survey)?);
    }
    for (key, value) in pairs(&sections.aqm) {
        if key == "tid0 drops" {
            link.tid0_aqm_drops = Some(number(key, value)?);
        }
    }
    Ok(link)
}

/// `iw dev <ap> info`'s channel line: `channel 13 (2472 MHz), width: 40
/// MHz, center1: 2462 MHz`; the width is absent for a channel without one
/// (`no HT`).
fn parse_channel(line: &str) -> Result<(u8, u32, Option<u32>)> {
    let invalid = || format!("the router's channel line is unreadable: `{}`", line.trim());
    let words = line.split_whitespace().collect::<Vec<_>>();
    let ["channel", channel, frequency, "MHz),", ..] = words.as_slice() else {
        return Err(invalid());
    };
    let channel = channel.parse().map_err(|_| invalid())?;
    let frequency = frequency
        .strip_prefix('(')
        .and_then(|frequency| frequency.parse().ok())
        .ok_or_else(invalid)?;
    let width = match words.iter().position(|word| *word == "width:") {
        Some(at) => match (words.get(at + 1), words.get(at + 2)) {
            (Some(width), Some(unit)) if unit.starts_with("MHz") => {
                Some(width.parse().map_err(|_| invalid())?)
            }
            _ => None,
        },
        None => None,
    };
    Ok((channel, frequency, width))
}

/// The survey block of the channel in use.
fn parse_survey(lines: &[&str]) -> Result<Survey> {
    let mut survey = Survey::default();
    let mut frequency = None;
    for (key, value) in pairs(lines) {
        let time = || with_unit::<u64>(key, value, "ms");
        match key {
            "frequency" => {
                if !value.ends_with("[in use]") {
                    return Err("the router's survey holds a channel not in use".to_owned());
                }
                frequency = Some(with_unit(key, value, "MHz")?);
            }
            "noise" => survey.noise_dbm = Some(with_unit(key, value, "dBm")?),
            "channel active time" => survey.active_ms = Some(time()?),
            "channel busy time" => survey.busy_ms = Some(time()?),
            "channel receive time" => survey.receive_ms = Some(time()?),
            "channel BSS receive time" => survey.bss_receive_ms = Some(time()?),
            "channel transmit time" => survey.transmit_ms = Some(time()?),
            _ => {}
        }
    }
    survey.frequency_mhz = frequency.ok_or("the router's survey names no channel in use")?;
    Ok(survey)
}

/// Parse one rate as `iw` prints it ([`Bitrate`]).
pub fn parse_bitrate(text: &str) -> Result<Bitrate> {
    let invalid = || format!("the router's rate is unreadable: `{text}`");
    let mut words = text.split_whitespace();
    let kbit_per_s = match words.next() {
        Some("(unknown)") => None,
        Some(rate) => {
            if words.next() != Some("MBit/s") {
                return Err(invalid());
            }
            let (whole, tenths) = rate.split_once('.').ok_or_else(invalid)?;
            let whole = whole.parse::<u64>().map_err(|_| invalid())?;
            let tenths = match tenths.as_bytes() {
                [digit] if digit.is_ascii_digit() => u64::from(digit - b'0'),
                _ => return Err(invalid()),
            };
            Some(
                whole
                    .checked_mul(1000)
                    .and_then(|kbit| kbit.checked_add(tenths * 100))
                    .ok_or_else(invalid)?,
            )
        }
        None => return Err(invalid()),
    };
    let mut rate = Bitrate {
        text: text.to_owned(),
        kbit_per_s,
        format: RateFormat::Legacy,
        mcs: None,
        streams: None,
        width_mhz: 20,
        short_gi: false,
    };
    let index = |words: &mut std::str::SplitWhitespace<'_>| -> Result<u8> {
        words
            .next()
            .and_then(|value| value.parse().ok())
            .ok_or_else(invalid)
    };
    while let Some(word) = words.next() {
        match word {
            "MCS" => (rate.format, rate.mcs) = (RateFormat::Ht, Some(index(&mut words)?)),
            "VHT-MCS" => (rate.format, rate.mcs) = (RateFormat::Vht, Some(index(&mut words)?)),
            "HE-MCS" => (rate.format, rate.mcs) = (RateFormat::He, Some(index(&mut words)?)),
            "EHT-MCS" => (rate.format, rate.mcs) = (RateFormat::Eht, Some(index(&mut words)?)),
            "VHT-NSS" | "HE-NSS" | "EHT-NSS" => rate.streams = Some(index(&mut words)?),
            "short" => rate.short_gi = words.next() == Some("GI"),
            "40MHz" => rate.width_mhz = 40,
            "80MHz" => rate.width_mhz = 80,
            "80P80MHz" | "160MHz" => rate.width_mhz = 160,
            "320MHz" => rate.width_mhz = 320,
            // Other HE and EHT fields (GI, DCM, RU allocation) stay in
            // the text.
            _ => {}
        }
    }
    Ok(rate)
}

#[cfg(test)]
mod tests;
