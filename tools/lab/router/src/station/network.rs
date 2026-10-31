//! The dedicated station BSS network. All non-station network sections are fingerprinted
//! before and after applying it; no LAN address, port or route is rewritten.
//! Forwarding is rejected both ways, except UDP between the station network
//! and one host address when a run asks for it (its load traffic).

use crate::{
    router::quote,
    ssh::{Remote, Result},
};
use std::{net::Ipv4Addr, time::Duration};

pub const NAME: &str = "oer_station";
pub const BRIDGE: &str = "br-oer-station";
pub const GATEWAY: Ipv4Addr = Ipv4Addr::new(10, 231, 0, 1);
pub const LEASE_FILE: &str = "/tmp/oer-station.leases";

#[derive(Clone, Debug, Eq, PartialEq)]
struct Setting {
    path: String,
    value: String,
    list: bool,
}

fn settings(host: Option<Ipv4Addr>) -> Vec<Setting> {
    let scalar = [
        ("network.oer_station_device", "device"),
        ("network.oer_station_device.name", BRIDGE),
        ("network.oer_station_device.type", "bridge"),
        ("network.oer_station_device.bridge_empty", "1"),
        ("network.oer_station_device.ipv6", "0"),
        ("network.oer_station", "interface"),
        ("network.oer_station.device", BRIDGE),
        ("network.oer_station.proto", "static"),
        ("network.oer_station.ipaddr", "10.231.0.1"),
        ("network.oer_station.netmask", "255.255.255.0"),
        ("network.oer_station.delegate", "0"),
        ("network.oer_station.defaultroute", "0"),
        ("dhcp.oer_station_dns", "dnsmasq"),
        ("dhcp.oer_station_dns.port", "0"),
        ("dhcp.oer_station_dns.nonwildcard", "1"),
        ("dhcp.oer_station_dns.localuse", "0"),
        ("dhcp.oer_station_dns.noresolv", "1"),
        ("dhcp.oer_station_dns.nohosts", "1"),
        ("dhcp.oer_station_dns.readethers", "0"),
        ("dhcp.oer_station_dns.authoritative", "1"),
        ("dhcp.oer_station_dns.leasefile", LEASE_FILE),
        ("dhcp.oer_station", "dhcp"),
        ("dhcp.oer_station.interface", NAME),
        ("dhcp.oer_station.instance", "oer_station_dns"),
        ("dhcp.oer_station.start", "100"),
        ("dhcp.oer_station.limit", "100"),
        ("dhcp.oer_station.leasetime", "15m"),
        ("dhcp.oer_station.force", "1"),
        ("dhcp.oer_station.ignore", "0"),
        ("dhcp.oer_station.ra", "disabled"),
        ("dhcp.oer_station.dhcpv6", "disabled"),
        ("dhcp.oer_station.ndp", "disabled"),
        ("firewall.oer_station", "zone"),
        ("firewall.oer_station.name", NAME),
        ("firewall.oer_station.input", "REJECT"),
        ("firewall.oer_station.output", "ACCEPT"),
        ("firewall.oer_station.forward", "REJECT"),
        ("firewall.oer_station.masq", "0"),
        ("firewall.oer_station.masq6", "0"),
        ("firewall.oer_station_dhcp", "rule"),
        ("firewall.oer_station_dhcp.name", "oer-station-dhcp"),
        ("firewall.oer_station_dhcp.src", NAME),
        ("firewall.oer_station_dhcp.proto", "udp"),
        ("firewall.oer_station_dhcp.src_port", "68"),
        ("firewall.oer_station_dhcp.dest_port", "67"),
        ("firewall.oer_station_dhcp.family", "ipv4"),
        ("firewall.oer_station_dhcp.target", "ACCEPT"),
        ("firewall.oer_station_ping", "rule"),
        ("firewall.oer_station_ping.name", "oer-station-ping"),
        ("firewall.oer_station_ping.src", NAME),
        ("firewall.oer_station_ping.proto", "icmp"),
        ("firewall.oer_station_ping.icmp_type", "echo-request"),
        ("firewall.oer_station_ping.dest_ip", "10.231.0.1"),
        ("firewall.oer_station_ping.family", "ipv4"),
        ("firewall.oer_station_ping.target", "ACCEPT"),
        ("firewall.oer_station_out", "rule"),
        (
            "firewall.oer_station_out.name",
            "oer-station-no-forward-out",
        ),
        ("firewall.oer_station_out.src", NAME),
        ("firewall.oer_station_out.dest", "*"),
        ("firewall.oer_station_out.proto", "all"),
        ("firewall.oer_station_out.target", "REJECT"),
        ("firewall.oer_station_in", "rule"),
        ("firewall.oer_station_in.name", "oer-station-no-forward-in"),
        ("firewall.oer_station_in.src", "*"),
        ("firewall.oer_station_in.dest", NAME),
        ("firewall.oer_station_in.proto", "all"),
        ("firewall.oer_station_in.target", "REJECT"),
    ];
    let mut values = scalar
        .into_iter()
        .map(|(path, value)| Setting {
            path: path.into(),
            value: value.into(),
            list: false,
        })
        .collect::<Vec<_>>();
    for (path, value) in [
        ("dhcp.oer_station_dns.interface", NAME),
        ("dhcp.oer_station_dns.notinterface", "loopback"),
        ("dhcp.oer_station.dhcp_option", "3,10.231.0.1"),
        ("dhcp.oer_station.dhcp_option", "6"),
        ("firewall.oer_station.network", NAME),
        ("firewall.oer_station.device", BRIDGE),
    ] {
        values.push(Setting {
            path: path.into(),
            value: value.into(),
            list: true,
        });
    }
    if let Some(host) = host {
        let host = host.to_string();
        // Both sit in the chains the rejecting rules sit in (`src` `*`
        // forwards in `forward`, `src` station in `forward_oer_station`),
        // reordered before them.
        for (path, value) in [
            ("firewall.oer_station_host_in", "rule"),
            ("firewall.oer_station_host_in.name", "oer-station-host-in"),
            ("firewall.oer_station_host_in.src", "*"),
            ("firewall.oer_station_host_in.src_ip", host.as_str()),
            ("firewall.oer_station_host_in.dest", NAME),
            ("firewall.oer_station_host_in.proto", "udp"),
            ("firewall.oer_station_host_in.family", "ipv4"),
            ("firewall.oer_station_host_in.target", "ACCEPT"),
            ("firewall.oer_station_host_out", "rule"),
            ("firewall.oer_station_host_out.name", "oer-station-host-out"),
            ("firewall.oer_station_host_out.src", NAME),
            ("firewall.oer_station_host_out.dest", "*"),
            ("firewall.oer_station_host_out.dest_ip", host.as_str()),
            ("firewall.oer_station_host_out.proto", "udp"),
            ("firewall.oer_station_host_out.family", "ipv4"),
            ("firewall.oer_station_host_out.target", "ACCEPT"),
        ] {
            values.push(Setting {
                path: path.into(),
                value: value.into(),
                list: false,
            });
        }
    }
    values
}

