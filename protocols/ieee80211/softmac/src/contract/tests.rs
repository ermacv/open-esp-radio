use super::*;

const CAPABILITIES: MacServiceCapabilities = MacServiceCapabilities {
    interfaces: MacInterfaceCapabilities {
        station_interfaces: 1,
        access_point_interfaces: 0,
        simultaneous_station_access_point: false,
        standalone_monitor: false,
        monitor_with_interfaces: false,
        raw_monitor_tap: false,
        normalized_monitor_tap: false,
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
        access_point_pairwise_ccmp_slots: 1,
        access_point_group_ccmp_slots: 1,
        access_point_association_entries: 1,
        access_point_encrypted_clients: 1,
    },
};

#[test]
fn zero_and_oversized_block_ack_windows_are_not_supported() {
    assert!(!CAPABILITIES.supports_rx_block_ack_window(0));
    assert!(CAPABILITIES.supports_rx_block_ack_window(64));
    assert!(!CAPABILITIES.supports_rx_block_ack_window(65));
    assert!(CAPABILITIES.supports_tx_block_ack_window(32));
    assert!(!CAPABILITIES.supports_tx_block_ack_window(33));
}

#[test]
fn implemented_roles_and_monitor_taps_are_explicit() {
    assert!(
        CAPABILITIES
            .interfaces
            .supports_role(interface::VifRole::Station)
    );
    assert!(
        !CAPABILITIES
            .interfaces
            .supports_role(interface::VifRole::AccessPoint)
    );
    assert!(
        !CAPABILITIES
            .interfaces
            .supports_monitor_tap(interface::MonitorTapPoint::Raw)
    );
}

#[test]
fn terminal_status_distinguishes_an_exchange_from_one_attempt() {
    let status = MacTxStatus {
        result: MacTxResult::Transmitted,
        attempts: 3,
        final_rate: 7_u8,
        acknowledged: Some(true),
        ack_snr_db: Some(18),
        airtime_micros: None,
    };
    assert_eq!(status.attempts, 3);
    assert_eq!(status.result, MacTxResult::Transmitted);
}

#[test]
fn tx_plan_contains_protocol_policy_but_no_hardware_queue_encoding() {
    let plan = MacTxPlan {
        access_category: WmmAccessCategory::Video,
        initial_rate: 7_u8,
        publication_limit: 4,
        publication_timeout: oer_time::Duration::from_micros(250_000),
    };
    assert_eq!(plan.access_category, WmmAccessCategory::Video);
    assert_eq!(plan.initial_rate, 7);
    assert_eq!(plan.publication_limit, 4);
}

#[test]
fn receive_metadata_keeps_absence_and_provenance_distinct() {
    let staged = MacRxMetadata {
        channel: MacRxEvidence::HardwareObserved(
            Channel::ghz2_4(6, oer_ieee80211_mac::channel::ChannelWidth::Mhz20).unwrap(),
        ),
        rate: MacRxEvidence::HardwareObserved(11_u8),
        rssi_dbm: MacRxEvidence::HardwareObserved(-47),
        crypto: MacRxEvidence::Unavailable,
        s_mpdu: MacRxEvidence::Unavailable,
        ampdu: MacRxEvidence::Unavailable,
        amsdu: MacRxEvidence::Unavailable,
    };
    assert!(staged.channel.is_available());
    assert!(!staged.crypto.is_available());

    let validated = MacRxMetadata {
        crypto: MacRxEvidence::ProtocolValidated(MacRxCryptoStatus::DecryptedAndIntegrityVerified),
        s_mpdu: MacRxEvidence::HardwareObserved(true),
        amsdu: MacRxEvidence::ProtocolValidated(false),
        ..staged
    };
    assert_ne!(validated.crypto, staged.crypto);
    assert_eq!(
        validated.s_mpdu.as_ref(),
        MacRxEvidence::HardwareObserved(&true)
    );
    assert_eq!(validated.ampdu, MacRxEvidence::Unavailable);
    assert_eq!(validated.amsdu, MacRxEvidence::ProtocolValidated(false));
}

