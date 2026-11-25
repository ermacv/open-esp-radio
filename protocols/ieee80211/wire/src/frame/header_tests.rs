use super::*;

const A: [u8; 6] = [2, 0, 0, 0, 0, 1];
const B: [u8; 6] = [2, 0, 0, 0, 0, 2];

fn qos() -> QosControl {
    QosControl::new(crate::qos::Tid::constant::<9>(), QosAckPolicy::NoAck, 0x72)
        .with_eosp(true)
        .with_amsdu(true)
}

/// The octets of a data or management header whose Frame Control is `raw`,
/// with every field its type holds.
fn header_octets(raw: u16) -> Option<std::vec::Vec<u8>> {
    let control = FrameControl::new(FrameControlFields::from_bytes(raw.to_le_bytes()))?;
    let mut octets = std::vec::Vec::new();
    octets.extend(raw.to_le_bytes());
    octets.extend(0x5634_u16.to_le_bytes());
    octets.extend([0xff; 6]);
    octets.extend(A);
    octets.extend(B);
    octets.extend([0x9a, 0x78]);
    if control.is_four_address() {
        octets.extend([4, 3, 2, 1, 0, 9]);
    }
    if control.is_qos_data() {
        octets.extend(qos().to_bytes());
    }
    if control.has_ht_control() {
        octets.extend([0x10, 0x20, 0x30, 0x40]);
    }
    Some(octets)
}

