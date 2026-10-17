use super::*;
use crate::{
    advertising::LEGACY_ADVERTISING_PDU_CAPACITY,
    connection::{
        LEGACY_CONNECT_IND_PAYLOAD_BYTES, LEGACY_CONNECT_IND_PDU_BYTES,
        LeChannelSelectionAlgorithm, LeLegacyConnectionRequest, LePeripheralConnection,
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
    pdu[0] = 0b0101 | (1 << 7) | if channel_selection_two { 1 << 5 } else { 0 };
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
fn legacy_channel_selection_uses_both_advertising_and_initiating_bits() {
    for (advertised, peer_two, expected) in [
        (
            LeChannelSelectionAlgorithm::AlgorithmOne,
            false,
            LeChannelSelectionAlgorithm::AlgorithmOne,
        ),
        (
            LeChannelSelectionAlgorithm::AlgorithmOne,
            true,
            LeChannelSelectionAlgorithm::AlgorithmOne,
        ),
        (
            LeChannelSelectionAlgorithm::AlgorithmTwo,
            false,
            LeChannelSelectionAlgorithm::AlgorithmOne,
        ),
        (
            LeChannelSelectionAlgorithm::AlgorithmTwo,
            true,
            LeChannelSelectionAlgorithm::AlgorithmTwo,
        ),
    ] {
        let pdu = connection_request(ADVERTISER_BYTES, peer_two);
        let request = LeLegacyConnectionRequest::decode(&pdu).unwrap();
        let connection = LePeripheralConnection::from_request(request, advertised);
        assert_eq!(connection.request(), request);
        assert_eq!(connection.channel_selection(), expected);
        let first = connection.prepare_event();
        if expected == LeChannelSelectionAlgorithm::AlgorithmOne {
            assert_eq!(first.channel().get(), 5);
        }
        assert_eq!(first.cancel().channel_selection(), expected);
    }
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
    assert_eq!(
        encoded,
        [
            0x01 | 0x20 | 0x80,
            12,
            1,
            2,
            3,
            4,
            5,
            6,
            7,
            8,
            9,
            10,
            11,
            0xcc
        ]
    );
    assert_eq!(
        directed.encode(&mut [0; 13]),
        Err(LegacyAdvertisingEncodeError::DestinationTooSmall {
            required: 14,
            available: 13,
        })
    );
}
