//! Independent router observations for the open W3 station. Remote output is
//! filtered before it leaves the router; identities stay masked in evidence.

use std::{collections::BTreeSet, net::Ipv4Addr, time::Duration};

use serde::{Deserialize, Serialize};

use crate::{
    mac::Mac,
    router::quote,
    ssh::{Remote, Result, Ssh},
};

pub mod network;
pub mod stream;

pub const ECHO_COUNT: u32 = 100;
pub const INTERVAL_MS: u32 = 1000;
/// Allowed jitter of each adjacent one-second echo interval on the router's
/// capture clock. Sender drift does not impose an absolute phase requirement.
pub const CADENCE_TOLERANCE_US: u64 = 250_000;

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

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Ping {
    pub sent: u32,
    pub received: u32,
    pub interval_ms: u32,
}

impl Ping {
    pub fn passed(self) -> bool {
        self.sent == ECHO_COUNT && self.received == ECHO_COUNT && self.interval_ms == INTERVAL_MS
    }
}

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

/// One run's observations. No raw console, capture, interface, SSID or MAC.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub schema: u32,
    pub target: String,
    pub bssid: String,
    pub channel: u8,
    pub open: bool,
    pub ht20: bool,
    pub he_disabled: bool,
    pub before: Station,
    pub joined: Station,
    pub lease: Option<Lease>,
    pub router_address: Ipv4Addr,
    pub router_ping: Ping,
    pub echoes: Vec<Echo>,
    pub capture_complete: bool,
    pub disconnect_event: bool,
    pub after: Station,
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
/// Expired and ambiguous leases cannot establish gate b.
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

pub fn router_address(remote: &mut dyn Remote, interface: &str) -> Result<Ipv4Addr> {
    let text = remote.run(&format!(
        "set -e\ninfo=$(ip -4 address show dev {} scope global)\nprintf '%s\\n' \"$info\" | awk '$1==\"inet\" {{split($2,a,\"/\"); print a[1]}}'", quote(interface)?
    ))?;
    let address = text
        .trim()
        .parse()
        .map_err(|_| "router ingress must have one IPv4 address")?;
    if !usable_address(address) {
        return Err("unusable router IPv4 address".to_owned());
    }
    Ok(address)
}

/// Runtime hostapd configuration, with all keys and identities filtered out.
pub fn verify_open_ht20(remote: &mut dyn Remote, radio: &str, interface: &str) -> Result<()> {
    // OpenWrt's per-radio hostapd configuration has a section per BSS.
    let script = format!(
        "set -e\nphy=$(ubus call iwinfo phyname {} | jsonfilter -e '@.phyname')\n\
         case \"$phy\" in phy[0-9]*) ;; *) exit 1;; esac\n\
         awk -F= -v iface={} '\n\
         $1==\"interface\" || $1==\"bss\" {{selected=($2==iface); if(selected) found++}}\n\
         $1==\"ieee80211n\" {{n=$2}}\n\
         $1==\"ieee80211ax\" {{ax=$2}}\n\
         $1==\"ht_capab\" {{wide=($2 ~ /HT40/)}}\n\
         selected && $1==\"wpa\" {{wpa=$2}}\n\
         selected && $1 ~ /^wep_(default_key|key[0-3])$/ {{wep=1}}\n\
         END {{printf \"%d %d %d %d %d %d\\n\", found, n, ax, wide, wpa, wep}}' \"/var/run/hostapd-$phy.conf\"",
        quote(&format!("{{\"section\":\"{radio}\"}}"))?,
        quote(interface)?,
    );
    if remote.run(&script)?.trim() != "1 1 0 0 0 0" {
        return Err("running hostapd does not confirm open HT20 with HE disabled".to_owned());
    }
    Ok(())
}

pub fn ping(ssh: &Ssh, address: Ipv4Addr, count: u16) -> Result<Ping> {
    if !matches!(count, 10 | 100) {
        return Err("invalid station echo count".to_owned());
    }
    if !usable_address(address) {
        return Err("unusable ping address".to_owned());
    }
    let script = format!(
        "LC_ALL=C ping -n -c {count} -i 1 -W 1 {address} | \
         awk '/packets transmitted/ {{ print $1, $4 }}'"
    );
    let output = ssh.output(&script, Duration::from_secs(u64::from(count) + 15))?;
    if !output.status.success() {
        return Err("router ping command failed".to_owned());
    }
    parse_ping(std::str::from_utf8(&output.stdout).map_err(|_| "invalid router ping encoding")?)
}

pub fn parse_ping(text: &str) -> Result<Ping> {
    let parts = text.split_whitespace().collect::<Vec<_>>();
    let [sent, received] = parts.as_slice() else {
        return Err("missing or repeated ping summary".to_owned());
    };
    let sent = sent.parse::<u32>().map_err(|_| "invalid ping sent count")?;
    let received = received
        .parse::<u32>()
        .map_err(|_| "invalid ping received count")?;
    if sent > ECHO_COUNT || received > sent {
        return Err("contradictory ping counts".to_owned());
    }
    Ok(Ping {
        sent,
        received,
        interval_ms: INTERVAL_MS,
    })
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

/// Exactly 100 distinct, paired sequences under one echo identifier. Foreign
/// addresses, duplicate requests/replies and reversed timestamps fail closed.
pub fn echo_gate(echoes: &[Echo], source: Ipv4Addr, destination: Ipv4Addr) -> bool {
    let requests = echoes
        .iter()
        .filter(|e| !e.reply && e.source == source && e.destination == destination)
        .collect::<Vec<_>>();
    let replies = echoes
        .iter()
        .filter(|e| e.reply && e.source == destination && e.destination == source)
        .collect::<Vec<_>>();
    if requests.len() != ECHO_COUNT as usize || replies.len() != ECHO_COUNT as usize {
        return false;
    }
    if requests.windows(2).any(|pair| {
        let [previous, next] = pair else {
            return true;
        };
        next.at_us
            .checked_sub(previous.at_us)
            .is_none_or(|interval| interval.abs_diff(1_000_000) > CADENCE_TOLERANCE_US)
    }) {
        return false;
    }
    let ids = requests
        .iter()
        .map(|e| e.identifier)
        .collect::<BTreeSet<_>>();
    let sequences = requests.iter().map(|e| e.sequence).collect::<BTreeSet<_>>();
    if ids.len() != 1 || sequences.len() != ECHO_COUNT as usize {
        return false;
    }
    let reply_keys = replies
        .iter()
        .map(|e| (e.identifier, e.sequence))
        .collect::<BTreeSet<_>>();
    reply_keys.len() == ECHO_COUNT as usize
        && requests.iter().all(|q| {
            replies.iter().any(|r| {
                r.identifier == q.identifier && r.sequence == q.sequence && r.at_us >= q.at_us
            })
        })
}

#[cfg(test)]
mod tests;
