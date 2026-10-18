use super::*;
use oer_ieee80211_mac::ccmp::CcmpPacketNumber;

#[test]
fn replay_evidence_keeps_both_observed_and_highest_packet_numbers() {
    let number = |n| CcmpPacketNumber::from_header_bytes([n, 0, 0, 0, 0, 0, 0, 0]);
    let record = AccessPointRxRejection {
        reason: DriverReason::Replay(CcmpReplayError::Replayed {
            packet_number: number(5),
            highest: number(7),
        }),
        at_micros: 12345,
        transmitter: Some([2; 6]),
        frame_control: Some(0x4188),
        sequence_control: Some(32),
        tid: Some(0),
        key_id: Some(0),
        packet_number: Some(5),
        mpdu_length: 100,
    };
    let wire = evidence(record);
    assert_eq!(
        wire.reason,
        Reason::Replay {
            packet_number: 5,
            highest: 7
        }
    );
    assert_eq!(wire.at_micros, 12345);
    assert_eq!(wire.transmitter, Some([2; 6]));
}
