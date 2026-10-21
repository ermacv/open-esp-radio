//! The dedicated W3 BSS network. All non-W3 network sections are fingerprinted
//! before and after applying it; no LAN address, port or route is rewritten.

use crate::{
    router::quote,
    ssh::{Remote, Result},
};
use serde::Serialize;
use std::{net::Ipv4Addr, time::Duration};

pub const NAME: &str = "oer_w3";
pub const BRIDGE: &str = "br-oer-w3";
pub const GATEWAY: Ipv4Addr = Ipv4Addr::new(10, 231, 0, 1);
pub const LEASE_FILE: &str = "/tmp/oer-w3.leases";

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Setting {
    pub path: String,
    pub value: String,
    pub list: bool,
}

fn settings() -> Vec<Setting> {
    let scalar = [
        ("network.oer_w3_device", "device"),
        ("network.oer_w3_device.name", BRIDGE),
        ("network.oer_w3_device.type", "bridge"),
        ("network.oer_w3_device.bridge_empty", "1"),
        ("network.oer_w3_device.ipv6", "0"),
        ("network.oer_w3", "interface"),
        ("network.oer_w3.device", BRIDGE),
        ("network.oer_w3.proto", "static"),
        ("network.oer_w3.ipaddr", "10.231.0.1"),
        ("network.oer_w3.netmask", "255.255.255.0"),
        ("network.oer_w3.delegate", "0"),
        ("network.oer_w3.defaultroute", "0"),
        ("dhcp.oer_w3_dns", "dnsmasq"),
        ("dhcp.oer_w3_dns.port", "0"),
        ("dhcp.oer_w3_dns.nonwildcard", "1"),
        ("dhcp.oer_w3_dns.localuse", "0"),
        ("dhcp.oer_w3_dns.noresolv", "1"),
        ("dhcp.oer_w3_dns.nohosts", "1"),
        ("dhcp.oer_w3_dns.readethers", "0"),
        ("dhcp.oer_w3_dns.authoritative", "1"),
        ("dhcp.oer_w3_dns.leasefile", LEASE_FILE),
        ("dhcp.oer_w3", "dhcp"),
        ("dhcp.oer_w3.interface", NAME),
        ("dhcp.oer_w3.instance", "oer_w3_dns"),
        ("dhcp.oer_w3.start", "100"),
        ("dhcp.oer_w3.limit", "100"),
        ("dhcp.oer_w3.leasetime", "15m"),
        ("dhcp.oer_w3.force", "1"),
        ("dhcp.oer_w3.ignore", "0"),
        ("dhcp.oer_w3.ra", "disabled"),
        ("dhcp.oer_w3.dhcpv6", "disabled"),
        ("dhcp.oer_w3.ndp", "disabled"),
        ("firewall.oer_w3", "zone"),
        ("firewall.oer_w3.name", NAME),
        ("firewall.oer_w3.input", "REJECT"),
        ("firewall.oer_w3.output", "ACCEPT"),
        ("firewall.oer_w3.forward", "REJECT"),
        ("firewall.oer_w3.masq", "0"),
        ("firewall.oer_w3.masq6", "0"),
        ("firewall.oer_w3_dhcp", "rule"),
        ("firewall.oer_w3_dhcp.name", "oer-w3-dhcp"),
        ("firewall.oer_w3_dhcp.src", NAME),
        ("firewall.oer_w3_dhcp.proto", "udp"),
        ("firewall.oer_w3_dhcp.src_port", "68"),
        ("firewall.oer_w3_dhcp.dest_port", "67"),
        ("firewall.oer_w3_dhcp.family", "ipv4"),
        ("firewall.oer_w3_dhcp.target", "ACCEPT"),
        ("firewall.oer_w3_ping", "rule"),
        ("firewall.oer_w3_ping.name", "oer-w3-ping"),
        ("firewall.oer_w3_ping.src", NAME),
        ("firewall.oer_w3_ping.proto", "icmp"),
        ("firewall.oer_w3_ping.icmp_type", "echo-request"),
        ("firewall.oer_w3_ping.dest_ip", "10.231.0.1"),
        ("firewall.oer_w3_ping.family", "ipv4"),
        ("firewall.oer_w3_ping.target", "ACCEPT"),
        ("firewall.oer_w3_out", "rule"),
        ("firewall.oer_w3_out.name", "oer-w3-no-forward-out"),
        ("firewall.oer_w3_out.src", NAME),
        ("firewall.oer_w3_out.dest", "*"),
        ("firewall.oer_w3_out.proto", "all"),
        ("firewall.oer_w3_out.target", "REJECT"),
        ("firewall.oer_w3_in", "rule"),
        ("firewall.oer_w3_in.name", "oer-w3-no-forward-in"),
        ("firewall.oer_w3_in.src", "*"),
        ("firewall.oer_w3_in.dest", NAME),
        ("firewall.oer_w3_in.proto", "all"),
        ("firewall.oer_w3_in.target", "REJECT"),
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
        ("dhcp.oer_w3_dns.interface", NAME),
        ("dhcp.oer_w3_dns.notinterface", "loopback"),
        ("dhcp.oer_w3.dhcp_option", "3,10.231.0.1"),
        ("dhcp.oer_w3.dhcp_option", "6"),
        ("firewall.oer_w3.network", NAME),
        ("firewall.oer_w3.device", BRIDGE),
    ] {
        values.push(Setting {
            path: path.into(),
            value: value.into(),
            list: true,
        });
    }
    values
}

