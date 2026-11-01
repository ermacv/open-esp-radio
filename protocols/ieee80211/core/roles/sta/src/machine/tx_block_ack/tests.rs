use super::*;
use oer_ieee80211_wire::sequence::SequenceNumber;

const POLICY: StaTxBlockAckPolicy = StaTxBlockAckPolicy {
    window: 32,
    negotiation_timeout: Duration::from_millis(100),
    retry: TxBlockAckRetry {
        attempts: 2,
        interval: Duration::from_millis(10),
    },
    inactivity: Duration::from_secs(5),
    retry_members: crate::data::AggregateRetry {
        lifetime: Duration::from_millis(1_500),
        attempts: 4,
    },
};

fn at(millis: u64) -> Instant {
    Instant::from_micros(millis * 1_000)
}

fn response(dialog_token: u8, tid: u8, status: u16, window: u16) -> BlockAckAction {
    BlockAckAction::AddbaResponse {
        dialog_token,
        status,
        tid,
        immediate: true,
        amsdu: false,
        window,
        timeout_tu: 0,
    }
}

/// An admitted association whose TID 0 agreement is operational.
fn operational() -> StaTxBlockAck {
    let mut tx = StaTxBlockAck::new(POLICY).unwrap();
    tx.associate(true);
    tx.traffic(0, at(0));
    assert_eq!(
        tx.poll(at(0), true),
        Some(TxBlockAckOwed::Negotiate { tid: 0 })
    );
    let request = tx.begin(0, SequenceNumber::new(5).unwrap(), at(0)).unwrap();
    assert!(tx.on_action(response(request.dialog_token, 0, 0, 32), at(1)));
    tx
}

#[test]
fn a_policy_that_cannot_negotiate_is_refused() {
    let invalid = |policy| StaTxBlockAck::new(policy).err();
    assert!(matches!(
        invalid(StaTxBlockAckPolicy {
            window: 65,
            ..POLICY
        }),
        Some(StaTxBlockAckPolicyError::Agreement(_))
    ));
    assert_eq!(
        invalid(StaTxBlockAckPolicy {
            retry: TxBlockAckRetry {
                attempts: 0,
                ..POLICY.retry
            },
            ..POLICY
        }),
        Some(StaTxBlockAckPolicyError::NoAttempts)
    );
    assert_eq!(
        invalid(StaTxBlockAckPolicy {
            inactivity: Duration::ZERO,
            ..POLICY
        }),
        Some(StaTxBlockAckPolicyError::ZeroInactivity)
    );
}

#[test]
fn data_of_a_tid_negotiates_it_only_with_an_admitting_association() {
    let mut tx = StaTxBlockAck::new(POLICY).unwrap();
    tx.associate(false);
    tx.traffic(0, at(0));
    assert_eq!(
        tx.poll(at(0), true),
        None,
        "a legacy or non-QoS association"
    );
    assert_eq!(tx.deadline(true), None);

    tx.associate(true);
    tx.traffic(6, at(0));
    assert_eq!(
        tx.deadline(false),
        None,
        "nothing to send takes no deadline"
    );
    assert_eq!(tx.deadline(true), Some(Instant::EPOCH));
    assert_eq!(tx.poll(at(0), false), None);
    assert_eq!(
        tx.poll(at(0), true),
        Some(TxBlockAckOwed::Negotiate { tid: 6 })
    );
    let request = tx
        .begin(6, SequenceNumber::new(40).unwrap(), at(0))
        .unwrap();
    assert_eq!(request.starting_sequence, SequenceNumber::new(40).unwrap());
    assert_eq!(tx.deadline(true), Some(at(100)), "the response deadline");
    assert!(tx.on_action(response(request.dialog_token, 6, 0, 16), at(3)));
    let agreement = tx.operational(6).unwrap();
    assert_eq!(
        (agreement.window, agreement.starting_sequence.get()),
        (16, 40)
    );
    assert_eq!(tx.deadline(true), Some(at(5_003)), "the inactivity end");
}