const SECTIONS: &[&str] = &[
    "network.oer_station_device",
    "network.oer_station",
    "dhcp.oer_station_dns",
    "dhcp.oer_station",
    "firewall.oer_station",
    "firewall.oer_station_dhcp",
    "firewall.oer_station_ping",
    "firewall.oer_station_out",
    "firewall.oer_station_in",
    "firewall.oer_station_host_in",
    "firewall.oer_station_host_out",
];

/// The station network, forwarding UDP to and from `host` when given.
pub fn render(host: Option<Ipv4Addr>) -> Result<String> {
    let mut script = String::from("set -e\n");
    for section in SECTIONS {
        script.push_str(&format!("uci delete {} || :\n", quote(section)?));
    }
    for value in settings(host) {
        script.push_str(&format!(
            "uci {} {}\n",
            if value.list { "add_list" } else { "set" },
            quote(&format!("{}={}", value.path, value.value))?
        ));
    }
    script.push_str(
        "uci reorder firewall.oer_station_out=0\nuci reorder firewall.oer_station_in=0\n",
    );
    if host.is_some() {
        script.push_str(
            "uci reorder firewall.oer_station_host_out=0\n\
             uci reorder firewall.oer_station_host_in=0\n",
        );
    }
    script.push_str(
        "fw4 check >/dev/null\n\
        uci commit firewall\n/etc/init.d/firewall reload >/dev/null\n\
        uci commit network\nuci commit dhcp\nubus call network reload >/dev/null\n\
        ifup oer_station\n",
    );
    Ok(script)
}

