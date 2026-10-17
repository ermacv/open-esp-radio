use super::*;
use crate::sequence::seq;

const RA: [u8; 6] = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
const TA: [u8; 6] = [0x70, 0x15, 0xfb, 0xa8, 0x48, 0xf0];

fn two_station_frame() -> [u8; 25] {
    let mut frame = [0_u8; 25];
    frame[0] = NDPA_FRAME_CONTROL as u8;
    frame[1] = 0x08;
    frame[2..4].copy_from_slice(&0x1234_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&RA);
    frame[10..16].copy_from_slice(&TA);
    frame[16] = (0x2a << 2) | HE_MARKER;
    frame[17..21].copy_from_slice(&0xa5a5_8123_u32.to_le_bytes());
    frame[21..25].copy_from_slice(&0x5a5a_8456_u32.to_le_bytes());
    frame
}

#[test]
fn decodes_the_complete_blob_geometry_and_aid_membership() {
    let frame = two_station_frame();
    let ndpa = HeNdpa::parse(&frame).unwrap();

    assert_eq!(ndpa.frame_control(), 0x0854);
    assert_eq!(ndpa.duration(), 0x1234);
    assert_eq!(ndpa.receiver_address(), &RA);
    assert_eq!(ndpa.transmitter_address(), &TA);
    assert_eq!(ndpa.dialog_token(), 0x2a);
    assert_eq!(ndpa.stations().len(), 2);
    let mut stations = ndpa.stations();
    assert_eq!(
        stations.next(),
        Some(HeNdpaStationInfo { raw: 0xa5a5_8123 })
    );
    assert_eq!(
        stations.next(),
        Some(HeNdpaStationInfo { raw: 0x5a5a_8456 })
    );
    assert_eq!(stations.next(), None);
    assert!(ndpa.contains_association_id(0x123));
    assert!(ndpa.contains_association_id(0x456));
    assert!(!ndpa.contains_association_id(0x124));
    assert!(!ndpa.contains_association_id(0x0800));
}

#[test]
fn fails_closed_on_non_he_or_partial_station_info() {
    let mut frame = two_station_frame();
    frame[16] &= !HE_MARKER;
    assert_eq!(HeNdpa::parse(&frame), Err(HeNdpaError::NotHe));

    frame[16] |= HE_MARKER;
    frame[0] = 0x44;
    assert_eq!(HeNdpa::parse(&frame), Err(HeNdpaError::NotNdpa));

    frame[0] = NDPA_FRAME_CONTROL as u8;
    assert_eq!(
        HeNdpa::parse(&frame[..24]),
        Err(HeNdpaError::MisalignedStationInfo)
    );
    assert_eq!(HeNdpa::parse(&frame[..20]), Err(HeNdpaError::TooShort));
}

#[test]
fn encodes_the_complete_vendor_he20_sounding_exchange_ndpa() {
    let station = HeNdpaStationEncoding {
        association_id: 29,
        resource_unit_start_index: 0,
        resource_unit_end_index: 8,
        feedback_type_and_ng_encoding: 0,
        disambiguation: true,
        codebook_size: false,
        nc_encoding: 0,
    };
    assert_eq!(station.encode(), Ok(0x0820_001d));

    let encoding = HeNdpaEncoding {
        duration: 100,
        receiver_address: RA,
        transmitter_address: [0xdc, 0x15, 0xc8, 0x54, 0xbc, 0x1e],
        dialog_token: 0x37,
        stations: core::slice::from_ref(&station),
    };
    let mut frame = [0_u8; 21];
    assert_eq!(encoding.encode(&mut frame), Ok(frame.len()));
    assert_eq!(
        frame,
        [
            0x54, 0x00, 0x64, 0x00, 0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0, 0xdc, 0x15, 0xc8, 0x54,
            0xbc, 0x1e, 0xde, 0x1d, 0x00, 0x20, 0x08,
        ]
    );

    let decoded = HeNdpa::parse(&frame).unwrap();
    let decoded_station = decoded.stations().next().unwrap();
    assert_eq!(decoded_station.association_id(), 29);
    assert_eq!(decoded_station.resource_unit_start_index(), 0);
    assert_eq!(decoded_station.resource_unit_end_index(), 8);
    assert_eq!(decoded_station.feedback_type_and_ng_encoding(), 0);
    assert!(decoded_station.disambiguation());
    assert!(!decoded_station.codebook_size());
    assert_eq!(decoded_station.nc_encoding(), 0);
}

#[test]
fn ndpa_encoder_rejects_every_unowned_boundary() {
    let valid = HeNdpaStationEncoding {
        association_id: 1,
        resource_unit_start_index: 0,
        resource_unit_end_index: 8,
        feedback_type_and_ng_encoding: 0,
        disambiguation: true,
        codebook_size: false,
        nc_encoding: 0,
    };
    assert_eq!(
        HeNdpaStationEncoding {
            association_id: 0x0800,
            ..valid
        }
        .encode(),
        Err(HeNdpaEncodingError::AssociationIdOutOfRange)
    );
    assert_eq!(
        HeNdpaStationEncoding {
            resource_unit_start_index: 9,
            resource_unit_end_index: 8,
            ..valid
        }
        .encode(),
        Err(HeNdpaEncodingError::ReversedResourceUnitRange)
    );
    assert_eq!(
        HeNdpaStationEncoding {
            resource_unit_end_index: 0x80,
            ..valid
        }
        .encode(),
        Err(HeNdpaEncodingError::ResourceUnitIndexOutOfRange)
    );
    assert_eq!(
        HeNdpaStationEncoding {
            feedback_type_and_ng_encoding: 4,
            ..valid
        }
        .encode(),
        Err(HeNdpaEncodingError::FeedbackTypeAndNgOutOfRange)
    );
    assert_eq!(
        HeNdpaStationEncoding {
            nc_encoding: 8,
            ..valid
        }
        .encode(),
        Err(HeNdpaEncodingError::NcOutOfRange)
    );

    let encoding = HeNdpaEncoding {
        duration: 100,
        receiver_address: RA,
        transmitter_address: TA,
        dialog_token: 64,
        stations: core::slice::from_ref(&valid),
    };
    assert_eq!(
        encoding.encode(&mut [0_u8; 21]),
        Err(HeNdpaEncodingError::DialogTokenOutOfRange)
    );
    assert_eq!(
        HeNdpaEncoding {
            dialog_token: 1,
            stations: &[],
            ..encoding
        }
        .encode(&mut [0_u8; 21]),
        Err(HeNdpaEncodingError::NoStations)
    );
    assert_eq!(
        HeNdpaEncoding {
            dialog_token: 1,
            stations: core::slice::from_ref(&valid),
            ..encoding
        }
        .encode(&mut [0_u8; 20]),
        Err(HeNdpaEncodingError::OutputTooShort)
    );
}

