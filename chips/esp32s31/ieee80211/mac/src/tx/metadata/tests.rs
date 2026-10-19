use super::*;

#[test]
fn queue_class_and_descriptor_byte_match_every_priority() {
    let expected = [
        (2, 0x20),
        (3, 0x31),
        (3, 0x32),
        (2, 0x23),
        (1, 0x14),
        (1, 0x15),
        (0, 0x06),
        (0, 0x07),
    ];
    for (priority, (class, descriptor)) in expected.into_iter().enumerate() {
        assert_eq!(queue_class(priority as u8), Some(class));
        assert_eq!(descriptor_priority_byte(priority as u8), Some(descriptor));
    }
}

#[test]
fn portable_data_intent_preserves_every_existing_s31_queue_and_packet_profile() {
    use oer_ieee80211_mac::data::plan_data_encapsulation;

    // Existing queue and packet-type profiles, ordered by IEEE 802.1D UP.
    // ACI discriminants have a different order and cannot be cast to queue IDs.
    let priorities = [
        (WmmAccessCategory::BestEffort, 2, 12),
        (WmmAccessCategory::Background, 3, 13),
        (WmmAccessCategory::Background, 3, 13),
        (WmmAccessCategory::BestEffort, 2, 12),
        (WmmAccessCategory::Video, 1, 11),
        (WmmAccessCategory::Video, 1, 11),
        (WmmAccessCategory::Voice, 0, 10),
        (WmmAccessCategory::Voice, 0, 10),
    ];
    let frames = [
        (DataInterfaceRole::Station, false, false, [0x08, 0x01], 24),
        (DataInterfaceRole::Station, true, false, [0x08, 0x01], 24),
        (DataInterfaceRole::Station, false, true, [0x88, 0x01], 26),
        (DataInterfaceRole::Station, true, true, [0x88, 0x01], 26),
        (
            DataInterfaceRole::AccessPoint,
            false,
            false,
            [0x08, 0x02],
            24,
        ),
        (
            DataInterfaceRole::AccessPoint,
            true,
            false,
            [0x08, 0x02],
            24,
        ),
        (
            DataInterfaceRole::AccessPoint,
            false,
            true,
            [0x88, 0x02],
            26,
        ),
        (DataInterfaceRole::AccessPoint, true, true, [0x08, 0x02], 24),
    ];

    for (priority, (category, queue, qos_packet_type)) in priorities.into_iter().enumerate() {
        for (role, multicast, peer_qos, frame_control, header_len) in frames {
            let mut ethernet = [0_u8; 14];
            ethernet[0] = if multicast { 0x11 } else { 0x10 };
            ethernet[6..12].copy_from_slice(&[0x20; 6]);
            ethernet[12..14].copy_from_slice(&[0x08, 0x00]);
            let plan = plan_data_encapsulation(
                role,
                [0x30; 6],
                [0x40; 6],
                ethernet,
                WmmUserPriority::new(priority as u8).unwrap(),
                peer_qos,
                false,
            );

            assert_eq!(plan.access_category, category);
            assert_eq!(&plan.header[..2], &frame_control);
            assert_eq!(plan.header_len, header_len);
            let metadata = DataTxMetadata::from_encapsulation(&plan);
            assert_eq!(metadata.queue_class, queue);
            let expected_packet_type = if frame_control[0] == 0x88 {
                assert_eq!(plan.header[24], priority as u8);
                qos_packet_type
            } else {
                10
            };
            assert_eq!(metadata.packet_type, expected_packet_type);
        }
    }
}

#[test]
fn invalid_user_priorities_never_reach_s31_metadata_encoding() {
    for priority in 8..=u8::MAX {
        assert_eq!(queue_class(priority), None);
        assert_eq!(descriptor_priority_byte(priority), None);
        assert!(WmmUserPriority::new(priority).is_none());
    }
}

#[test]
fn completion_routing_keeps_station_eapol_separate_from_ap_power_save() {
    // Callback identities are S31 TX metadata, not PAC register fields.
    for (role, ether_type, expected_callbacks) in [
        (DataInterfaceRole::Station, 0x0800, 0),
        (DataInterfaceRole::Station, 0x888e, 8),
        (DataInterfaceRole::AccessPoint, 0x0800, 4096),
        (DataInterfaceRole::AccessPoint, 0x888e, 4096),
    ] {
        assert_eq!(
            completion_callback_mask(role, ether_type),
            expected_callbacks
        );
    }
}
