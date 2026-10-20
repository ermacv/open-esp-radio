use super::*;

#[test]
fn fixed_rate_is_limited_to_recovered_control_traffic() {
    assert!(uses_fixed_per_packet_rate(ETHER_TYPE_EAPOL, false, None));
    assert!(uses_fixed_per_packet_rate(ETHER_TYPE_WAPI, false, None));
    assert!(uses_fixed_per_packet_rate(ETHER_TYPE_ARP, true, None));
    assert!(!uses_fixed_per_packet_rate(ETHER_TYPE_ARP, false, None));
    assert!(uses_fixed_per_packet_rate(
        ETHER_TYPE_IPV4,
        false,
        Some((68, 67))
    ));
    assert!(uses_fixed_per_packet_rate(
        ETHER_TYPE_IPV4,
        false,
        Some((49152, 53))
    ));
    assert!(!uses_fixed_per_packet_rate(
        ETHER_TYPE_IPV4,
        false,
        Some((49152, 443))
    ));
}

#[test]
fn network_priority_uses_only_the_recovered_three_bits() {
    assert_eq!(user_priority(ETHER_TYPE_IPV4, Some(0xe0), None), 7);
    assert_eq!(user_priority(ETHER_TYPE_IPV6, None, Some(0xae00_0000)), 7);
    assert_eq!(user_priority(ETHER_TYPE_VENDOR_PRIORITY, None, None), 5);
    assert_eq!(user_priority(ETHER_TYPE_ARP, None, None), 0);
}

#[test]
fn wmm_downgrade_graph_is_finite_and_exact() {
    assert_eq!(apply_wmm_admission(7, [false; 3]), 7);
    assert_eq!(apply_wmm_admission(7, [true, false, false]), 5);
    assert_eq!(apply_wmm_admission(7, [true, true, false]), 0);
    assert_eq!(apply_wmm_admission(7, [true, true, true]), 1);
    assert_eq!(apply_wmm_admission(2, [true; 3]), 2);
    assert_eq!(apply_wmm_admission(8, [false; 3]), 7);
}

#[test]
fn default_ip_priority_ignores_ecn_version_and_flow_label() {
    for (services, priority) in [
        (0x03, 0),
        (0x27, 1),
        (0x4b, 2),
        (0x6f, 3),
        (0x93, 4),
        (0xb7, 5),
        (0xdb, 6),
        (0xff, 7),
    ] {
        assert_eq!(
            user_priority(ETHER_TYPE_IPV4, Some(services), None),
            priority
        );
        for version in [0, 6, 15] {
            for flow_label in [0, 0x000f_ffff] {
                let prefix = (version * 0x1000_0000) + (services as u32 * 0x0010_0000) + flow_label;
                assert_eq!(user_priority(ETHER_TYPE_IPV6, None, Some(prefix)), priority);
            }
        }
    }
}