pub fn check_routes(text: &str) -> Result<()> {
    if text.len() > 64 * 1024 {
        return Err("oversized router route observation".into());
    }
    for route in text.lines() {
        if route == "default" {
            continue;
        }
        let (address, prefix) = route.split_once('/').unwrap_or((route, "32"));
        let address: Ipv4Addr = address
            .parse()
            .map_err(|_| "invalid router route observation")?;
        let prefix: u32 = prefix.parse().map_err(|_| "invalid router route prefix")?;
        if prefix > 32 {
            return Err("invalid router route prefix".into());
        }
        if prefix == 0 {
            continue;
        }
        let mask = u32::MAX
            .checked_shl(32 - prefix.min(24))
            .ok_or("invalid subnet mask")?;
        if u32::from(address) & mask == u32::from(GATEWAY) & mask {
            return Err("station subnet overlaps an existing non-station route".into());
        }
    }
    Ok(())
}

fn fingerprint(remote: &mut dyn Remote) -> Result<String> {
    let value = remote.run("set -e\nconfig=$(uci show network)\nprintf '%s\\n' \"$config\" | awk '$0 !~ /^network\\.oer_station(_device)?[.=]/' | sha256sum | cut -d ' ' -f 1")?;
    let value = value.trim();
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid non-station network fingerprint".into());
    }
    Ok(value.into())
}

/// Read-only preflight. Returns the fingerprint of every non-station network section.
pub fn prepare(remote: &mut dyn Remote) -> Result<String> {
    let changes = remote.run("set -e\nfor package in network dhcp firewall; do changes=$(uci changes \"$package\"); printf '%s\\n' \"$changes\" | sed 's/=.*//'; done")?;
    if !changes.trim().is_empty() {
        return Err("staged network, DHCP or firewall changes refuse station".into());
    }
    let routes = remote.run(&format!("set -e\ncommand -v fw4 >/dev/null\nnft list table inet fw4 >/dev/null\ntest -x /etc/init.d/dnsmasq\nroutes=$(ip -4 route show table all)\nprintf '%s\\n' \"$routes\" | awk -v own_bridge={} '{{own=0; for(i=1;i<NF;i++) if($i==\"dev\" && $(i+1)==own_bridge) own=1; if(!own && NF) {{ if($1 ~ /^(local|broadcast|unreachable|blackhole|prohibit|throw)$/) print $2; else print $1 }} }}'", quote(BRIDGE)?))?;
    check_routes(&routes)?;
    fingerprint(remote)
}

fn verify_script(interface: &str, host: Option<Ipv4Addr>) -> Result<String> {
    let mut script = String::from("set -e\n");
    // Check each scalar and list as committed, not only the rendered script.
    let mut expected = std::collections::BTreeMap::<String, String>::new();
    for value in settings(host) {
        if value.list {
            let entry = expected.entry(value.path).or_default();
            if !entry.is_empty() {
                entry.push(' ');
            }
            entry.push_str(&value.value);
        } else {
            expected.insert(value.path, value.value);
        }
    }
    for (path, value) in expected {
        script.push_str(&format!(
            "test \"$(uci get {})\" = {}\n",
            quote(&path)?,
            quote(&value)?
        ));
    }
    script.push_str(&format!(
        "test \"$(readlink -f /sys/class/net/{}/master)\" = /sys/devices/virtual/net/{}\n\
         test \"$(ls /sys/class/net/{}/brif)\" = {}\n\
         test \"$(ip -4 -o addr show dev {} | awk '{{print $4}}')\" = 10.231.0.1/24\n\
         nft list chain inet fw4 forward_oer_station | grep -qF oer-station-no-forward-out\n\
         nft list chain inet fw4 forward | grep -qF oer-station-no-forward-in\n",
        quote(interface)?,
        BRIDGE,
        BRIDGE,
        quote(interface)?,
        BRIDGE
    ));
    if host.is_some() {
        script.push_str(
            "nft list chain inet fw4 forward_oer_station | grep -qF oer-station-host-out\n\
             nft list chain inet fw4 forward | grep -qF oer-station-host-in\n",
        );
    }
    script.push_str("printf 'verified\\n'\n");
    Ok(script)
}

