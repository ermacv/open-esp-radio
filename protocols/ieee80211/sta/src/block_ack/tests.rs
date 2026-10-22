use super::*;
use oer_time::Clock;
type VirtualClock = oer_time_virtual::VirtualClock<8>;

fn request(tid: u8, timeout_tu: u16) -> RxAddbaRequest {
    RxAddbaRequest::from_action(BlockAckAction::AddbaRequest {
        dialog_token: 17,
        tid,
        immediate: true,
        amsdu: true,
        window: 64,
        timeout_tu,
        starting_sequence: SequenceNumber::new(4094).unwrap(),
    })
    .unwrap()
}

#[derive(Debug, Eq, PartialEq)]
struct Bank(u8);

fn session(clock: &VirtualClock, timeout: u16) -> RxBlockAckSession<Bank, 8, 8> {
    let negotiation =
        RxBlockAckNegotiation::request(request(0, timeout), 7, 8, true, clock.now()).unwrap();
    RxBlockAckSession::activate(negotiation, Bank(3)).unwrap()
}

fn frame(sequence: u16, slot: u8) -> RxReorderMpdu {
    RxReorderMpdu {
        sequence: SequenceNumber::new(sequence).unwrap(),
        slot,
    }
}

fn released<const N: usize>(release: &RxReorderRelease<N>) -> std::vec::Vec<u8> {
    release.iter().map(|frame| frame.slot).collect()
}

#[test]
fn acceptance_is_bounded_by_the_frame_pool_and_preserves_dialog_and_amsdu() {
    let clock = VirtualClock::new();
    let negotiation =
        RxBlockAckNegotiation::request(request(3, 0), 7, 8, true, clock.now()).unwrap();
    let response = negotiation.response();
    assert_eq!(response.status, AddbaStatus::Success);
    assert_eq!(response.window, 8);
    assert_eq!(response.dialog_token, 17);
    assert_eq!(response.tid, 3);
    assert!(response.amsdu);
    assert!(matches!(
        oer_ieee80211_wire::block_ack::frame::parse_block_ack_action(&response.encode().unwrap()),
        Some(BlockAckAction::AddbaResponse {
            dialog_token: 17,
            status: 0,
            tid: 3,
            window: 8,
            amsdu: true,
            ..
        })
    ));
}

#[test]
fn a_zero_request_window_uses_the_ht_default_bounded_by_real_capacity() {
    let clock = VirtualClock::new();
    let mut request = request(0, 0);
    request.window = 0;
    let accepted = RxBlockAckNegotiation::request(request, 7, 7, true, clock.now()).unwrap();
    assert_eq!(accepted.response().window, 7);
}

#[test]
fn unavailable_bank_and_unsupported_parameters_have_explicit_responses() {
    let clock = VirtualClock::new();
    let mut delayed = request(0, 0);
    delayed.immediate = false;
    for (request, capacity, bank_available) in [
        (request(0, 0), 8, false),
        (request(15, 0), 8, true),
        (delayed, 8, true),
        (request(0, 0), 0, true),
    ] {
        let response =
            RxBlockAckNegotiation::request(request, 7, capacity, bank_available, clock.now())
                .err()
                .unwrap();
        assert_eq!(response.status, AddbaStatus::RequestDeclined);
        assert_eq!(response.dialog_token, 17);
        assert_eq!(response.tid, request.tid);
        assert!(response.encode().is_some());
    }
}

