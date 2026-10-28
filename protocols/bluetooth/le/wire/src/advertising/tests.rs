use super::*;

fn sample_advertisement<'a>(data: &'a [u8]) -> LegacyNonconnectableAdvertisement<'a> {
    LegacyNonconnectableAdvertisement::new(
        LegacyNonconnectableKind::NonScannable,
        LeDeviceAddress::from_wire_bytes(
            [0xa6, 0xa5, 0xa4, 0xa3, 0xa2, 0xc1],
            LeDeviceAddressKind::Random,
        ),
        LegacyAdvertisingData::new(data).unwrap(),
    )
}

#[test]
fn core_sample_adv_nonconn_ind_roundtrips_at_the_air_interface_boundary() {
    let advertisement = sample_advertisement(&[1, 2, 3]);
    let mut encoded = [0; LEGACY_ADVERTISING_PDU_CAPACITY];
    let length = advertisement.encode(&mut encoded).unwrap();

    assert_eq!(
        &encoded[..length],
        &[0x42, 0x09, 0xa6, 0xa5, 0xa4, 0xa3, 0xa2, 0xc1, 1, 2, 3]
    );
    assert_eq!(
        LegacyNonconnectableAdvertisement::decode(&encoded[..length]),
        Ok(advertisement)
    );
}

#[test]
fn a_scannable_advertisement_is_an_adv_scan_ind() {
    let scannable = LegacyNonconnectableAdvertisement::new(
        LegacyNonconnectableKind::Scannable,
        LeDeviceAddress::from_wire_bytes([1, 2, 3, 4, 5, 6], LeDeviceAddressKind::Public),
        LegacyAdvertisingData::new(&[9]).unwrap(),
    );
    let mut encoded = [0; LEGACY_ADVERTISING_PDU_CAPACITY];
    let length = scannable.encode(&mut encoded).unwrap();
    assert_eq!(&encoded[..length], &[0x06, 7, 1, 2, 3, 4, 5, 6, 9]);
    assert_eq!(
        LegacyNonconnectableAdvertisement::decode(&encoded[..length]),
        Ok(scannable)
    );
}

#[test]
fn owned_data_survives_ephemeral_source_reuse() {
    let mut source = [2, 1, 6];
    let owned = LegacyAdvertisingData::new_owned(&source).expect("the data fits");
    source.fill(0xff);
    assert_eq!(owned.as_bytes(), &[2, 1, 6]);
    assert_eq!(owned, LegacyAdvertisingData::new_owned(&[2, 1, 6]).unwrap());
}

#[test]
fn every_legacy_data_length_encodes_and_destination_pressure_is_lossless() {
    let bytes = [0x5a; LEGACY_ADVERTISING_DATA_CAPACITY];
    for length in 0..=LEGACY_ADVERTISING_DATA_CAPACITY {
        let advertisement = sample_advertisement(&bytes[..length]);
        let required = advertisement.encoded_len();
        let mut encoded = [0; LEGACY_ADVERTISING_PDU_CAPACITY];
        assert_eq!(advertisement.encode(&mut encoded), Ok(required));
        assert_eq!(
            LegacyNonconnectableAdvertisement::decode(&encoded[..required]),
            Ok(advertisement)
        );

        let mut short = [0; LEGACY_ADVERTISING_PDU_CAPACITY - 1];
        assert_eq!(
            advertisement.encode(&mut short[..required - 1]),
            Err(LegacyAdvertisingEncodeError::DestinationTooSmall {
                required,
                available: required - 1,
            })
        );
    }

    assert_eq!(
        LegacyAdvertisingData::new(&[0; LEGACY_ADVERTISING_DATA_CAPACITY + 1]),
        Err(LegacyAdvertisingDataError::TooLong { length: 32 })
    );
}

#[test]
fn malformed_or_trailing_pdu_input_fails_closed() {
    assert_eq!(
        LegacyNonconnectableAdvertisement::decode(&[0x42]),
        Err(LegacyAdvertisingDecodeError::TruncatedHeader { available: 1 })
    );
    assert_eq!(
        LegacyNonconnectableAdvertisement::decode(&[0x40, 6, 0, 0, 0, 0, 0, 0]),
        Err(LegacyAdvertisingDecodeError::UnexpectedPduType { pdu_type: 0 })
    );
    assert_eq!(
        LegacyNonconnectableAdvertisement::decode(&[0x62, 6, 0, 0, 0, 0, 0, 0]),
        Err(LegacyAdvertisingDecodeError::ReservedHeaderBitsSet)
    );
    assert_eq!(
        LegacyNonconnectableAdvertisement::decode(&[0x42, 5, 0, 0, 0, 0, 0]),
        Err(LegacyAdvertisingDecodeError::InvalidPayloadLength { length: 5 })
    );
    assert_eq!(
        LegacyNonconnectableAdvertisement::decode(&[0x42, 6, 0, 0, 0, 0, 0, 0, 0]),
        Err(LegacyAdvertisingDecodeError::LengthMismatch {
            declared: 8,
            available: 9,
        })
    );
}

