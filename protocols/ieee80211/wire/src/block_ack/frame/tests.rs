use super::*;
use crate::sequence::seq;

#[test]
fn parses_all_block_ack_action_bodies() {
    assert_eq!(
        parse_block_ack_action(&[3, 0, 7, 0x87, 0x07, 0, 0, 0x30, 0x12]),
        Some(BlockAckAction::AddbaRequest {
            dialog_token: DialogToken::new(7),
            timeout: crate::tsf::TimeUnits::new(0),
            starting_sequence: seq(0x123),
            parameters: crate::block_ack::frame::AddbaParameters::new(
                crate::block_ack::frame::BlockAckPolicy::Immediate,
                crate::block_ack::frame::AmsduSupport::Supported,
                Tid::constant::<1>(),
                crate::block_ack::frame::BufferSize::Window(
                    crate::block_ack::frame::BlockAckWindow::constant::<30>()
                )
            ),
        })
    );
    assert_eq!(
        parse_block_ack_action(&[3, 1, 7, 0, 0, 0x86, 0x07, 5, 0]),
        Some(BlockAckAction::AddbaResponse {
            dialog_token: DialogToken::new(7),
            status: crate::status::StatusCode::SUCCESS,
            timeout: crate::tsf::TimeUnits::new(5),
            parameters: crate::block_ack::frame::AddbaParameters::new(
                crate::block_ack::frame::BlockAckPolicy::Immediate,
                crate::block_ack::frame::AmsduSupport::NotSupported,
                Tid::constant::<1>(),
                crate::block_ack::frame::BufferSize::Window(
                    crate::block_ack::frame::BlockAckWindow::constant::<30>()
                )
            ),
        })
    );
    assert_eq!(
        parse_block_ack_action(&[3, 2, 0, 0x58, 39, 0]),
        Some(BlockAckAction::Delba {
            tid: Tid::constant::<5>(),
            initiator: crate::block_ack::frame::DelbaInitiator::Originator,
            reason: crate::status::ReasonCode::TIMEOUT,
        })
    );
    assert_eq!(parse_block_ack_action(&[4, 0, 0]), None);
}

