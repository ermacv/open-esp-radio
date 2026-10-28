use super::*;
use crate::{
    advertising::LEGACY_ADVERTISING_PDU_CAPACITY,
    connection::{
        LEGACY_CONNECT_IND_PAYLOAD_BYTES, LEGACY_CONNECT_IND_PDU_BYTES, LeLegacyConnectionRequest,
    },
};

const ADVERTISER_BYTES: [u8; 6] = [7, 8, 9, 10, 11, 12];

fn advertisement(
    support: LeChannelSelectionAlgorithmTwoSupport,
) -> LegacyConnectableAdvertisement<'static> {
    LegacyConnectableAdvertisement::new(
        LeDeviceAddress::from_wire_bytes(ADVERTISER_BYTES, LeDeviceAddressKind::Random),
        LegacyAdvertisingData::new(&[2, 1, 6]).unwrap(),
        support,
    )
}

fn connection_request(
    advertiser: [u8; 6],
    channel_selection_two: bool,
) -> [u8; LEGACY_CONNECT_IND_PDU_BYTES] {
    let mut pdu = [0; LEGACY_CONNECT_IND_PDU_BYTES];
    pdu[0] = if channel_selection_two { 0xa5 } else { 0x85 };
    pdu[1] = LEGACY_CONNECT_IND_PAYLOAD_BYTES as u8;
    pdu[2..8].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
    pdu[8..14].copy_from_slice(&advertiser);
    pdu[14..18].copy_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
    pdu[18..21].copy_from_slice(&[0x33, 0x22, 0x11]);
    pdu[21] = 2;
    pdu[22..24].copy_from_slice(&1u16.to_le_bytes());
    pdu[24..26].copy_from_slice(&24u16.to_le_bytes());
    pdu[26..28].copy_from_slice(&0u16.to_le_bytes());
    pdu[28..30].copy_from_slice(&200u16.to_le_bytes());
    pdu[30..35].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x1f]);
    pdu[35] = 5 | (4 << 5);
    pdu
}

#[test]
fn adv_ind_roundtrips_with_channel_selection_capability() {
    let advertisement = advertisement(LeChannelSelectionAlgorithmTwoSupport::Supported);
    let mut encoded = [0; LEGACY_ADVERTISING_PDU_CAPACITY];
    let length = advertisement.encode(&mut encoded).unwrap();

    assert_eq!(
        LegacyConnectableAdvertisement::decode(&encoded[..length]),
        Ok(advertisement)
    );
    assert_eq!(encoded[0], 0x60);
    assert_eq!(&encoded[2..8], &ADVERTISER_BYTES);
}

#[test]
fn a_connection_indication_names_its_advertiser() {
    let advertiser = advertisement(LeChannelSelectionAlgorithmTwoSupport::Supported).advertiser();
    let request =
        LeLegacyConnectionRequest::decode(&connection_request(ADVERTISER_BYTES, true)).unwrap();
    assert!(request.is_addressed_to(advertiser));
    let other = LeLegacyConnectionRequest::decode(&connection_request([9; 6], true)).unwrap();
    assert!(!other.is_addressed_to(advertiser));
}

#[test]
fn a_scan_response_carries_the_advertiser_and_its_data() {
    let data = LegacyScanResponseData::new(&[3, 9, 8, 7]).unwrap();
    let advertiser =
        LeDeviceAddress::from_wire_bytes(ADVERTISER_BYTES, LeDeviceAddressKind::Random);
    let mut pdu = [0; LEGACY_ADVERTISING_PDU_CAPACITY];
    let length = data.encode(advertiser, &mut pdu).unwrap();
    assert_eq!(&pdu[..length], [0x44, 10, 7, 8, 9, 10, 11, 12, 3, 9, 8, 7]);
    assert!(data.encode(advertiser, &mut [0; 11]).is_err());
}

#[test]
fn a_directed_advertisement_carries_both_addresses_and_their_kinds() {
    let advertiser =
        LeDeviceAddress::from_wire_bytes([1, 2, 3, 4, 5, 6], LeDeviceAddressKind::Public);
    let target =
        LeDeviceAddress::from_wire_bytes([7, 8, 9, 10, 11, 0xcc], LeDeviceAddressKind::Random);
    let directed = LegacyDirectedAdvertisement::new(
        advertiser,
        target,
        LeChannelSelectionAlgorithmTwoSupport::Supported,
    );
    let mut encoded = [0; 14];
    assert_eq!(directed.encode(&mut encoded), Ok(14));
    // ADV_DIRECT_IND, ChSel, public TxAdd, random RxAdd.
    assert_eq!(encoded, [0xa1, 12, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 0xcc]);
    assert_eq!(
        directed.encode(&mut [0; 13]),
        Err(LegacyAdvertisingEncodeError::DestinationTooSmall {
            required: 14,
            available: 13,
        })
    );
}