/// Every data or management Frame Control decodes to a header whose
/// derived Frame Control is the same field, and the header encodes to the
/// same octets: no bit is lost or invented by the layout.
#[test]
fn every_valid_header_shape_round_trips_and_leaves_its_tail_untouched() {
    for raw in 0..=u16::MAX {
        let Some(expected) = header_octets(raw) else {
            continue;
        };
        let header = MacHeader::decode(&expected).unwrap();
        assert_eq!(
            header.control().fields().to_bytes(),
            raw.to_le_bytes(),
            "control {raw:#06x}"
        );
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

/// The layout is the frame type's: a fourth address with both DS bits, QoS
/// Control in a QoS data frame, HT Control with the Order bit of a QoS
/// data or management frame; a non-QoS data frame's Order bit stays a bit.
#[test]
fn a_header_s_layout_is_the_one_its_frame_control_names() {
    let layout = |raw| {
        MacHeader::decode(&header_octets(raw).unwrap())
            .unwrap()
            .layout()
    };
    assert_eq!(
        layout(0x0088),
        HeaderLayout::QosData {
            direction: DataDirection::Neither,
            qos: qos(),
            ht_control: None
        }
    );
    assert_eq!(
        layout(0x0308),
        HeaderLayout::Data {
            direction: DataDirection::Wds(Eui48::new([4, 3, 2, 1, 0, 9]))
        }
    );
    assert_eq!(
        layout(0x0108),
        HeaderLayout::Data {
            direction: DataDirection::ToDs
        }
    );
    assert_eq!(
        layout(0x0208),
        HeaderLayout::Data {
            direction: DataDirection::FromDs
        }
    );
    assert_eq!(
        layout(0x8088),
        HeaderLayout::QosData {
            direction: DataDirection::Neither,
            qos: qos(),
            ht_control: Some([0x10, 0x20, 0x30, 0x40])
        }
    );
    assert_eq!(
        layout(0x8080),
        HeaderLayout::Management {
            ht_control: Some([0x10, 0x20, 0x30, 0x40])
        }
    );
    // StrictlyOrdered: no HT Control, the bit kept.
    assert_eq!(
        layout(0x8008),
        HeaderLayout::Data {
            direction: DataDirection::Neither
        }
    );
    let ordered = MacHeader::decode(&header_octets(0x8008).unwrap()).unwrap();
    assert!(ordered.control().fields().order());
    // A management frame's DS bits and its subtype's bit B7 are its own.
    let action_no_ack = MacHeader::decode(&header_octets(0x03e0).unwrap()).unwrap();
    assert_eq!(action_no_ack.control().fields().to_bytes(), [0xe0, 0x03]);
    assert_eq!(action_no_ack.address4(), None);
    // Only a QoS data header has a QoS Control to change.
    let plain = MacHeader::decode(&header_octets(0x0008).unwrap()).unwrap();
    assert_eq!(plain.map_qos(|_| QosControl::from_bytes([1, 2])), plain);
    let qos_header = MacHeader::decode(&header_octets(0x0088).unwrap()).unwrap();
    assert_eq!(
        qos_header.map_qos(|_| QosControl::from_bytes([1, 2])).qos(),
        Some(QosControl::from_bytes([1, 2]))
    );
}

#[test]
fn a_short_buffer_is_refused_unwritten_and_a_short_frame_undecoded() {
    let bytes = header_octets(0x8388).unwrap();
    let header = MacHeader::decode(&bytes).unwrap();
    assert_eq!(header.header_len(), 36);
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
    let field = QosControl::new(crate::qos::Tid::constant::<6>(), QosAckPolicy::Normal, 0);
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
    assert_eq!(beacon.to_bytes(), [0x80, 0x00]);
    assert!(beacon.is_management(ManagementSubtype::Beacon));
    assert!(!beacon.is_management(ManagementSubtype::ProbeResponse));
    assert!(!beacon.is_control(ControlSubtype::BlockAck));
    let request = FrameControlFields::control(ControlSubtype::BlockAckRequest).unwrap();
    assert_eq!(request.to_bytes(), [0x84, 0x00]);
    assert!(request.is_control(ControlSubtype::BlockAckRequest));
    assert_eq!(
        FrameControlFields::from_bytes([0xd0, 0x40]).management_subtype(),
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
    assert_eq!(fields.receiver(), Eui48::new(address_at(&frame, 4)));
    assert_eq!(
        fields.transmitter(),
        Some(Eui48::new(address_at(&frame, 10)))
    );
    assert_eq!(fields.address3(), Some(Eui48::new(address_at(&frame, 16))));
    assert_eq!(
        fields.sequence_control(),
        Some(SequenceControl::from_bytes([22, 23]))
    );
    assert_eq!(fields.address4(), None);
    assert_eq!(fields.addresses().count(), 3);
    assert_eq!(
        fields.rest(),
        AfterReceiver::Three {
            transmitter: Eui48::new(address_at(&frame, 10)),
            address3: Eui48::new(address_at(&frame, 16)),
            sequence_control: SequenceControl::from_bytes([22, 23]),
        }
    );
}

#[test]
fn a_four_address_data_frame_holds_address4_or_is_truncated() {
    let frame = addressed([0x88, 0x03], 40);
    let fields = AddressFields::parse(&frame).unwrap();
    assert_eq!(fields.address4(), Some(Eui48::new(address_at(&frame, 24))));
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
    assert_eq!(fields.transmitter(), Some(Eui48::new(address_at(&rts, 10))));
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
                    Some(Eui48::new(address_at(&frame, 10))),
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

/// A Null and a QoS Null are data frames with no body; Data, QoS Data and a
/// management frame of the same subtype numbers are not (IEEE Std
/// 802.11-2020, 9.2.4.1.3, Table 9-1).
#[test]
fn a_null_and_a_qos_null_are_the_data_frames_with_no_body() {
    let control = |byte: u8| FrameControl::new(FrameControlFields::from_bytes([byte, 0])).unwrap();
    // Type in bits 2-3, subtype in bits 4-7.
    assert!(control(0x48).is_null_data(), "Null");
    assert!(control(0xc8).is_null_data(), "QoS Null");
    assert!(!control(0x08).is_null_data(), "Data");
    assert!(!control(0x88).is_null_data(), "QoS Data");
    assert!(!control(0x40).is_null_data(), "a probe request, subtype 4");
    assert!(
        !control(0xc0).is_null_data(),
        "a deauthentication, subtype 12"
    );
}

/// A decoded header's every field is the one at its place in the octets.
#[test]
fn a_decoded_header_reads_each_field_from_its_place() {
    // QoS data, both DS bits, Order: Address 4, QoS Control, HT Control.
    let octets = header_octets(0x8388).unwrap();
    let decoded = DecodedHeader::decode(&octets).unwrap();
    assert_eq!(decoded.control().fields().to_bytes(), [0x88, 0x83]);
    assert_eq!(decoded.duration().value(), 0x5634);
    assert_eq!(decoded.receiver(), Eui48::new([0xff; 6]));
    assert_eq!(decoded.transmitter(), Eui48::new(A));
    assert_eq!(decoded.address3(), Eui48::new(B));
    assert_eq!(decoded.sequence_control().to_bytes(), [0x9a, 0x78]);
    assert_eq!(decoded.address4(), Some(Eui48::new([4, 3, 2, 1, 0, 9])));
    assert_eq!(decoded.qos(), Some(qos()));
    assert_eq!(decoded.ht_control(), Some([0x10, 0x20, 0x30, 0x40]));
    assert_eq!(decoded.body(), &[] as &[u8]);
    assert_eq!(decoded.header(), MacHeader::decode(&octets).unwrap());
}

/// The builders by value change their field and keep every other.
#[test]
fn a_builder_changes_its_field_alone() {
    let header = MacHeader::decode(&header_octets(0x0388).unwrap()).unwrap();
    let changed = header
        .with_address1(Eui48::new(A))
        .with_address3(Eui48::new(A))
        .with_sequence_control(SequenceControl::from_bytes([1, 2]))
        .with_protected(FrameProtection::Protected);
    assert_eq!(changed.address1(), Eui48::new(A));
    assert_eq!(changed.address2(), header.address2());
    assert_eq!(changed.address3(), Eui48::new(A));
    assert_eq!(changed.sequence_control().to_bytes(), [1, 2]);
    assert!(changed.control().protected());
    assert_eq!(changed.layout(), header.layout());
    assert_eq!(changed.duration(), header.duration());
}
