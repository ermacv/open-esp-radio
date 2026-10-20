//! Stand build parameters for the W2 active scanner. The owner cap belongs
//! to this lab profile; a board and the production driver have no default cap.

use crate::w1;
use oer_ieee80211_port::backend_protection::BasicRates;

pub const IMAGE: &str = "w2";
pub const LAB_POWER_CAP_DBM: i8 = 10;
pub const ENV: [&str; 5] = [
    "OER_W2_CHANNEL",
    "OER_W2_BSSID",
    "OER_W2_SSID_HEX",
    "OER_W2_COUNTRY",
    "OER_W2_POWER_CAP_DBM",
];

pub fn runtime_env(config: &w1::Config) -> Vec<(String, Option<String>)> {
    config
        .runtime_env()
        .into_iter()
        .map(|(key, value)| (key.replace("OER_W1_", "OER_W2_"), Some(value)))
        .chain([(ENV[4].to_owned(), Some(LAB_POWER_CAP_DBM.to_string()))])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn w2_uses_the_router_country_and_an_explicit_lab_cap() {
        let config = w1::Config {
            channel: 13,
            bssid: oer_lab_router::mac::Mac::new([2, 0, 0, 0, 0, 1]).expect("MAC"),
            ssid: w1::Ssid::new(b"Lab").expect("SSID"),
            country: *b"DE",
        };
        let env = runtime_env(&config);
        assert_eq!(env.len(), 5);
        assert!(env.contains(&(ENV[3].to_owned(), Some("DE".to_owned()))));
        assert!(env.contains(&(ENV[4].to_owned(), Some("10".to_owned()))));
        assert!(env.iter().all(|(key, _)| key.starts_with("OER_W2_")));
    }
}

use oer_hil_lab::air::{Frame, acknowledged_probes};
use oer_ieee80211_port::Band;
use oer_ieee80211_upper_mac::rate_selection::management_rate;
use oer_ieee80211_wire::channel::{WifiChannel, WifiChannelWidth};
use oer_lab_router::mac::Mac;
use oer_radio_regulatory::{Country, Initiate, PowerLimits, RegulatoryDomain};
use std::collections::{BTreeMap, BTreeSet};

pub struct Checked {
    pub passed: bool,
    pub summary: Vec<String>,
}
type Fields<'a> = BTreeMap<&'a str, &'a str>;
fn fields<'a>(line: &'a str, name: &str, keys: &[&str]) -> Result<Fields<'a>, String> {
    let mut words = line.split_whitespace();
    if words.next() != Some(name) {
        return Err(format!("expected {name}"));
    }
    let mut fields = BTreeMap::new();
    for word in words {
        let (key, value) = word.split_once('=').ok_or("invalid report field")?;
        if !keys.contains(&key) || fields.insert(key, value).is_some() {
            return Err(format!("unknown or repeated field in {name}"));
        }
    }
    if fields.len() != keys.len() {
        return Err(format!("missing field in {name}"));
    }
    Ok(fields)
}
fn number(f: &Fields<'_>, key: &str, max: u64) -> Result<u64, String> {
    let n = f
        .get(key)
        .ok_or_else(|| format!("missing {key}"))?
        .parse::<u64>()
        .map_err(|_| format!("invalid {key}"))?;
    if n > max {
        return Err(format!("out-of-range {key}"));
    }
    Ok(n)
}
fn flag(f: &Fields<'_>, key: &str) -> Result<bool, String> {
    number(f, key, 1).map(|n| n == 1)
}
fn signed(f: &Fields<'_>, key: &str) -> Result<i32, String> {
    f.get(key)
        .ok_or_else(|| format!("missing {key}"))?
        .parse()
        .map_err(|_| format!("invalid {key}"))
}
fn powers(f: &Fields<'_>) -> Result<[i8; 4], String> {
    f["power"]
        .split(',')
        .map(|s| s.parse::<i8>().map_err(|_| "invalid power code".to_owned()))
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| "power readback must have four codes".to_owned())
}
fn hex(s: &str, max: usize) -> Result<Vec<u8>, String> {
    if s == "-" {
        return Ok(Vec::new());
    }
    if !s.is_ascii() || !s.len().is_multiple_of(2) || s.len() > max * 2 {
        return Err("invalid rate elements".to_owned());
    }
    hex::decode(s).map_err(|_| "invalid rate elements".to_owned())
}

