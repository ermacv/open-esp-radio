use super::*;
use oer_ieee80211_contracts_port::{
    MacInterfaceCapabilities, MacOperationOwner, MacOperationOwnership, MacResourceLimits,
    MacServiceCapabilities,
};
use oer_ieee80211_core_arbiter::configuration::{
    WifiAccessPointConfig, WifiConfig, WifiMonitorConfig, WifiStationConfig,
};
use oer_ieee80211_wire::address::IndividualAddress;

const fn capabilities(interfaces: MacInterfaceCapabilities) -> MacServiceCapabilities {
    MacServiceCapabilities {
        interfaces,
        operations: MacOperationOwnership {
            tx_fcs_generation: MacOperationOwner::Hardware,
            immediate_ack_response: MacOperationOwner::Hardware,
            csma_ca_backoff_countdown: MacOperationOwner::Hardware,
            unicast_retry_policy: MacOperationOwner::Software,
            tx_sequence_assignment: MacOperationOwner::Software,
            ccmp_key_selection: MacOperationOwner::Software,
            ccmp_packet_number: MacOperationOwner::Software,
            ccmp_transform: MacOperationOwner::Hardware,
            rx_block_ack_matching: MacOperationOwner::Hardware,
            rx_reorder: MacOperationOwner::Software,
            tx_block_ack_capture: MacOperationOwner::Hardware,
            tx_ampdu_retry_selection: MacOperationOwner::Software,
        },
        resources: MacResourceLimits {
            channel_contexts: 1,
            ordinary_tx_queues: 4,
            rx_block_ack_entries: 8,
            rx_block_ack_max_tid: 7,
            rx_block_ack_max_window: 64,
            tx_block_ack_max_window: 32,
            tx_ampdu_max_subframes: 32,
            station_pairwise_ccmp_slots: 1,
            station_group_ccmp_slots: 1,
            access_point_pairwise_ccmp_slots: 1,
            access_point_group_ccmp_slots: 1,
            access_point_association_entries: 1,
            access_point_encrypted_clients: 1,
        },
    }
}

const ALL_INTERFACES: MacInterfaceCapabilities = MacInterfaceCapabilities {
    station_interfaces: 1,
    access_point_interfaces: 1,
    simultaneous_station_access_point: true,
    standalone_monitor: true,
    monitor_with_interfaces: true,
    raw_monitor_tap: false,
    normalized_monitor_tap: true,
    protocol_validated_monitor_tap: false,
};

fn address(last: u8) -> IndividualAddress {
    IndividualAddress::for_interface([0x02, 0, 0, 0, 0, last]).unwrap()
}

#[test]
fn standalone_esp_now_preserves_the_checked_station_binding() {
    let plan = WifiConfig::station(WifiStationConfig::new(address(1)))
        .validate(capabilities(ALL_INTERFACES))
        .unwrap();
    let standalone = WifiStandaloneEspNowPlan::new(plan).unwrap();
    assert_eq!(Some(standalone.station()), plan.station());
    assert_eq!(standalone.channel_context(), ChannelContextId::PRIMARY);
}

#[test]
fn standalone_esp_now_refuses_every_topology_with_an_ap_or_monitor() {
    let station = WifiStationConfig::new(address(1));
    let access_point = WifiAccessPointConfig::new(address(2));
    let monitor = WifiMonitorConfig::normalized();
    for config in [
        WifiConfig::access_point(access_point),
        WifiConfig::monitor(monitor),
        WifiConfig::station_access_point(station, access_point),
        WifiConfig::station(station).with_monitor(monitor),
        WifiConfig::access_point(access_point).with_monitor(monitor),
        WifiConfig::station_access_point(station, access_point).with_monitor(monitor),
    ] {
        let plan = config.validate(capabilities(ALL_INTERFACES)).unwrap();
        assert!(WifiStandaloneEspNowPlan::new(plan).is_none());
    }
}
