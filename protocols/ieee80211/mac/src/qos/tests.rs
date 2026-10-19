use super::{
    Dscp, WmmAccessCategory, WmmClassificationSource, WmmUserPriority, classify_ethernet_wmm,
};

#[test]
fn typed_user_priorities_map_to_the_four_standard_access_categories() {
    let expected = [
        WmmAccessCategory::BestEffort,
        WmmAccessCategory::Background,
        WmmAccessCategory::Background,
        WmmAccessCategory::BestEffort,
        WmmAccessCategory::Video,
        WmmAccessCategory::Video,
        WmmAccessCategory::Voice,
        WmmAccessCategory::Voice,
    ];
    for (value, category) in expected.into_iter().enumerate() {
        let priority = WmmUserPriority::new(value as u8).unwrap();
        assert_eq!(priority.value(), value as u8);
        assert_eq!(priority.access_category(), category);
    }
    assert_eq!(WmmUserPriority::new(8), None);
}

#[test]
fn station_dscp_mapping_bleaches_unassigned_and_network_control_values() {
    let cases = [
        (0, 0),
        (1, 1),
        (8, 1),
        (10, 0),
        (18, 3),
        (24, 4),
        (34, 4),
        (40, 5),
        (44, 6),
        (46, 6),
        (48, 0),
        (56, 0),
        (63, 0),
    ];
    for (dscp, priority) in cases {
        assert_eq!(
            Dscp::new(dscp).unwrap().station_user_priority().value(),
            priority
        );
    }
    assert_eq!(Dscp::new(64), None);
    assert_eq!(Dscp::new(46).unwrap().default_user_priority().value(), 5);
}

#[test]
fn ethernet_classifier_uses_vlan_pcp_before_ipv4_dscp() {
    let mut frame = [0_u8; 22];
    frame[12..14].copy_from_slice(&0x8100_u16.to_be_bytes());
    frame[14..16].copy_from_slice(&(5_u16 << 13).to_be_bytes());
    frame[16..18].copy_from_slice(&0x0800_u16.to_be_bytes());
    frame[18] = 0x45;
    frame[19] = 46 << 2;

    let class = classify_ethernet_wmm(&frame);
    assert_eq!(class.user_priority, WmmUserPriority::UP5);
    assert_eq!(class.access_category, WmmAccessCategory::Video);
    assert_eq!(
        class.source,
        WmmClassificationSource::Ieee8021d(WmmUserPriority::UP5)
    );
}

#[test]
fn ethernet_classifier_extracts_ipv4_and_ipv6_dscp_fail_closed() {
    let mut ipv4 = [0_u8; 16];
    ipv4[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
    ipv4[14] = 0x45;
    ipv4[15] = 46 << 2;
    let class = classify_ethernet_wmm(&ipv4);
    assert_eq!(class.user_priority, WmmUserPriority::UP6);
    assert_eq!(class.access_category, WmmAccessCategory::Voice);

    let mut ipv6 = [0_u8; 16];
    ipv6[12..14].copy_from_slice(&0x86dd_u16.to_be_bytes());
    let traffic_class = 40_u8 << 2;
    ipv6[14] = 0x60 | (traffic_class >> 4);
    ipv6[15] = traffic_class << 4;
    let class = classify_ethernet_wmm(&ipv6);
    assert_eq!(class.user_priority, WmmUserPriority::UP5);

    ipv6[14] = 0x40;
    assert_eq!(
        classify_ethernet_wmm(&ipv6).user_priority,
        WmmUserPriority::UP0
    );
    assert_eq!(
        classify_ethernet_wmm(&[0; 13]).user_priority,
        WmmUserPriority::UP0
    );
}
