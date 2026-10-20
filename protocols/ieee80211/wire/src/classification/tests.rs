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
