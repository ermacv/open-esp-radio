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
