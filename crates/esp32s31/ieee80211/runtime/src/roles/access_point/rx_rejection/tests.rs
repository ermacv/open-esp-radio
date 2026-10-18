use oer_ieee80211_mac::{
    ccmp::{CcmpKeyId, CcmpPacketNumber},
    data::DataDecapError,
};

use super::*;

#[test]
fn rejection_extracts_qos_ccmp_context_without_retaining_payload() {
    let mut mpdu = [0_u8; 64];
    mpdu[..2].copy_from_slice(&0x4188_u16.to_le_bytes());
    mpdu[10..16].copy_from_slice(&[2, 3, 4, 5, 6, 7]);
    mpdu[22..24].copy_from_slice(&0x1232_u16.to_le_bytes());
    mpdu[24] = 6;
    mpdu[26..34].copy_from_slice(
        &CcmpHeader::new(
            CcmpPacketNumber::from_header_bytes([5, 0, 0, 0, 0, 0, 0, 0]),
            CcmpKeyId::PAIRWISE,
        )
        .encode(),
    );
    let record = AccessPointRxRejection::from_mpdu(
        AccessPointRxRejectionReason::Data(DataDecapError::InvalidLlcSnap),
        &mpdu,
        900,
    );
    assert_eq!(record.transmitter, Some([2, 3, 4, 5, 6, 7]));
    assert_eq!(record.at_micros, 900);
    assert_eq!(record.sequence_control, Some(0x1232));
    assert_eq!(record.tid, Some(6));
    assert_eq!(record.key_id, Some(0));
    assert_eq!(record.packet_number, Some(5));
    mpdu[34..].fill(0xff);
    assert_eq!(
        AccessPointRxRejection::from_mpdu(record.reason, &mpdu, 900),
        record
    );
}

#[test]
fn truncated_headers_remain_unknown_instead_of_fabricating_identifiers() {
    let record = AccessPointRxRejection::from_mpdu(
        AccessPointRxRejectionReason::Data(DataDecapError::Truncated),
        &[0x88],
        12,
    );
    assert_eq!(record.frame_control, None);
    assert_eq!(record.transmitter, None);
    assert_eq!(record.sequence_control, None);
    assert_eq!(record.tid, None);
    assert_eq!(record.packet_number, None);
}

#[test]
fn later_errors_do_not_overwrite_the_first_failure_or_its_time() {
    let mut observation = AccessPointControlObservation::default();
    let segment = RxSegment {
        descriptor_address: 0,
        descriptor_word0: 0,
        buffer: &[],
        next_descriptor_address: 0,
    };
    let first = ApRxDispatch::Rejected(ApRxError::Data(DataDecapError::Truncated));
    observation.record_dispatch_rejection(first, segment, 42);
    let retained = observation.first_rx_protocol_rejection.unwrap();
    observation.record_rx_rejection(
        AccessPointRxRejectionReason::DeferredOutputCapacity,
        segment,
        43,
    );
    assert_eq!(observation.first_rx_protocol_rejection, Some(retained));
    assert_eq!(retained.at_micros, 42);
    assert_eq!(
        retained.reason,
        AccessPointRxRejectionReason::Data(DataDecapError::Truncated)
    );
    assert_eq!(
        AccessPointControlObservation::default().first_rx_protocol_rejection,
        None
    );
}
