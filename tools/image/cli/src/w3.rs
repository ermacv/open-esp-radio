//! Strict W3 console contract and host recomputation. Claims never establish
//! their own truth: the host requires independent router evidence.

use std::{collections::BTreeMap, io::Read as _, net::Ipv4Addr};

use oer_lab_router::w3::{Evidence, Ping, echo_gate, usable_address};

#[cfg(target_os = "linux")]
pub mod ax211;
#[cfg(target_os = "linux")]
pub mod firmware;
mod loss;

type Result<T> = std::result::Result<T, String>;
type Fields<'a> = BTreeMap<&'a str, &'a str>;

fn value<'a>(fields: &Fields<'a>, key: &str) -> Result<&'a str> {
    fields
        .get(key)
        .copied()
        .ok_or_else(|| format!("missing {key}"))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Report {
    pub channel: u8,
    pub target: String,
    pub bssid: String,
    pub authenticated: bool,
    pub associated: bool,
    pub acquired: bool,
    pub address: Ipv4Addr,
    pub router: Ipv4Addr,
    pub prefix: u8,
    pub ping: Ping,
    pub peer_requests: u32,
    pub peer_replies: u32,
    pub tx_timeout: u32,
    pub queue_collision: u32,
    pub irq_free: u32,
    pub deauth_sent: bool,
    pub claims: [bool; 5],
    pub result: bool,
}

fn fields<'a>(line: &'a str, name: &str, keys: &[&str]) -> Result<Fields<'a>> {
    if line.len() > 512 || !line.is_ascii() {
        return Err("oversized or non-ASCII W3 row".to_owned());
    }
    let mut words = line.split_whitespace();
    if words.next() != Some(name) {
        return Err(format!("expected {name}"));
    }
    let mut result = BTreeMap::new();
    for word in words {
        let (key, value) = word.split_once('=').ok_or("invalid W3 field")?;
        if value.is_empty() || !keys.contains(&key) || result.insert(key, value).is_some() {
            return Err(format!("unknown, empty or repeated field in {name}"));
        }
    }
    if result.len() != keys.len() {
        return Err(format!("missing field in {name}"));
    }
    Ok(result)
}

fn number(f: &Fields<'_>, key: &str, max: u32) -> Result<u32> {
    let text = f.get(key).ok_or("missing W3 field")?;
    if !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("invalid {key}"));
    }
    text.parse::<u32>()
        .ok()
        .filter(|n| *n <= max)
        .ok_or_else(|| format!("out-of-range {key}"))
}
fn flag(f: &Fields<'_>, key: &str) -> Result<bool> {
    number(f, key, 1).map(|n| n == 1)
}
fn masked(text: &str) -> bool {
    text.strip_prefix("xx:xx:xx:xx:").is_some_and(|tail| {
        tail.len() == 5
            && tail.as_bytes().get(2) == Some(&b':')
            && tail
                .bytes()
                .enumerate()
                .all(|(i, b)| i == 2 || b.is_ascii_hexdigit() && !b.is_ascii_lowercase())
    })
}
fn address(f: &Fields<'_>, key: &str) -> Result<Ipv4Addr> {
    f.get(key)
        .ok_or("missing W3 address")?
        .parse()
        .map_err(|_| "invalid W3 IPv4 address".to_owned())
}