/// Complete, secret-free mutation inventory, also used in the run report.
/// The generated SSID is represented by its role rather than its bytes.
pub fn changes(radio: &str, ap: &str, channel: u8) -> Vec<Setting> {
    let mut values = settings();
    for (section, option, value) in [
        (radio, "channel", channel.to_string()),
        (radio, "htmode", "HT20".into()),
        (radio, "beacon_int", "100".into()),
        (radio, "disabled", "0".into()),
        (radio, "ieee80211ax", "0".into()),
        (ap, "ssid", "<generated>".into()),
        (ap, "disabled", "0".into()),
        (ap, "hidden", "0".into()),
        (ap, "encryption", "none".into()),
        (ap, "network", NAME.into()),
        (ap, "isolate", "1".into()),
    ] {
        values.push(Setting {
            path: format!("wireless.{section}.{option}"),
            value,
            list: false,
        });
    }
    for value in &mut values {
        if value.value == BRIDGE {
            value.value = "<W3 bridge>".into();
        }
    }
    values
}

const SECTIONS: &[&str] = &[
    "network.oer_w3_device",
    "network.oer_w3",
    "dhcp.oer_w3_dns",
    "dhcp.oer_w3",
    "firewall.oer_w3",
    "firewall.oer_w3_dhcp",
    "firewall.oer_w3_ping",
    "firewall.oer_w3_out",
    "firewall.oer_w3_in",
];

pub fn render() -> Result<String> {
    let mut script = String::from("set -e\n");
    for section in SECTIONS {
        script.push_str(&format!("uci -q delete {} || :\n", quote(section)?));
    }
    for value in settings() {
        script.push_str(&format!(
            "uci {} {}\n",
            if value.list { "add_list" } else { "set" },
            quote(&format!("{}={}", value.path, value.value))?
        ));
    }
    script.push_str(
        "uci reorder firewall.oer_w3_out=0\nuci reorder firewall.oer_w3_in=0\n\
        fw4 check >/dev/null 2>&1\n\
        uci commit firewall\n/etc/init.d/firewall reload >/dev/null 2>&1\n\
        uci commit network\nuci commit dhcp\nubus call network reload >/dev/null\n\
        ifup oer_w3\n",
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
            return Err("W3 subnet overlaps an existing non-W3 route".into());
        }
    }
    Ok(())
}

fn fingerprint(remote: &mut dyn Remote) -> Result<String> {
    let value = remote.run("set -e\nconfig=$(uci show network)\nprintf '%s\\n' \"$config\" | awk '$0 !~ /^network\\.oer_w3(_device)?[.=]/' | sha256sum | cut -d ' ' -f 1")?;
    let value = value.trim();
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid non-W3 network fingerprint".into());
    }
    Ok(value.into())
}

/// Read-only preflight. Returns the fingerprint of every non-W3 network section.
pub fn prepare(remote: &mut dyn Remote) -> Result<String> {
    let changes = remote.run("set -e\nfor package in network dhcp firewall; do changes=$(uci changes \"$package\"); printf '%s\\n' \"$changes\" | sed 's/=.*//'; done")?;
    if !changes.trim().is_empty() {
        return Err("staged network, DHCP or firewall changes refuse W3".into());
    }
    let routes = remote.run(&format!("set -e\ncommand -v fw4 >/dev/null\nnft list table inet fw4 >/dev/null\ntest -x /etc/init.d/dnsmasq\nroutes=$(ip -4 route show table all)\nprintf '%s\\n' \"$routes\" | awk -v own_bridge={} '{{own=0; for(i=1;i<NF;i++) if($i==\"dev\" && $(i+1)==own_bridge) own=1; if(!own && NF) {{ if($1 ~ /^(local|broadcast|unreachable|blackhole|prohibit|throw)$/) print $2; else print $1 }} }}'", quote(BRIDGE)?))?;
    check_routes(&routes)?;
    fingerprint(remote)
}

