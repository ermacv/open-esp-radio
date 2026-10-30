use super::{DsField, Dscp, IpError, Ipv4Header, Ipv6Header};

/// RFC 791 header, 20 octets: DSCP 46 (EF) with ECN 1, total length 24,
/// UDP, 192.0.2.1 to 198.51.100.2, then four payload octets and one past
/// the total length.
const IPV4: [u8; 25] = [
    0x45, 0xb9, 0x00, 0x18, 0x00, 0x00, 0x00, 0x00, 0x40, 0x11, 0x00, 0x00, 192, 0, 2, 1, 198, 51,
    100, 2, 1, 2, 3, 4, 0xff,
];

#[test]
fn an_ipv4_header_reads_its_fields_within_its_total_length() {
    let header = Ipv4Header::parse(&IPV4).unwrap();
    assert_eq!(header.dscp().value(), 46);
    assert_eq!(header.services().ecn(), 1);
    assert_eq!(header.protocol(), 17);
    assert_eq!(header.addresses(), [192, 0, 2, 1, 198, 51, 100, 2]);
    assert!(!header.is_fragment());
    assert_eq!(header.payload(), &[1, 2, 3, 4]);
}

#[test]
fn an_ipv4_header_refuses_what_it_cannot_be() {
    assert_eq!(Ipv4Header::parse(&IPV4[..19]), Err(IpError::Truncated));
    let mut version = IPV4;
    version[0] = 0x65;
    assert_eq!(Ipv4Header::parse(&version), Err(IpError::Version));
    let mut short_ihl = IPV4;
    short_ihl[0] = 0x44;
    assert_eq!(Ipv4Header::parse(&short_ihl), Err(IpError::HeaderLength));
    let mut long_ihl = IPV4;
    long_ihl[0] = 0x47;
    assert_eq!(Ipv4Header::parse(&long_ihl), Err(IpError::HeaderLength));
    let mut long_total = IPV4;
    long_total[3] = 0x1a;
    assert_eq!(Ipv4Header::parse(&long_total), Err(IpError::Length));
}

#[test]
fn an_ipv4_fragment_is_the_first_of_several_or_a_later_one() {
    for (flags, offset) in [(0x20, 0x00), (0x00, 0x01), (0x1f, 0xff)] {
        let mut fragment = IPV4;
        fragment[6] = flags;
        fragment[7] = offset;
        assert!(Ipv4Header::parse(&fragment).unwrap().is_fragment());
    }
    let mut dont_fragment = IPV4;
    dont_fragment[6] = 0x40;
    assert!(!Ipv4Header::parse(&dont_fragment).unwrap().is_fragment());
}

#[test]
fn an_ipv6_header_reads_its_traffic_class_and_bounds_its_payload() {
    let mut packet = [0u8; 43];
    // Version 6, Traffic Class 0xb8 (DSCP 46), payload length 2, TCP.
    packet[..8].copy_from_slice(&[0x6b, 0x80, 0x00, 0x00, 0x00, 0x02, 0x06, 0x40]);
    packet[8] = 0x20;
    packet[39] = 0x01;
    packet[40..].copy_from_slice(&[7, 8, 9]);
    let header = Ipv6Header::parse(&packet).unwrap();
    assert_eq!(header.dscp().value(), 46);
    assert_eq!(header.next_header(), 6);
    assert_eq!(header.addresses()[0], 0x20);
    assert_eq!(header.addresses()[31], 0x01);
    assert_eq!(header.payload(), &[7, 8]);
    assert_eq!(Ipv6Header::parse(&packet[..39]), Err(IpError::Truncated));
    packet[5] = 4;
    assert_eq!(Ipv6Header::parse(&packet), Err(IpError::Length));
    packet[0] = 0x4b;
    assert_eq!(Ipv6Header::parse(&packet), Err(IpError::Version));
}

#[test]
fn a_dscp_names_its_class_selector_and_refuses_seven_bits() {
    assert_eq!(Dscp::new(46).unwrap().class_selector(), 5);
    assert_eq!(Dscp::new(64), None);
    let field = DsField::from_bytes([46 << 2 | 1]);
    assert_eq!(Dscp::from_field(field.dscp()).value(), 46);
}