/// One complete block, exactly twelve rows, followed by the exact result
/// marker. Diagnostic lines before the block are ignored; protocol garbage,
/// repeated blocks, trailing rows and truncated blocks are refused.
pub fn parse(console: &str) -> Result<Report> {
    if console.len() > 256 * 1024 {
        return Err("oversized W3 console".to_owned());
    }
    let lines = console.lines().collect::<Vec<_>>();
    let begin = lines
        .iter()
        .position(|line| line.starts_with("OER-W3-"))
        .ok_or("missing W3 block")?;
    let tail = lines.get(begin..).ok_or("missing W3 block")?;
    let [
        "OER-W3-BEGIN v=1",
        config,
        association,
        lease,
        ping,
        peer,
        stats,
        departure,
        a,
        b,
        c,
        d,
        f,
        "OER-W3-END lines=12",
        result,
    ] = tail
    else {
        return Err("malformed, truncated or repeated W3 block".to_owned());
    };
    let cfg = fields(config, "w3.config", &["channel", "target", "stand"])?;
    if !masked(value(&cfg, "target")?) || !masked(value(&cfg, "stand")?) {
        return Err("W3 identities must be masked".to_owned());
    }
    let channel = number(&cfg, "channel", 13)? as u8;
    if channel == 0 {
        return Err("invalid W3 channel".to_owned());
    }
    let assoc = fields(
        association,
        "w3.association",
        &["authenticated", "associated"],
    )?;
    let lease = fields(
        lease,
        "w3.lease",
        &["acquired", "address", "router", "prefix"],
    )?;
    let ping = fields(ping, "w3.ping", &["sent", "received", "interval_ms"])?;
    let peer = fields(peer, "w3.peer_ping", &["requests", "replies"])?;
    let stats = fields(
        stats,
        "w3.stats",
        &["tx_timeout", "queue_collision", "irq_free"],
    )?;
    let departure = fields(departure, "w3.departure", &["deauth_sent"])?;
    let mut claims = [false; 5];
    for ((line, id), claim) in [a, b, c, d, f]
        .into_iter()
        .zip(["a", "b", "c", "d", "f"])
        .zip(&mut claims)
    {
        let gate = fields(line, "w3.gate", &["id", "pass"])?;
        if value(&gate, "id")? != id {
            return Err("W3 gates missing, repeated or reordered".to_owned());
        }
        *claim = flag(&gate, "pass")?;
    }
    let result = match *result {
        "OER-RESULT: pass w3" => true,
        "OER-RESULT: fail w3" => false,
        _ => return Err("invalid W3 result marker".to_owned()),
    };
    let report = Report {
        channel,
        target: value(&cfg, "target")?.to_owned(),
        bssid: value(&cfg, "stand")?.to_owned(),
        authenticated: flag(&assoc, "authenticated")?,
        associated: flag(&assoc, "associated")?,
        acquired: flag(&lease, "acquired")?,
        address: address(&lease, "address")?,
        router: address(&lease, "router")?,
        prefix: number(&lease, "prefix", 32)? as u8,
        ping: Ping {
            sent: number(&ping, "sent", 100)?,
            received: number(&ping, "received", 100)?,
            interval_ms: number(&ping, "interval_ms", u32::MAX)?,
        },
        peer_requests: number(&peer, "requests", 100)?,
        peer_replies: number(&peer, "replies", 100)?,
        tx_timeout: number(&stats, "tx_timeout", u32::MAX)?,
        queue_collision: number(&stats, "queue_collision", u32::MAX)?,
        irq_free: number(&stats, "irq_free", u32::MAX)?,
        deauth_sent: flag(&departure, "deauth_sent")?,
        claims,
        result,
    };
    if report.ping.received > report.ping.sent || report.peer_replies > report.peer_requests {
        return Err("contradictory W3 ping counts".to_owned());
    }
    if report.acquired
        && (!usable_address(report.address) || !usable_address(report.router) || report.prefix == 0)
    {
        return Err("invalid acquired W3 lease".to_owned());
    }
    if !report.acquired
        && (report.address != Ipv4Addr::UNSPECIFIED
            || report.router != Ipv4Addr::UNSPECIFIED
            || report.prefix != 0)
    {
        return Err("unacquired W3 lease contains addresses".to_owned());
    }
    Ok(report)
}

#[derive(Debug, Eq, PartialEq)]
pub struct Checked {
    pub gates: [bool; 5],
    pub irq_free: u32,
}
impl Checked {
    pub fn passed(&self) -> bool {
        self.gates.into_iter().all(|p| p)
    }
}

