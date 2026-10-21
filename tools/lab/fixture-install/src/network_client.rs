//! Typed, secret-safe station profiles and DHCP/ping observations for linux-net.
use serde::{Deserialize, Serialize};
use std::{fmt, net::Ipv4Addr};

pub const CONTRACT: &str = "schema=16 station_ap=ht20,ht40,he20 legacy_ap=dsss client=ht,non-ht client_security=open,wpa2 client_ipv4=none,static,dhcp client_session=status,ping,disconnect client_ping=100x1s client_aqm=1 observer=20,40 managed=1 rfkill=restore";

type Result<T> = std::result::Result<T, &'static str>;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Security {
    Open,
    Wpa2,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phy {
    Ht,
    NonHt,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Ipv4 {
    None {},
    Static { address: String },
    Dhcp {},
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub ssid_hex: String,
    pub security: Security,
    #[serde(default)]
    pub passphrase: Option<String>,
    pub frequency_mhz: u16,
    pub phy: Phy,
    pub ipv4: Ipv4,
}
impl fmt::Debug for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Profile")
            .field("ssid_bytes", &(self.ssid_hex.len() / 2))
            .field("security", &self.security)
            .field("frequency_mhz", &self.frequency_mhz)
            .field("phy", &self.phy)
            .field("ipv4", &self.ipv4)
            .finish_non_exhaustive()
    }
}
impl Profile {
    pub fn parse(input: &str) -> Result<Self> {
        if input.len() > 2048 {
            return Err("oversized station profile");
        }
        let value: Self =
            serde_json::from_str(input).map_err(|_| "invalid station JSON profile")?;
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        if self.ssid_hex.is_empty()
            || self.ssid_hex.len() > 64
            || !self.ssid_hex.len().is_multiple_of(2)
            || !self.ssid_hex.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("invalid station SSID encoding");
        }
        if !(2412..=2472).contains(&self.frequency_mhz)
            || !(self.frequency_mhz - 2412).is_multiple_of(5)
        {
            return Err("invalid station frequency");
        }
        match (self.security, self.passphrase.as_deref()) {
            (Security::Open, None) => {}
            (Security::Wpa2, Some(p))
                if (8..=63).contains(&p.len())
                    && p.bytes().all(|b| b.is_ascii_graphic() || b == b' ') => {}
            _ => return Err("station security and credential disagree"),
        }
        if let Ipv4::Static { address } = &self.ipv4 {
            parse_cidr(address)?;
        }
        Ok(())
    }
    /// Only the private root-owned configuration file receives this string.
    pub fn supplicant(&self) -> Result<String> {
        self.validate()?;
        let mut config = format!(
            "ctrl_interface=DIR=/run/open-radio-wpa-control GROUP=network\np2p_disabled=1\nmac_addr=0\npreassoc_mac_addr=0\nnetwork={{\n    disabled=1\n    ssid={}\n    scan_freq={}\n",
            self.ssid_hex, self.frequency_mhz
        );
        match self.security {
            Security::Open => config.push_str("    key_mgmt=NONE\n    auth_alg=OPEN\n"),
            Security::Wpa2 => {
                let p = self
                    .passphrase
                    .as_deref()
                    .ok_or("missing station credential")?
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"");
                config.push_str(&format!(
                    "    psk=\"{p}\"\n    key_mgmt=WPA-PSK\n    proto=RSN\n    pairwise=CCMP\n"
                ));
            }
        }
        if self.phy == Phy::NonHt {
            config.push_str("    disable_ht=1\n    disable_vht=1\n    disable_he=1\n");
        }
        config.push_str("}\n");
        Ok(config)
    }
    pub fn setup(&self) -> String {
        let address = match &self.ipv4 {
            Ipv4::None {} => "none",
            Ipv4::Dhcp {} => "dhcp",
            Ipv4::Static { address } => address,
        };
        format!("{address} {}\n", self.frequency_mhz)
    }
}