#[test]
fn every_owned_prefix_matches_its_borrowed_source_and_rejects_over_capacity() {
    let source = [0x5a; LEGACY_ADVERTISING_DATA_CAPACITY + 1];
    for length in 0..=LEGACY_ADVERTISING_DATA_CAPACITY {
        let owned = LegacyAdvertisingData::new_owned(&source[..length]).unwrap();
        assert_eq!(owned.as_bytes(), &source[..length]);
        assert_eq!(owned.len(), length);
        let borrowed = LegacyAdvertisingData::new(&source[..length]).unwrap();
        let mut a = [0; LEGACY_ADVERTISING_PDU_CAPACITY];
        let mut b = [0; LEGACY_ADVERTISING_PDU_CAPACITY];
        let address = sample_advertisement(&[]).advertiser();
        let owned = LegacyNonconnectableAdvertisement::new(
            LegacyNonconnectableKind::NonScannable,
            address,
            owned,
        );
        let borrowed = LegacyNonconnectableAdvertisement::new(
            LegacyNonconnectableKind::NonScannable,
            address,
            borrowed,
        );
        assert_eq!(owned.encode(&mut a), borrowed.encode(&mut b));
        assert_eq!(a, b);
    }
    assert_eq!(
        LegacyAdvertisingData::new_owned(&source),
        Err(super::LegacyAdvertisingDataError::TooLong {
            length: source.len()
        })
    );
}

#[test]
fn all_nonconnectable_encodings_match_the_previous_wire_bytes_without_partial_writes() {
    for (kind, public_octet, random_octet) in [
        (LegacyNonconnectableKind::NonScannable, 0x02, 0x42),
        (LegacyNonconnectableKind::Scannable, 0x06, 0x46),
    ] {
        for (class, octet) in [
            (LeDeviceAddressKind::Public, public_octet),
            (LeDeviceAddressKind::Random, random_octet),
        ] {
            for length in 0..=31 {
                let data = [0x5a; 31];
                let advertisement = LegacyNonconnectableAdvertisement::new(
                    kind,
                    LeDeviceAddress::from_wire_bytes([1, 2, 3, 4, 5, 6], class),
                    LegacyAdvertisingData::new(&data[..length]).unwrap(),
                );
                let mut expected = [0x5a; 39];
                expected[..8].copy_from_slice(&[octet, (6 + length) as u8, 1, 2, 3, 4, 5, 6]);
                let mut encoded = [0xa5; 40];
                assert_eq!(advertisement.encode(&mut encoded), Ok(8 + length));
                assert_eq!(&encoded[..8 + length], &expected[..8 + length]);
                assert!(encoded[8 + length..].iter().all(|byte| *byte == 0xa5));
                for available in 0..8 + length {
                    let mut short = [0xa5; 39];
                    assert_eq!(
                        advertisement.encode(&mut short[..available]),
                        Err(LegacyAdvertisingEncodeError::DestinationTooSmall {
                            required: 8 + length,
                            available
                        })
                    );
                    assert_eq!(short, [0xa5; 39]);
                }
            }
        }
    }
}

#[test]
fn nonconnectable_decoding_preserves_every_header_and_length_decision() {
    for octet in u8::MIN..=u8::MAX {
        for length in u8::MIN..=u8::MAX {
            let pdu = [octet, length, 1, 2, 3, 4, 5, 6];
            let result = LegacyNonconnectableAdvertisement::decode(&pdu);
            let code = octet % 16;
            if code != 2 && code != 6 {
                assert_eq!(
                    result,
                    Err(LegacyAdvertisingDecodeError::UnexpectedPduType { pdu_type: code })
                );
            } else if ![0x02, 0x42, 0x06, 0x46].contains(&octet) {
                assert_eq!(
                    result,
                    Err(LegacyAdvertisingDecodeError::ReservedHeaderBitsSet)
                );
            } else if !(6..=37).contains(&length) {
                assert_eq!(
                    result,
                    Err(LegacyAdvertisingDecodeError::InvalidPayloadLength {
                        length: usize::from(length)
                    })
                );
            } else if length != 6 {
                assert_eq!(
                    result,
                    Err(LegacyAdvertisingDecodeError::LengthMismatch {
                        declared: 2 + usize::from(length),
                        available: 8
                    })
                );
            } else {
                let decoded = result.unwrap();
                assert_eq!(
                    decoded.kind(),
                    if code == 2 {
                        LegacyNonconnectableKind::NonScannable
                    } else {
                        LegacyNonconnectableKind::Scannable
                    }
                );
                assert_eq!(decoded.advertiser().wire_bytes(), [1, 2, 3, 4, 5, 6]);
                assert!(decoded.data().is_empty());
            }
        }
    }
}
