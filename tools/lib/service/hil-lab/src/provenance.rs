//! Secret-free, pre-run observations of the physical HIL cell.

use oer_durable::unix_millis;
use oer_process::CommandExt as _;
use std::{collections::BTreeMap, fs, net::Ipv4Addr, path::Path};

use crate::wifi::NetworkIpv4Configuration;

use crate::wifi::WifiChannelWidth;
use crate::{
    Result,
    config::{LabConfig, OpenWrtConfig, StationFixtureConfig},
};
use oer_hil_run_bundle_format::lab::AccessPointDefinition;
use oer_hil_run_bundle_format::lab::ChannelWidth;
use oer_hil_run_bundle_format::lab::FixtureObservation;
use oer_hil_run_bundle_format::lab::HostInterfaceObservation;
use oer_hil_run_bundle_format::lab::HostIpv4Route;
use oer_hil_run_bundle_format::lab::HostObservation;
use oer_hil_run_bundle_format::lab::HostWirelessLink;
use oer_hil_run_bundle_format::lab::LAB_PROVENANCE_SCHEMA;
use oer_hil_run_bundle_format::lab::LabDefinition;
use oer_hil_run_bundle_format::lab::LabProvenance;
use oer_hil_run_bundle_format::lab::ObservationScope;
use oer_hil_run_bundle_format::lab::OpenWrtInterface;
use oer_hil_run_bundle_format::lab::OpenWrtObservation;
use oer_hil_run_bundle_format::lab::Phy;
use oer_hil_run_bundle_format::lab::SensitiveValueDisposition;
use oer_hil_run_bundle_format::lab::StationFixtureDefinition;
use oer_hil_run_bundle_format::lab::StationIpv4Definition;

/// Observe the cell of `lab` before a run that needs `required`.
pub fn capture(
    lab: &LabConfig,
    required: oer_hil_scenario_catalog::requirements::Requirements,
) -> Result<LabProvenance> {
    {
        let definition = definition(lab);
        let scope = if required.network() {
            ObservationScope::Network
        } else {
            ObservationScope::System
        };
        let host = capture_host(scope)?;
        let fixture = if scope == ObservationScope::System {
            FixtureObservation::NotUsed
        } else {
            match &lab.station_fixture {
                StationFixtureConfig::LocalLinux(_) => FixtureObservation::LocalLinux,
                StationFixtureConfig::OpenWrt(config) => capture_openwrt_before(config)?,
                StationFixtureConfig::External(_) => {
                    FixtureObservation::External { managed: false }
                }
            }
        };
        Ok(LabProvenance {
            schema: LAB_PROVENANCE_SCHEMA,
            captured_unix_millis: unix_millis(),
            definition,
            host,
            fixture,
            scope,
        })
    }
}

/// The secret-free definition of `lab`.
fn definition(lab: &LabConfig) -> LabDefinition {
    {
        let station_ipv4 = match lab.station.ipv4() {
            NetworkIpv4Configuration::Dhcp => StationIpv4Definition::Dhcp,
            NetworkIpv4Configuration::Static {
                address,
                prefix_length,
                gateway,
            } => StationIpv4Definition::Static {
                address: Ipv4Addr::from(address),
                prefix_length,
                gateway: gateway.map(Ipv4Addr::from),
            },
        };
        let station_fixture = match &lab.station_fixture {
            StationFixtureConfig::LocalLinux(config) => StationFixtureDefinition::LocalLinux {
                interface: config.interface.clone(),
                phys: recorded_phys(&config.phys),
            },
            StationFixtureConfig::OpenWrt(config) => StationFixtureDefinition::OpenWrt {
                wireless_interface: config.wireless_interface.clone(),
                ingress_interface: config.ingress_interface.clone(),
                monitor_interface: config.monitor_interface.clone(),
                phys: recorded_phys(&config.phys),
                independent_laptop_monitor: config.independent_laptop_monitor,
            },
            StationFixtureConfig::External(config) => StationFixtureDefinition::External {
                phys: recorded_phys(&config.phys),
            },
        };
        LabDefinition {
            cell_id: lab.cell_id().to_owned(),
            device_id: lab.dut.id.clone(),
            bluetooth_adapter: lab.bluetooth_adapter.map(|adapter| adapter.to_string()),
            peer: lab.peer().ok().map(|peer| peer.id),
            station_ipv4,
            access_point: AccessPointDefinition {
                channel: lab.access_point.channel(),
                channel_width: match lab.access_point.channel_width() {
                    WifiChannelWidth::Mhz20 => ChannelWidth::Mhz20,
                    WifiChannelWidth::Mhz40Above => ChannelWidth::Mhz40Above,
                    WifiChannelWidth::Mhz40Below => ChannelWidth::Mhz40Below,
                },
                client_limit: lab.access_point.client_limit(),
                target_address: lab.access_point.target_address(),
                client_address: lab.access_point.client_address(),
                secondary_client_address: lab.access_point.secondary_client_address(),
            },
            station_fixture,
            sensitive_network_values: SensitiveValueDisposition::Omitted,
        }
    }
}