#[test]
fn an_unanswered_negotiation_retries_then_closes_its_tid() {
    let mut tx = StaTxBlockAck::new(POLICY).unwrap();
    tx.associate(true);
    tx.traffic(0, at(0));
    let mut now = at(0);
    for _ in 0..2 {
        assert_eq!(
            tx.poll(now, true),
            Some(TxBlockAckOwed::Negotiate { tid: 0 })
        );
        tx.begin(0, SequenceNumber::ZERO, now).unwrap();
        now = now.saturating_add(Duration::from_millis(100));
        assert_eq!(
            tx.poll(now, true),
            None,
            "the next attempt waits its interval"
        );
        now = now.saturating_add(Duration::from_millis(10));
    }
    assert_eq!(tx.poll(now, true), None);
    tx.traffic(0, now);
    assert_eq!(
        tx.poll(now, true),
        None,
        "a TID whose attempts ran out stays closed"
    );
    tx.associate(true);
    tx.traffic(0, now);
    assert_eq!(
        tx.poll(now, true),
        Some(TxBlockAckOwed::Negotiate { tid: 0 }),
        "a new association offers it again"
    );
}

#[test]
fn a_request_that_did_not_leave_retries_after_the_interval() {
    let mut tx = StaTxBlockAck::new(POLICY).unwrap();
    tx.associate(true);
    tx.traffic(1, at(0));
    assert_eq!(
        tx.poll(at(0), true),
        Some(TxBlockAckOwed::Negotiate { tid: 1 })
    );
    tx.begin(1, SequenceNumber::ZERO, at(0)).unwrap();
    tx.request_failed(1, at(2));
    assert_eq!(tx.deadline(true), Some(at(12)));
    assert_eq!(tx.poll(at(11), true), None);
    assert_eq!(
        tx.poll(at(12), true),
        Some(TxBlockAckOwed::Negotiate { tid: 1 })
    );
}

#[test]
fn only_due_work_makes_the_agreements_owe() {
    let mut tx = StaTxBlockAck::new(POLICY).unwrap();
    tx.associate(true);
    assert!(!tx.may_owe(at(0)), "a new association owes nothing");
    tx.traffic(2, at(0));
    assert!(tx.may_owe(at(0)), "queued data negotiates at once");
    assert_eq!(
        tx.poll(at(0), true),
        Some(TxBlockAckOwed::Negotiate { tid: 2 })
    );
    tx.begin(2, SequenceNumber::ZERO, at(0)).unwrap();
    assert!(!tx.may_owe(at(99)), "the response is awaited");
    assert!(tx.may_owe(at(100)), "the response deadline is due");
    assert_eq!(tx.poll(at(100), true), None);
    assert!(!tx.may_owe(at(109)), "the next attempt waits its interval");
    assert!(tx.may_owe(at(110)));
    assert_eq!(tx.deadline(true), Some(at(110)));
    assert_eq!(
        tx.poll(at(110), true),
        Some(TxBlockAckOwed::Negotiate { tid: 2 })
    );
    let request = tx.begin(2, SequenceNumber::ZERO, at(110)).unwrap();
    assert!(tx.on_action(response(request.dialog_token, 2, 0, 32), at(111)));
    tx.traffic(2, at(111));
    assert!(
        !tx.may_owe(at(112)),
        "a live agreement with fresh data owes nothing"
    );
    assert_eq!(tx.deadline(true), Some(at(5_111)));
    assert!(tx.may_owe(at(5_111)), "its inactivity end is due");
}

#[test]
fn a_refusal_closes_the_tid_without_a_delba() {
    let mut tx = StaTxBlockAck::new(POLICY).unwrap();
    tx.associate(true);
    tx.traffic(0, at(0));
    tx.poll(at(0), true);
    let request = tx.begin(0, SequenceNumber::ZERO, at(0)).unwrap();
    assert!(tx.on_action(response(request.dialog_token, 0, 37, 0), at(1)));
    assert_eq!(tx.operational(0), None);
    tx.traffic(0, at(2));
    assert_eq!(tx.poll(at(2), true), None);
}

#[test]
fn a_response_beyond_the_requested_window_ends_with_a_delba() {
    let mut tx = StaTxBlockAck::new(POLICY).unwrap();
    tx.associate(true);
    tx.traffic(0, at(0));
    tx.poll(at(0), true);
    let request = tx.begin(0, SequenceNumber::ZERO, at(0)).unwrap();
    assert!(tx.on_action(response(request.dialog_token, 0, 0, 64), at(1)));
    assert_eq!(tx.operational(0), None);
    assert_eq!(tx.poll(at(1), true), Some(TxBlockAckOwed::Delba { tid: 0 }));
    tx.delba_sent(0);
    tx.traffic(0, at(2));
    assert_eq!(tx.poll(at(2), true), None, "the TID stays closed");
}

