use super::*;
use crate::sequence::seq;

#[test]
fn parses_all_block_ack_action_bodies() {
    assert_eq!(
        parse_block_ack_action(&[3, 0, 7, 0x87, 0x07, 0, 0, 0x30, 0x12]),
        Some(BlockAckAction::AddbaRequest {
            dialog_token: 7,
            timeout_tu: 0,
            starting_sequence: seq(0x123),
            parameters: crate::block_ack::frame::AddbaParameters::new(
                crate::block_ack::frame::BlockAckPolicy::Immediate,
                crate::block_ack::frame::AmsduSupport::Supported,
                1,
                30
            )
            .unwrap(),
        })
    );
    assert_eq!(
        parse_block_ack_action(&[3, 1, 7, 0, 0, 0x86, 0x07, 5, 0]),
        Some(BlockAckAction::AddbaResponse {
            dialog_token: 7,
            status: 0,
            timeout_tu: 5,
            parameters: crate::block_ack::frame::AddbaParameters::new(
                crate::block_ack::frame::BlockAckPolicy::Immediate,
                crate::block_ack::frame::AmsduSupport::NotSupported,
                1,
                30
            )
            .unwrap(),
        })
    );
    assert_eq!(
        parse_block_ack_action(&[3, 2, 0, 0x58, 39, 0]),
        Some(BlockAckAction::Delba {
            tid: 5,
            initiator: crate::block_ack::frame::DelbaInitiator::Originator,
            reason: 39,
        })
    );
    assert_eq!(parse_block_ack_action(&[4, 0, 0]), None);
}

#[test]
fn block_ack_request_names_the_tid_and_window_start() {
    let frame = encode_block_ack_request(
        Eui48::new([0x20, 0x21, 0x22, 0x23, 0x24, 0x25]),
        Eui48::new([2, 3, 4, 5, 6, 7]),
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
            timeout_tu: 0,
            parameters: crate::block_ack::frame::AddbaParameters::new(
                crate::block_ack::frame::BlockAckPolicy::Immediate,
                crate::block_ack::frame::AmsduSupport::NotSupported,
                0,
                16
            )
            .unwrap(),
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
            timeout_tu: 0,
            parameters: crate::block_ack::frame::AddbaParameters::new(
                crate::block_ack::frame::BlockAckPolicy::Immediate,
                crate::block_ack::frame::AmsduSupport::NotSupported,
                6,
                64
            )
            .unwrap(),
        })
    );
}

/// The Block Ack Action bodies of the tests above and the bodies the
/// writers produce.
fn action_seeds() -> std::vec::Vec<std::vec::Vec<u8>> {
    let mut successful = [0; 9];
    write_successful_addba_response(&mut successful, 137, 0, 16).unwrap();
    let mut declined = [0; 9];
    write_declined_addba_response(&mut declined, 23, 6, 64).unwrap();
    std::vec![
        std::vec![3, 0, 7, 0x87, 0x07, 0, 0, 0x30, 0x12],
        std::vec![3, 1, 7, 0, 0, 0x86, 0x07, 5, 0],
        std::vec![3, 2, 0, 0x58, 39, 0],
        successful.to_vec(),
        declined.to_vec(),
    ]
}

fn actions(count: usize) {
    let seeds = action_seeds();
    let seeds: std::vec::Vec<&[u8]> = seeds.iter().map(std::vec::Vec::as_slice).collect();
    crate::fuzz::inputs(&seeds, count, 32, |bytes| {
        let _ = parse_block_ack_action(bytes);
    });
}

#[test]
fn block_ack_action_parser_never_panics() {
    actions(crate::fuzz::QUICK);
}

#[test]
#[ignore = "the full run"]
fn block_ack_action_parser_never_panics_full() {
    actions(crate::fuzz::FULL);
}

