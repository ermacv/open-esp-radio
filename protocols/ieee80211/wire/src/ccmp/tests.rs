use super::*;

#[test]
fn strict_header_round_trip_rejects_reserved_encodings() {
    let packet_number = CcmpPacketNumber::new(0x6655_4433_2211).unwrap();
    let header = CcmpHeader::new(packet_number, CcmpKeyId::new(2).unwrap());
    let encoded = header.encode();
    assert_eq!(encoded, [0x11, 0x22, 0, 0xa0, 0x33, 0x44, 0x55, 0x66]);
    assert_eq!(CcmpHeader::parse(encoded), Ok(header));

    let mut reserved_octet = encoded;
    reserved_octet[2] = 1;
    assert_eq!(
        CcmpHeader::parse(reserved_octet),
        Err(CcmpHeaderError::ReservedOctet)
    );
    let mut no_ext_iv = encoded;
    no_ext_iv[3] &= !0x20;
    assert_eq!(
        CcmpHeader::parse(no_ext_iv),
        Err(CcmpHeaderError::ExtIvMissing)
    );
    let mut reserved_key_bits = encoded;
    reserved_key_bits[3] |= 1;
    assert_eq!(
        CcmpHeader::parse(reserved_key_bits),
        Err(CcmpHeaderError::ReservedKeyBits)
    );
}

#[test]
fn receive_sequence_rejects_truncating_high_bits() {
    assert_eq!(
        CcmpPacketNumber::from_receive_sequence([1, 2, 3, 4, 5, 6, 0, 0]),
        Ok(CcmpPacketNumber::new(0x0605_0403_0201).unwrap())
    );
    assert_eq!(
        CcmpPacketNumber::from_receive_sequence([0, 0, 0, 0, 0, 0, 1, 0]),
        Err(CcmpReceiveSequenceError::HighBitsSet)
    );
}

#[test]
fn receive_sequence_round_trips_every_packet_number_width() {
    for value in [0, 1, 0x0605_0403_0201, CCMP_PACKET_NUMBER_MAX] {
        let packet_number = CcmpPacketNumber::new(value).unwrap();
        let sequence = packet_number.receive_sequence();
        assert_eq!(sequence[6..], [0, 0]);
        assert_eq!(
            CcmpPacketNumber::from_receive_sequence(sequence),
            Ok(packet_number)
        );
    }
    assert_eq!(
        CcmpPacketNumber::new(0x0605_0403_0201)
            .unwrap()
            .receive_sequence(),
        [1, 2, 3, 4, 5, 6, 0, 0]
    );
}

/// The highest 48-bit PN is written and read whole; the first beyond it
/// is refused where the number enters.
#[test]
fn the_packet_number_holds_48_bits() {
    let highest = CcmpPacketNumber::new(CCMP_PACKET_NUMBER_MAX).unwrap();
    let header = CcmpHeader::new(highest, CcmpKeyId::new(3).unwrap()).encode();
    assert_eq!(header, [0xff, 0xff, 0, 0xe0, 0xff, 0xff, 0xff, 0xff]);
    assert_eq!(CcmpHeader::parse(header).unwrap().packet_number(), highest);
    assert_eq!(
        CcmpPacketNumber::from_receive_sequence(highest.receive_sequence()),
        Ok(highest)
    );
    assert_eq!(CcmpPacketNumber::new(CCMP_PACKET_NUMBER_MAX + 1), None);
    for key_id in 0..4 {
        let group = GroupKeyId::new(key_id).unwrap();
        assert_eq!(group.wire().get(), key_id);
        assert_eq!(group.get(), key_id);
    }
    assert_eq!(GroupKeyId::new(4), None);
}
