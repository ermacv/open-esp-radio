use super::*;
use std::{vec, vec::Vec};

/// The class of a frame of `ether_type` around `payload`.
fn classify(ether_type: u16, payload: &[u8]) -> WmmTrafficClass {
    classify_wmm(EtherType::new(ether_type), payload)
}

/// A valid IPv4 header with the services octet and `ihl` words, no payload.
fn ipv4(services: u8, ihl: u8) -> Vec<u8> {
    let length = usize::from(ihl) * 4;
    let mut header = vec![0; length];
    header[0] = 0x40 + ihl;
    header[1] = services;
    header[2..4].copy_from_slice(&(length as u16).to_be_bytes());
    header[9] = 17;
    header
}

/// A valid IPv6 fixed header with these first two octets, no payload.
fn ipv6(first: u8, second: u8) -> Vec<u8> {
    let mut header = vec![0; 40];
    header[0] = first;
    header[1] = second;
    header[6] = 17;
    header
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
            station_user_priority(Dscp::new(dscp).unwrap()).value(),
            priority
        );
    }
    assert_eq!(default_user_priority(Dscp::new(46).unwrap()).value(), 5);
}

#[test]
fn ethernet_classifier_uses_vlan_pcp_before_ipv4_dscp() {
    let mut tagged = (5_u16 << 13).to_be_bytes().to_vec();
    tagged.extend_from_slice(&0x0800_u16.to_be_bytes());
    tagged.extend_from_slice(&ipv4(46 << 2, 5));
    let class = classify(0x8100, &tagged);
    assert_eq!(class.user_priority, WmmUserPriority::UP5);
    assert_eq!(class.access_category, WmmAccessCategory::Video);
    assert_eq!(
        class.source,
        WmmClassificationSource::Ieee8021d(WmmUserPriority::UP5)
    );
}

#[test]
fn ethernet_classifier_extracts_ipv4_and_ipv6_dscp() {
    let class = classify(0x0800, &ipv4(46 << 2, 5));
    assert_eq!(class.user_priority, WmmUserPriority::UP6);
    assert_eq!(class.access_category, WmmAccessCategory::Voice);

    let traffic_class = 40_u8 << 2;
    let class = classify(
        0x86dd,
        &ipv6(0x60 | (traffic_class >> 4), traffic_class << 4),
    );
    assert_eq!(class.user_priority, WmmUserPriority::UP5);
}

#[test]
fn a_valid_frame_of_another_type_is_unmarked_and_a_refused_one_malformed() {
    let arp = classify(0x0806, &[0; 28]);
    assert_eq!(arp.source, WmmClassificationSource::Unmarked);
    assert_eq!(arp.user_priority, WmmUserPriority::UP0);
    for (ether_type, payload) in [
        (0x86dd, ipv6(0x40, 0)),
        (0x0800, ipv4(46 << 2, 5)[..19].to_vec()),
        (0x0800, ipv4(46 << 2, 4)),
        (0x8100, vec![0xa0]),
    ] {
        let class = classify(ether_type, &payload);
        assert_eq!(class.source, WmmClassificationSource::Malformed);
        assert_eq!(class.user_priority, WmmUserPriority::UP0);
        assert_eq!(class.access_category, WmmAccessCategory::BestEffort);
    }
}

#[test]
fn every_prefix_of_a_payload_is_classified() {
    for (ether_type, payload) in [
        (0x8100, vec![0xa0; 8]),
        (0x88a8, vec![0xa0; 8]),
        (0x0800, ipv4(46 << 2, 5)),
        (0x86dd, ipv6(0x6b, 0x80)),
    ] {
        for len in 0..=payload.len() {
            let _ = classify(ether_type, &payload[..len]);
        }
    }
}

#[test]
fn ip_classification_keeps_ihl_ecn_and_flow_label_out_of_the_dscp() {
    for (codepoint, priority) in [
        (0, 0),
        (1, 1),
        (8, 1),
        (18, 3),
        (24, 4),
        (34, 4),
        (40, 5),
        (44, 6),
        (46, 6),
        (48, 0),
        (56, 0),
        (63, 0),
    ] {
        for ecn in 0..4 {
            let services = codepoint * 4 + ecn;
            for ihl in [5, 15] {
                let class = classify(0x0800, &ipv4(services, ihl));
                assert_eq!(
                    class.source,
                    WmmClassificationSource::Dscp(Dscp::new(codepoint).unwrap())
                );
                assert_eq!(class.user_priority.value(), priority);
            }
            for flow_label_high in [0, 15] {
                let class = classify(
                    0x86dd,
                    &ipv6(
                        0x60 + services / 16,
                        services.wrapping_mul(16) + flow_label_high,
                    ),
                );
                assert_eq!(
                    class.source,
                    WmmClassificationSource::Dscp(Dscp::new(codepoint).unwrap())
                );
                assert_eq!(class.user_priority.value(), priority);
            }
        }
    }
}

#[test]
fn vlan_classification_keeps_dei_and_vid_out_of_the_pcp() {
    for pcp in 0..8 {
        for other_tci in [0, 0x0fff, 0x1000, 0x1fff] {
            let [high, low] = (pcp as u16 * 8192 + other_tci).to_be_bytes();
            let tag = [high, low, 0x08, 0x00];
            let class = classify(0x8100, &tag);
            let expected = WmmUserPriority::new(pcp).unwrap();
            assert_eq!(class.user_priority, expected);
            assert_eq!(class.source, WmmClassificationSource::Ieee8021d(expected));
            assert_eq!(
                classify(0x8100, &tag[..3]).source,
                WmmClassificationSource::Malformed
            );
        }
    }
}
