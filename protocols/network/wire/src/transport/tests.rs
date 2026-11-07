use super::{PROTOCOL_TCP, PROTOCOL_UDP, Ports, TransportError, UdpHeader};

#[test]
fn tcp_and_udp_headers_give_their_ports_within_their_lengths() {
    let mut tcp = [0u8; 24];
    tcp[..4].copy_from_slice(&[0x01, 0xbb, 0xc0, 0x00]);
    tcp[12] = 0x60;
    assert_eq!(
        Ports::parse(PROTOCOL_TCP, &tcp),
        Ok(Some(Ports {
            source: 443,
            destination: 0xc000
        }))
    );
    assert_eq!(
        Ports::parse(PROTOCOL_TCP, &tcp)
            .unwrap()
            .unwrap()
            .to_bytes(),
        [0x01, 0xbb, 0xc0, 0x00]
    );
    let udp = [0x00, 0x35, 0x12, 0x34, 0x00, 0x08, 0x00, 0x00];
    assert_eq!(
        Ports::parse(PROTOCOL_UDP, &udp),
        Ok(Some(Ports {
            source: 53,
            destination: 0x1234
        }))
    );
    assert_eq!(Ports::parse(1, &udp), Ok(None));
}

#[test]
fn a_transport_header_refuses_lengths_it_cannot_have() {
    let mut tcp = [0u8; 20];
    assert_eq!(
        Ports::parse(PROTOCOL_TCP, &tcp[..19]),
        Err(TransportError::Truncated)
    );
    tcp[12] = 0x40;
    assert_eq!(
        Ports::parse(PROTOCOL_TCP, &tcp),
        Err(TransportError::Length)
    );
    tcp[12] = 0x60;
    assert_eq!(
        Ports::parse(PROTOCOL_TCP, &tcp),
        Err(TransportError::Length)
    );
    let mut udp = [0u8; 8];
    udp[5] = 7;
    assert_eq!(
        Ports::parse(PROTOCOL_UDP, &udp),
        Err(TransportError::Length)
    );
    udp[5] = 9;
    assert_eq!(
        Ports::parse(PROTOCOL_UDP, &udp),
        Err(TransportError::Length)
    );
    assert_eq!(
        Ports::parse(PROTOCOL_UDP, &udp[..7]),
        Err(TransportError::Truncated)
    );
}

#[test]
fn a_udp_header_bounds_its_data_by_its_length() {
    let datagram = [0, 7, 0x13, 0x88, 0, 10, 0xAB, 0xCD, 1, 2, 3, 4];
    let (header, data) = UdpHeader::parse(&datagram).unwrap();
    assert_eq!(header.ports.source, 7);
    assert_eq!(header.ports.destination, 5000);
    assert_eq!(header.checksum, 0xABCD);
    assert_eq!(data, [1, 2]);
    assert_eq!(header.to_bytes(), datagram[..8]);
}

#[test]
fn a_udp_length_outside_its_octets_is_refused() {
    let short = [0, 7, 0, 7, 0, 7, 0, 0];
    assert_eq!(UdpHeader::parse(&short), Err(TransportError::Length));
    let long = [0, 7, 0, 7, 0, 9, 0, 0];
    assert_eq!(UdpHeader::parse(&long), Err(TransportError::Length));
    assert_eq!(UdpHeader::parse(&long[..7]), Err(TransportError::Truncated));
}
