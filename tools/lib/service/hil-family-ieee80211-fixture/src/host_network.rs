//! Qualification preflights for the host-side IP topology.

use oer_process::CommandExt as _;
use std::{
    net::Ipv4Addr,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::Result;
use oer_hil_lab::config::StationFixtureConfig;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BenchmarkIpv4Route {
    interface: String,
    source: Ipv4Addr,
    medium: RouteMedium,
    expected_medium: Option<RouteMedium>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RouteMedium {
    Ethernet,
    Wireless,
}

#[derive(Debug, Serialize)]
struct HostRouteEvidence<'a> {
    schema: u16,
    target: Ipv4Addr,
    interface: &'a str,
    route_source: Ipv4Addr,
    socket_source: Ipv4Addr,
    medium: RouteMedium,
    expected_medium: Option<RouteMedium>,
    medium_assertion_passed: bool,
    socket_source_assertion_passed: bool,
}

impl BenchmarkIpv4Route {
    /// The route to a device behind the station fixture: over Ethernet to
    /// an OpenWrt fixture, over the air from a local Linux one.
    pub fn discover(device: Ipv4Addr, fixture: &StationFixtureConfig) -> Result<Self> {
        Self::discover_over(
            device,
            match fixture {
                StationFixtureConfig::OpenWrt(_) => Some(RouteMedium::Ethernet),
                StationFixtureConfig::LocalLinux(_) => Some(RouteMedium::Wireless),
                StationFixtureConfig::External(_) => None,
            },
        )
    }

    /// The host's route to `device`, refused unless its interface is of
    /// `expected_medium` (any, for `None`).
    pub fn discover_over(device: Ipv4Addr, expected_medium: Option<RouteMedium>) -> Result<Self> {
        reject_overlapping_ipv4_links(device)?;
        let output = oer_process::command("ip")
            .args(["-4", "route", "get", &device.to_string()])
            .supervised_output()
            .map_err(|error| format!("cannot inspect the host route to {device}: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "cannot inspect the host route to {device}: `ip route get` exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
        let mut route = parse_ipv4_route(&String::from_utf8(output.stdout)?, device)?;
        route.medium = interface_medium(&route.interface);
        route.expected_medium = expected_medium;
        if route
            .expected_medium
            .is_some_and(|expected| route.medium != expected)
        {
            return Err(format!(
                "qualification route to {device} uses {:?} interface `{}`, expected {:?}",
                route.medium,
                route.interface,
                route.expected_medium.expect("checked above")
            )
            .into());
        }
        Ok(route)
    }

    pub fn capture_wire(
        &self,
        target: Ipv4Addr,
        output: &Path,
        duration: std::time::Duration,
    ) -> Result<crate::local::wire_capture::Capture> {
        crate::local::wire_capture::Capture::start(&self.interface, target, output, duration)
    }

    /// The source address the route selects.
    pub const fn source(&self) -> Ipv4Addr {
        self.source
    }

    pub fn verify_socket_source(&self, actual: Ipv4Addr) -> Result<()> {
        if actual != self.source {
            return Err(format!(
                "benchmark socket selected source {actual}, but the validated `{}` route selected {}",
                self.interface, self.source
            )
            .into());
        }
        Ok(())
    }

    pub fn record(&self, output: &Path, target: Ipv4Addr, socket_source: Ipv4Addr) -> Result<()> {
        self.verify_socket_source(socket_source)?;
        oer_durable::atomic_json(
            &output.join("host-route.json"),
            &HostRouteEvidence {
                schema: 1,
                target,
                interface: &self.interface,
                route_source: self.source,
                socket_source,
                medium: self.medium,
                expected_medium: self.expected_medium,
                medium_assertion_passed: self
                    .expected_medium
                    .is_none_or(|expected| expected == self.medium),
                socket_source_assertion_passed: true,
            },
        )
    }
}

fn interface_medium(interface: &str) -> RouteMedium {
    let interface = PathBuf::from("/sys/class/net").join(interface);
    if interface.join("phy80211").exists() || interface.join("wireless").exists() {
        RouteMedium::Wireless
    } else {
        RouteMedium::Ethernet
    }
}

fn parse_ipv4_route(output: &str, device: Ipv4Addr) -> Result<BenchmarkIpv4Route> {
    let fields = output
        .lines()
        .next()
        .ok_or_else(|| format!("host route lookup for {device} returned no route"))?
        .split_whitespace()
        .collect::<Vec<_>>();
    let value_after = |key: &str| {
        fields
            .windows(2)
            .find(|pair| pair[0] == key)
            .map(|pair| pair[1])
    };
    let interface = value_after("dev")
        .ok_or_else(|| format!("host route lookup for {device} omitted its interface"))?;
    let source = value_after("src")
        .ok_or_else(|| format!("host route lookup for {device} omitted its source address"))?
        .parse::<Ipv4Addr>()?;
    Ok(BenchmarkIpv4Route {
        interface: interface.to_owned(),
        source,
        medium: RouteMedium::Ethernet,
        expected_medium: None,
    })
}

/// Reject a dual-homed host on the target's L2 subnet.
///
/// With Linux's default ARP-flux policy (`arp_ignore=0`), either interface may
/// answer for the source address selected by the benchmark socket. The target
/// can then legitimately update its neighbor cache to the other interface's
/// MAC. A TX flow addressed to the host's Ethernet IP may consequently return
/// through a co-channel WLAN interface, measuring a second radio hop instead
/// of the DUT-to-fixture link. A qualification cell must expose one
/// unambiguous L2 identity instead of silently measuring that topology.
pub fn reject_overlapping_ipv4_links(device: Ipv4Addr) -> Result<()> {
    let output = match oer_process::command("ip")
        .args(["-o", "-4", "addr", "show", "up", "scope", "global"])
        .supervised_output()
    {
        Ok(output) if output.status.success() => output.stdout,
        Ok(output) => {
            return Err(crate::Error::new(format!(
                "cannot inspect host IPv4 topology: {}",
                String::from_utf8_lossy(&output.stderr)
            ))
            .into());
        }
        Err(error) => return Err(crate::Error::context(error)),
    };
    let links = overlapping_ipv4_links(&String::from_utf8_lossy(&output), device);
    if links.len() > 1 {
        return Err(format!(
            "qualification has multiple active interfaces on the target subnet ({}); disable all but one to prevent ARP flux and a second radio hop",
            links.join(", ")
        )
        .into());
    }
    Ok(())
}

fn overlapping_ipv4_links(output: &str, device: Ipv4Addr) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let _index = fields.next()?;
            let name = fields.next()?.trim_end_matches(':');
            fields.find(|field| *field == "inet")?;
            let (address, prefix) = fields.next()?.split_once('/')?;
            let address = address.parse::<Ipv4Addr>().ok()?;
            let prefix = prefix.parse::<u8>().ok()?;
            same_ipv4_subnet(address, device, prefix).then(|| format!("{name}={address}/{prefix}"))
        })
        .collect()
}

fn same_ipv4_subnet(left: Ipv4Addr, right: Ipv4Addr, prefix: u8) -> bool {
    if prefix > 32 {
        return false;
    }
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    u32::from(left) & mask == u32::from(right) & mask
}

#[cfg(test)]
mod tests;
