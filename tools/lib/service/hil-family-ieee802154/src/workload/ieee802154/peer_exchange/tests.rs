use oer_hil_protocol::{
    ieee802154::Ieee802154SessionAck, ieee802154::Ieee802154SessionReceivedFrame,
};

use super::*;
use crate::peer::{PeerAck, PeerFrame};

fn ack(sequence: u8, pending: bool) -> Vec<u8> {
    vec![if pending { 0x12 } else { 0x02 }, 0x00, sequence]
}

#[test]
fn frames_carry_the_documented_headers() {
    assert_eq!(
        data_frame(true, 7, PEER_SHORT, DEVICE_SHORT)[..9],
        [0x61, 0x88, 7, 0x45, 0x4f, 0x02, 0x00, 0x01, 0x00]
    );
    assert_eq!(
        data_frame(false, 7, PEER_SHORT, DEVICE_SHORT)[..2],
        [0x41, 0x88]
    );
    assert_eq!(
        data_request(9, DEVICE_SHORT, PEER_SHORT),
        [0x63, 0x88, 9, 0x45, 0x4f, 0x01, 0x00, 0x02, 0x00, 0x04]
    );
    assert_eq!(acknowledgement(&ack(3, true), 3), Some(true));
    assert_eq!(acknowledgement(&ack(3, false), 3), Some(false));
    assert_eq!(acknowledgement(&ack(3, false), 4), None);
    assert_eq!(acknowledgement(&[0x41, 0x88, 3], 3), None);
}

#[test]
fn the_device_transmit_needs_a_matching_plain_acknowledgement() {
    let evidence = |outcome, frame: Option<Vec<u8>>| Ieee802154SessionTransmitEvidence {
        result: Ieee802154SessionResult::Done,
        outcome,
        acknowledgement: frame.map(|frame| Ieee802154SessionAck {
            frame: Ieee802154SessionFrame::from_slice(&frame).unwrap(),
            rssi_dbm: -40,
            lqi: 200,
        }),
    };
    check_device_transmit(
        &evidence(Ieee802154AirTxOutcome::Success, Some(ack(5, false))),
        5,
    )
    .unwrap();
    for (outcome, frame) in [
        (Ieee802154AirTxOutcome::NoAcknowledgement, None),
        (Ieee802154AirTxOutcome::Success, None),
        (Ieee802154AirTxOutcome::Success, Some(ack(6, false))),
        (Ieee802154AirTxOutcome::Success, Some(ack(5, true))),
    ] {
        assert!(check_device_transmit(&evidence(outcome, frame), 5).is_err());
    }
}

#[test]
fn peer_reports_are_matched_exactly() {
    let frame = data_frame(true, 1, PEER_SHORT, DEVICE_SHORT);
    let received = |bytes: Vec<u8>| {
        Some(PeerEvent::Received(PeerFrame {
            bytes,
            rssi_dbm: -30,
            lqi: 255,
            pending: false,
            channel: 15,
        }))
    };
    check_peer_received(received(frame.clone()), &frame).unwrap();
    assert!(check_peer_received(received(frame[1..].to_vec()), &frame).is_err());
    assert!(check_peer_received(None, &frame).is_err());

    let transmitted = |bytes: Vec<u8>| {
        Some(PeerEvent::Transmitted {
            acknowledgement: Some(PeerAck {
                bytes,
                pending: false,
                rssi_dbm: -30,
                lqi: 255,
            }),
        })
    };
    check_peer_acknowledged(transmitted(ack(0x80, false)), 0x80, false).unwrap();
    check_peer_acknowledged(transmitted(ack(0x71, true)), 0x71, true).unwrap();
    assert!(check_peer_acknowledged(transmitted(ack(0x71, false)), 0x71, true).is_err());
    assert!(
        check_peer_acknowledged(
            Some(PeerEvent::Transmitted {
                acknowledgement: None
            }),
            0x80,
            false
        )
        .is_err()
    );
    check_peer_unacknowledged(Some(PeerEvent::TransmitFailed(3))).unwrap();
    assert!(check_peer_unacknowledged(transmitted(ack(0x70, false))).is_err());
}

