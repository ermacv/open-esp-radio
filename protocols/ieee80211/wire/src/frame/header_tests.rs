use super::*;

const A: [u8; 6] = [2, 0, 0, 0, 0, 1];
const B: [u8; 6] = [2, 0, 0, 0, 0, 2];

fn qos() -> QosControl {
    QosControl::new(
        oer_wire::U8::constant::<9>(),
        true,
        QosAckPolicy::NoAck,
        true,
        0x72,
    )
}

#[test]
fn every_valid_header_shape_writes_all_fields_and_leaves_its_tail_untouched() {
    for raw in 0..=u16::MAX {
        let Some(control) = FrameControl::new(raw) else {
            continue;
        };
        let address4 = control.is_four_address().then_some([4, 3, 2, 1, 0, 9]);
        let qos = control.is_qos_data().then_some(qos());
        let ht = control.has_ht_control().then_some([0x10, 0x20, 0x30, 0x40]);
        let header =
            MacHeader::new(control, 0x5634, [0xff; 6], A, B, 0x789a, address4, qos, ht).unwrap();
        let mut expected = std::vec::Vec::new();
        expected.extend(raw.to_le_bytes());
        expected.extend(0x5634_u16.to_le_bytes());
        expected.extend([0xff; 6]);
        expected.extend(A);
        expected.extend(B);
        expected.extend(0x789a_u16.to_le_bytes());
        if let Some(address4) = address4 {
            expected.extend(address4);
        }
        if qos.is_some() {
            expected.extend([0xb9, 0x72]);
        }
        if let Some(ht) = ht {
            expected.extend(ht);
        }
        let mut out = [0xa5; 40];
        assert_eq!(
            header.encode(&mut out),
            Ok(expected.len()),
            "control {raw:#06x}"
        );
        assert_eq!(header.header_len(), expected.len(), "control {raw:#06x}");
        assert_eq!(&out[..expected.len()], expected, "control {raw:#06x}");
        assert!(out[expected.len()..].iter().all(|byte| *byte == 0xa5));
        assert_eq!(MacHeader::decode(&out), Some(header));
    }
}

#[test]
fn optional_fields_agree_with_the_frame_control() {
    let header = |raw, address4: Option<[u8; 6]>, qos: Option<QosControl>, ht: Option<[u8; 4]>| {
        MacHeader::new(FrameControl::new(raw)?, 0, B, A, B, 0, address4, qos, ht)
    };
    assert!(header(0x0088, None, Some(qos()), None).is_some());
    assert!(header(0x0088, None, None, None).is_none());
    assert!(header(0x0008, None, Some(qos()), None).is_none());
    assert!(header(0x0308, Some(A), None, None).is_some());
    assert!(header(0x0308, None, None, None).is_none());
    assert!(header(0x0108, Some(A), None, None).is_none());
    assert!(header(0x8088, None, Some(qos()), Some([0; 4])).is_some());
    assert!(header(0x8088, None, Some(qos()), None).is_none());
    assert!(header(0x8080, None, None, Some([0; 4])).is_some());
    assert!(header(0x8008, None, None, None).is_some());
    assert!(header(0x8008, None, None, Some([0; 4])).is_none());
    // Only a QoS data header takes a new QoS Control.
    assert!(
        header(0x0008, None, None, None)
            .unwrap()
            .with_qos(qos())
            .is_none()
    );
}

#[test]
fn a_short_buffer_is_refused_unwritten_and_a_short_frame_undecoded() {
    let header = MacHeader::new(
        FrameControl::new(0x8388).unwrap(),
        0,
        B,
        A,
        B,
        0,
        Some(A),
        Some(qos()),
        Some([1, 2, 3, 4]),
    )
    .unwrap();
    let mut bytes = [0; 36];
    assert_eq!(header.encode(&mut bytes), Ok(36));
    for len in 0..header.header_len() {
        let mut short = [0xa5; 36];
        assert!(header.encode(&mut short[..len]).is_err());
        assert_eq!(short, [0xa5; 36]);
        assert!(MacHeader::decode(&bytes[..len]).is_none());
    }
    // A control frame has no data or management header.
    assert!(MacHeader::decode(&[0xd4, 0, 0, 0]).is_none());
}

#[test]
fn the_qos_field_places_each_subfield() {
    let field = QosControl::new(
        oer_wire::U8::constant::<6>(),
        false,
        QosAckPolicy::Normal,
        false,
        0,
    );
    assert_eq!(field.to_bytes(), [0x06, 0x00]);
    assert_eq!(qos().to_bytes(), [0xb9, 0x72]);
    assert_eq!(
        qos()
            .with_ack_policy(QosAckPolicy::Normal.field())
            .to_bytes(),
        [0x99, 0x72]
    );
}

#[test]
fn named_subtypes_round_trip_through_the_frame_control_field() {
    let beacon = FrameControlFields::management(ManagementSubtype::Beacon).unwrap();
    assert_eq!(beacon.raw(), 0x0080);
    assert!(beacon.is_management(ManagementSubtype::Beacon));
    assert!(!beacon.is_management(ManagementSubtype::ProbeResponse));
    assert!(!beacon.is_control(ControlSubtype::BlockAck));
    let request = FrameControlFields::control(ControlSubtype::BlockAckRequest).unwrap();
    assert_eq!(request.raw(), 0x0084);
    assert!(request.is_control(ControlSubtype::BlockAckRequest));
    assert_eq!(
        FrameControlFields::from_raw(0x00d0 | 0x4000).management_subtype(),
        ManagementSubtype::Action
    );
    // A reserved encoding wider than the subtype is refused.
    assert_eq!(
        FrameControlFields::management(ManagementSubtype::Reserved(16)),
        None
    );
}

#[test]
fn address_fields_end_where_the_frame_does() {
    let mut frame = [0_u8; 30];
    frame[..2].copy_from_slice(&[0x88, 0x03]);
    for (index, byte) in frame.iter_mut().enumerate().skip(4) {
        *byte = index as u8;
    }
    let address = |at: usize| -> [u8; 6] { frame[at..at + 6].try_into().unwrap() };

    assert_eq!(AddressFields::parse(&frame[..9]), None);
    let rts = AddressFields::parse(&frame[..16]).unwrap();
    assert_eq!(rts.receiver(), address(4));
    assert_eq!(rts.transmitter(), Some(address(10)));
    assert_eq!(rts.address3(), None);
    let short = AddressFields::parse(&frame[..23]).unwrap();
    assert_eq!(short.address3(), Some(address(16)));
    assert_eq!(short.sequence_control(), None);
    let full = AddressFields::parse(&frame).unwrap();
    assert_eq!(full.control().raw(), 0x0388);
    assert_eq!(full.sequence_control(), Some(u16::from_le_bytes([22, 23])));
    assert_eq!(full.address4(), Some(address(24)));
    assert_eq!(AddressFields::parse(&frame[..29]).unwrap().address4(), None);
}
