use super::*;

#[test]
fn a_header_is_the_frame_s_fields_and_the_offset_of_its_body() {
    let mut frame = [0; 30];
    frame[..2].copy_from_slice(&[0x88, 0x02]);
    for (at, octet) in [(4, 2), (10, 4), (16, 6)] {
        frame[at..at + 6].fill(octet);
    }
    frame[22..24].copy_from_slice(&[0x34, 0x12]);
    frame[24] = 5;
    let decoded = DecodedHeader::decode(&frame).unwrap();
    let header = RxHeader::of(&decoded, &frame).unwrap();
    assert_eq!(header.receiver, Eui48::new([2; 6]));
    assert_eq!(header.transmitter, Eui48::new([4; 6]));
    assert_eq!(header.address3, Eui48::new([6; 6]));
    assert_eq!(header.sequence_control.to_bytes(), [0x34, 0x12]);
    assert_eq!(header.qos.map(|qos| qos.to_bytes()), Some([5, 0]));
    assert_eq!(header.body, 26);
    // A frame shorter than the body the header was read with.
    assert_eq!(RxHeader::of(&decoded, &frame[..3]), None);
}