/// Verify the applied network, the forwarding for `host` included.
pub fn verify(
    remote: &mut dyn Remote,
    interface: &str,
    before: &str,
    host: Option<Ipv4Addr>,
) -> Result<()> {
    // fw4 resolves the source-zone device set again now that netifd has
    // created the bridge. The initial reload precedes station activation.
    remote
        .run("/etc/init.d/firewall reload >/dev/null")
        .map_err(|_| "cannot reload firewall for the active station device")?;
    let script = verify_script(interface, host)?;
    let mut verified = false;
    for attempt in 0..10 {
        if remote
            .run(&script)
            .is_ok_and(|output| output.trim() == "verified")
        {
            verified = true;
            break;
        }
        if attempt < 9 {
            remote.pause(Duration::from_secs(1));
        }
    }
    if !verified {
        return Err("station isolated network runtime verification failed".into());
    }
    if fingerprint(remote)? != before {
        return Err("non-station network configuration changed".into());
    }
    // DHCP starts only once the AP belongs to its addressed bridge.
    remote
        .run("/etc/init.d/dnsmasq reload >/dev/null")
        .map_err(|_| "cannot reload station DHCP service")?;
    let ready = format!(
        "set -e\ntest -r {}\ntest \"$(ubus call service list '{{\"name\":\"dnsmasq\"}}' | jsonfilter -e '@.dnsmasq.instances.oer_station_dns.running')\" = true\nprintf 'ready\\n'",
        quote(LEASE_FILE)?
    );
    for attempt in 0..10 {
        if remote
            .run(&ready)
            .is_ok_and(|output| output.trim() == "ready")
        {
            return Ok(());
        }
        if attempt < 9 {
            remote.pause(Duration::from_secs(1));
        }
    }
    Err("station DHCP instance did not become ready".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rendered_network_is_dedicated_and_exposes_only_dhcp_and_gateway_echo() {
        let script = render(None).unwrap();
        assert!(script.contains("network.oer_station_device.type=bridge"));
        assert!(!script.contains(".ports="));
        assert!(script.contains("network.oer_station.ipaddr=10.231.0.1"));
        assert!(script.contains("dhcp.oer_station.instance=oer_station_dns"));
        assert!(script.contains("dhcp.oer_station_dns.port=0"));
        assert!(script.contains("dhcp.oer_station_dns.leasefile=/tmp/oer-station.leases"));
        assert!(script.contains("dhcp.oer_station.dhcp_option=3,10.231.0.1"));
        assert!(script.contains("firewall.oer_station.forward=REJECT"));
        assert!(script.contains("firewall.oer_station.masq=0"));
        assert!(script.contains("firewall.oer_station_ping.dest_ip=10.231.0.1"));
        assert!(!script.contains("forwarding"));
        assert!(!script.contains("network.lan"));
        assert!(!script.contains("network.wan"));
        assert!(script.find("firewall reload").unwrap() < script.find("network reload").unwrap());
        assert!(!script.contains("network restart"));
        assert!(!script.contains("oer-station-host"));
        for setting in settings(Some(Ipv4Addr::new(192, 168, 178, 129))) {
            assert!(SECTIONS.iter().any(|section| setting.path == *section
                || setting.path.starts_with(&format!("{section}."))));
        }
    }
    #[test]
    fn subnet_rejects_intersections_but_not_the_default_route() {
        check_routes("default\n192.168.178.0/24\n192.168.178.2\n10.232.0.0/16").unwrap();
        for text in [
            "10.231.0.0/24",
            "10.231.0.100/32",
            "10.0.0.0/8",
            "garbage",
            "10.231.1.0/33",
        ] {
            assert!(check_routes(text).is_err());
        }
        check_routes("10.231.1.0/24").unwrap();
    }
    #[test]
    fn runtime_verification_quotes_the_interface_and_checks_isolation() {
        let script = verify_script("made-up'interface", None).unwrap();
        assert!(script.contains(&quote("made-up'interface").unwrap()));
        assert!(script.contains("forward_oer_station | grep -qF oer-station-no-forward-out"));
        assert!(script.contains("fw4 forward | grep -qF oer-station-no-forward-in"));
        assert!(!script.contains("oer-station-host"));
    }

    #[test]
    fn a_host_gets_udp_forwarding_both_ways_ahead_of_the_rejections() {
        let host = Ipv4Addr::new(192, 168, 178, 129);
        let script = render(Some(host)).unwrap();
        for line in [
            "firewall.oer_station_host_in.src_ip=192.168.178.129",
            "firewall.oer_station_host_in.dest=oer_station",
            "firewall.oer_station_host_out.dest_ip=192.168.178.129",
            "firewall.oer_station_host_out.src=oer_station",
        ] {
            assert!(script.contains(line), "{line}");
        }
        assert!(script.contains("firewall.oer_station_host_in.proto=udp"));
        assert!(script.contains("firewall.oer_station_host_out.proto=udp"));
        // Reordered to the front after the rejections were.
        assert!(
            script.find("reorder firewall.oer_station_in=0").unwrap()
                < script
                    .find("reorder firewall.oer_station_host_in=0")
                    .unwrap()
        );
        let verify = verify_script("phy0-ap0", Some(host)).unwrap();
        assert!(verify.contains("grep -qF oer-station-host-out"));
        assert!(verify.contains("uci get firewall.oer_station_host_in.src_ip"));
    }

    struct Fake {
        replies: std::collections::VecDeque<String>,
        scripts: Vec<String>,
    }
    impl Remote for Fake {
        fn run(&mut self, script: &str) -> Result<String> {
            self.scripts.push(script.into());
            self.replies
                .pop_front()
                .ok_or("unexpected observation".into())
        }
        fn pause(&mut self, _: Duration) {
            panic!("this fixture must not wait");
        }
    }
    #[test]
    fn preflight_refuses_uncommitted_work_and_only_excludes_the_dedicated_bridge_routes() {
        let hash = "a".repeat(64);
        let mut remote = Fake {
            replies: [
                "".into(),
                "default\n192.168.178.0/24\n".into(),
                hash.clone(),
            ]
            .into(),
            scripts: vec![],
        };
        assert_eq!(prepare(&mut remote).unwrap(), hash);
        assert!(
            remote
                .scripts
                .iter()
                .any(|s| s.contains("$(i+1)==own_bridge"))
        );
        let mut remote = Fake {
            replies: ["network.someone_else\n".into()].into(),
            scripts: vec![],
        };
        assert!(prepare(&mut remote).is_err());
        assert_eq!(remote.scripts.len(), 1);
    }
    #[test]
    fn runtime_requires_unchanged_non_station_network_and_starts_dhcp_after_bridge_checks() {
        let hash = "a".repeat(64);
        let mut remote = Fake {
            replies: [
                "".into(),
                "verified\n".into(),
                hash.clone(),
                "".into(),
                "ready\n".into(),
            ]
            .into(),
            scripts: vec![],
        };
        verify(&mut remote, "made-up-interface", &hash, None).unwrap();
        assert!(remote.scripts.first().unwrap().contains("firewall reload"));
        assert!(remote.scripts.iter().any(|s| s.contains("/master")));
        assert!(
            remote
                .scripts
                .iter()
                .position(|s| s.contains("sha256sum"))
                .unwrap()
                < remote
                    .scripts
                    .iter()
                    .position(|s| s.contains("dnsmasq reload"))
                    .unwrap()
        );
        let mut remote = Fake {
            replies: ["".into(), "verified\n".into(), "b".repeat(64)].into(),
            scripts: vec![],
        };
        assert!(verify(&mut remote, "made-up-interface", &hash, None).is_err());
        assert!(!remote.scripts.iter().any(|s| s.contains("dnsmasq reload")));
    }
}