fn capture_host(scope: ObservationScope) -> Result<HostObservation> {
    let network = scope == ObservationScope::Network;
    let addresses = if network {
        host_ipv4_addresses()?
    } else {
        BTreeMap::new()
    };
    let mut interfaces = Vec::new();
    let mut entries = fs::read_dir("/sys/class/net")?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let wireless = path.join("phy80211").exists() || path.join("wireless").exists();
        interfaces.push(HostInterfaceObservation {
            operstate: read_trimmed(path.join("operstate"))?.unwrap_or_default(),
            mac_address: read_trimmed(path.join("address"))?,
            master: fs::read_link(path.join("master")).ok().and_then(|master| {
                master
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            }),
            ipv4_addresses: addresses.get(&name).cloned().unwrap_or_default(),
            wireless_link: (wireless && network)
                .then(|| capture_host_wireless_link(&name))
                .transpose()?,
            wireless,
            name,
        });
    }
    Ok(HostObservation {
        kernel_release: command_stdout("uname", &["-r"])?,
        machine: command_stdout("uname", &["-m"])?,
        os_release: os_release(),
        boot_id: read_trimmed("/proc/sys/kernel/random/boot_id")?,
        interfaces,
        ipv4_routes: if network {
            host_ipv4_routes()?
        } else {
            Vec::new()
        },
    })
}

fn host_ipv4_addresses() -> Result<BTreeMap<String, Vec<String>>> {
    let output = command_output("ip", &["-o", "-4", "addr", "show"])?;
    let mut addresses = BTreeMap::<String, Vec<String>>::new();
    for line in output.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        let Some(inet) = fields.iter().position(|field| *field == "inet") else {
            continue;
        };
        if inet < 2 || inet + 1 >= fields.len() {
            return Err(format!("invalid `ip -o -4 addr` record: {line}").into());
        }
        addresses
            .entry(fields[1].trim_end_matches(':').to_owned())
            .or_default()
            .push(fields[inet + 1].to_owned());
    }
    for values in addresses.values_mut() {
        values.sort();
    }
    Ok(addresses)
}

fn host_ipv4_routes() -> Result<Vec<HostIpv4Route>> {
    let output = command_output("ip", &["-o", "-4", "route", "show", "table", "main"])?;
    parse_host_ipv4_routes(&output)
}

fn parse_host_ipv4_routes(output: &str) -> Result<Vec<HostIpv4Route>> {
    let mut routes = Vec::new();
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        let value_after = |key: &str| {
            fields
                .windows(2)
                .find(|pair| pair[0] == key)
                .map(|pair| pair[1])
        };
        routes.push(HostIpv4Route {
            destination: fields
                .first()
                .ok_or("host IPv4 route has no destination")?
                .to_string(),
            interface: value_after("dev").map(str::to_owned),
            gateway: value_after("via").map(str::parse).transpose()?,
            preferred_source: value_after("src").map(str::parse).transpose()?,
            metric: value_after("metric").map(str::parse).transpose()?,
        });
    }
    Ok(routes)
}

fn capture_host_wireless_link(interface: &str) -> Result<HostWirelessLink> {
    let info = oer_process::command("iw")
        .args(["dev", interface, "info"])
        .supervised_output()?;
    let interface_type = info
        .status
        .success()
        .then(|| {
            let output = String::from_utf8_lossy(&info.stdout);
            tagged_iw_value(&output, "type").map(str::to_owned)
        })
        .flatten();
    let link = oer_process::command("iw")
        .args(["dev", interface, "link"])
        .supervised_output()?;
    if !link.status.success() {
        return Err(format!("cannot inspect wireless link `{interface}`").into());
    }
    let output = String::from_utf8(link.stdout)?;
    let bssid = output.lines().find_map(|line| {
        line.trim()
            .strip_prefix("Connected to ")?
            .split_whitespace()
            .next()
            .map(str::to_owned)
    });
    Ok(HostWirelessLink {
        interface_type,
        connected: bssid.is_some(),
        bssid,
    })
}

