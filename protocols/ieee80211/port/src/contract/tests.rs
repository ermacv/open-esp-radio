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
            .supports_role(crate::VifRole::Station)
    );
    assert!(
        !CAPABILITIES
            .interfaces
            .supports_role(crate::VifRole::AccessPoint)
    );
    assert!(
        !CAPABILITIES
            .interfaces
            .supports_monitor_tap(crate::MonitorTapPoint::Raw)
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
fn receive_metadata_keeps_absence_and_provenance_distinct() {
    let staged = MacRxMetadata {
        channel: RxEvidence::HardwareObserved(
            Channel::ghz2_4(6, oer_ieee80211_wire::channel::ChannelWidth::Mhz20).unwrap(),
        ),
        rate: RxEvidence::HardwareObserved(11_u8),
        rssi_dbm: RxEvidence::HardwareObserved(-47),
        crypto: RxEvidence::Unavailable,
        s_mpdu: RxEvidence::Unavailable,
        ampdu: RxEvidence::Unavailable,
        amsdu: RxEvidence::Unavailable,
    };
    assert!(staged.channel.is_available());
    assert!(!staged.crypto.is_available());

    let validated = MacRxMetadata {
        crypto: RxEvidence::ProtocolValidated(RxCryptoStatus::DecryptedAndIntegrityVerified),
        s_mpdu: RxEvidence::HardwareObserved(true),
        amsdu: RxEvidence::ProtocolValidated(false),
        ..staged
    };
    assert_ne!(validated.crypto, staged.crypto);
    assert_eq!(
        validated.s_mpdu.as_ref(),
        RxEvidence::HardwareObserved(&true)
    );
    assert_eq!(validated.ampdu, RxEvidence::Unavailable);
    assert_eq!(validated.amsdu, RxEvidence::ProtocolValidated(false));
}

#[test]
fn hardware_owned_operations_become_the_port_services() {
    use crate::HardwareServices;

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
    use oer_ieee80211_wire::phy::LegacyRate;

    let chip = MacRxMetadata {
        rate: RxEvidence::HardwareObserved(0x0b_u8),
        rssi_dbm: RxEvidence::HardwareObserved(-50),
        ..MacRxMetadata::unavailable()
    };
    let decode = |code| (code == 0x0b).then_some(PhyRate::Legacy(LegacyRate::Ofdm6M));
    let portable: MacRxMetadata = chip.map_rate(decode);
    assert_eq!(
        portable.rate,
        RxEvidence::HardwareObserved(PhyRate::Legacy(LegacyRate::Ofdm6M))
    );
    assert_eq!(portable.rssi_dbm, chip.rssi_dbm);
    let unknown = MacRxMetadata {
        rate: RxEvidence::HardwareObserved(0x1f_u8),
        ..chip
    };
    assert_eq!(unknown.map_rate(decode).rate, RxEvidence::Unavailable);
}