#[test]
fn an_idle_agreement_ends_with_a_delba_and_new_data_renegotiates() {
    let mut tx = operational();
    tx.traffic(0, at(1_000));
    assert_eq!(tx.poll(at(5_999), true), None, "data kept it active");
    assert_eq!(tx.poll(at(6_000), false), None, "no transmission to give");
    assert_eq!(tx.operational(0), None, "but the idle agreement ended");
    assert_eq!(tx.deadline(true), Some(Instant::EPOCH));
    assert_eq!(
        tx.poll(at(6_000), true),
        Some(TxBlockAckOwed::Delba { tid: 0 })
    );
    tx.delba_sent(0);
    assert_eq!(tx.poll(at(6_000), true), None);
    tx.traffic(0, at(7_000));
    assert_eq!(
        tx.poll(at(7_000), true),
        Some(TxBlockAckOwed::Negotiate { tid: 0 })
    );
}

#[test]
fn the_recipient_s_delba_ends_the_agreement_and_receive_actions_pass_by() {
    let mut tx = operational();
    let initiator = |initiator| BlockAckAction::Delba {
        tid: 0,
        initiator,
        reason: 37,
    };
    assert!(
        !tx.on_action(initiator(true), at(2)),
        "the AP's own agreement"
    );
    assert!(tx.operational(0).is_some());
    assert!(tx.on_action(initiator(false), at(2)));
    assert_eq!(tx.operational(0), None);
    assert_eq!(tx.poll(at(2), true), None, "the recipient wants no answer");
    let request = BlockAckAction::AddbaRequest {
        dialog_token: 1,
        tid: 0,
        amsdu: false,
        immediate: true,
        window: 64,
        timeout_tu: 0,
        starting_sequence: SequenceNumber::ZERO,
    };
    assert!(!tx.on_action(request, at(2)));
}

#[test]
fn a_stale_response_changes_nothing() {
    let mut tx = operational();
    assert!(tx.on_action(response(99, 0, 0, 32), at(2)));
    assert!(tx.operational(0).is_some());
    assert_eq!(tx.poll(at(2), true), None);
}

#[test]
fn a_dropped_member_owes_a_block_ack_request_at_the_new_head() {
    let mut tx = operational();
    let marks = |delivered, expired| crate::data::BlockAckMarks {
        tid: oer_ieee80211_wire::qos::WmmUserPriority::UP0,
        start: SequenceNumber::new(5).unwrap(),
        delivered,
        expired,
    };
    // 5 and 6 delivered: the window moves, no request.
    tx.settle(marks(0b11, 0), [1; 6], [2; 6]);
    assert_eq!(tx.operational(0).unwrap().starting_sequence.get(), 7);
    assert_eq!(tx.poll(at(2), true), None);
    // 7 dropped, 8 delivered: the head passes 7 and owes a request at 9.
    let marks = crate::data::BlockAckMarks {
        start: SequenceNumber::new(7).unwrap(),
        ..marks(0b10, 0b01)
    };
    tx.settle(marks, [1; 6], [2; 6]);
    let start = SequenceNumber::new(9).unwrap();
    assert_eq!(tx.deadline(true), Some(Instant::EPOCH));
    assert_eq!(
        tx.poll(at(3), true),
        Some(TxBlockAckOwed::Request { tid: 0, start })
    );
    tx.request_sent(0);
    assert_eq!(tx.poll(at(3), true), None);
    // Unanswered, it is owed again at the head.
    tx.request_unanswered(0);
    assert_eq!(
        tx.poll(at(4), true),
        Some(TxBlockAckOwed::Request { tid: 0, start })
    );
}

#[test]
fn traffic_changes_the_agreements_only_when_it_queues_a_negotiation() {
    let mut tx = StaTxBlockAck::new(POLICY).unwrap();
    assert!(!tx.traffic(0, at(0)), "before admission it changes nothing");
    tx.associate(true);
    assert!(tx.traffic(0, at(0)), "an idle TID queues its negotiation");
    assert!(!tx.traffic(0, at(1)), "a queued one is only recorded");
    let mut tx = operational();
    let deadline = tx.deadline(true);
    assert!(deadline.is_some());
    assert!(
        !tx.traffic(0, at(1_000)),
        "an operational one keeps its earlier inactivity end"
    );
    assert_eq!(tx.deadline(true), deadline);
    assert!(!tx.may_owe(at(1_000)));
}