fn capture_openwrt_before(config: &OpenWrtConfig) -> Result<FixtureObservation> {
    let output = oer_stand_ssh::command(
        &config.ssh_target,
        &format!(
            "if test -d /sys/class/net/{interface}; then iw dev {interface} info; else exit 42; fi",
            interface = config.wireless_interface
        ),
    )
    .supervised_output()
    .and_then(super::Error::ssh_output)?;
    if openwrt_has_active_channel(output.status.code(), std::str::from_utf8(&output.stdout)?)? {
        Ok(FixtureObservation::OpenWrt(Box::new(capture_openwrt(
            config,
        )?)))
    } else {
        Ok(FixtureObservation::OpenWrtInactive {
            wireless_interface: config.wireless_interface.clone(),
        })
    }
}

fn openwrt_has_active_channel(status: Option<i32>, info: &str) -> Result<bool> {
    match status {
        // A present but down VIF can have no channel. This is a before-state
        // observation, not a requirement for a ready AP.
        Some(0) => Ok(tagged_iw_value(info, "channel").is_some()),
        Some(42) => Ok(false),
        _ => Err("cannot observe OpenWrt interface state".into()),
    }
}

fn capture_openwrt(config: &OpenWrtConfig) -> Result<OpenWrtObservation> {
    let script = format!(
        "set -eu; . /etc/openwrt_release; \
         info=$(iw dev {wireless} info); \
         printf 'release=%s\\n' \"$DISTRIB_DESCRIPTION\"; \
         printf 'revision=%s\\n' \"$DISTRIB_REVISION\"; \
         printf 'kernel=%s\\n' \"$(uname -r)\"; \
         printf 'machine=%s\\n' \"$(uname -m)\"; \
         printf 'boot_id=%s\\n' \"$(cat /proc/sys/kernel/random/boot_id)\"; \
         printf 'ingress_operstate=%s\\n' \"$(cat /sys/class/net/{ingress}/operstate)\"; \
         ip -o -4 addr show dev {ingress} | awk '{{print \"ingress_ipv4=\" $4}}'; \
         printf 'wiphy=%s\\n' \"$(printf '%s\\n' \"$info\" | awk '$1 == \"wiphy\" {{print $2; exit}}')\"; \
         printf 'interface_type=%s\\n' \"$(printf '%s\\n' \"$info\" | awk '$1 == \"type\" {{print $2; exit}}')\"; \
         printf 'channel=%s\\n' \"$(printf '%s\\n' \"$info\" | awk '$1 == \"channel\" {{print $2; exit}}')\"; \
         printf 'frequency_mhz=%s\\n' \"$(printf '%s\\n' \"$info\" | sed -n 's/.*(\\([0-9][0-9]*\\) MHz).*/\\1/p')\"; \
         printf 'width_mhz=%s\\n' \"$(printf '%s\\n' \"$info\" | sed -n 's/.*width: \\([0-9][0-9]*\\) MHz.*/\\1/p')\"; \
         printf 'center1_mhz=%s\\n' \"$(printf '%s\\n' \"$info\" | sed -n 's/.*center1: \\([0-9][0-9]*\\) MHz.*/\\1/p')\"; \
         printf 'tx_power_dbm=%s\\n' \"$(printf '%s\\n' \"$info\" | awk '$1 == \"txpower\" {{print $2; exit}}')\"; \
         printf 'country=%s\\n' \"$(iw reg get | sed -n 's/^country \\([A-Z0-9][A-Z0-9]\\):.*/\\1/p' | head -n1)\"; \
         printf 'driver=%s\\n' \"$(basename \"$(readlink -f /sys/class/net/{wireless}/device/driver 2>/dev/null)\" 2>/dev/null || true)\"; \
         printf 'firmware=%s\\n' \"$(ethtool -i {wireless} 2>/dev/null | sed -n 's/^firmware-version: //p' || true)\"; \
         printf 'associated_stations=%s\\n' \"$(iw dev {wireless} station dump | awk '/^Station / {{n++}} END {{print n+0}}')\"; \
         iw dev | awk '$1 == \"Interface\" {{name=$2}} $1 == \"type\" {{print \"vif=\" name \"|\" $2}}'",
        wireless = config.wireless_interface,
        ingress = config.ingress_interface,
    );
    let output = oer_stand_ssh::command(&config.ssh_target, &script).supervised_output()?;
    if !output.status.success() {
        return Err(format!(
            "cannot capture secret-free OpenWrt lab provenance: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let observation = parse_openwrt(
        &String::from_utf8(output.stdout)?,
        &config.wireless_interface,
        &config.ingress_interface,
    )?;
    Ok(observation)
}

fn parse_openwrt(output: &str, wireless: &str, ingress: &str) -> Result<OpenWrtObservation> {
    let tagged = |key: &str| -> Result<&str> {
        output
            .lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .ok_or_else(|| format!("OpenWrt lab snapshot omitted `{key}`").into())
    };
    let nonempty = |key: &str| -> Result<&str> {
        tagged(key).and_then(|value| {
            (!value.is_empty())
                .then_some(value)
                .ok_or_else(|| format!("OpenWrt lab snapshot reported an empty `{key}`").into())
        })
    };
    let optional = |key: &str| -> Result<Option<String>> {
        Ok(match tagged(key)? {
            "" => None,
            value => Some(value.to_owned()),
        })
    };
    let tx_power_dbm = nonempty("tx_power_dbm")?.parse::<f64>()?;
    if !tx_power_dbm.is_finite() {
        return Err("OpenWrt TX power is not finite".into());
    }
    let mut concurrent_interfaces = output
        .lines()
        .filter_map(|line| line.strip_prefix("vif="))
        .map(|value| -> Result<_> {
            let (name, interface_type) = value
                .split_once('|')
                .ok_or("invalid OpenWrt VIF observation")?;
            if name.is_empty() || interface_type.is_empty() {
                return Err("empty OpenWrt VIF name or type".into());
            }
            Ok(OpenWrtInterface {
                name: name.to_owned(),
                interface_type: interface_type.to_owned(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    concurrent_interfaces.sort_by(|left, right| left.name.cmp(&right.name));
    let ingress_ipv4_addresses = output
        .lines()
        .filter_map(|line| line.strip_prefix("ingress_ipv4="))
        .map(str::to_owned)
        .collect();
    Ok(OpenWrtObservation {
        release: nonempty("release")?.to_owned(),
        revision: nonempty("revision")?.to_owned(),
        kernel_release: nonempty("kernel")?.to_owned(),
        machine: nonempty("machine")?.to_owned(),
        boot_id: nonempty("boot_id")?.to_owned(),
        wireless_interface: wireless.to_owned(),
        ingress_interface: ingress.to_owned(),
        ingress_operstate: nonempty("ingress_operstate")?.to_owned(),
        ingress_ipv4_addresses,
        wiphy: nonempty("wiphy")?.parse()?,
        interface_type: nonempty("interface_type")?.to_owned(),
        channel: nonempty("channel")?.parse()?,
        frequency_mhz: nonempty("frequency_mhz")?.parse()?,
        width_mhz: nonempty("width_mhz")?.parse()?,
        center1_mhz: nonempty("center1_mhz")?.parse()?,
        tx_power_milli_dbm: i32::try_from((tx_power_dbm * 1_000.0).round() as i64)?,
        country: optional("country")?,
        driver: nonempty("driver")?.to_owned(),
        firmware: optional("firmware")?,
        associated_stations: nonempty("associated_stations")?.parse()?,
        concurrent_interfaces,
    })
}

fn tagged_iw_value<'a>(output: &'a str, key: &str) -> Option<&'a str> {
    output.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        (fields.next()? == key).then(|| fields.next()).flatten()
    })
}

fn command_stdout(program: &str, arguments: &[&str]) -> Result<String> {
    let output = command_output(program, arguments)?;
    let value = output.trim();
    if value.is_empty() {
        return Err(format!("`{program}` returned empty provenance").into());
    }
    Ok(value.to_owned())
}

fn command_output(program: &str, arguments: &[&str]) -> Result<String> {
    let output = oer_process::command(program)
        .args(arguments)
        .supervised_output()?;
    if !output.status.success() {
        return Err(format!("cannot capture host provenance with `{program}`").into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

fn read_trimmed(path: impl AsRef<Path>) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(Some(value.trim().to_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn os_release() -> Option<String> {
    let source = fs::read_to_string("/etc/os-release").ok()?;
    source.lines().find_map(|line| {
        line.strip_prefix("PRETTY_NAME=")
            .map(|value| value.trim_matches('"').to_owned())
    })
}

#[cfg(test)]
mod tests;

/// The run bundle's spelling of the PHYs a fixture offers.
fn recorded_phys(phys: &[oer_hil_scenario_catalog::link::PhyExpectation]) -> Vec<Phy> {
    use oer_hil_scenario_catalog::link::PhyExpectation;
    phys.iter()
        .map(|phy| match phy {
            PhyExpectation::Legacy => Phy::Legacy,
            PhyExpectation::He20 => Phy::He20,
            PhyExpectation::Ht20 => Phy::Ht20,
            PhyExpectation::Ht40 => Phy::Ht40,
        })
        .collect()
}