pub fn usable(ip: Ipv4Addr) -> bool {
    !ip.is_unspecified()
        && !ip.is_loopback()
        && !ip.is_multicast()
        && !ip.is_broadcast()
        && !ip.is_link_local()
}
pub fn parse_cidr(text: &str) -> Result<(Ipv4Addr, u8)> {
    let (ip, prefix) = text.split_once('/').ok_or("invalid station CIDR")?;
    let ip = ip.parse().map_err(|_| "invalid station IPv4 address")?;
    let prefix = prefix.parse::<u8>().map_err(|_| "invalid station prefix")?;
    if !usable(ip) || !(1..=32).contains(&prefix) {
        return Err("unusable station CIDR");
    }
    Ok((ip, prefix))
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Lease {
    pub address: Ipv4Addr,
    pub prefix: u8,
    pub router: Ipv4Addr,
    pub server: Ipv4Addr,
    /// CLOCK_BOOTTIME seconds, shared by hooks and the owning session.
    pub obtained_at: u64,
    /// CLOCK_BOOTTIME seconds; wall-clock adjustments do not extend a lease.
    pub expires_at: u64,
}
impl Lease {
    pub fn from_hook_options(now: u64, option: impl Fn(&str) -> Result<String>) -> Result<Self> {
        Self::from_options(
            &option("ip")?,
            &option("subnet")?,
            &option("router")?,
            &option("serverid")?,
            &option("lease")?,
            now,
        )
    }

    pub fn from_options(
        ip: &str,
        subnet: &str,
        routers: &str,
        server: &str,
        lifetime: &str,
        now: u64,
    ) -> Result<Self> {
        let address = ip.parse::<Ipv4Addr>().map_err(|_| "invalid DHCP address")?;
        let mask = u32::from(
            subnet
                .parse::<Ipv4Addr>()
                .map_err(|_| "invalid DHCP subnet")?,
        );
        let prefix = mask.leading_ones() as u8;
        if prefix == 0
            || prefix > 30
            || mask != u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0)
        {
            return Err("noncontiguous or unusable DHCP subnet");
        }
        let mut words = routers.split_whitespace();
        let router = words
            .next()
            .ok_or("missing DHCP router")?
            .parse::<Ipv4Addr>()
            .map_err(|_| "invalid DHCP router")?;
        if words.next().is_some() {
            return Err("ambiguous DHCP routers");
        }
        let server = server
            .parse::<Ipv4Addr>()
            .map_err(|_| "invalid DHCP server")?;
        let seconds = lifetime
            .parse::<u32>()
            .map_err(|_| "invalid DHCP lifetime")?;
        if seconds == 0
            || !usable(address)
            || !usable(router)
            || !usable(server)
            || address == router
            || (u32::from(address) & mask) != (u32::from(router) & mask)
        {
            return Err("unusable DHCP lease");
        }
        let host = u32::from(address) & !mask;
        let gateway_host = u32::from(router) & !mask;
        if host == 0 || host == !mask || gateway_host == 0 || gateway_host == !mask {
            return Err("DHCP address is a subnet endpoint");
        }
        Ok(Self {
            address,
            prefix,
            router,
            server,
            obtained_at: now,
            expires_at: now
                .checked_add(u64::from(seconds))
                .ok_or("DHCP expiration overflow")?,
        })
    }
    pub fn active(&self, now: u64) -> bool {
        self.obtained_at <= now && now < self.expires_at
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Hook {
    Deconfig,
    Bound,
    Renew,
    LeaseFail,
    Nak,
}
impl Hook {
    pub fn parse(event: &str, interface: &str) -> Result<Self> {
        if interface != "wlan0" {
            return Err("DHCP hook names another interface");
        }
        match event {
            "deconfig" => Ok(Self::Deconfig),
            "bound" => Ok(Self::Bound),
            "renew" => Ok(Self::Renew),
            "leasefail" => Ok(Self::LeaseFail),
            "nak" => Ok(Self::Nak),
            _ => Err("unknown DHCP event"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionCommand {
    Status,
    Ping,
    Disconnect,
}

/// Monotonic microsecond deadlines, supplied by the runtime or a virtual clock.
pub struct SessionState {
    deadline: u64,
    ping_started: bool,
    closed: bool,
}
impl SessionState {
    pub fn new(now: u64) -> Result<Self> {
        Ok(Self {
            deadline: now
                .checked_add(240_000_000)
                .ok_or("station session deadline overflow")?,
            ping_started: false,
            closed: false,
        })
    }
    pub fn remaining(&self, now: u64) -> Result<u64> {
        if self.closed || now >= self.deadline {
            return Err("station session closed or timed out");
        }
        Ok(self.deadline - now)
    }
    pub fn command(&mut self, command: SessionCommand, now: u64) -> Result<u64> {
        if self.closed {
            return Err("station session is closed");
        }
        if command == SessionCommand::Disconnect {
            self.closed = true;
            return Ok(0);
        }
        let remaining = self.remaining(now)?;
        if command == SessionCommand::Ping {
            if self.ping_started {
                return Err("station ping already executed");
            }
            self.ping_started = true;
        }
        Ok(remaining)
    }
}
impl SessionCommand {
    pub fn parse(text: &str) -> Result<Self> {
        match text.trim_end_matches(['\r', '\n']) {
            "status" => Ok(Self::Status),
            "ping" => Ok(Self::Ping),
            "disconnect" => Ok(Self::Disconnect),
            _ => Err("unknown station session command"),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Ping {
    pub sent: u32,
    pub received: u32,
    pub interval_ms: u32,
}
impl Ping {
    pub fn parse(text: &str, status: i32) -> Result<Self> {
        if ![0, 1].contains(&status) {
            return Err("station ping command failed");
        }
        let rows = text
            .lines()
            .filter(|s| s.contains("packets transmitted"))
            .collect::<Vec<_>>();
        let [row] = rows.as_slice() else {
            return Err("missing or repeated station ping summary");
        };
        let parts = row.split_whitespace().collect::<Vec<_>>();
        let [sent, "packets", "transmitted,", received, "received,", ..] = parts.as_slice() else {
            return Err("invalid station ping summary");
        };
        let sent = sent
            .parse::<u32>()
            .map_err(|_| "invalid station ping sent count")?;
        let received = received
            .parse::<u32>()
            .map_err(|_| "invalid station ping received count")?;
        if sent != 100
            || received > sent
            // iputils without -w returns success for any received reply,
            // including partial loss; no replies produce exit status 1.
            || (status == 0 && received == 0)
            || (status == 1 && received != 0)
        {
            return Err("contradictory station ping result");
        }
        Ok(Self {
            sent,
            received,
            interval_ms: 1000,
        })
    }
}

#[cfg(test)]
mod tests;
