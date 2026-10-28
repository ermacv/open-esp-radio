use super::TransportFlow;

fn ipv4() -> [u8; 64] {
    let mut frame = [0; 64];
    frame[12..14].copy_from_slice(&[8, 0]);
    frame[14] = 0x45;
    frame[16..18].copy_from_slice(&28_u16.to_be_bytes());
    frame[23] = 17;
    frame[26..34].copy_from_slice(&[10, 0, 0, 1, 10, 0, 0, 2]);
    frame[34..38].copy_from_slice(&[0, 1, 0, 2]);
    frame[38..40].copy_from_slice(&8_u16.to_be_bytes());
    frame
}

fn ipv6() -> [u8; 80] {
    let mut frame = [0; 80];
    frame[12..14].copy_from_slice(&[0x86, 0xdd]);
    frame[14] = 0x60;
    frame[18..20].copy_from_slice(&8_u16.to_be_bytes());
    frame[20] = 17;
    frame[22] = 1;
    frame[38] = 2;
    frame[54..58].copy_from_slice(&[0, 1, 0, 2]);
    frame[58..60].copy_from_slice(&8_u16.to_be_bytes());
    frame
}

#[test]
fn ipv4_flow_distinguishes_ports_addresses_and_protocol_but_not_payload() {
    let frame = ipv4();
    let original = TransportFlow::from_ethernet(&frame);
    assert!(matches!(
        original,
        TransportFlow::Ipv4 { ports: Some(_), .. }
    ));
    for index in [26, 30, 34, 36, 23] {
        let mut changed = frame;
        changed[index] ^= 1;
        assert_ne!(TransportFlow::from_ethernet(&changed), original);
    }
    let mut changed = frame;
    changed[50] = 1;
    assert_eq!(TransportFlow::from_ethernet(&changed), original);
}

#[test]
fn ipv4_options_shift_ports_and_tcp_header_length_is_checked() {
    let mut frame = ipv4();
    let original = TransportFlow::from_ethernet(&frame);
    frame.copy_within(34..42, 38);
    frame[34..38].fill(1);
    frame[14] = 0x46;
    frame[16..18].copy_from_slice(&32_u16.to_be_bytes());
    assert_eq!(TransportFlow::from_ethernet(&frame), original);
    frame[23] = 6;
    frame[16..18].copy_from_slice(&44_u16.to_be_bytes());
    frame[50] = 0x50;
    assert!(matches!(
        TransportFlow::from_ethernet(&frame),
        TransportFlow::Ipv4 {
            protocol: 6,
            ports: Some(_),
            ..
        }
    ));
    frame[50] = 0xf0;
    assert!(matches!(
        TransportFlow::from_ethernet(&frame),
        TransportFlow::Ipv4 { ports: None, .. }
    ));
}

#[test]
fn all_ipv4_fragments_use_one_coarse_key_without_reading_fake_ports() {
    let mut first = ipv4();
    first[20] = 0x20;
    let mut later = first;
    later[20] = 0;
    later[21] = 1;
    later[34..42].fill(123);
    let key = TransportFlow::from_ethernet(&first);
    assert!(matches!(key, TransportFlow::Ipv4 { ports: None, .. }));
    assert_eq!(TransportFlow::from_ethernet(&later), key);
}

#[test]
fn ethernet_padding_cannot_supply_missing_ip_or_transport_bytes() {
    let mut frame = ipv4();
    frame[16..18].copy_from_slice(&20_u16.to_be_bytes());
    assert!(matches!(
        TransportFlow::from_ethernet(&frame),
        TransportFlow::Ipv4 { ports: None, .. }
    ));
    frame[16..18].copy_from_slice(&19_u16.to_be_bytes());
    assert_eq!(TransportFlow::from_ethernet(&frame), TransportFlow::Opaque);
    frame[16..18].copy_from_slice(&200_u16.to_be_bytes());
    assert_eq!(TransportFlow::from_ethernet(&frame), TransportFlow::Opaque);
}