fn verify_script(interface: &str) -> Result<String> {
    let mut script = String::from("set -e\n");
    // Check each scalar and list as committed, not only the rendered script.
    let mut expected = std::collections::BTreeMap::<String, String>::new();
    for value in settings() {
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
         nft list chain inet fw4 forward | grep -qF oer-w3-no-forward-out\n\
         nft list chain inet fw4 forward | grep -qF oer-w3-no-forward-in\n\
         printf 'verified\\n'\n",
        quote(interface)?,
        BRIDGE,
        BRIDGE,
        quote(interface)?,
        BRIDGE
    ));
    Ok(script)
}

pub fn verify(remote: &mut dyn Remote, interface: &str, before: &str) -> Result<()> {
    // fw4 resolves the source-zone device set again now that netifd has
    // created the bridge. The initial reload precedes W3 activation.
    remote
        .run("/etc/init.d/firewall reload >/dev/null 2>&1")
        .map_err(|_| "cannot reload firewall for the active W3 device")?;
    let script = verify_script(interface)?;
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
        return Err("W3 isolated network runtime verification failed".into());
    }
    if fingerprint(remote)? != before {
        return Err("non-W3 network configuration changed".into());
    }
    // DHCP starts only once the AP belongs to its addressed bridge.
    remote
        .run("/etc/init.d/dnsmasq reload >/dev/null 2>&1")
        .map_err(|_| "cannot reload W3 DHCP service")?;
    let ready = format!(
        "set -e\ntest -r {}\ntest \"$(ubus call service list '{{\"name\":\"dnsmasq\"}}' | jsonfilter -e '@.dnsmasq.instances.oer_w3_dns.running')\" = true\nprintf 'ready\\n'",
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
    Err("W3 DHCP instance did not become ready".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rendered_network_is_dedicated_and_exposes_only_dhcp_and_gateway_echo() {
        let script = render().unwrap();
        assert!(script.contains("network.oer_w3_device.type=bridge"));
        assert!(!script.contains(".ports="));
        assert!(script.contains("network.oer_w3.ipaddr=10.231.0.1"));
        assert!(script.contains("dhcp.oer_w3.instance=oer_w3_dns"));
        assert!(script.contains("dhcp.oer_w3_dns.port=0"));
        assert!(script.contains("dhcp.oer_w3_dns.leasefile=/tmp/oer-w3.leases"));
        assert!(script.contains("dhcp.oer_w3.dhcp_option=3,10.231.0.1"));
        assert!(script.contains("firewall.oer_w3.forward=REJECT"));
        assert!(script.contains("firewall.oer_w3.masq=0"));
        assert!(script.contains("firewall.oer_w3_ping.dest_ip=10.231.0.1"));
        assert!(!script.contains("forwarding"));
        assert!(!script.contains("network.lan"));
        assert!(!script.contains("network.wan"));
        assert!(script.find("firewall reload").unwrap() < script.find("network reload").unwrap());
        assert!(!script.contains("network restart"));
        for setting in settings() {
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
    fn inventory_redacts_only_generated_and_interface_values() {
        let values = changes("radio9", "lab_ap", 6);
        assert!(
            values
                .iter()
                .any(|s| s.path == "wireless.lab_ap.ssid" && s.value == "<generated>")
        );
        assert!(
            values
                .iter()
                .any(|s| s.path == "wireless.lab_ap.network" && s.value == NAME)
        );
        assert!(values.iter().all(|s| s.value != BRIDGE));
        let script = verify_script("made-up'interface").unwrap();
        assert!(script.contains(&quote("made-up'interface").unwrap()));
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
    fn runtime_requires_unchanged_non_w3_network_and_starts_dhcp_after_bridge_checks() {
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
        verify(&mut remote, "made-up-interface", &hash).unwrap();
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
        assert!(verify(&mut remote, "made-up-interface", &hash).is_err());
        assert!(!remote.scripts.iter().any(|s| s.contains("dnsmasq reload")));
    }
}
