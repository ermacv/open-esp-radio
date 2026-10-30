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

fn addressed(frame_control: [u8; 2], length: usize) -> std::vec::Vec<u8> {
    let mut frame = std::vec![0_u8; length];
    frame[..2].copy_from_slice(&frame_control);
    for (index, byte) in frame.iter_mut().enumerate().skip(4) {
        *byte = index as u8;
    }
    frame
}

fn address_at(frame: &[u8], at: usize) -> [u8; 6] {
    *frame[at..].first_chunk::<6>().unwrap()
}

#[test]
fn a_three_address_qos_data_frame_has_no_address4() {
    // QoS Data, From DS: QoS Control and the body follow Sequence Control.
    let frame = addressed([0x88, 0x02], 40);
    let fields = AddressFields::parse(&frame).unwrap();
    assert_eq!(fields.receiver(), address_at(&frame, 4));
    assert_eq!(fields.transmitter(), Some(address_at(&frame, 10)));
    assert_eq!(fields.address3(), Some(address_at(&frame, 16)));
    assert_eq!(
        fields.sequence_control(),
        Some(u16::from_le_bytes([22, 23]))
    );
    assert_eq!(fields.address4(), None);
    assert_eq!(fields.addresses().count(), 3);
}

#[test]
fn a_four_address_data_frame_holds_address4_or_is_truncated() {
    let frame = addressed([0x88, 0x03], 40);
    let fields = AddressFields::parse(&frame).unwrap();
    assert_eq!(fields.address4(), Some(address_at(&frame, 24)));
    assert_eq!(fields.addresses().count(), 4);
    assert_eq!(
        AddressFields::parse(&frame[..29]),
        Err(AddressFieldsError::Truncated)
    );
}

#[test]
fn management_and_data_frames_need_their_whole_three_address_header() {
    for frame_control in [[0xc0, 0x00], [0x08, 0x01]] {
        let frame = addressed(frame_control, 24);
        let fields = AddressFields::parse(&frame).unwrap();
        assert_eq!(fields.address4(), None);
        assert_eq!(
            AddressFields::parse(&frame[..23]),
            Err(AddressFieldsError::Truncated)
        );
    }
    // A management frame never has Address 4, whatever its DS bits.
    let management = addressed([0xd0, 0x03], 40);
    assert_eq!(AddressFields::parse(&management).unwrap().address4(), None);
}

#[test]
fn control_frames_hold_a_transmitter_only_by_subtype() {
    let rts = addressed([0xb4, 0x00], 16);
    let fields = AddressFields::parse(&rts).unwrap();
    assert_eq!(fields.transmitter(), Some(address_at(&rts, 10)));
    assert_eq!(fields.address3(), None);
    assert_eq!(
        AddressFields::parse(&rts[..15]),
        Err(AddressFieldsError::Truncated)
    );
    // An Ack ends after its RA; octets after it are not a TA.
    let ack = addressed([0xd4, 0x00], 16);
    let fields = AddressFields::parse(&ack).unwrap();
    assert_eq!(fields.transmitter(), None);
    assert_eq!(fields.addresses().count(), 1);
    assert!(AddressFields::parse(&ack[..10]).is_ok());
    assert_eq!(
        AddressFields::parse(&ack[..9]),
        Err(AddressFieldsError::Truncated)
    );
}

#[test]
fn another_protocol_version_has_no_address_layout() {
    let frame = addressed([0x89, 0x02], 40);
    assert_eq!(
        AddressFields::parse(&frame),
        Err(AddressFieldsError::UnknownVersion)
    );
}

#[test]
fn every_control_subtype_has_a_known_or_refused_layout() {
    // The layouts of IEEE Std 802.11-2020, 9.3.1, written out independently
    // of `control_layout`: Some(true) RA and TA, Some(false) RA alone, None
    // not interpreted.
    const EXPECTED: [(u8, Option<bool>); 16] = [
        (0, None),         // Reserved
        (1, None),         // Reserved
        (2, Some(true)),   // Trigger
        (3, None),         // TACK
        (4, Some(true)),   // Beamforming Report Poll
        (5, Some(true)),   // VHT/HE NDP Announcement
        (6, None),         // Control Frame Extension
        (7, Some(false)),  // Control Wrapper
        (8, Some(true)),   // BlockAckReq
        (9, Some(true)),   // BlockAck
        (10, Some(true)),  // PS-Poll
        (11, Some(true)),  // RTS
        (12, Some(false)), // CTS
        (13, Some(false)), // Ack
        (14, Some(true)),  // CF-End
        (15, Some(true)),  // CF-End +CF-Ack
    ];
    for (subtype, layout) in EXPECTED {
        let frame = addressed([(subtype << 4) | 0x04, 0x00], 16);
        match layout {
            Some(true) => {
                assert_eq!(
                    AddressFields::parse(&frame).unwrap().transmitter(),
                    Some(address_at(&frame, 10)),
                    "subtype {subtype}"
                );
                assert_eq!(
                    AddressFields::parse(&frame[..15]),
                    Err(AddressFieldsError::Truncated),
                    "subtype {subtype}"
                );
            }
            Some(false) => {
                let fields = AddressFields::parse(&frame[..10]).unwrap();
                assert_eq!(fields.transmitter(), None, "subtype {subtype}");
                assert_eq!(fields.addresses().count(), 1, "subtype {subtype}");
            }
            None => {
                // Refused from Frame Control alone, before any field it
                // governs is read.
                for length in [2, 3, 9, 16] {
                    assert_eq!(
                        AddressFields::parse(&frame[..length]),
                        Err(AddressFieldsError::UnknownLayout),
                        "subtype {subtype}, {length} octets"
                    );
                }
            }
        }
    }
    // An extension frame is refused as early.
    for length in [2, 9, 40] {
        assert_eq!(
            AddressFields::parse(&addressed([0x0c, 0x00], 40)[..length]),
            Err(AddressFieldsError::UnknownLayout)
        );
    }
    // Only a frame shorter than Frame Control is truncated before its layout.
    assert_eq!(
        AddressFields::parse(&[0x64]),
        Err(AddressFieldsError::Truncated)
    );
}