#[test]
fn ampdu_status_joins_block_ack_and_individual_retries() {
    let status = MacAmpduTxStatus {
        result: MacAmpduTxResult::Delivered,
        original_subframes: 3,
        aggregate_attempts: 2,
        aggregate_rate: 7_u8,
        block_acknowledged_subframes: 2,
        individual_retries: MacIndividualRetries::NONE,
        ack_snr_db: None,
    };
    assert_eq!(status.delivered_subframes(), 2);
    assert!(!status.fully_delivered());

    let mut delivered = MacAmpduTxStatus {
        original_subframes: 4,
        ..status
    };
    for attempts in [2, 1] {
        delivered.individual_retries.record(MacTxStatus {
            result: MacTxResult::Transmitted,
            attempts,
            final_rate: 5,
            acknowledged: Some(true),
            ack_snr_db: Some(12),
            airtime_micros: None,
        });
    }
    assert_eq!(delivered.delivered_subframes(), 4);
    assert_eq!(delivered.total_publication_attempts(), 5);
    assert_eq!(delivered.individual_retries.final_rate, Some(5));
    assert!(delivered.fully_delivered());
}

#[test]
fn hardware_owned_operations_become_the_port_services() {
    use oer_ieee80211_lower_mac::HardwareServices;

    let services = CAPABILITIES.operations.hardware_services();
    assert!(
        services.contains(
            HardwareServices::FCS
                .union(HardwareServices::IMMEDIATE_ACK)
                .union(HardwareServices::BACKOFF_COUNTDOWN)
        )
    );
    assert!(!services.contains(HardwareServices::SEQUENCE_NUMBERS));
    let everything = MacOperationOwnership {
        tx_fcs_generation: MacOperationOwner::Hardware,
        immediate_ack_response: MacOperationOwner::Hardware,
        csma_ca_backoff_countdown: MacOperationOwner::Hardware,
        unicast_retry_policy: MacOperationOwner::Hardware,
        tx_sequence_assignment: MacOperationOwner::Hardware,
        ccmp_key_selection: MacOperationOwner::Hardware,
        ccmp_packet_number: MacOperationOwner::Hardware,
        ccmp_transform: MacOperationOwner::Hardware,
        rx_block_ack_matching: MacOperationOwner::Hardware,
        rx_reorder: MacOperationOwner::Hardware,
        tx_block_ack_capture: MacOperationOwner::Hardware,
        tx_ampdu_retry_selection: MacOperationOwner::Hardware,
    };
    let nothing = MacOperationOwnership {
        tx_fcs_generation: MacOperationOwner::Software,
        immediate_ack_response: MacOperationOwner::Unsupported,
        csma_ca_backoff_countdown: MacOperationOwner::Software,
        unicast_retry_policy: MacOperationOwner::Software,
        tx_sequence_assignment: MacOperationOwner::Software,
        ccmp_key_selection: MacOperationOwner::Software,
        ccmp_packet_number: MacOperationOwner::Software,
        ccmp_transform: MacOperationOwner::Software,
        rx_block_ack_matching: MacOperationOwner::Software,
        rx_reorder: MacOperationOwner::Software,
        tx_block_ack_capture: MacOperationOwner::Software,
        tx_ampdu_retry_selection: MacOperationOwner::Software,
    };
    assert_eq!(nothing.hardware_services(), HardwareServices::NONE);
    assert!(
        everything.hardware_services().contains(
            HardwareServices::RX_REORDER
                .union(HardwareServices::AMPDU_RETRY_SELECTION)
                .union(HardwareServices::KEY_SELECTION)
                .union(HardwareServices::CIPHER_TRANSFORM)
                .union(HardwareServices::RX_BLOCK_ACK_MATCHING)
                .union(HardwareServices::TX_BLOCK_ACK_CAPTURE)
        )
    );
}

#[test]
fn a_chip_rate_maps_to_the_portable_rate_with_its_provenance() {
    use oer_ieee80211_mac::phy::LegacyRate;

    let chip = MacRxMetadata {
        rate: MacRxEvidence::HardwareObserved(0x0b_u8),
        rssi_dbm: MacRxEvidence::HardwareObserved(-50),
        ..MacRxMetadata::unavailable()
    };
    let decode = |code| (code == 0x0b).then_some(PhyRate::Legacy(LegacyRate::Ofdm6M));
    let portable: MacRxMetadata = chip.map_rate(decode);
    assert_eq!(
        portable.rate,
        MacRxEvidence::HardwareObserved(PhyRate::Legacy(LegacyRate::Ofdm6M))
    );
    assert_eq!(portable.rssi_dbm, chip.rssi_dbm);
    let unknown = MacRxMetadata {
        rate: MacRxEvidence::HardwareObserved(0x1f_u8),
        ..chip
    };
    assert_eq!(unknown.map_rate(decode).rate, MacRxEvidence::Unavailable);
}
