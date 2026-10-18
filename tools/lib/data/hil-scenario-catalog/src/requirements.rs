//! Execution prerequisites derived from workload behavior, never scenario names.
//!
//! Each scenario family computes its requirements; this type only names the
//! shared laboratory services and combines them.

use serde::{Deserialize, Serialize};

/// Serial ownership and firmware are common to every scenario. These are the
/// additional fixture services consumed by the selected workload and evidence.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Requirements {
    pub station_network: bool,
    /// The IEEE 802.15.4 reference peer.
    #[serde(default)]
    pub peer: bool,
    pub station_control: bool,
    pub station_udp_rx_capture: bool,
    pub station_udp_tx_capture: bool,
    pub laptop_client: bool,
    pub openwrt_client: bool,
    pub openwrt_tx_monitor: bool,
    pub laptop_air_monitor: bool,
    #[serde(default)]
    pub probe_load: bool,
    /// The laptop joins the station fixture AP as a non-HT member.
    #[serde(default)]
    pub non_ht_member: bool,
    /// The laptop hosts a non-ERP BSS on the station fixture's channel.
    #[serde(default)]
    pub legacy_bss: bool,
    /// The dedicated independent air observer.
    #[serde(default)]
    pub air_observer: bool,
}

impl Requirements {
    /// The fixture services a set of scenarios needs together.
    pub fn union(requirements: impl IntoIterator<Item = Self>) -> Self {
        requirements
            .into_iter()
            .fold(Self::default(), |required, next| Self {
                station_network: required.station_network | next.station_network,
                peer: required.peer | next.peer,
                station_control: required.station_control | next.station_control,
                station_udp_rx_capture: required.station_udp_rx_capture
                    | next.station_udp_rx_capture,
                station_udp_tx_capture: required.station_udp_tx_capture
                    | next.station_udp_tx_capture,
                laptop_client: required.laptop_client | next.laptop_client,
                openwrt_client: required.openwrt_client | next.openwrt_client,
                openwrt_tx_monitor: required.openwrt_tx_monitor | next.openwrt_tx_monitor,
                laptop_air_monitor: required.laptop_air_monitor | next.laptop_air_monitor,
                probe_load: required.probe_load | next.probe_load,
                non_ht_member: required.non_ht_member | next.non_ht_member,
                legacy_bss: required.legacy_bss | next.legacy_bss,
                air_observer: required.air_observer | next.air_observer,
            })
    }

    pub fn network(self) -> bool {
        self.station_network
            || self.station_control
            || self.station_udp_rx_capture
            || self.station_udp_tx_capture
            || self.laptop_client
            || self.openwrt_client
            || self.openwrt_tx_monitor
            || self.laptop_air_monitor
            || self.non_ht_member
            || self.legacy_bss
            || self.air_observer
    }

    pub fn local_radio(self) -> bool {
        self.laptop_client || self.laptop_air_monitor || self.non_ht_member || self.legacy_bss
    }
}

#[cfg(test)]
mod tests;