/// Recompute the device gates from primitive report rows, then add independent
/// AX211 evidence to b/c. Raw frames and full identities are never printed.
pub fn check(
    console: &[String],
    result: &str,
    config: &w1::Config,
    target: Mac,
    air: &[Frame],
) -> Result<Checked, String> {
    let begin = console
        .iter()
        .position(|s| s.trim() == "OER-W2-BEGIN v=1")
        .ok_or("missing W2 block")?;
    let end = console
        .iter()
        .enumerate()
        .skip(begin + 1)
        .find(|(_, s)| s.starts_with("OER-W2-END "))
        .map(|(i, _)| i)
        .ok_or("unterminated W2 block")?;
    if end - begin > 64 {
        return Err("oversized W2 block".to_owned());
    }
    let body = &console[begin + 1..end];
    let ending = fields(&console[end], "OER-W2-END", &["lines"])?;
    if number(&ending, "lines", 64)? as usize != body.len() {
        return Err("W2 line count mismatch".to_owned());
    }
    let cfg = fields(
        body.first().ok_or("empty W2 block")?,
        "w2.config",
        &[
            "channel",
            "country",
            "cap",
            "antenna_mbm",
            "chip_mbm",
            "target",
            "stand",
        ],
    )?;
    if number(&cfg, "channel", 13)? != u64::from(config.channel)
        || cfg["country"] != w1::country_text(config.country)
        || signed(&cfg, "cap")? != i32::from(LAB_POWER_CAP_DBM)
        || cfg["target"] != target.masked()
        || cfg["stand"] != config.bssid.masked()
    {
        return Err("W2 build identity differs from the stand".to_owned());
    }
    let domain = RegulatoryDomain::of(Country::new(config.country).map_err(|_| "invalid country")?)
        .ok_or("unknown country")?;
    let limits = PowerLimits {
        antenna_gain_mbm: signed(&cfg, "antenna_mbm")?,
        chip_maximum_mbm: signed(&cfg, "chip_mbm")?,
        owner_cap_mbm: Some(i32::from(LAB_POWER_CAP_DBM) * 100),
    };
    if !(0..=10000).contains(&limits.antenna_gain_mbm)
        || !(0..=3200).contains(&limits.chip_maximum_mbm)
    {
        return Err("invalid board/chip power limits".to_owned());
    }
    let expected = (1..=13)
        .filter_map(|c| {
            domain
                .permit(WifiChannel::new_2_4_ghz(c, WifiChannelWidth::Mhz20).ok()?)
                .ok()
                .map(|p| (c, p))
        })
        .collect::<BTreeMap<_, _>>();
    let group_rate = management_rate(Band::Ghz2_4, None)
        .ok_or("no band rate")?
        .kbps();
    let mut channels = BTreeSet::new();
    let mut broadcast_sequences = BTreeSet::new();
    let mut lab_sequence = None;
    let (mut found, mut scan, mut power_ok) = (false, true, true);
    let mut unicast = None;
    let mut basic = None;
    let mut stats = None;
    let mut claims = BTreeMap::new();
    for line in body.iter().skip(1) {
        match line.split_whitespace().next() {
            Some("w2.channel") => {
                let f = fields(
                    line,
                    "w2.channel",
                    &[
                        "channel",
                        "sequence",
                        "allowed",
                        "sent",
                        "success",
                        "responses",
                        "rate_kbps",
                        "retune_us",
                        "power",
                        "ceiling",
                    ],
                )?;
                let channel = number(&f, "channel", 13)? as u8;
                if !channels.insert(channel) {
                    return Err("duplicate scan channel".to_owned());
                }
                let permitted = expected.get(&channel).ok_or("unpermitted scan channel")?;
                let allowed = permitted.initiate() == Initiate::Allowed;
                if flag(&f, "allowed")? != allowed {
                    return Err("wrong NO_IR classification".to_owned());
                }
                let sent = number(&f, "sent", 1)?;
                let sequence = number(&f, "sequence", 4095)? as u16;
                if sent > 0 {
                    if !broadcast_sequences.insert(sequence) {
                        return Err("duplicate broadcast sequence".to_owned());
                    }
                    if channel == config.channel {
                        lab_sequence = Some(sequence);
                    }
                }

                scan &= if allowed {
                    sent == 1 && flag(&f, "success")?
                } else {
                    sent == 0
                };
                scan &= number(&f, "rate_kbps", 54000)? == u64::from(group_rate);
                number(&f, "retune_us", 60_000_000)?;
                let responses = number(&f, "responses", u32::MAX as u64)?;
                found |= channel == config.channel && responses > 0;
                let limit = permitted.tx_ceiling(limits).dbm();
                if signed(&f, "ceiling")? != i32::from(limit) {
                    return Err(
                        "channel power ceiling differs from the domain/board/chip/cap minimum"
                            .to_owned(),
                    );
                }
                if sent > 0 {
                    power_ok &= powers(&f)?
                        .into_iter()
                        .all(|p| p <= limit && p <= LAB_POWER_CAP_DBM);
                }
            }
            Some("w2.basic") => {
                if basic.is_some() {
                    return Err("duplicate basic rates".to_owned());
                }
                let f = fields(line, "w2.basic", &["supported", "extended"])?;
                basic = Some(BasicRates::from_rate_elements(
                    &hex(f["supported"], 8)?,
                    &hex(f["extended"], 16)?,
                ));
            }
            Some("w2.unicast") => {
                if unicast.is_some() {
                    return Err("duplicate unicast row".to_owned());
                }
                unicast = Some(fields(
                    line,
                    "w2.unicast",
                    &[
                        "rate_kbps",
                        "sequence",
                        "success",
                        "responses",
                        "attempts",
                        "raw",
                        "power",
                        "ceiling",
                    ],
                )?);
            }
            Some("w2.stats") => {
                if stats.is_some() {
                    return Err("duplicate stats".to_owned());
                }
                stats = Some(fields(
                    line,
                    "w2.stats",
                    &[
                        "timeouts",
                        "collisions",
                        "colors",
                        "errors",
                        "irq_free",
                        "error",
                    ],
                )?);
            }
            Some("w2.gate") => {
                let f = fields(line, "w2.gate", &["name", "pass"])?;
                if !["a", "b", "c", "d", "e"].contains(&f["name"])
                    || claims.insert(f["name"], flag(&f, "pass")?).is_some()
                {
                    return Err("invalid/repeated gate".to_owned());
                }
            }
            _ => return Err("unknown W2 row".to_owned()),
        }
    }
    let unicast = unicast.ok_or("missing unicast")?;
    let stats = stats.ok_or("missing stats")?;
    let basic = basic.ok_or("missing basic rates")?;
    let no_error = stats["error"] == "-";
    scan &= no_error && channels.len() == expected.len();
    let success = flag(&unicast, "success")?;
    let attempts = number(&unicast, "attempts", 7)?;
    let responses = number(&unicast, "responses", u32::MAX as u64)?;
    let seq = number(&unicast, "sequence", 4095)? as u16;
    let selected_rate = management_rate(Band::Ghz2_4, Some(&basic)).map(|r| r.kbps());
    let rate = number(&unicast, "rate_kbps", 54000)? as u32;
    let unicast_ok = success && responses > 0 && attempts > 0 && selected_rate == Some(rate);
    // SOURCE: chips/esp32s31/ieee80211/mac/src/tx/completion.rs::decode;
    // ordinary status zero is the only successful completion dispatch.
    let raw = number(&unicast, "raw", 255)?;
    if success && raw != 0 {
        return Err("unicast success contradicts its raw completion".to_owned());
    }
    let limit = expected
        .get(&config.channel)
        .ok_or("lab channel not permitted")?
        .tx_ceiling(limits)
        .dbm();
    if success && signed(&unicast, "ceiling")? != i32::from(limit) {
        return Err("unicast ceiling differs from the minimum".to_owned());
    }
    power_ok &= success
        && powers(&unicast)?
            .into_iter()
            .all(|p| p <= limit && p <= LAB_POWER_CAP_DBM);
    let quiet = number(&stats, "irq_free", 65536)? >= 4096
        && no_error
        && ["timeouts", "collisions", "colors", "errors"]
            .into_iter()
            .map(|key| number(&stats, key, u32::MAX as u64))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .all(|n| n == 0);
    let device = [found, scan, unicast_ok, quiet, power_ok];
    if claims.len() != 5
        || ["a", "b", "c", "d", "e"]
            .into_iter()
            .zip(device)
            .any(|(name, computed)| claims.get(name) != Some(&computed))
    {
        return Err("image gates disagree with recomputed rows".to_owned());
    }
    let image_pass = device.into_iter().all(|p| p) && no_error;
    let result = result.trim();
    if result
        != format!(
            "OER-RESULT: {} w2",
            if image_pass { "pass" } else { "fail" }
        )
    {
        return Err("image verdict disagrees with its rows".to_owned());
    }
    let target = target.octets();
    let ap = config.bssid.octets();
    let lab_frequency = 2407 + u16::from(config.channel) * 5;
    let probes = air
        .iter()
        .filter(|f| f.subtype == 4 && f.source == Some(target))
        .collect::<Vec<_>>();
    let broadcast = probes
        .iter()
        .filter(|f| {
            f.receiver == [255; 6] && f.sequence == lab_sequence && f.frequency_mhz == lab_frequency
        })
        .collect::<Vec<_>>();
    let mut observed_broadcasts = BTreeSet::new();
    let air_b = broadcast.len() == 1
        && broadcast
            .iter()
            .all(|f| !f.retry && f.rate_mbps == Some(group_rate as f32 / 1000.0))
        && probes.iter().all(|f| {
            (f.receiver == [255; 6]
                && f.bssid == Some([255; 6])
                && !f.retry
                && f.rate_mbps == Some(group_rate as f32 / 1000.0)
                && f.sequence.is_some_and(|seq| {
                    broadcast_sequences.contains(&seq) && observed_broadcasts.insert(seq)
                }))
                || (f.receiver == config.bssid.octets() && f.sequence == Some(seq))
        });
    let individual = probes
        .iter()
        .filter(|f| {
            f.receiver == ap
                && f.bssid == Some(ap)
                && f.sequence == Some(seq)
                && f.frequency_mhz == lab_frequency
        })
        .collect::<Vec<_>>();
    let ack = acknowledged_probes(air, target, ap);
    let air_responses = air
        .iter()
        .filter(|f| {
            f.subtype == 5
                && f.source == Some(ap)
                && f.receiver == target
                && f.bssid == Some(ap)
                && f.ssid == config.ssid.bytes()
                && f.frequency_mhz == lab_frequency
        })
        .count();
    let air_c = individual.len() as u64 == attempts
        && !individual.is_empty()
        && individual
            .iter()
            .enumerate()
            .all(|(i, f)| f.retry == (i > 0) && f.rate_mbps == Some(rate as f32 / 1000.0))
        && ack > 0
        && air_responses > 0;
    let host = [found, scan && air_b, unicast_ok && air_c, quiet, power_ok];
    let mut summary = ["a", "b", "c", "d", "e"]
        .into_iter()
        .zip(device)
        .zip(host)
        .map(|((name, device), host)| {
            format!(
                "gate {name}: image={} recomputed={} air={}",
                u8::from(device),
                u8::from(host),
                if name == "b" {
                    if air_b { "pass" } else { "fail" }
                } else if name == "c" {
                    if air_c { "pass" } else { "fail" }
                } else {
                    "-"
                }
            )
        })
        .collect::<Vec<_>>();
    summary.push(format!("AX211: {} target broadcast, {} unicast attempt(s), {ack} matching ACK(s), {air_responses} probe response(s); {} NO_IR channel(s)",broadcast.len(),individual.len(),expected.values().filter(|p|p.initiate()==Initiate::PassiveOnly).count()));
    summary.push(format!(
        "IRQ stack free: {} bytes; unicast derived rate: {rate} kb/s; power ceiling: {limit} dBm",
        stats["irq_free"]
    ));
    for probe in probes.iter().take(32) {
        summary.push(format!(
            "air probe sa={} seq={} rate_mbps={} retry={} to={}",
            cfg["target"],
            probe
                .sequence
                .map_or_else(|| "-".to_owned(), |s| s.to_string()),
            probe
                .rate_mbps
                .map_or_else(|| "-".to_owned(), |r| r.to_string()),
            u8::from(probe.retry),
            if probe.receiver == [255; 6] {
                "broadcast"
            } else {
                "lab"
            }
        ));
    }
    Ok(Checked {
        passed: host.into_iter().all(|g| g),
        summary,
    })
}

#[cfg(test)]
mod check_tests;

#[cfg(test)]
mod library_tests {
    use super::*;
    #[test]
    fn rate_elements_keep_the_dash_limit_and_error_text() {
        assert_eq!(hex("-", 0).unwrap(), Vec::<u8>::new());
        assert_eq!(hex("00aF", 2).unwrap(), [0, 175]);
        for (text, max) in [("00af", 1), ("0", 2), ("0g", 2), ("é", 2)] {
            assert_eq!(hex(text, max).unwrap_err(), "invalid rate elements");
        }
    }
}
