//! Independent router observations for the HIL station. Remote output is
//! filtered before it leaves the router, except numeric ping output parsed on
//! the host; identities stay masked in evidence.

use std::{net::Ipv4Addr, time::Duration};

use serde::{Deserialize, Serialize};

use crate::{
    mac::Mac,
    router::quote,
    ssh::{Remote, Result, Ssh},
};

pub mod link;
pub mod network;
pub mod stream;

pub const ECHO_COUNT: u32 = 100;
pub const INTERVAL_MS: u32 = 1000;
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Station {
    pub present: bool,
    pub authenticated: bool,
    pub associated: bool,
    pub authorized: bool,
}

impl Station {
    pub fn joined(self) -> bool {
        self.present && self.authenticated && self.associated && self.authorized
    }

    pub fn valid(self) -> bool {
        self.present || !(self.authenticated || self.associated || self.authorized)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Lease {
    pub address: Ipv4Addr,
    pub remaining_seconds: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Ping {
    pub sent: u32,
    pub received: u32,
    pub interval_ms: u32,
    /// Arrival order, including duplicates. Lost sequences have no entry.
    pub replies: Vec<PingReply>,
    pub rtt: Option<PingRtt>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PingReply {
    pub sequence: u16,
    pub rtt_us: u64,
    pub duplicate: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PingRtt {
    pub min_us: u64,
    pub avg_us: u64,
    pub max_us: u64,
    /// BusyBox reports min/avg/max without a deviation.
    pub mdev_us: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PingParseError {
    MissingPacketSummary,
    RepeatedPacketSummary,
    InvalidPacketSummary,
    ContradictoryCounts,
    InvalidReply,
    InvalidRoundTripTime,
    RepeatedRoundTripSummary,
    InvalidRoundTripSummary,
}
impl std::fmt::Display for PingParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::MissingPacketSummary => "missing ping packet summary",
            Self::RepeatedPacketSummary => "repeated ping packet summary",
            Self::InvalidPacketSummary => "invalid ping packet summary",
            Self::ContradictoryCounts => "contradictory ping counts",
            Self::InvalidReply => "invalid ping reply",
            Self::InvalidRoundTripTime => "invalid ping round-trip time",
            Self::RepeatedRoundTripSummary => "repeated ping round-trip summary",
            Self::InvalidRoundTripSummary => "invalid ping round-trip summary",
        })
    }
}
impl std::error::Error for PingParseError {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Echo {
    pub at_us: u64,
    pub source: Ipv4Addr,
    pub destination: Ipv4Addr,
    pub reply: bool,
    pub identifier: u16,
    pub sequence: u16,
}

pub fn usable_address(address: Ipv4Addr) -> bool {
    !address.is_unspecified()
        && !address.is_loopback()
        && !address.is_multicast()
        && !address.is_broadcast()
        && !address.is_link_local()
}

fn mac_text(mac: Mac) -> String {
    mac.octets().map(|b| format!("{b:02x}")).join(":")
}

/// Read only the selected target's station state, never another peer's MAC.
pub fn station(remote: &mut dyn Remote, interface: &str, target: Mac) -> Result<Station> {
    let script = format!(
        "set -e\ninfo=$(iw dev {} station dump)\nprintf '%s\\n' \"$info\" | \
         awk -v target={} '\n\
         $1 == \"Station\" {{ selected=(tolower($2)==target); if(selected) found++ }}\n\
         selected && $1 == \"authorized:\" {{ authorized=($2==\"yes\") }}\n\
         selected && $1 == \"authenticated:\" {{ authenticated=($2==\"yes\") }}\n\
         selected && $1 == \"associated:\" {{ associated=($2==\"yes\") }}\n\
         END {{ printf \"%d %d %d %d\\n\", found, authenticated, associated, authorized }}'",
        quote(interface)?,
        quote(&mac_text(target))?
    );
    parse_station(&remote.run(&script)?)
}

pub fn parse_station(text: &str) -> Result<Station> {
    let values = text.split_whitespace().collect::<Vec<_>>();
    let [present, authenticated, associated, authorized] = values.as_slice() else {
        return Err("invalid router station observation".to_owned());
    };
    let flag = |s: &str| match s {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err("invalid router station flag".to_owned()),
    };
    let state = Station {
        present: flag(present)?,
        authenticated: flag(authenticated)?,
        associated: flag(associated)?,
        authorized: flag(authorized)?,
    };
    if !state.present && (state.authenticated || state.associated || state.authorized) {
        return Err("absent router station has association flags".to_owned());
    }
    Ok(state)
}

/// dnsmasq's matching MAC only; hostnames and client identifiers stay remote.
/// Ambiguous leases refuse the observation.
pub fn lease(remote: &mut dyn Remote, target: Mac) -> Result<Option<Lease>> {
    let script = format!(
        "set -e\nnow=$(date +%s)\nawk -v target={} -v now=\"$now\" \
         'tolower($2)==target {{ if(NF!=5 || $1 !~ /^[0-9]+$/) {{print \"invalid\"; next}}; \
         if($1==0) print $3, 4294967295; else print $3, ($1>now ? $1-now : 0) }}' {}",
        quote(&mac_text(target))?,
        quote(network::LEASE_FILE)?
    );
    parse_lease(&remote.run(&script)?)
}

pub fn parse_lease(text: &str) -> Result<Option<Lease>> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let values = text.split_whitespace().collect::<Vec<_>>();
    let [address, remaining] = values.as_slice() else {
        return Err("ambiguous or invalid router DHCP lease".to_owned());
    };
    let address = address
        .parse()
        .map_err(|_| "invalid router lease address")?;
    if !usable_address(address) {
        return Err("unusable router lease address".to_owned());
    }
    let remaining_seconds = remaining
        .parse()
        .map_err(|_| "invalid router lease lifetime")?;
    Ok(Some(Lease {
        address,
        remaining_seconds,
    }))
}

pub fn ping(ssh: &Ssh, address: Ipv4Addr, count: u16) -> Result<Ping> {
    if !matches!(count, 10 | 100) {
        return Err("invalid station echo count".to_owned());
    }
    if !usable_address(address) {
        return Err("unusable ping address".to_owned());
    }
    let script = format!("LC_ALL=C ping -n -c {count} -i 1 -W 1 {address}");
    let output = ssh.output(&script, Duration::from_secs(u64::from(count) + 15))?;
    // Native ping may exit 1 on packet loss; its counts still feed the gates.
    if !matches!(output.status.code(), Some(0 | 1)) {
        return Err("router ping command failed".to_owned());
    }
    parse_ping(std::str::from_utf8(&output.stdout).map_err(|_| "invalid router ping encoding")?)
        .map_err(|error| error.to_string())
}

/// Parse native iputils or BusyBox output without retaining headers or addresses.
/// A missing RTT summary stays absent; it is never reconstructed from replies.
pub fn parse_ping(text: &str) -> std::result::Result<Ping, PingParseError> {
    use PingParseError as Error;
    let mut counts = None;
    let mut replies = Vec::new();
    let mut rtt = None;
    for line in text.lines().map(str::trim) {
        if line.contains("packets transmitted") {
            if counts.is_some() {
                return Err(Error::RepeatedPacketSummary);
            }
            let mut fields = line.split(',');
            let sent = fields.next().ok_or(Error::InvalidPacketSummary)?;
            let received = fields.next().ok_or(Error::InvalidPacketSummary)?;
            let sent = sent.split_whitespace().collect::<Vec<_>>();
            let [sent, "packets", "transmitted"] = sent.as_slice() else {
                return Err(Error::InvalidPacketSummary);
            };
            let sent = sent
                .parse::<u32>()
                .map_err(|_| Error::InvalidPacketSummary)?;
            let received = received.split_whitespace().collect::<Vec<_>>();
            let received = match received.as_slice() {
                [received, "received"] | [received, "packets", "received"] => received
                    .parse::<u32>()
                    .map_err(|_| Error::InvalidPacketSummary)?,
                _ => return Err(Error::InvalidPacketSummary),
            };
            counts = Some((sent, received));
        } else if line.contains(" bytes from ") {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            let sequence = fields
                .iter()
                .find_map(|field| {
                    field
                        .strip_prefix("icmp_seq=")
                        .or_else(|| field.strip_prefix("seq="))
                })
                .ok_or(Error::InvalidReply)?
                .parse()
                .map_err(|_| Error::InvalidReply)?;
            let mut fields = fields.iter();
            let time = fields
                .find_map(|field| field.strip_prefix("time="))
                .ok_or(Error::InvalidReply)?;
            if fields.next().copied() != Some("ms") {
                return Err(Error::InvalidReply);
            }
            replies.push(PingReply {
                sequence,
                rtt_us: ping_time_us(time)?,
                duplicate: line.contains("DUP!"),
            });
        } else if line.starts_with("rtt ") || line.starts_with("round-trip ") {
            if rtt.is_some() {
                return Err(Error::RepeatedRoundTripSummary);
            }
            let (names, values) = line
                .split_once(" = ")
                .ok_or(Error::InvalidRoundTripSummary)?;
            let values = values
                .strip_suffix(" ms")
                .ok_or(Error::InvalidRoundTripSummary)?;
            let values = values.split('/').collect::<Vec<_>>();
            let (min, avg, max, mdev) = match (names, values.as_slice()) {
                ("rtt min/avg/max/mdev", [min, avg, max, mdev]) => {
                    (*min, *avg, *max, Some(ping_time_us(mdev)?))
                }
                ("round-trip min/avg/max", [min, avg, max]) => (*min, *avg, *max, None),
                _ => return Err(Error::InvalidRoundTripSummary),
            };
            rtt = Some(PingRtt {
                min_us: ping_time_us(min)?,
                avg_us: ping_time_us(avg)?,
                max_us: ping_time_us(max)?,
                mdev_us: mdev,
            });
        }
    }
    let (sent, received) = counts.ok_or(Error::MissingPacketSummary)?;
    if sent > ECHO_COUNT || received > sent {
        return Err(Error::ContradictoryCounts);
    }
    Ok(Ping {
        sent,
        received,
        interval_ms: INTERVAL_MS,
        replies,
        rtt,
    })
}

/// Decimal milliseconds to exact integer microseconds, without float rounding.
fn ping_time_us(text: &str) -> std::result::Result<u64, PingParseError> {
    let invalid = PingParseError::InvalidRoundTripTime;
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(invalid);
    }
    let fraction = fraction.trim_end_matches('0');
    let scale = match fraction.len() {
        0 => 0,
        1 => 100,
        2 => 10,
        3 => 1,
        _ => return Err(invalid),
    };
    let fractional_us = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<u64>()
            .ok()
            .and_then(|value| value.checked_mul(scale))
            .ok_or(invalid)?
    };
    whole
        .parse::<u64>()
        .ok()
        .and_then(|value| value.checked_mul(1000))
        .and_then(|value| value.checked_add(fractional_us))
        .ok_or(invalid)
}

