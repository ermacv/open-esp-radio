use super::{EtherType, EthernetHeader, HEADER_LEN};
use crate::vlan::{TAG_LEN, VlanTag};
use oer_wire::{Error, Writer};

const FRAME: [u8; 18] = [
    0x02, 0x11, 0x22, 0x33, 0x44, 0x55, // destination
    0x02, 0x66, 0x77, 0x88, 0x99, 0xaa, // source
    0x08, 0x00, // IPv4
    0x45, 0x00, 0x00, 0x14, // payload
];

#[test]
fn a_header_reads_its_addresses_and_type_and_lends_the_payload() {
    let (header, payload) = EthernetHeader::parse(&FRAME).unwrap();
    assert_eq!(header.destination, [0x02, 0x11, 0x22, 0x33, 0x44, 0x55]);
    assert_eq!(header.source, [0x02, 0x66, 0x77, 0x88, 0x99, 0xaa]);
    assert_eq!(header.ether_type, EtherType::IPV4);
    assert_eq!(payload, &[0x45, 0x00, 0x00, 0x14]);
    assert_eq!(header.to_bytes(), FRAME[..HEADER_LEN]);
}

#[test]
fn a_short_frame_has_no_header() {
    assert_eq!(
        EthernetHeader::parse(&FRAME[..13]),
        Err(Error::Truncated {
            required: 14,
            remaining: 13
        })
    );
}

#[test]
fn a_header_writes_its_octets_or_nothing() {
    let (header, _) = EthernetHeader::parse(&FRAME).unwrap();
    let mut buffer = [0xee; 16];
    let mut writer = Writer::new(&mut buffer);
    header.write(&mut writer).unwrap();
    assert_eq!(writer.written(), HEADER_LEN);
    assert_eq!(buffer[..HEADER_LEN], FRAME[..HEADER_LEN]);
    let mut short = [0xee; 13];
    let mut writer = Writer::new(&mut short);
    assert!(header.write(&mut writer).is_err());
    assert_eq!(short, [0xee; 13], "a refused write leaves the buffer");
}

#[test]
fn ether_types_tell_types_from_lengths_and_name_vlan_tags() {
    assert!(EtherType::new(0x0600).is_type());
    assert!(!EtherType::new(0x05dc).is_type());
    assert!(EtherType::VLAN.is_vlan_tag());
    assert!(EtherType::PROVIDER_VLAN.is_vlan_tag());
    assert!(!EtherType::IPV6.is_vlan_tag());
    assert_eq!(EtherType::IPV6.value(), 0x86dd);
    assert_eq!(EtherType::ARP.value(), 0x0806);
}

#[test]
fn a_vlan_tag_reads_its_priority_and_inner_type() {
    // PCP 5, DEI 0, VID 0x123; inner IPv6.
    let tag = [0xa1, 0x23, 0x86, 0xdd, 0x60];
    let (parsed, rest) = VlanTag::parse(&tag).unwrap();
    assert_eq!(parsed.tci.pcp(), 5);
    assert!(!parsed.tci.dei());
    assert_eq!(parsed.tci.vid(), 0x123);
    assert_eq!(parsed.ether_type, EtherType::IPV6);
    assert_eq!(rest, &[0x60]);
    assert!(VlanTag::parse(&tag[..TAG_LEN - 1]).is_err());
}
