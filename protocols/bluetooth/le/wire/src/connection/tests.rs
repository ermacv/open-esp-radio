use super::*;

const INITIATOR: [u8; 6] = [1, 2, 3, 4, 5, 6];
const ADVERTISER: [u8; 6] = [7, 8, 9, 10, 11, 12];

pub(super) fn connection_request(
    channel_selection_two: bool,
) -> [u8; LEGACY_CONNECT_IND_PDU_BYTES] {
    let mut pdu = [0; LEGACY_CONNECT_IND_PDU_BYTES];
    pdu[0] = CONNECT_IND_TYPE
        | if channel_selection_two {
            CHANNEL_SELECTION_TWO
        } else {
            0
        };
    pdu[1] = LEGACY_CONNECT_IND_PAYLOAD_BYTES as u8;
    pdu[2..8].copy_from_slice(&INITIATOR);
    pdu[8..14].copy_from_slice(&ADVERTISER);
    pdu[14..18].copy_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
    pdu[18..21].copy_from_slice(&[0x33, 0x22, 0x11]);
    pdu[21] = 2;
    pdu[22..24].copy_from_slice(&1u16.to_le_bytes());
    pdu[24..26].copy_from_slice(&24u16.to_le_bytes());
    pdu[26..28].copy_from_slice(&0u16.to_le_bytes());
    pdu[28..30].copy_from_slice(&200u16.to_le_bytes());
    pdu[30..35].copy_from_slice(&LeDataChannelMap::all().wire_bytes());
    pdu[35] = 5 | (4 << 5);
    pdu
}

#[test]
fn complete_connect_ind_becomes_semantic_peripheral_input() {
    let request = LeLegacyConnectionRequest::decode(&connection_request(true)).unwrap();

    assert_eq!(request.initiator().wire_bytes(), INITIATOR);
    assert_eq!(request.advertiser().wire_bytes(), ADVERTISER);
    assert_eq!(request.access_address().value(), 0xa1b2_c3d4);
    assert_eq!(request.crc_initialization().value(), 0x11_2233);
    assert_eq!(request.timing().interval_micros(), 30_000);
    assert_eq!(request.timing().first_window_start_micros(), 2_500);
    assert_eq!(request.timing().first_window_end_micros(), 5_000);
    assert_eq!(request.channel_map().used_channel_count(), 37);
    assert_eq!(request.hop_increment(), 5);
    assert_eq!(request.sleep_clock_accuracy().encoded(), 4);
    assert_eq!(request.sleep_clock_accuracy().worst_case_ppm(), 75);
    assert_eq!(
        request.channel_selection(),
        LeChannelSelectionAlgorithm::AlgorithmTwo
    );
}

#[test]
fn every_sleep_clock_accuracy_class_exposes_its_worst_case_ppm() {
    for (encoded, expected_ppm) in [500, 250, 150, 100, 75, 50, 30, 20].into_iter().enumerate() {
        let mut pdu = connection_request(false);
        pdu[35] = 5 | ((encoded as u8) << 5);
        let request = LeLegacyConnectionRequest::decode(&pdu).unwrap();

        assert_eq!(
            request.sleep_clock_accuracy().worst_case_ppm(),
            expected_ppm
        );
    }
}

#[test]
fn malformed_request_fails_before_connection_ownership_exists() {
    let mut pdu = connection_request(false);
    pdu[0] |= HEADER_RESERVED;
    assert_eq!(
        LeLegacyConnectionRequest::decode(&pdu),
        Err(LeLegacyConnectionRequestError::ReservedHeaderBitsSet)
    );

    let mut pdu = connection_request(false);
    pdu[21] = 0;
    assert_eq!(
        LeLegacyConnectionRequest::decode(&pdu),
        Err(LeLegacyConnectionRequestError::Timing(
            LeConnectionTimingError::WindowSizeOutsideRange
        ))
    );

    let mut pdu = connection_request(false);
    pdu[30..35].copy_from_slice(&[1, 0, 0, 0, 0]);
    assert_eq!(
        LeLegacyConnectionRequest::decode(&pdu),
        Err(LeLegacyConnectionRequestError::ChannelMap(
            LeDataChannelMapError::FewerThanTwoUsedChannels
        ))
    );
}

#[test]
fn access_address_rejects_intrinsically_ambiguous_patterns() {
    assert_eq!(
        LeUncodedAccessAddress::new(ADVERTISING_ACCESS_ADDRESS),
        Err(LeUncodedAccessAddressError::AdvertisingAddressOrOneBitAway)
    );
    assert_eq!(
        LeUncodedAccessAddress::new(0x1212_1212),
        Err(LeUncodedAccessAddressError::AllOctetsEqual)
    );
    assert!(LeUncodedAccessAddress::new(0xa1b2_c3d4).is_ok());
}

#[test]
fn supervision_relation_is_strict_and_lossless() {
    assert_eq!(
        LeConnectionTiming::new(1, 0, 24, 4, 30),
        Err(LeConnectionTimingError::SupervisionTimeoutTooShort)
    );
    assert!(LeConnectionTiming::new(1, 0, 24, 4, 31).is_ok());
}

#[test]
fn sparse_channel_maps_remap_ranks_cyclically_without_selecting_unused_channels() {
    for first in 0..37 {
        for second in first + 1..37 {
            let mut bytes = [0; 5];
            bytes[first / 8] |= 1 << (first % 8);
            bytes[second / 8] |= 1 << (second % 8);
            let map = LeDataChannelMap::new(bytes).unwrap();
            assert_eq!(map.wire_bytes(), bytes);
            assert_eq!(map.used_channel_count(), 2);
            for rank in 0..=u8::MAX {
                let selected = map.remap(rank);
                assert_eq!(
                    usize::from(selected.get()),
                    if rank % 2 == 0 { first } else { second }
                );
                assert!(map.contains(selected));
            }
        }
    }
}

#[test]
fn maximum_admitted_connection_timing_keeps_widened_arithmetic_exact() {
    let timing = LeConnectionTiming::new(8, 3200, 3200, 0, 3200).unwrap();
    assert_eq!(timing.interval_micros(), 4_000_000);
    assert_eq!(timing.supervision_timeout_micros(), 32_000_000);
    assert_eq!(timing.first_window_start_micros(), 4_001_250);
    assert_eq!(timing.first_window_end_micros(), 4_011_250);
    assert!(LeConnectionTiming::new(8, 3200, 3200, 499, 3200).is_err());
    assert!(LeConnectionTiming::new(1, 0, 6, 499, 3200).is_ok());
}