#[test]
fn device_receptions_match_the_sent_frames_in_order() {
    let sent = [
        data_frame(true, 0x80, DEVICE_SHORT, PEER_SHORT),
        data_frame(true, 0x81, DEVICE_SHORT, PEER_SHORT),
    ];
    let digest = |frame: &[u8]| Ieee802154SessionReceivedFrame {
        length: frame.len() as u8,
        crc32c: ieee802154_frame_crc32c(frame),
        rssi_dbm: -30,
        lqi: 255,
    };
    let evidence = |frames: &[&Vec<u8>]| Ieee802154SessionReceiveEvidence {
        result: Ieee802154SessionResult::Done,
        total: frames.len() as u16,
        frames: frames.iter().map(|frame| digest(frame)).collect(),
    };
    check_device_received(&evidence(&[&sent[0], &sent[1]]), &sent).unwrap();
    check_device_received(&evidence(&[]), &[]).unwrap();
    assert!(check_device_received(&evidence(&[&sent[1], &sent[0]]), &sent).is_err());
    assert!(check_device_received(&evidence(&[&sent[0]]), &sent).is_err());
    let mut lost = evidence(&[&sent[0], &sent[1]]);
    lost.result = Ieee802154SessionResult::EventsLost;
    assert!(check_device_received(&lost, &sent).is_err());
}

/// The 2015 frame differs from the 2006 one only in its frame version, and
/// its enhanced ACK is an unsecured 2015 ACK to the source with the PAN ID.
#[test]
fn the_2015_exchange_carries_the_documented_headers() {
    assert_eq!(
        data_frame_2015(0x72, DEVICE_SHORT, PEER_SHORT)[..9],
        [0x61, 0xa8, 0x72, 0x45, 0x4f, 0x01, 0x00, 0x02, 0x00]
    );
    assert_eq!(
        enhanced_acknowledgement(0x72, PEER_SHORT, true),
        [0x12, 0x28, 0x72, 0x45, 0x4f, 0x02, 0x00]
    );
    assert_eq!(
        enhanced_acknowledgement(0x72, PEER_SHORT, false)[..2],
        [0x02, 0x28]
    );
}

#[test]
fn the_enhanced_ack_is_matched_exactly() {
    let expected = enhanced_acknowledgement(0x72, PEER_SHORT, true);
    let transmitted = |bytes: Vec<u8>| {
        Some(PeerEvent::Transmitted {
            acknowledgement: Some(PeerAck {
                bytes,
                pending: true,
                rssi_dbm: -30,
                lqi: 255,
            }),
        })
    };
    check_peer_enhanced_ack(transmitted(expected.clone()), &expected).unwrap();
    assert!(
        check_peer_enhanced_ack(
            transmitted(enhanced_acknowledgement(0x72, PEER_SHORT, false)),
            &expected
        )
        .is_err()
    );
    assert!(check_peer_enhanced_ack(transmitted(ack(0x72, true)), &expected).is_err());
    assert!(check_peer_enhanced_ack(Some(PeerEvent::TransmitFailed(3)), &expected).is_err());
}

#[test]
fn only_an_unacknowledged_transmit_passes_the_foreign_check() {
    let mut evidence = Ieee802154SessionTransmitEvidence {
        result: Ieee802154SessionResult::Done,
        outcome: Ieee802154AirTxOutcome::NoAcknowledgement,
        acknowledgement: None,
    };
    check_device_unacknowledged(&evidence, 0x73).unwrap();
    evidence.outcome = Ieee802154AirTxOutcome::Success;
    assert!(check_device_unacknowledged(&evidence, 0x73).is_err());
    evidence.outcome = Ieee802154AirTxOutcome::ChannelBusy;
    assert!(check_device_unacknowledged(&evidence, 0x73).is_err());
}

/// Coexistence must be taken part in exactly when requested and left
/// cleanly.
#[test]
fn coexistence_matches_the_request_and_is_left_cleanly() {
    use oer_hil_protocol::ieee802154::Ieee802154SessionCoexistence;
    let evidence = |enabled, disable_failed| Ieee802154SessionCoexistence {
        enabled,
        disable_failed,
    };
    super::validate_coexistence(evidence(true, false), true).unwrap();
    super::validate_coexistence(evidence(false, false), false).unwrap();
    assert!(super::validate_coexistence(evidence(false, false), true).is_err());
    assert!(super::validate_coexistence(evidence(true, false), false).is_err());
    assert!(super::validate_coexistence(evidence(true, true), true).is_err());
}