#[test]
fn immediate_request_round_trips_and_refuses_unrepresentable_fields() {
    let sequence = crate::sequence::seq(0xabc);
    let body = encode_addba_request(
        7,
        15,
        1023,
        12,
        crate::block_ack::frame::AmsduSupport::Supported,
        sequence,
    )
    .unwrap();
    assert_eq!(
        parse_block_ack_action(&body),
        Some(BlockAckAction::AddbaRequest {
            dialog_token: 7,
            timeout_tu: 12,
            starting_sequence: sequence,
            parameters: crate::block_ack::frame::AddbaParameters::new(
                crate::block_ack::frame::BlockAckPolicy::Immediate,
                crate::block_ack::frame::AmsduSupport::Supported,
                15,
                1023
            )
            .unwrap(),
        })
    );
    assert_eq!(
        encode_addba_request(
            7,
            16,
            32,
            0,
            crate::block_ack::frame::AmsduSupport::NotSupported,
            sequence
        ),
        Err(AddbaRequestError::InvalidTid(16))
    );
    for window in [0, 1024, u16::MAX] {
        assert_eq!(
            encode_addba_request(
                7,
                0,
                window,
                0,
                crate::block_ack::frame::AmsduSupport::NotSupported,
                sequence
            ),
            Err(AddbaRequestError::InvalidWindow(window))
        );
    }
}

#[test]
fn addba_response_and_delba_round_trip_and_refuse_wide_fields() {
    let response = encode_addba_response(
        9,
        0,
        5,
        64,
        0x1234,
        crate::block_ack::frame::AmsduSupport::Supported,
    )
    .unwrap();
    assert_eq!(
        parse_block_ack_action(&response),
        Some(BlockAckAction::AddbaResponse {
            dialog_token: 9,
            status: 0,
            timeout_tu: 0x1234,
            parameters: crate::block_ack::frame::AddbaParameters::new(
                crate::block_ack::frame::BlockAckPolicy::Immediate,
                crate::block_ack::frame::AmsduSupport::Supported,
                5,
                64
            )
            .unwrap(),
        })
    );
    assert_eq!(
        encode_addba_response(
            9,
            0,
            16,
            64,
            0,
            crate::block_ack::frame::AmsduSupport::NotSupported
        ),
        Err(RxAddbaResponseError::InvalidTid(16))
    );
    assert_eq!(
        encode_addba_response(
            9,
            0,
            5,
            1024,
            0,
            crate::block_ack::frame::AmsduSupport::NotSupported
        ),
        Err(RxAddbaResponseError::InvalidWindow(1024))
    );

    let delba = encode_delba(6, crate::block_ack::frame::DelbaInitiator::Recipient, 39).unwrap();
    assert_eq!(delba, [3, 2, 0x00, 0x60, 39, 0]);
    assert_eq!(
        parse_block_ack_action(&delba),
        Some(BlockAckAction::Delba {
            tid: 6,
            initiator: crate::block_ack::frame::DelbaInitiator::Recipient,
            reason: 39,
        })
    );
    assert_eq!(
        encode_delba(16, crate::block_ack::frame::DelbaInitiator::Originator, 1),
        Err(AddbaRequestError::InvalidTid(16))
    );
    assert_eq!(parse_block_ack_action(&delba[..5]), None);
}

#[test]
fn a_compressed_bar_round_trips_and_other_variants_are_refused() {
    let frame = encode_block_ack_request(Eui48::new([2; 6]), Eui48::new([4; 6]), 6, seq(0x123));
    assert_eq!(
        parse_compressed_block_ack_request(&frame),
        Some(CompressedBlockAckRequest {
            receiver: Eui48::new([2; 6]),
            transmitter: Eui48::new([4; 6]),
            tid: 6,
            starting_sequence: seq(0x123),
        })
    );
    assert_eq!(parse_compressed_block_ack_request(&frame[..19]), None);
    let mut basic = frame;
    basic[16] = 0;
    assert_eq!(parse_compressed_block_ack_request(&basic), None);
    let mut fragment = frame;
    fragment[18] |= 1;
    assert_eq!(parse_compressed_block_ack_request(&fragment), None);
    let mut protected = frame;
    protected[1] |= 0x40;
    assert_eq!(parse_compressed_block_ack_request(&protected), None);
}