pub fn check(report: &Report, evidence: &Evidence) -> Result<Checked> {
    if evidence.schema != 1
        || ![evidence.before, evidence.joined, evidence.after]
            .into_iter()
            .all(|s| s.valid())
        || evidence.router_ping.received > evidence.router_ping.sent
        || !masked(&evidence.target)
        || !masked(&evidence.bssid)
        || report.target != evidence.target
        || report.bssid != evidence.bssid
        || report.channel != evidence.channel
        || !evidence.open
        || !evidence.ht20
        || !evidence.he_disabled
        || !usable_address(evidence.router_address)
        || evidence.echoes.len() > 400
    {
        return Err("W3 router evidence identity/profile mismatch".to_owned());
    }
    if evidence.before.present {
        return Err("W3 station was already present before this run".to_owned());
    }
    let lease_ok = evidence.lease.is_some_and(|lease| {
        report.acquired
            && usable_address(lease.address)
            && lease.remaining_seconds > 0
            && lease.address == report.address
            && report.router == evidence.router_address
            && report.address != report.router
    });
    let endpoints = evidence
        .lease
        .map(|l| l.address)
        .unwrap_or(Ipv4Addr::UNSPECIFIED);
    if evidence.echoes.iter().any(|e| {
        !((e.source == endpoints && e.destination == evidence.router_address)
            || (e.destination == endpoints && e.source == evidence.router_address))
    }) {
        return Err("foreign endpoints in W3 ICMP evidence".to_owned());
    }
    let gates = [
        report.authenticated && report.associated && evidence.joined.joined(),
        lease_ok,
        lease_ok
            && report.ping.passed()
            && evidence.capture_complete
            && echo_gate(&evidence.echoes, endpoints, evidence.router_address),
        lease_ok
            && report.peer_requests == 100
            && report.peer_replies == 100
            && evidence.router_ping.passed()
            && evidence.capture_complete
            && echo_gate(&evidence.echoes, evidence.router_address, endpoints),
        report.tx_timeout == 0
            && report.queue_collision == 0
            && report.deauth_sent
            && evidence.disconnect_event
            && !evidence.after.present
            && !evidence.after.authenticated
            && !evidence.after.associated
            && !evidence.after.authorized,
    ];
    if report.claims != gates || report.result != gates.into_iter().all(|p| p) {
        return Err("W3 firmware claims contradict host recomputation".to_owned());
    }
    Ok(Checked {
        gates,
        irq_free: report.irq_free,
    })
}

pub fn check_files(
    console: &std::path::Path,
    observations: &std::path::Path,
) -> oer_image_build::Result<std::process::ExitCode> {
    let result = (|| -> oer_image_build::Result<Checked> {
        let console = read_bounded(std::fs::File::open(console)?)?;
        let observations = read_bounded(std::fs::File::open(observations)?)?;
        let report = parse(&console)?;
        let evidence: Evidence =
            serde_json::from_str(&observations).map_err(|_| "invalid W3 router JSON")?;
        Ok(check(&report, &evidence)?)
    })();
    match result {
        Ok(checked) => {
            for (id, passed) in ["a", "b", "c", "d", "f"].into_iter().zip(checked.gates) {
                println!("fw: w3 gate {id}: {}", if passed { "pass" } else { "fail" });
            }
            println!("fw: w3 free IRQ stack: {} bytes", checked.irq_free);
            Ok(std::process::ExitCode::from(if checked.passed() {
                0
            } else {
                1
            }))
        }
        Err(_) => {
            eprintln!("fw: w3 malformed report/evidence or contradiction");
            Ok(std::process::ExitCode::from(3))
        }
    }
}

fn read_bounded(reader: impl std::io::Read) -> Result<String> {
    let mut bytes = Vec::new();
    reader
        .take(256 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read W3 input")?;
    if bytes.len() > 256 * 1024 {
        return Err("oversized W3 input".to_owned());
    }
    String::from_utf8(bytes).map_err(|_| "W3 input is not UTF-8".to_owned())
}

#[cfg(test)]
mod tests;

/// The firmware image composed for this stand.
pub const IMAGE: &str = "w3";
pub const ENV: [&str; 5] = [
    "OER_W3_CHANNEL",
    "OER_W3_BSSID",
    "OER_W3_SSID_HEX",
    "OER_W3_COUNTRY",
    "OER_W3_POWER_CAP_DBM",
];
pub fn runtime_env(config: &crate::w1::Config) -> Vec<(String, Option<String>)> {
    let values = [
        config.channel.to_string(),
        config
            .bssid
            .octets()
            .iter()
            .map(|octet| format!("{octet:02X}"))
            .collect::<Vec<_>>()
            .join(":"),
        hex::encode(config.ssid.bytes()),
        crate::w1::country_text(config.country),
        crate::w2::LAB_POWER_CAP_DBM.to_string(),
    ];
    ENV.into_iter()
        .zip(values)
        .map(|(name, value)| (name.to_owned(), Some(value)))
        .collect()
}