#[test]
fn ipv6_plain_udp_is_split_but_extensions_are_not_mistaken_for_ports() {
    let mut frame = ipv6();
    let original = TransportFlow::from_ethernet(&frame);
    assert!(matches!(
        original,
        TransportFlow::Ipv6 { ports: Some(_), .. }
    ));
    frame[56] = 3;
    assert_ne!(TransportFlow::from_ethernet(&frame), original);
    frame[20] = 44;
    let fragment = TransportFlow::from_ethernet(&frame);
    assert!(matches!(fragment, TransportFlow::Ipv6 { ports: None, .. }));
    frame[54..62].fill(123);
    assert_eq!(TransportFlow::from_ethernet(&frame), fragment);
    frame[20] = 0;
    assert!(matches!(
        TransportFlow::from_ethernet(&frame),
        TransportFlow::Ipv6 { ports: None, .. }
    ));
}

#[test]
fn truncated_frames_and_unknown_encapsulations_are_safe() {
    let frame = ipv4();
    for length in 0..42 {
        assert_eq!(
            TransportFlow::from_ethernet(&frame[..length]),
            TransportFlow::Opaque,
            "length {length}"
        );
    }
    assert_eq!(
        TransportFlow::from_ethernet(&frame[..42]),
        TransportFlow::Ipv4 {
            addresses: [10, 0, 0, 1, 10, 0, 0, 2],
            protocol: 17,
            ports: Some([0, 1, 0, 2]),
        }
    );
    let mut frame = frame;
    frame[12..14].copy_from_slice(&[0x81, 0]);
    assert_eq!(TransportFlow::from_ethernet(&frame), TransportFlow::Opaque);
}

#[test]
fn ipv6_truncation_and_maximum_payload_length_remain_opaque() {
    let mut frame = ipv6();
    for length in 0..62 {
        assert_eq!(
            TransportFlow::from_ethernet(&frame[..length]),
            TransportFlow::Opaque,
            "length {length}"
        );
    }
    let mut addresses = [0; 32];
    addresses[0] = 1;
    addresses[16] = 2;
    assert_eq!(
        TransportFlow::from_ethernet(&frame[..62]),
        TransportFlow::Ipv6 {
            addresses,
            protocol: 17,
            ports: Some([0, 1, 0, 2]),
        }
    );
    frame[18..20].copy_from_slice(&u16::MAX.to_be_bytes());
    assert_eq!(TransportFlow::from_ethernet(&frame), TransportFlow::Opaque);
}

#[test]
fn invalid_ip_versions_and_ipv4_header_lengths_remain_opaque() {
    let mut frame = ipv4();
    // All IHL values except 5, 6 and 7 are outside this 28-byte IP packet.
    for ihl in [0, 1, 2, 3, 4, 8, 9, 10, 11, 12, 13, 14, 15] {
        frame[14] = 0x40 | ihl;
        assert_eq!(TransportFlow::from_ethernet(&frame), TransportFlow::Opaque);
    }
    frame[14] = 0x65;
    assert_eq!(TransportFlow::from_ethernet(&frame), TransportFlow::Opaque);
    let mut frame = ipv6();
    frame[14] = 0x40;
    assert_eq!(TransportFlow::from_ethernet(&frame), TransportFlow::Opaque);
}

#[test]
fn short_transport_headers_and_invalid_lengths_keep_coarse_flow() {
    for protocol in [6, 17] {
        let minimum = if protocol == 6 { 20 } else { 8 };
        for length in 0..minimum {
            assert_eq!(super::ports(protocol, &[0; 20][..length]), None);
        }
    }
    let mut frame = ipv4();
    for length in [0_u16, 7, 9, u16::MAX] {
        frame[38..40].copy_from_slice(&length.to_be_bytes());
        assert!(matches!(
            TransportFlow::from_ethernet(&frame),
            TransportFlow::Ipv4 {
                protocol: 17,
                ports: None,
                ..
            }
        ));
    }
    let mut tcp = [0; 60];
    tcp[..4].copy_from_slice(&[0, 1, 0, 2]);
    for offset in 0_u8..=15 {
        tcp[12] = offset << 4;
        assert_eq!(
            super::ports(6, &tcp[..20]),
            if offset == 5 {
                Some([0, 1, 0, 2])
            } else {
                None
            }
        );
    }
    tcp[12] = 0xf0;
    assert_eq!(super::ports(6, &tcp), Some([0, 1, 0, 2]));
}
