//! Synthetic service profiles for hardware-independent facade tests.

use oer_ieee80211_softmac::{
    MacInterfaceCapabilities, MacOperationOwner, MacOperationOwnership, MacResourceLimits,
    MacServiceCapabilities,
};

pub const TEST_CAPABILITIES: MacServiceCapabilities = MacServiceCapabilities {
    interfaces: MacInterfaceCapabilities {
        station_interfaces: 1,
        access_point_interfaces: 0,
        simultaneous_station_access_point: false,
        standalone_monitor: true,
        monitor_with_interfaces: false,
        raw_monitor_tap: false,
        normalized_monitor_tap: true,
        protocol_validated_monitor_tap: false,
    },
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
        access_point_pairwise_ccmp_slots: 0,
        access_point_group_ccmp_slots: 0,
        access_point_association_entries: 0,
        access_point_encrypted_clients: 0,
    },
};

pub const AP_TEST_CAPABILITIES: MacServiceCapabilities = MacServiceCapabilities {
    interfaces: MacInterfaceCapabilities {
        station_interfaces: 1,
        access_point_interfaces: 1,
        simultaneous_station_access_point: false,
        standalone_monitor: true,
        monitor_with_interfaces: false,
        raw_monitor_tap: false,
        normalized_monitor_tap: true,
        protocol_validated_monitor_tap: false,
    },
    resources: MacResourceLimits {
        access_point_pairwise_ccmp_slots: 15,
        access_point_group_ccmp_slots: 1,
        access_point_association_entries: 15,
        access_point_encrypted_clients: 15,
        ..TEST_CAPABILITIES.resources
    },
    ..TEST_CAPABILITIES
};

pub const STA_AP_TEST_CAPABILITIES: MacServiceCapabilities = MacServiceCapabilities {
    interfaces: MacInterfaceCapabilities {
        simultaneous_station_access_point: true,
        ..AP_TEST_CAPABILITIES.interfaces
    },
    ..AP_TEST_CAPABILITIES
};
