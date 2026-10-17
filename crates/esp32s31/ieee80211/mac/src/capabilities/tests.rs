use super::*;

#[test]
fn profile_exposes_the_real_split_mac_boundaries() {
    let operations = ESP32S31_MAC_SERVICE_CAPABILITIES.operations;
    assert_eq!(
        operations.immediate_ack_response,
        MacOperationOwner::Hardware
    );
    assert_eq!(operations.unicast_retry_policy, MacOperationOwner::Software);
    assert_eq!(
        operations.tx_sequence_assignment,
        MacOperationOwner::Software
    );
    assert_eq!(operations.ccmp_transform, MacOperationOwner::Hardware);
    assert_eq!(operations.rx_reorder, MacOperationOwner::Software);
    assert_eq!(operations.tx_block_ack_capture, MacOperationOwner::Hardware);
    assert_eq!(
        operations.tx_ampdu_retry_selection,
        MacOperationOwner::Software
    );
}

#[test]
fn profile_limits_are_derived_from_the_owners_that_enforce_them() {
    let resources = ESP32S31_MAC_SERVICE_CAPABILITIES.resources;
    assert_eq!(resources.ordinary_tx_queues, 4);
    assert_eq!(
        resources.rx_block_ack_entries as usize,
        RX_BLOCK_ACK_BANK_COUNT
    );
    assert_eq!(resources.rx_block_ack_max_window, RX_BLOCK_ACK_MAX_WINDOW);
    assert_eq!(resources.tx_block_ack_max_window, TX_BLOCK_ACK_MAX_WINDOW);
    assert_eq!(
        resources.tx_ampdu_max_subframes as usize,
        TX_AMPDU_SLOT_CAPACITY
    );
    assert_eq!(resources.access_point_pairwise_ccmp_slots, 15);
    assert_eq!(resources.access_point_association_entries, 15);
    assert_eq!(resources.access_point_encrypted_clients, 15);
}

#[test]
fn profile_advertises_only_materialized_interface_and_monitor_topologies() {
    let interfaces = ESP32S31_MAC_SERVICE_CAPABILITIES.interfaces;
    assert_eq!(interfaces.station_interfaces, 1);
    assert_eq!(interfaces.access_point_interfaces, 1);
    assert!(interfaces.simultaneous_station_access_point);
    assert!(interfaces.standalone_monitor);
    assert!(!interfaces.monitor_with_interfaces);
    assert!(!interfaces.raw_monitor_tap);
    assert!(interfaces.normalized_monitor_tap);
    assert!(!interfaces.protocol_validated_monitor_tap);
}
