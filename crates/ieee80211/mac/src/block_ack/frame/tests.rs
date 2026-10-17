use super::*;
use crate::sequence::seq;

#[test]
fn parses_all_block_ack_action_bodies() {
    assert_eq!(
        parse_block_ack_action(&[3, 0, 7, 0x87, 0x07, 0, 0, 0x30, 0x12]),
        Some(BlockAckAction::AddbaRequest {
            dialog_token: 7,
            tid: 1,
            immediate: true,
            amsdu: true,
            window: 30,
            timeout_tu: 0,
            starting_sequence: seq(0x123),
        })
    );
    assert_eq!(
        parse_block_ack_action(&[3, 1, 7, 0, 0, 0x86, 0x07, 5, 0]),
        Some(BlockAckAction::AddbaResponse {
            dialog_token: 7,
            status: 0,
            tid: 1,
            immediate: true,
            amsdu: false,
            window: 30,
            timeout_tu: 5,
        })
    );
    assert_eq!(
        parse_block_ack_action(&[3, 2, 0, 0x58, 39, 0]),
        Some(BlockAckAction::Delba {
            tid: 5,
            initiator: true,
            reason: 39,
        })
    );
    assert_eq!(parse_block_ack_action(&[4, 0, 0]), None);
}

#[test]
fn block_ack_request_names_the_tid_and_window_start() {
    let frame = encode_block_ack_request(
        [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
        [2, 3, 4, 5, 6, 7],
        6,
        SequenceNumber::new(0x123).unwrap(),
    );
    assert_eq!(
        frame,
        [
            0x84, 0x00, 0x00, 0x00, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 2, 3, 4, 5, 6, 7, 0x04,
            0x60, 0x30, 0x12,
        ]
    );
}

#[test]
fn successful_response_narrows_the_window_and_disables_amsdu() {
    let mut body = [0xff; 9];
    write_successful_addba_response(&mut body, 137, 0, 16).unwrap();
    assert_eq!(body, [3, 1, 137, 0, 0, 0x02, 0x04, 0, 0]);
    assert_eq!(
        parse_block_ack_action(&body),
        Some(BlockAckAction::AddbaResponse {
            dialog_token: 137,
            status: 0,
            tid: 0,
            immediate: true,
            amsdu: false,
            window: 16,
            timeout_tu: 0,
        })
    );
}

#[test]
fn declined_response_preserves_request_identity_without_claiming_success() {
    let mut body = [0xff; 9];
    write_declined_addba_response(&mut body, 23, 6, 64).unwrap();
    assert_eq!(
        parse_block_ack_action(&body),
        Some(BlockAckAction::AddbaResponse {
            dialog_token: 23,
            status: ADDBA_STATUS_REQUEST_DECLINED,
            tid: 6,
            immediate: true,
            amsdu: false,
            window: 64,
            timeout_tu: 0,
        })
    );
}
