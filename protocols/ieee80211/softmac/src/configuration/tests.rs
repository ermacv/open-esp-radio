use super::*;
use crate::{
    MacInterfaceCapabilities, MacOperationOwner, MacOperationOwnership, MacResourceLimits,
};

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

fn address(last: u8) -> WifiMacAddress {
    WifiMacAddress::new([0x02, 0, 0, 0, 0, last]).unwrap()
}

#[test]
fn rejects_unspecified_and_multicast_addresses() {
    assert_eq!(
        WifiMacAddress::new([0; 6]),
        Err(WifiMacAddressError::Unspecified)
    );
    assert_eq!(
        WifiMacAddress::new([1, 0, 0, 0, 0, 1]),
        Err(WifiMacAddressError::Multicast)
    );
}

#[test]
fn assigns_distinct_vifs_to_station_and_ap_on_one_channel_context() {
    let plan = WifiConfig::station_access_point(
        WifiStationConfig::new(address(1)),
        WifiAccessPointConfig::new(address(2)),
    )
    .with_monitor(WifiMonitorConfig::normalized())
    .validate(capabilities(ALL_INTERFACES))
    .unwrap();

    let station = plan.station().unwrap();
    let access_point = plan.access_point().unwrap();
    assert_eq!(station.interface.id, VifId::PRIMARY);
    assert_eq!(access_point.interface.id, VifId::new(1));
    assert_eq!(station.channel_context, access_point.channel_context);
    assert_eq!(
        plan.monitor_channel_context(),
        Some(ChannelContextId::PRIMARY)
    );
    assert!(plan.standalone_monitor().is_none());
}

#[test]
fn standalone_monitor_plan_proves_exclusive_rx_ownership() {
    let plan = WifiConfig::monitor(WifiMonitorConfig::normalized())
        .validate(capabilities(ALL_INTERFACES))
        .unwrap()
        .standalone_monitor()
        .unwrap();
    assert_eq!(plan.monitor(), WifiMonitorConfig::normalized());
    assert_eq!(plan.channel_context(), ChannelContextId::PRIMARY);
}

#[test]
fn rejects_roles_and_taps_not_owned_by_the_complete_service() {
    let station_only = MacInterfaceCapabilities {
        station_interfaces: 1,
        access_point_interfaces: 0,
        simultaneous_station_access_point: false,
        standalone_monitor: false,
        monitor_with_interfaces: false,
        raw_monitor_tap: false,
        normalized_monitor_tap: false,
        protocol_validated_monitor_tap: false,
    };
    assert_eq!(
        WifiConfig::access_point(WifiAccessPointConfig::new(address(2)))
            .validate(capabilities(station_only)),
        Err(WifiConfigError::UnsupportedAccessPoint)
    );
    assert_eq!(
        WifiConfig::station(WifiStationConfig::new(address(1)))
            .with_monitor(WifiMonitorConfig::normalized())
            .validate(capabilities(station_only)),
        Err(WifiConfigError::UnsupportedMonitorTap(
            MonitorTapPoint::Normalized
        ))
    );
}

#[test]
fn rejects_duplicate_station_and_ap_addresses() {
    let shared = address(1);
    assert_eq!(
        WifiConfig::station_access_point(
            WifiStationConfig::new(shared),
            WifiAccessPointConfig::new(shared),
        )
        .validate(capabilities(ALL_INTERFACES)),
        Err(WifiConfigError::DuplicateInterfaceAddress)
    );
}

#[test]
fn distinguishes_standalone_monitor_from_concurrent_interface_tap() {
    let standalone_only = MacInterfaceCapabilities {
        station_interfaces: 1,
        access_point_interfaces: 0,
        simultaneous_station_access_point: false,
        standalone_monitor: true,
        monitor_with_interfaces: false,
        raw_monitor_tap: false,
        normalized_monitor_tap: true,
        protocol_validated_monitor_tap: false,
    };
    assert!(
        WifiConfig::monitor(WifiMonitorConfig::normalized())
            .validate(capabilities(standalone_only))
            .is_ok()
    );
    assert_eq!(
        WifiConfig::station(WifiStationConfig::new(address(1)))
            .with_monitor(WifiMonitorConfig::normalized())
            .validate(capabilities(standalone_only)),
        Err(WifiConfigError::UnsupportedMonitorWithInterfaces)
    );
}
