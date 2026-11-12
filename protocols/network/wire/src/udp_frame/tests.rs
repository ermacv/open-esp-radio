extern crate std;

use std::vec;

use super::*;
use crate::ethernet::EtherType;
use crate::ip::Dscp;

/// A template's headers are the ones the fields make, field by field, for
/// every length and identification, and its checksums verify.
#[test]
fn a_template_s_headers_are_the_ones_its_fields_make() {
    let ethernet = EthernetHeader {
        destination: [2, 0, 0, 0, 0, 2],
        source: [2, 0, 0, 0, 0, 1],
        ether_type: EtherType::IPV4,
    };
    let fields = Ipv4Fields {
        services: Dscp::new(46).unwrap().field(),
        total_length: 0,
        identification: 0,
        time_to_live: 64,
        protocol: PROTOCOL_UDP,
        source: [192, 0, 2, 1],
        destination: [198, 51, 100, 7],
    };
    let ports = Ports {
        source: 5000,
        destination: 7,
    };
    let template = UdpFrameTemplate::new(&ethernet, &fields, ports);
    for (length, identification) in [(0, 0), (5, 1), (1472, 0x8000), (65507, 0xffff)] {
        let data = vec![0xa5; length];
        let mut head = [0; UDP_FRAME_HEADERS];
        let sum = template.write(&mut head, length, identification).unwrap();
        UdpFrameTemplate::close(&mut head, sum.with(&data));
        let udp_length = (UDP_HEADER_LEN + length) as u16;
        let mut expected = std::vec::Vec::new();
        expected.extend_from_slice(&ethernet.to_bytes());
        expected.extend_from_slice(
            &Ipv4Fields {
                total_length: udp_length + IPV4_HEADER_LEN as u16,
                identification,
                ..fields
            }
            .to_bytes(),
        );
        let pseudo =
            ipv4_pseudo_header(fields.source, fields.destination, PROTOCOL_UDP, udp_length);
        let udp = UdpHeader {
            ports,
            length: udp_length,
            checksum: 0,
        };
        let checksum = udp_field(
            Checksum::new()
                .with(&pseudo)
                .with(&udp.to_bytes())
                .with(&data)
                .finish(),
        );
        expected.extend_from_slice(&UdpHeader { checksum, ..udp }.to_bytes());
        assert_eq!(
            head.as_slice(),
            expected.as_slice(),
            "{length} {identification}"
        );
    }
    let mut head = [7; UDP_FRAME_HEADERS];
    assert_eq!(template.write(&mut head, 65508, 0), Err(TooLong));
    assert_eq!(head, [7; UDP_FRAME_HEADERS], "untouched");
}