/// TID 15 is the largest a four-bit TID holds: an ADDBA Response for it
/// is written, one for 16 is refused.
#[test]
fn an_addba_response_holds_tid_15_and_refuses_16() {
    let written = encode_addba_response(1, 0, 15, 64, 0, AmsduSupport::NotSupported).unwrap();
    assert!(matches!(
        parse_block_ack_action(&written),
        Some(BlockAckAction::AddbaResponse { parameters, .. }) if parameters.tid() == 15
    ));
    assert_eq!(
        encode_addba_response(1, 0, 16, 64, 0, AmsduSupport::NotSupported),
        Err(RxAddbaResponseError::InvalidTid(16))
    );
}

/// The Parameter Set names why it refuses: a TID past four bits or a
/// window past ten, each at its first value out.
#[test]
fn the_parameter_set_names_its_refusal() {
    let new = |tid, window| {
        AddbaParameters::new(
            BlockAckPolicy::Delayed,
            AmsduSupport::Supported,
            tid,
            window,
        )
    };
    let largest = new(15, 0x03ff).unwrap();
    assert_eq!(largest.tid(), 15);
    assert_eq!(largest.window(), 0x03ff);
    assert_eq!(largest.policy(), BlockAckPolicy::Delayed);
    assert!(largest.amsdu());
    assert_eq!(new(16, 1), Err(AddbaParametersError::TidOutOfRange(16)));
    assert_eq!(
        new(0, 0x0400),
        Err(AddbaParametersError::WindowOutOfRange(0x0400))
    );
    // The TID is the first refusal of two.
    assert_eq!(
        new(16, 0x0400),
        Err(AddbaParametersError::TidOutOfRange(16))
    );
}

/// Each encoder's error is the Parameter Set's reason; a zero window is
/// refused where an agreement needs a buffer, and written in a refusal.
#[test]
fn each_encoder_reports_the_parameter_reason() {
    let sequence = SequenceNumber::new(0).unwrap();
    let request =
        |tid, window| encode_addba_request(1, tid, window, 0, AmsduSupport::NotSupported, sequence);
    assert_eq!(request(16, 0), Err(AddbaRequestError::InvalidTid(16)));
    assert_eq!(
        request(0, 0x0400),
        Err(AddbaRequestError::InvalidWindow(0x0400))
    );
    assert_eq!(request(0, 0), Err(AddbaRequestError::InvalidWindow(0)));
    assert!(request(15, 0x03ff).is_ok());

    let mut body = [0_u8; ADDBA_ACTION_BODY_LEN];
    assert_eq!(
        write_successful_addba_response(&mut body, 1, 16, 0),
        Err(RxAddbaResponseError::InvalidTid(16))
    );
    assert_eq!(
        write_successful_addba_response(&mut body, 1, 0, 0x0400),
        Err(RxAddbaResponseError::InvalidWindow(0x0400))
    );
    assert_eq!(
        write_successful_addba_response(&mut body, 1, 0, 0),
        Err(RxAddbaResponseError::InvalidWindow(0))
    );
    // A refused write leaves the body as it was.
    assert_eq!(body, [0; ADDBA_ACTION_BODY_LEN]);

    assert_eq!(
        write_declined_addba_response(&mut body, 1, 16, 1),
        Err(RxAddbaResponseError::InvalidTid(16))
    );
    assert_eq!(
        write_declined_addba_response(&mut body, 1, 0, 0x0400),
        Err(RxAddbaResponseError::InvalidWindow(0x0400))
    );
    assert_eq!(body, [0; ADDBA_ACTION_BODY_LEN]);
    write_declined_addba_response(&mut body, 1, 0, 0).unwrap();
    assert!(matches!(
        parse_block_ack_action(&body),
        Some(BlockAckAction::AddbaResponse { parameters, .. }) if parameters.window() == 0
    ));
}