#[test]
fn reorder_wraparound_duplicate_and_delba_return_each_owner_once() {
    let clock = VirtualClock::new();
    let mut receiver = session(&clock, 0);
    let gap = Duration::from_millis(100);
    assert!(
        receiver
            .ingest(clock.now(), frame(0, 0), gap)
            .unwrap()
            .buffered
    );
    assert!(
        receiver
            .ingest(clock.now(), frame(4095, 1), gap)
            .unwrap()
            .buffered
    );
    assert!(matches!(
        receiver.ingest(clock.now(), frame(0, 2), gap),
        Err(RxBlockAckError::Reorder(RxReorderError::DuplicateSequence(
            _
        )))
    ));
    let release = receiver.ingest(clock.now(), frame(4094, 3), gap).unwrap();
    assert_eq!(released(&release), [3, 1, 0]);
    assert!(!receiver.peer_delba(BlockAckAction::Delba {
        tid: 0,
        initiator: false,
        reason: 37,
    }));
    assert!(!receiver.peer_delba(BlockAckAction::Delba {
        tid: 1,
        initiator: true,
        reason: 37,
    }));
    assert!(receiver.peer_delba(BlockAckAction::Delba {
        tid: 0,
        initiator: true,
        reason: 37,
    }));
    receiver.ingest(clock.now(), frame(3, 4), gap).unwrap();
    let (bank, release) = receiver.stop();
    assert_eq!(bank, Bank(3));
    assert_eq!(released(&release), [4]);
}

#[test]
fn a_hole_expires_even_with_continuous_out_of_order_rx() {
    let clock = VirtualClock::new();
    let mut receiver = session(&clock, 0);
    let gap = Duration::from_millis(100);
    receiver.ingest(clock.now(), frame(4095, 1), gap).unwrap();
    let deadline = receiver.deadline().unwrap();
    clock.advance(Duration::from_millis(99)).unwrap();
    receiver.ingest(clock.now(), frame(0, 2), gap).unwrap();
    assert_eq!(receiver.deadline(), Some(deadline));
    assert!(
        receiver
            .block_ack_request(clock.now(), SequenceNumber::new(4094).unwrap(), gap)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        receiver.deadline(),
        Some(deadline),
        "a stale BAR cannot postpone the hole"
    );
    assert!(receiver.expire_gap(clock.now(), gap).unwrap().is_none());
    clock.advance_to(deadline);
    let release = receiver.expire_gap(clock.now(), gap).unwrap().unwrap();
    assert_eq!(released(&release), [1, 2]);
    assert_eq!(receiver.deadline(), None);
}

#[test]
fn inactivity_refreshes_on_rx_and_expiry_releases_the_bank_and_frames() {
    let clock = VirtualClock::new();
    let mut receiver = session(&clock, 20);
    let gap = Duration::from_millis(100);
    let initial = receiver.deadline().unwrap();
    clock.advance(Duration::from_millis(10)).unwrap();
    receiver.ingest(clock.now(), frame(4095, 2), gap).unwrap();
    clock.advance_to(initial);
    assert!(!receiver.expired(clock.now()));
    clock.advance_to(receiver.deadline().unwrap());
    assert!(receiver.expired(clock.now()));
    let (bank, release) = receiver.stop();
    assert_eq!(bank, Bank(3));
    assert_eq!(released(&release), [2]);
}

#[test]
fn overflowing_deadline_refuses_negotiation_without_creating_a_session() {
    let clock = VirtualClock::starting_at(Instant::from_micros(u64::MAX));
    let response = RxBlockAckNegotiation::request(request(0, 1), 7, 8, true, clock.now())
        .err()
        .unwrap();
    assert_eq!(response.status, AddbaStatus::RequestDeclined);
}

#[test]
fn invalid_timer_inputs_leave_the_reorder_owner_unchanged() {
    let clock = VirtualClock::new();
    let mut receiver = session(&clock, 0);
    assert_eq!(
        receiver.ingest(clock.now(), frame(4095, 1), Duration::ZERO),
        Err(RxBlockAckError::ZeroGapTimeout)
    );
    clock.advance_to(Instant::from_micros(u64::MAX));
    assert_eq!(
        receiver.ingest(clock.now(), frame(4095, 1), Duration::from_micros(1)),
        Err(RxBlockAckError::DeadlineOverflow)
    );
    let (bank, release) = receiver.stop();
    assert_eq!(bank, Bank(3));
    assert!(released(&release).is_empty());
}