/// Decode only numeric IPv4 ICMP echo rows. No raw packet or identifiers are
/// retained. A truncated, foreign or unsupported line refuses the observation.
pub fn parse_echoes(text: &str) -> Result<Vec<Echo>> {
    if text.len() > 128 * 1024 {
        return Err("oversized ICMP observation".to_owned());
    }
    let mut echoes = Vec::new();
    for line in text.lines() {
        let p = line.split_whitespace().collect::<Vec<_>>();
        let [
            at,
            "IP",
            source,
            ">",
            destination,
            "ICMP",
            "echo",
            kind,
            "id",
            id,
            "seq",
            sequence,
            "length",
            length,
        ] = p.as_slice()
        else {
            return Err("invalid ICMP observation row".to_owned());
        };
        let reply = match *kind {
            "request," => false,
            "reply," => true,
            _ => return Err("non-echo ICMP observation".to_owned()),
        };
        let (seconds, fraction) = at.split_once('.').ok_or("invalid ICMP timestamp")?;
        if fraction.len() != 6 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
            return Err("invalid ICMP timestamp precision".to_owned());
        }
        let at_us = seconds
            .parse::<u64>()
            .ok()
            .and_then(|s| s.checked_mul(1_000_000))
            .and_then(|s| s.checked_add(fraction.parse().ok()?))
            .ok_or("invalid ICMP timestamp")?;
        let parse_u16 = |s: &str| {
            s.strip_suffix(',')
                .ok_or("invalid ICMP delimiter")?
                .parse::<u16>()
                .map_err(|_| "invalid ICMP number")
        };
        if length.parse::<u16>().map_err(|_| "invalid ICMP length")? < 8 {
            return Err("short ICMP echo".to_owned());
        }
        echoes.push(Echo {
            at_us,
            reply,
            source: source.parse().map_err(|_| "invalid ICMP source")?,
            destination: destination
                .strip_suffix(':')
                .ok_or("invalid ICMP destination")?
                .parse()
                .map_err(|_| "invalid ICMP destination")?,
            identifier: parse_u16(id)?,
            sequence: parse_u16(sequence)?,
        });
        if echoes.len() > 400 {
            return Err("too many ICMP observations".to_owned());
        }
    }
    Ok(echoes)
}

#[cfg(test)]
mod tests;