#[test]
fn block_ack_request_names_the_tid_and_window_start() {
    let frame = encode_block_ack_request(
        Eui48::new([0x20, 0x21, 0x22, 0x23, 0x24, 0x25]),
        Eui48::new([2, 3, 4, 5, 6, 7]),
        Tid::constant::<6>(),
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
    write_successful_addba_response(
        &mut body,
        DialogToken::new(137),
        Tid::constant::<0>(),
        BlockAckWindow::constant::<16>(),
    )
    .unwrap();
    assert_eq!(body, [3, 1, 137, 0, 0, 0x02, 0x04, 0, 0]);
    assert_eq!(
        parse_block_ack_action(&body),
        Some(BlockAckAction::AddbaResponse {
            dialog_token: DialogToken::new(137),
            status: crate::status::StatusCode::SUCCESS,
            timeout: crate::tsf::TimeUnits::new(0),
            parameters: crate::block_ack::frame::AddbaParameters::new(
                crate::block_ack::frame::BlockAckPolicy::Immediate,
                crate::block_ack::frame::AmsduSupport::NotSupported,
                Tid::constant::<0>(),
                crate::block_ack::frame::BufferSize::Window(
                    crate::block_ack::frame::BlockAckWindow::constant::<16>()
                )
            ),
        })
    );
}

#[test]
fn declined_response_preserves_request_identity_without_claiming_success() {
    let mut body = [0xff; 9];
    write_declined_addba_response(
        &mut body,
        DialogToken::new(23),
        Tid::constant::<6>(),
        BufferSize::Window(BlockAckWindow::constant::<64>()),
    )
    .unwrap();
    assert_eq!(
        parse_block_ack_action(&body),
        Some(BlockAckAction::AddbaResponse {
            dialog_token: DialogToken::new(23),
            status: crate::status::StatusCode::REQUEST_DECLINED,
            timeout: crate::tsf::TimeUnits::new(0),
            parameters: crate::block_ack::frame::AddbaParameters::new(
                crate::block_ack::frame::BlockAckPolicy::Immediate,
                crate::block_ack::frame::AmsduSupport::NotSupported,
                Tid::constant::<6>(),
                crate::block_ack::frame::BufferSize::Window(
                    crate::block_ack::frame::BlockAckWindow::constant::<64>()
                )
            ),
        })
    );
}

/// The Block Ack Action bodies of the tests above and the bodies the
/// writers produce.
fn action_seeds() -> std::vec::Vec<std::vec::Vec<u8>> {
    let mut successful = [0; 9];
    write_successful_addba_response(
        &mut successful,
        DialogToken::new(137),
        Tid::constant::<0>(),
        BlockAckWindow::constant::<16>(),
    )
    .unwrap();
    let mut declined = [0; 9];
    write_declined_addba_response(
        &mut declined,
        DialogToken::new(23),
        Tid::constant::<6>(),
        BufferSize::Window(BlockAckWindow::constant::<64>()),
    )
    .unwrap();
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
        DialogToken::new(7),
        Tid::constant::<15>(),
        BlockAckWindow::constant::<1023>(),
        crate::tsf::TimeUnits::new(12),
        crate::block_ack::frame::AmsduSupport::Supported,
        sequence,
    );
    assert_eq!(
        parse_block_ack_action(&body),
        Some(BlockAckAction::AddbaRequest {
            dialog_token: DialogToken::new(7),
            timeout: crate::tsf::TimeUnits::new(12),
            starting_sequence: sequence,
            parameters: crate::block_ack::frame::AddbaParameters::new(
                crate::block_ack::frame::BlockAckPolicy::Immediate,
                crate::block_ack::frame::AmsduSupport::Supported,
                Tid::constant::<15>(),
                crate::block_ack::frame::BufferSize::Window(
                    crate::block_ack::frame::BlockAckWindow::constant::<1023>()
                )
            ),
        })
    );
    // A request's window is 1 to 1023: no other is one.
    for window in [0, 1024, u16::MAX] {
        assert_eq!(BlockAckWindow::new(window), None);
    }
}

#[test]
fn addba_response_and_delba_round_trip_and_refuse_wide_fields() {
    let response = encode_addba_response(
        DialogToken::new(9),
        crate::status::StatusCode::SUCCESS,
        Tid::constant::<5>(),
        BufferSize::Window(BlockAckWindow::constant::<64>()),
        crate::tsf::TimeUnits::new(0x1234),
        crate::block_ack::frame::AmsduSupport::Supported,
    );
    assert_eq!(
        parse_block_ack_action(&response),
        Some(BlockAckAction::AddbaResponse {
            dialog_token: DialogToken::new(9),
            status: crate::status::StatusCode::SUCCESS,
            timeout: crate::tsf::TimeUnits::new(0x1234),
            parameters: crate::block_ack::frame::AddbaParameters::new(
                crate::block_ack::frame::BlockAckPolicy::Immediate,
                crate::block_ack::frame::AmsduSupport::Supported,
                Tid::constant::<5>(),
                crate::block_ack::frame::BufferSize::Window(
                    crate::block_ack::frame::BlockAckWindow::constant::<64>()
                )
            ),
        })
    );
    // A Buffer Size wider than ten bits is no window.
    assert_eq!(BlockAckWindow::new(1024), None);

    let delba = encode_delba(
        Tid::constant::<6>(),
        crate::block_ack::frame::DelbaInitiator::Recipient,
        crate::status::ReasonCode::TIMEOUT,
    );
    assert_eq!(delba, [3, 2, 0x00, 0x60, 39, 0]);
    assert_eq!(
        parse_block_ack_action(&delba),
        Some(BlockAckAction::Delba {
            tid: Tid::constant::<6>(),
            initiator: crate::block_ack::frame::DelbaInitiator::Recipient,
            reason: crate::status::ReasonCode::TIMEOUT,
        })
    );
    assert_eq!(parse_block_ack_action(&delba[..5]), None);
}