#[test]
fn decodes_the_complete_vendor_he20_compressed_feedback_header() {
    let mut report = [0_u8; 112];
    report[0..2].copy_from_slice(&ACTION_NO_ACK_FRAME_CONTROL.to_le_bytes());
    report[4..10].copy_from_slice(&[0xdc, 0x15, 0xc8, 0x54, 0xbc, 0x1e]);
    report[10..16].copy_from_slice(&RA);
    report[16..22].copy_from_slice(&[0xdc, 0x15, 0xc8, 0x54, 0xbc, 0x1e]);
    report[22..24].copy_from_slice(&0x00a0_u16.to_le_bytes());
    report[24] = HE_ACTION_CATEGORY;
    report[25] = HE_COMPRESSED_BEAMFORMING_AND_CQI_ACTION;
    report[26..31].copy_from_slice(&[0x08, 0x82, 0x00, 0xc4, 0x0d]);
    report[31] = 0x14;
    report[32..].copy_from_slice(&[
        0x51, 0x47, 0x1d, 0x75, 0xd4, 0x51, 0x43, 0x1d, 0x75, 0xd4, 0x50, 0x47, 0x0d, 0x75, 0xd4,
        0x51, 0x47, 0x0d, 0x35, 0xd4, 0x51, 0x43, 0x1d, 0x75, 0xd4, 0x51, 0x43, 0x1d, 0x75, 0xd4,
        0x52, 0x47, 0x1d, 0x75, 0xd4, 0x51, 0x47, 0x0d, 0xb5, 0xd4, 0x51, 0x4b, 0x2d, 0x75, 0xd4,
        0x51, 0x47, 0x1d, 0xb5, 0xd4, 0x52, 0x47, 0x2d, 0x75, 0xd4, 0x52, 0x47, 0x2d, 0x75, 0xd4,
        0x52, 0x4b, 0x2d, 0xb5, 0xd4, 0x52, 0x4b, 0x2d, 0xb5, 0xd4, 0x52, 0x4b, 0x2d, 0x75, 0xd4,
        0x52, 0x4b, 0x1d, 0xb5, 0xe4,
    ]);

    let decoded = HeCompressedBeamformingReport::parse(&report).unwrap();
    assert_eq!(
        decoded.receiver_address(),
        &[0xdc, 0x15, 0xc8, 0x54, 0xbc, 0x1e]
    );
    assert_eq!(decoded.transmitter_address(), &RA);
    assert_eq!(decoded.sequence_number(), seq(10));
    assert_eq!(decoded.mimo_control(), 0x000d_c400_8208);
    assert_eq!(decoded.column_count(), 1);
    assert_eq!(decoded.row_count(), 2);
    assert_eq!(decoded.bandwidth_encoding(), 0);
    assert!(!decoded.grouping());
    assert!(decoded.codebook_information());
    assert_eq!(decoded.feedback_type_encoding(), 0);
    assert_eq!(decoded.remaining_feedback_segments(), 0);
    assert!(decoded.first_feedback_segment());
    assert_eq!(decoded.resource_unit_start_index(), 0);
    assert_eq!(decoded.resource_unit_end_index(), 8);
    assert_eq!(decoded.sounding_dialog_token(), 0x37);
    assert_eq!(decoded.reserved(), 0);
    assert_eq!(decoded.average_snr(), &[0x14]);
    assert_eq!(decoded.feedback_matrices().len(), 80);
}

#[test]
fn compressed_feedback_parser_rejects_other_actions_and_missing_snr() {
    let mut report = [0_u8; 32];
    report[0..2].copy_from_slice(&ACTION_NO_ACK_FRAME_CONTROL.to_le_bytes());
    report[24] = HE_ACTION_CATEGORY;
    report[25] = HE_COMPRESSED_BEAMFORMING_AND_CQI_ACTION;
    report[26] = 1;
    assert_eq!(
        HeCompressedBeamformingReport::parse(&report[..31]),
        Err(HeCompressedBeamformingReportError::MissingAverageSnr)
    );
    report[25] = 1;
    assert_eq!(
        HeCompressedBeamformingReport::parse(&report),
        Err(HeCompressedBeamformingReportError::NotHeCompressedBeamformingAndCqi)
    );
    report[25] = HE_COMPRESSED_BEAMFORMING_AND_CQI_ACTION;
    report[0] = 0xd0;
    assert_eq!(
        HeCompressedBeamformingReport::parse(&report),
        Err(HeCompressedBeamformingReportError::NotActionNoAck)
    );
}
