use super::*;
use oer_protocols_ieee802_wire::IndividualAddressError;
use oer_protocols_ieee80211_contracts_port::{
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
            rx_block_ack_max_tid: oer_protocols_ieee80211_wire::qos::Tid::constant::<7>(),
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
    standalone_monitor: oer_protocols_ieee80211_contracts_port::Support::Supported,
    monitor_with_interfaces: oer_protocols_ieee80211_contracts_port::Support::Supported,
    raw_monitor_tap: oer_protocols_ieee80211_contracts_port::Support::Unsupported,
    normalized_monitor_tap: oer_protocols_ieee80211_contracts_port::Support::Supported,
    protocol_validated_monitor_tap: oer_protocols_ieee80211_contracts_port::Support::Unsupported,
};

fn address(last: u8) -> IndividualAddress {
    IndividualAddress::for_interface(Eui48::new([0x02, 0, 0, 0, 0, last])).unwrap()
}

#[test]
fn rejects_unspecified_and_multicast_addresses() {
    assert_eq!(
        IndividualAddress::for_interface(Eui48::new([0; 6])),
        Err(IndividualAddressError::Unspecified)
    );
    assert_eq!(
        IndividualAddress::for_interface(Eui48::new([1, 0, 0, 0, 0, 1])),
        Err(IndividualAddressError::Multicast)
    );
}

#[test]
fn a_general_zero_address_cannot_enter_either_interface_plan() {
    let zero = IndividualAddress::new(Eui48::new([0; 6])).unwrap();
    for config in [
        WifiConfig::station(WifiStationConfig::new(zero)),
        WifiConfig::access_point(WifiAccessPointConfig::new(zero)),
    ] {
        assert_eq!(
            config.validate(capabilities(ALL_INTERFACES)),
            Err(WifiConfigError::UnspecifiedInterfaceAddress)
        );
    }
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
fn rejects_taps_not_owned_by_the_complete_service() {
    let without_taps = MacInterfaceCapabilities {
        standalone_monitor: oer_protocols_ieee80211_contracts_port::Support::Unsupported,
        monitor_with_interfaces: oer_protocols_ieee80211_contracts_port::Support::Unsupported,
        raw_monitor_tap: oer_protocols_ieee80211_contracts_port::Support::Unsupported,
        normalized_monitor_tap: oer_protocols_ieee80211_contracts_port::Support::Unsupported,
        protocol_validated_monitor_tap:
            oer_protocols_ieee80211_contracts_port::Support::Unsupported,
    };
    assert_eq!(
        WifiConfig::station(WifiStationConfig::new(address(1)))
            .with_monitor(WifiMonitorConfig::normalized())
            .validate(capabilities(without_taps)),
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
        standalone_monitor: oer_protocols_ieee80211_contracts_port::Support::Supported,
        monitor_with_interfaces: oer_protocols_ieee80211_contracts_port::Support::Unsupported,
        raw_monitor_tap: oer_protocols_ieee80211_contracts_port::Support::Unsupported,
        normalized_monitor_tap: oer_protocols_ieee80211_contracts_port::Support::Supported,
        protocol_validated_monitor_tap:
            oer_protocols_ieee80211_contracts_port::Support::Unsupported,
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