#[test]
fn connectable_and_scan_response_encodings_match_previous_wire_bytes() {
    for (class, adv_octets, scan_octet) in [
        (LeDeviceAddressKind::Public, [0x00, 0x20], 0x04),
        (LeDeviceAddressKind::Random, [0x40, 0x60], 0x44),
    ] {
        let address = LeDeviceAddress::from_wire_bytes(ADVERTISER_BYTES, class);
        for (support, octet) in [
            LeChannelSelectionAlgorithmTwoSupport::Unsupported,
            LeChannelSelectionAlgorithmTwoSupport::Supported,
        ]
        .into_iter()
        .zip(adv_octets)
        {
            for length in 0..=31 {
                let data = [0x5a; 31];
                let advertisement = LegacyConnectableAdvertisement::new(
                    address,
                    LegacyAdvertisingData::new(&data[..length]).unwrap(),
                    support,
                );
                let response = LegacyScanResponseData::new(&data[..length]).unwrap();
                let mut expected = [0x5a; 39];
                expected[1] = (6 + length) as u8;
                expected[2..8].copy_from_slice(&ADVERTISER_BYTES);
                let mut encoded = [0xa5; 40];
                expected[0] = octet;
                assert_eq!(advertisement.encode(&mut encoded), Ok(8 + length));
                assert_eq!(&encoded[..8 + length], &expected[..8 + length]);
                assert_eq!(
                    LegacyConnectableAdvertisement::decode(&encoded[..8 + length]),
                    Ok(advertisement)
                );
                assert!(encoded[8 + length..].iter().all(|byte| *byte == 0xa5));
                expected[0] = scan_octet;
                assert_eq!(response.encode(address, &mut encoded), Ok(8 + length));
                assert_eq!(&encoded[..8 + length], &expected[..8 + length]);
                for available in 0..8 + length {
                    let mut short = [0xa5; 39];
                    let error = Err(LegacyAdvertisingEncodeError::DestinationTooSmall {
                        required: 8 + length,
                        available,
                    });
                    assert_eq!(advertisement.encode(&mut short[..available]), error);
                    assert_eq!(short, [0xa5; 39]);
                    assert_eq!(response.encode(address, &mut short[..available]), error);
                    assert_eq!(short, [0xa5; 39]);
                }
            }
        }
    }
}

#[test]
fn directed_encodings_match_all_previous_address_and_channel_selection_flags() {
    for (advertiser_kind, octets) in [
        (LeDeviceAddressKind::Public, [0x01, 0x21, 0x81, 0xa1]),
        (LeDeviceAddressKind::Random, [0x41, 0x61, 0xc1, 0xe1]),
    ] {
        let mut octets = octets.into_iter();
        for target_kind in [LeDeviceAddressKind::Public, LeDeviceAddressKind::Random] {
            for support in [
                LeChannelSelectionAlgorithmTwoSupport::Unsupported,
                LeChannelSelectionAlgorithmTwoSupport::Supported,
            ] {
                let octet = octets.next().unwrap();
                let directed = LegacyDirectedAdvertisement::new(
                    LeDeviceAddress::from_wire_bytes([1, 2, 3, 4, 5, 6], advertiser_kind),
                    LeDeviceAddress::from_wire_bytes([7, 8, 9, 10, 11, 12], target_kind),
                    support,
                );
                let mut encoded = [0xa5; 15];
                assert_eq!(directed.encode(&mut encoded), Ok(14));
                assert_eq!(
                    encoded,
                    [octet, 12, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 0xa5]
                );
                for available in 0..14 {
                    let mut short = [0xa5; 14];
                    assert_eq!(
                        directed.encode(&mut short[..available]),
                        Err(LegacyAdvertisingEncodeError::DestinationTooSmall {
                            required: 14,
                            available
                        })
                    );
                    assert_eq!(short, [0xa5; 14]);
                }
            }
        }
    }
}

#[test]
fn connectable_decoding_preserves_every_header_and_length_decision() {
    for octet in u8::MIN..=u8::MAX {
        for length in u8::MIN..=u8::MAX {
            let pdu = [octet, length, 1, 2, 3, 4, 5, 6];
            let result = LegacyConnectableAdvertisement::decode(&pdu);
            let code = octet % 16;
            if code != 0 {
                assert_eq!(
                    result,
                    Err(
                        LegacyConnectableAdvertisementDecodeError::UnexpectedPduType {
                            pdu_type: code
                        }
                    )
                );
            } else if ![0x00, 0x20, 0x40, 0x60].contains(&octet) {
                assert_eq!(
                    result,
                    Err(LegacyConnectableAdvertisementDecodeError::ReservedHeaderBitsSet)
                );
            } else if !(6..=37).contains(&length) {
                assert_eq!(
                    result,
                    Err(
                        LegacyConnectableAdvertisementDecodeError::InvalidPayloadLength {
                            length: usize::from(length)
                        }
                    )
                );
            } else if length != 6 {
                assert_eq!(
                    result,
                    Err(LegacyConnectableAdvertisementDecodeError::LengthMismatch {
                        declared: 2 + usize::from(length),
                        available: 8
                    })
                );
            } else {
                let decoded = result.unwrap();
                assert_eq!(decoded.advertiser().wire_bytes(), [1, 2, 3, 4, 5, 6]);
                assert_eq!(
                    decoded.channel_selection_two(),
                    if [0x20, 0x60].contains(&octet) {
                        LeChannelSelectionAlgorithmTwoSupport::Supported
                    } else {
                        LeChannelSelectionAlgorithmTwoSupport::Unsupported
                    }
                );
            }
        }
    }
}
