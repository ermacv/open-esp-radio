//! How the router reads the AP's view of one station's link
//! (`oer_tools_lab_client::protocol::station_link::StationLink`), as its
//! Linux wireless stack reports it: the station's counters and the rates of its last frames each way
//! (`iw dev <ap> station get`), the AP's channel and width (`iw dev <ap>
//! info`), the survey of the channel in use (`iw dev <ap> survey dump`) and
//! the station's transmit queues per TID (mac80211's per-station `aqm`
//! debugfs file) and the radio's own queue (the phy's `aqm` file), both
//! verbatim and typed, and the radio's own counters
//! (mac80211's `statistics` debugfs files and mt76's BlockAck misses, when
//! the router has them). Directions are the AP's: `tx` is what the AP sent
//! the station. Each read carries the router's boot clock just before it.
//!
//! The router filters before anything leaves it: the station line that
//! names the peer, every `info` line but the channel's and every survey
//! block but the one in use stay there. The host parses the rest; an `iw`
//! line this parser does not know is ignored, a known one it cannot read
//! refuses the observation.
pub mod queue;

use std::collections::BTreeMap;

use oer_tools_lab_client::protocol::station_link::{Bitrate, RateFormat, StationLink, Survey};

use super::mac_text;
use crate::{
    mac::Mac,
    router::quote,
    ssh::{Remote, Result},
};

/// Station `target`'s link on the AP `interface`.
pub fn link(remote: &mut dyn Remote, interface: &str, target: Mac) -> Result<StationLink> {
    parse_link(&remote.run(&script(interface, target)?)?)
}

/// The router's script: its sections, each after its `[name]` line. A
/// debugfs file is printed whole after a `clock:` line, the boot clock
/// just before its read, and a line break after it: a file whose last line
/// has none must not glue the next section's name to its last value.
fn script(interface: &str, target: Mac) -> Result<String> {
    Ok(format!(
        "set -e\niface={}\nmac={}\n\
         clock() {{ cut -d ' ' -f 1 /proc/uptime; }}\n\
         station_clock=$(clock)\n\
         stats=$(iw dev \"$iface\" station get \"$mac\")\n\
         info=$(iw dev \"$iface\" info)\n\
         survey=$(iw dev \"$iface\" survey dump)\n\
         echo '[station]'\n\
         printf '%s\\n' \"$stats\" | grep -v '^Station ' || true\n\
         echo '[station-clock]'\n\
         printf '%s\\n' \"$station_clock\"\n\
         echo '[channel]'\n\
         printf '%s\\n' \"$info\" | grep '^[[:space:]]*channel ' || true\n\
         echo '[survey]'\n\
         printf '%s\\n' \"$survey\" | awk '/frequency:/ {{ used = /in use/ }} used'\n\
         phy=/sys/kernel/debug/ieee80211/phy$(printf '%s\\n' \"$info\" | awk '$1 == \"wiphy\" {{ print $2 }}')\n\
         echo '[station-aqm]'\n\
         aqm=\"$phy/netdev:$iface/stations/$mac/aqm\"\n\
         if test -r \"$aqm\"; then echo \"clock: $(clock)\"; cat \"$aqm\"; echo; fi\n\
         echo '[radio-aqm]'\n\
         if test -r \"$phy/aqm\"; then echo \"clock: $(clock)\"; cat \"$phy/aqm\"; echo; fi\n\
         echo '[radio]'\n\
         for counter in \"$phy\"/statistics/*; do\n\
         if test -r \"$counter\"; then echo \"${{counter##*/}}: $(cat \"$counter\")\"; fi\n\
         done\n\
         awk -F': ' '/^BA miss count/ {{ print \"ba_miss: \" $2 }}' \"$phy\"/mt76/tx_stats 2>/dev/null || true\n",
        quote(interface)?,
        quote(&mac_text(target))?
    ))
}

/// The sections of the script's output, by name.
#[derive(Default)]
struct Sections<'a> {
    station: Vec<&'a str>,
    channel: Vec<&'a str>,
    station_clock: Vec<&'a str>,
    survey: Vec<&'a str>,
    station_aqm: Vec<&'a str>,
    radio_aqm: Vec<&'a str>,
    radio: Vec<&'a str>,
}

fn sections(text: &str) -> Result<Sections<'_>> {
    let mut sections = Sections::default();
    let mut current: Option<&mut Vec<&str>> = None;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        match line.trim() {
            "[station]" => current = Some(&mut sections.station),
            "[channel]" => current = Some(&mut sections.channel),
            "[station-clock]" => current = Some(&mut sections.station_clock),
            "[survey]" => current = Some(&mut sections.survey),
            "[station-aqm]" => current = Some(&mut sections.station_aqm),
            "[radio-aqm]" => current = Some(&mut sections.radio_aqm),
            "[radio]" => current = Some(&mut sections.radio),
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
    let observed_at_ms = match sections.station_clock.as_slice() {
        [clock] => queue::parse_clock(clock)?,
        _ => return Err("the router's link observation has no clock".to_owned()),
    };
    let mut link = StationLink {
        observed_at_ms,
        associated_at_boottime_ms: None,
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
        station_queues: None,
        radio_queues: None,
        radio: BTreeMap::new(),
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
            "associated at [boottime]" => {
                link.associated_at_boottime_ms = Some(parse_boottime(value)?);
            }
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
    link.station_queues = queue::parse_station(&sections.station_aqm)?;
    link.radio_queues = queue::parse_radio(&sections.radio_aqm)?;
    link.tid0_aqm_drops = link
        .station_queues
        .as_ref()
        .and_then(|queues| queues.tids.iter().find(|queue| queue.tid == 0))
        .and_then(|queue| queue.drops)
        .map(|drops| drops.value);
    // A driver's counters are its own: one that reads no number is not one.
    link.radio = pairs(&sections.radio)
        .filter_map(|(key, value)| Some((key.to_owned(), value.parse().ok()?)))
        .collect();
    Ok(link)
}

/// `associated at [boottime]`'s seconds with three decimals (`1934.180s`),
/// in milliseconds.
///
/// SOURCE: iw 6.9 `station.c::print_sta_handler`, `"%llu.%.3llus"` of
/// `NL80211_STA_INFO_ASSOC_AT_BOOTTIME` in seconds and milliseconds.
fn parse_boottime(value: &str) -> Result<u64> {
    let invalid = || format!("the router's association time is unreadable: `{value}`");
    let (seconds, thousandths) = value
        .strip_suffix('s')
        .and_then(|value| value.split_once('.'))
        .ok_or_else(invalid)?;
    if thousandths.len() != 3 || !thousandths.bytes().all(|digit| digit.is_ascii_digit()) {
        return Err(invalid());
    }
    let seconds = seconds.parse::<u64>().map_err(|_| invalid())?;
    let thousandths = thousandths.parse::<u64>().map_err(|_| invalid())?;
    seconds
        .checked_mul(1000)
        .and_then(|ms| ms.checked_add(thousandths))
        .ok_or_else(invalid)
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
