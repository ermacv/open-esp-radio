//! The record of the physical HIL cell a run observed before it started:
//! its secret-free definition, the host and the station fixture.

use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

use crate::Result;

pub const LAB_PROVENANCE_SCHEMA: u16 = 2;

/// The channel width a recorded access point runs, as the record spells it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChannelWidth {
    Mhz20,
    Mhz40Above,
    Mhz40Below,
}

/// A PHY a recorded station fixture offers, as the record spells it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phy {
    /// A non-HT BSS: no HT or HE elements.
    Legacy,
    He20,
    Ht20,
    Ht40,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LabProvenance {
    pub schema: u16,
    pub captured_unix_millis: u64,
    pub definition: LabDefinition,
    pub host: HostObservation,
    pub fixture: FixtureObservation,
    pub scope: ObservationScope,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ObservationScope {
    System,
    Network,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LabDefinition {
    pub cell_id: String,
    pub device_id: String,
    pub bluetooth_adapter: Option<String>,
    /// Stable identity of the reference peer board.
    pub peer: Option<String>,
    pub station_ipv4: StationIpv4Definition,
    pub access_point: AccessPointDefinition,
    pub station_fixture: StationFixtureDefinition,
    /// Network names, credentials and SSH endpoints are deliberately absent.
    pub sensitive_network_values: SensitiveValueDisposition,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SensitiveValueDisposition {
    Omitted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum StationIpv4Definition {
    Dhcp,
    Static {
        address: Ipv4Addr,
        prefix_length: u8,
        gateway: Option<Ipv4Addr>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccessPointDefinition {
    pub channel: u8,
    pub channel_width: ChannelWidth,
    pub client_limit: u8,
    pub target_address: Ipv4Addr,
    pub client_address: Ipv4Addr,
    pub secondary_client_address: Option<Ipv4Addr>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum StationFixtureDefinition {
    LocalLinux {
        interface: String,
        phys: Vec<Phy>,
    },
    OpenWrt {
        wireless_interface: String,
        ingress_interface: String,
        monitor_interface: Option<String>,
        phys: Vec<Phy>,
        independent_laptop_monitor: bool,
    },
    External {
        phys: Vec<Phy>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostObservation {
    pub kernel_release: String,
    pub machine: String,
    pub os_release: Option<String>,
    pub boot_id: Option<String>,
    pub interfaces: Vec<HostInterfaceObservation>,
    pub ipv4_routes: Vec<HostIpv4Route>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostInterfaceObservation {
    pub name: String,
    pub operstate: String,
    pub mac_address: Option<String>,
    pub master: Option<String>,
    pub wireless: bool,
    pub ipv4_addresses: Vec<String>,
    pub wireless_link: Option<HostWirelessLink>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostWirelessLink {
    pub interface_type: Option<String>,
    pub connected: bool,
    pub bssid: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostIpv4Route {
    pub destination: String,
    pub interface: Option<String>,
    pub gateway: Option<Ipv4Addr>,
    pub preferred_source: Option<Ipv4Addr>,
    pub metric: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum FixtureObservation {
    NotUsed,
    LocalLinux,
    OpenWrt(Box<OpenWrtObservation>),
    OpenWrtInactive { wireless_interface: String },
    External { managed: bool },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OpenWrtObservation {
    pub release: String,
    pub revision: String,
    pub kernel_release: String,
    pub machine: String,
    pub boot_id: String,
    pub wireless_interface: String,
    pub ingress_interface: String,
    pub ingress_operstate: String,
    pub ingress_ipv4_addresses: Vec<String>,
    pub wiphy: u32,
    pub interface_type: String,
    pub channel: u8,
    pub frequency_mhz: u16,
    pub width_mhz: u16,
    pub center1_mhz: u16,
    pub tx_power_milli_dbm: i32,
    pub country: Option<String>,
    pub driver: String,
    pub firmware: Option<String>,
    pub associated_stations: u16,
    pub concurrent_interfaces: Vec<OpenWrtInterface>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OpenWrtInterface {
    pub name: String,
    pub interface_type: String,
}

impl LabProvenance {
    pub fn validate_binding(
        &self,
        cell_id: &str,
        device_id: &str,
        run_started_unix_millis: u64,
        run_finished_unix_millis: Option<u64>,
    ) -> Result<()> {
        if self.schema != LAB_PROVENANCE_SCHEMA
            || self.definition.cell_id != cell_id
            || self.definition.device_id != device_id
            || self.captured_unix_millis < run_started_unix_millis
            || run_finished_unix_millis.is_some_and(|finished| self.captured_unix_millis > finished)
        {
            return Err("lab provenance is not bound to its containing HIL run".into());
        }
        let mut host_interfaces = self
            .host
            .interfaces
            .iter()
            .map(|interface| interface.name.as_str())
            .collect::<Vec<_>>();
        host_interfaces.sort_unstable();
        if host_interfaces.is_empty()
            || host_interfaces.windows(2).any(|pair| pair[0] == pair[1])
            || self.host.kernel_release.is_empty()
            || self.host.machine.is_empty()
        {
            return Err("lab provenance has an invalid host observation".into());
        }
        match (&self.definition.station_fixture, &self.fixture) {
            (_, FixtureObservation::NotUsed) if self.scope == ObservationScope::System => {}
            (
                StationFixtureDefinition::LocalLinux { interface, .. },
                FixtureObservation::LocalLinux,
            ) if self.host.interfaces.iter().any(|entry| {
                entry.name == *interface && entry.wireless && entry.wireless_link.is_some()
            }) => {}
            (
                StationFixtureDefinition::OpenWrt {
                    wireless_interface,
                    ingress_interface,
                    ..
                },
                FixtureObservation::OpenWrt(observation),
            ) => {
                if observation.wireless_interface != *wireless_interface
                    || observation.ingress_interface != *ingress_interface
                    || !observation.concurrent_interfaces.iter().any(|interface| {
                        interface.name == *wireless_interface
                            && interface.interface_type == observation.interface_type
                    })
                {
                    return Err("lab provenance has an inconsistent OpenWrt observation".into());
                }
            }
            (
                StationFixtureDefinition::OpenWrt {
                    wireless_interface, ..
                },
                FixtureObservation::OpenWrtInactive {
                    wireless_interface: observed,
                },
            ) if wireless_interface == observed => {}
            (
                StationFixtureDefinition::External { .. },
                FixtureObservation::External { managed: false },
            ) => {}
            _ => return Err("lab provenance fixture definition and observation disagree".into()),
        }
        if (self.scope == ObservationScope::System)
            != matches!(self.fixture, FixtureObservation::NotUsed)
        {
            return Err("lab provenance scope and fixture observation disagree".into());
        }
        Ok(())
    }
}