#[test]
fn a_compressed_bar_round_trips_and_other_variants_are_refused() {
    let frame = encode_block_ack_request(
        Eui48::new([2; 6]),
        Eui48::new([4; 6]),
        Tid::constant::<6>(),
        seq(0x123),
    );
    assert_eq!(
        parse_compressed_block_ack_request(&frame),
        Some(CompressedBlockAckRequest {
            receiver: Eui48::new([2; 6]),
            transmitter: Eui48::new([4; 6]),
            tid: Tid::constant::<6>(),
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

/// TID 15 is the largest a four-bit TID holds: an ADDBA Response for it is
/// written, and 16 is no TID.
#[test]
fn an_addba_response_holds_tid_15_and_refuses_16() {
    let written = encode_addba_response(
        DialogToken::new(1),
        crate::status::StatusCode::SUCCESS,
        Tid::constant::<15>(),
        BufferSize::Window(BlockAckWindow::constant::<64>()),
        crate::tsf::TimeUnits::new(0),
        AmsduSupport::NotSupported,
    );
    assert!(matches!(
        parse_block_ack_action(&written),
        Some(BlockAckAction::AddbaResponse { parameters, .. }) if parameters.tid() == Tid::constant::<15>()
    ));
    assert_eq!(Tid::new(16), None);
}

/// The Parameter Set holds every Buffer Size: the largest window, and zero.
#[test]
fn the_parameter_set_holds_its_buffer_size() {
    let new = |tid, buffer| {
        AddbaParameters::new(
            BlockAckPolicy::Delayed,
            AmsduSupport::Supported,
            tid,
            buffer,
        )
    };
    let largest = new(
        Tid::constant::<15>(),
        BufferSize::Window(BlockAckWindow::constant::<0x03ff>()),
    );
    assert_eq!(largest.tid(), Tid::constant::<15>());
    assert_eq!(
        largest.buffer_size(),
        BufferSize::Window(BlockAckWindow::constant::<0x03ff>())
    );
    assert_eq!(largest.policy(), BlockAckPolicy::Delayed);
    assert!(largest.amsdu());
    assert_eq!(
        new(Tid::constant::<0>(), BufferSize::Unnamed).buffer_size(),
        BufferSize::Unnamed
    );
}

/// A window is 1 to 1023 MPDUs; the smaller of two is their bound.
#[test]
fn a_window_is_one_to_1023_and_bounds_another() {
    assert_eq!(BlockAckWindow::new(0), None);
    assert_eq!(BlockAckWindow::new(1).map(BlockAckWindow::get), Some(1));
    assert_eq!(
        BlockAckWindow::new(1023).map(BlockAckWindow::get),
        Some(1023)
    );
    assert_eq!(BlockAckWindow::new(1024), None);
    let (narrow, wide) = (
        BlockAckWindow::constant::<16>(),
        BlockAckWindow::constant::<64>(),
    );
    assert_eq!(wide.at_most(narrow), narrow);
    assert_eq!(narrow.at_most(wide), narrow);
    assert_eq!(narrow.at_most(narrow), narrow);
}

/// A refusal of an unnamed Buffer Size writes zero; a body of the wrong
/// length is refused and left as it was.
#[test]
fn a_declined_response_writes_an_unnamed_buffer_size() {
    let mut short = [0_u8; ADDBA_ACTION_BODY_LEN - 1];
    assert_eq!(
        write_successful_addba_response(
            &mut short,
            DialogToken::new(1),
            Tid::constant::<0>(),
            BlockAckWindow::constant::<1>()
        ),
        Err(RxAddbaResponseError::InvalidBodyLength(
            ADDBA_ACTION_BODY_LEN - 1
        ))
    );
    assert_eq!(short, [0; ADDBA_ACTION_BODY_LEN - 1]);
    let mut body = [0_u8; ADDBA_ACTION_BODY_LEN];
    write_declined_addba_response(
        &mut body,
        DialogToken::new(1),
        Tid::constant::<0>(),
        BufferSize::Unnamed,
    )
    .unwrap();
    assert!(matches!(
        parse_block_ack_action(&body),
        Some(BlockAckAction::AddbaResponse { parameters, .. })
            if parameters.buffer_size() == BufferSize::Unnamed
    ));
}
