use super::*;
use oer_protocols_ieee80211_wire::management::DialogToken;
use oer_protocols_ieee80211_wire::qos::Tid;
use oer_protocols_ieee80211_wire::sequence::SequenceNumber;

const POLICY: StaTxBlockAckPolicy = StaTxBlockAckPolicy {
    window: oer_protocols_ieee80211_wire::block_ack::frame::BlockAckWindow::constant::<32>(),
    negotiation_timeout: Duration::from_millis(100),
    retry: TxBlockAckRetry {
        attempts: 2,
        interval: Duration::from_millis(10),
    },
    inactivity: Duration::from_secs(5),
    retry_members: oer_protocols_ieee80211_core_datapath::AggregateRetry {
        lifetime: Duration::from_millis(1_500),
        attempts: 4,
    },
};

fn at(millis: u64) -> Instant {
    Instant::from_micros(millis * 1_000)
}

fn response(dialog_token: u8, tid: u8, status: u16, window: u16) -> BlockAckAction {
    BlockAckAction::AddbaResponse {
        dialog_token: DialogToken::new(dialog_token),
        status: oer_protocols_ieee80211_wire::status::StatusCode::new(status),
        timeout: oer_protocols_ieee80211_wire::tsf::TimeUnits::new(0),
        parameters: oer_protocols_ieee80211_wire::block_ack::frame::AddbaParameters::new(
            oer_protocols_ieee80211_wire::block_ack::frame::BlockAckPolicy::Immediate,
            oer_protocols_ieee80211_wire::block_ack::frame::AmsduSupport::NotSupported,
            Tid::new(tid).unwrap(),
            oer_protocols_ieee80211_wire::block_ack::frame::BlockAckWindow::new(window).map_or(
                oer_protocols_ieee80211_wire::block_ack::frame::BufferSize::Unnamed,
                oer_protocols_ieee80211_wire::block_ack::frame::BufferSize::Window,
            ),
        ),
    }
}

/// An admitted association with data of `tid` and no agreement yet.
pub(crate) fn negotiating(tid: WmmUserPriority) -> StaTxBlockAck {
    let mut tx = StaTxBlockAck::new(POLICY).unwrap();
    tx.associate(BlockAckAdmission::Admitted);
    tx.traffic(tid, at(0));
    tx
}

/// A refused TID above the first owes a DELBA of itself alone.
#[test]
fn a_refused_response_owes_the_delba_of_its_own_tid() {
    let mut tx = negotiating(WmmUserPriority::UP5);
    tx.poll(at(0), SendRoom::Free);
    let request = tx
        .begin(WmmUserPriority::UP5, SequenceNumber::ZERO, at(0))
        .unwrap();
    assert!(tx.on_action(response(request.dialog_token.value(), 5, 0, 64), at(1)));
    assert_eq!(
        tx.poll(at(1), SendRoom::Free),
        Some(TxBlockAckOwed::Delba {
            tid: WmmUserPriority::UP5
        })
    );
    tx.delba_sent(WmmUserPriority::UP0);
    assert!(tx.may_owe(at(1)), "another TID's DELBA went");
    tx.delba_sent(WmmUserPriority::UP5);
    assert_eq!(tx.poll(at(1), SendRoom::Free), None);
}

/// An admitted association whose TID 0 agreement is operational.
pub(crate) fn operational() -> StaTxBlockAck {
    let mut tx = StaTxBlockAck::new(POLICY).unwrap();
    tx.associate(BlockAckAdmission::Admitted);
    tx.traffic(WmmUserPriority::UP0, at(0));
    assert_eq!(
        tx.poll(at(0), SendRoom::Free),
        Some(TxBlockAckOwed::Negotiate {
            tid: WmmUserPriority::UP0
        })
    );
    let request = tx
        .begin(WmmUserPriority::UP0, SequenceNumber::new(5).unwrap(), at(0))
        .unwrap();
    assert!(tx.on_action(response(request.dialog_token.value(), 0, 0, 32), at(1)));
    tx
}

#[test]
fn a_policy_that_cannot_negotiate_is_refused() {
    let invalid = |policy| StaTxBlockAck::new(policy).err();
    assert!(matches!(
        invalid(StaTxBlockAckPolicy {
            window: oer_protocols_ieee80211_wire::block_ack::frame::BlockAckWindow::constant::<65>(
            ),
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
    tx.associate(BlockAckAdmission::Refused);
    tx.traffic(WmmUserPriority::UP0, at(0));
    assert_eq!(
        tx.poll(at(0), SendRoom::Free),
        None,
        "a legacy or non-QoS association"
    );
    assert_eq!(tx.deadline(SendRoom::Free), None);

    tx.associate(BlockAckAdmission::Admitted);
    tx.traffic(WmmUserPriority::UP6, at(0));
    assert_eq!(
        tx.deadline(SendRoom::Blocked),
        None,
        "nothing to send takes no deadline"
    );
    assert_eq!(tx.deadline(SendRoom::Free), Some(Instant::EPOCH));
    assert_eq!(tx.poll(at(0), SendRoom::Blocked), None);
    assert_eq!(
        tx.poll(at(0), SendRoom::Free),
        Some(TxBlockAckOwed::Negotiate {
            tid: WmmUserPriority::UP6
        })
    );
    let request = tx
        .begin(
            WmmUserPriority::UP6,
            SequenceNumber::new(40).unwrap(),
            at(0),
        )
        .unwrap();
    assert_eq!(request.starting_sequence, SequenceNumber::new(40).unwrap());
    assert_eq!(
        tx.deadline(SendRoom::Free),
        Some(at(100)),
        "the response deadline"
    );
    assert!(tx.on_action(response(request.dialog_token.value(), 6, 0, 16), at(3)));
    let agreement = tx.operational(WmmUserPriority::UP6).unwrap();
    assert_eq!(
        (agreement.window.get(), agreement.starting_sequence.get()),
        (16, 40)
    );
    assert_eq!(
        tx.deadline(SendRoom::Free),
        Some(at(5_003)),
        "the inactivity end"
    );
}

#[test]
fn an_unanswered_negotiation_retries_then_closes_its_tid() {
    let mut tx = StaTxBlockAck::new(POLICY).unwrap();
    tx.associate(BlockAckAdmission::Admitted);
    tx.traffic(WmmUserPriority::UP0, at(0));
    let mut now = at(0);
    for _ in 0..2 {
        assert_eq!(
            tx.poll(now, SendRoom::Free),
            Some(TxBlockAckOwed::Negotiate {
                tid: WmmUserPriority::UP0
            })
        );
        tx.begin(WmmUserPriority::UP0, SequenceNumber::ZERO, now)
            .unwrap();
        now = now.saturating_add(Duration::from_millis(100));
        assert_eq!(
            tx.poll(now, SendRoom::Free),
            None,
            "the next attempt waits its interval"
        );
        now = now.saturating_add(Duration::from_millis(10));
    }
    assert_eq!(tx.poll(now, SendRoom::Free), None);
    tx.traffic(WmmUserPriority::UP0, now);
    assert_eq!(
        tx.poll(now, SendRoom::Free),
        None,
        "a TID whose attempts ran out stays closed"
    );
    tx.associate(BlockAckAdmission::Admitted);
    tx.traffic(WmmUserPriority::UP0, now);
    assert_eq!(
        tx.poll(now, SendRoom::Free),
        Some(TxBlockAckOwed::Negotiate {
            tid: WmmUserPriority::UP0
        }),
        "a new association offers it again"
    );
}

#[test]
fn a_request_that_did_not_leave_retries_after_the_interval() {
    let mut tx = StaTxBlockAck::new(POLICY).unwrap();
    tx.associate(BlockAckAdmission::Admitted);
    tx.traffic(WmmUserPriority::UP1, at(0));
    assert_eq!(
        tx.poll(at(0), SendRoom::Free),
        Some(TxBlockAckOwed::Negotiate {
            tid: WmmUserPriority::UP1
        })
    );
    tx.begin(WmmUserPriority::UP1, SequenceNumber::ZERO, at(0))
        .unwrap();
    tx.request_failed(WmmUserPriority::UP1, at(2));
    assert_eq!(tx.deadline(SendRoom::Free), Some(at(12)));
    assert_eq!(tx.poll(at(11), SendRoom::Free), None);
    assert_eq!(
        tx.poll(at(12), SendRoom::Free),
        Some(TxBlockAckOwed::Negotiate {
            tid: WmmUserPriority::UP1
        })
    );
}

#[test]
fn only_due_work_makes_the_agreements_owe() {
    let mut tx = StaTxBlockAck::new(POLICY).unwrap();
    tx.associate(BlockAckAdmission::Admitted);
    assert!(!tx.may_owe(at(0)), "a new association owes nothing");
    tx.traffic(WmmUserPriority::UP2, at(0));
    assert!(tx.may_owe(at(0)), "queued data negotiates at once");
    assert_eq!(
        tx.poll(at(0), SendRoom::Free),
        Some(TxBlockAckOwed::Negotiate {
            tid: WmmUserPriority::UP2
        })
    );
    tx.begin(WmmUserPriority::UP2, SequenceNumber::ZERO, at(0))
        .unwrap();
    assert!(!tx.may_owe(at(99)), "the response is awaited");
    assert!(tx.may_owe(at(100)), "the response deadline is due");
    assert_eq!(tx.poll(at(100), SendRoom::Free), None);
    assert!(!tx.may_owe(at(109)), "the next attempt waits its interval");
    assert!(tx.may_owe(at(110)));
    assert_eq!(tx.deadline(SendRoom::Free), Some(at(110)));
    assert_eq!(
        tx.poll(at(110), SendRoom::Free),
        Some(TxBlockAckOwed::Negotiate {
            tid: WmmUserPriority::UP2
        })
    );
    let request = tx
        .begin(WmmUserPriority::UP2, SequenceNumber::ZERO, at(110))
        .unwrap();
    assert!(tx.on_action(response(request.dialog_token.value(), 2, 0, 32), at(111)));
    tx.traffic(WmmUserPriority::UP2, at(111));
    assert!(
        !tx.may_owe(at(112)),
        "a live agreement with fresh data owes nothing"
    );
    assert_eq!(tx.deadline(SendRoom::Free), Some(at(5_111)));
    assert!(tx.may_owe(at(5_111)), "its inactivity end is due");
}

#[test]
fn a_refusal_closes_the_tid_without_a_delba() {
    let mut tx = StaTxBlockAck::new(POLICY).unwrap();
    tx.associate(BlockAckAdmission::Admitted);
    tx.traffic(WmmUserPriority::UP0, at(0));
    tx.poll(at(0), SendRoom::Free);
    let request = tx
        .begin(WmmUserPriority::UP0, SequenceNumber::ZERO, at(0))
        .unwrap();
    assert!(tx.on_action(response(request.dialog_token.value(), 0, 37, 0), at(1)));
    assert_eq!(tx.operational(WmmUserPriority::UP0), None);
    tx.traffic(WmmUserPriority::UP0, at(2));
    assert_eq!(tx.poll(at(2), SendRoom::Free), None);
}

#[test]
fn a_response_beyond_the_requested_window_ends_with_a_delba() {
    let mut tx = StaTxBlockAck::new(POLICY).unwrap();
    tx.associate(BlockAckAdmission::Admitted);
    tx.traffic(WmmUserPriority::UP0, at(0));
    tx.poll(at(0), SendRoom::Free);
    let request = tx
        .begin(WmmUserPriority::UP0, SequenceNumber::ZERO, at(0))
        .unwrap();
    assert!(tx.on_action(response(request.dialog_token.value(), 0, 0, 64), at(1)));
    assert_eq!(tx.operational(WmmUserPriority::UP0), None);
    assert_eq!(
        tx.poll(at(1), SendRoom::Free),
        Some(TxBlockAckOwed::Delba {
            tid: WmmUserPriority::UP0
        })
    );
    tx.delba_sent(WmmUserPriority::UP0);
    tx.traffic(WmmUserPriority::UP0, at(2));
    assert_eq!(tx.poll(at(2), SendRoom::Free), None, "the TID stays closed");
}

#[test]
fn an_idle_agreement_ends_with_a_delba_and_new_data_renegotiates() {
    let mut tx = operational();
    tx.traffic(WmmUserPriority::UP0, at(1_000));
    assert_eq!(
        tx.poll(at(5_999), SendRoom::Free),
        None,
        "data kept it active"
    );
    assert_eq!(
        tx.poll(at(6_000), SendRoom::Blocked),
        None,
        "no transmission to give"
    );
    assert_eq!(
        tx.operational(WmmUserPriority::UP0),
        None,
        "but the idle agreement ended"
    );
    assert_eq!(tx.deadline(SendRoom::Free), Some(Instant::EPOCH));
    assert_eq!(
        tx.poll(at(6_000), SendRoom::Free),
        Some(TxBlockAckOwed::Delba {
            tid: WmmUserPriority::UP0
        })
    );
    tx.delba_sent(WmmUserPriority::UP0);
    assert_eq!(tx.poll(at(6_000), SendRoom::Free), None);
    tx.traffic(WmmUserPriority::UP0, at(7_000));
    assert_eq!(
        tx.poll(at(7_000), SendRoom::Free),
        Some(TxBlockAckOwed::Negotiate {
            tid: WmmUserPriority::UP0
        })
    );
}

#[test]
fn the_recipient_s_delba_ends_the_agreement_and_receive_actions_pass_by() {
    let mut tx = operational();
    let initiator = |initiator| BlockAckAction::Delba {
        tid: Tid::constant::<0>(),
        initiator,
        reason: oer_protocols_ieee80211_wire::status::ReasonCode::new(37),
    };
    assert!(
        !tx.on_action(
            initiator(oer_protocols_ieee80211_wire::block_ack::frame::DelbaInitiator::Originator),
            at(2)
        ),
        "the AP's own agreement"
    );
    assert!(tx.operational(WmmUserPriority::UP0).is_some());
    assert!(tx.on_action(
        initiator(oer_protocols_ieee80211_wire::block_ack::frame::DelbaInitiator::Recipient),
        at(2)
    ));
    assert_eq!(tx.operational(WmmUserPriority::UP0), None);
    assert_eq!(
        tx.poll(at(2), SendRoom::Free),
        None,
        "the recipient wants no answer"
    );
    let request = BlockAckAction::AddbaRequest {
        dialog_token: DialogToken::new(1),
        timeout: oer_protocols_ieee80211_wire::tsf::TimeUnits::new(0),
        starting_sequence: SequenceNumber::ZERO,
        parameters: oer_protocols_ieee80211_wire::block_ack::frame::AddbaParameters::new(
            oer_protocols_ieee80211_wire::block_ack::frame::BlockAckPolicy::Immediate,
            oer_protocols_ieee80211_wire::block_ack::frame::AmsduSupport::NotSupported,
            Tid::constant::<0>(),
            oer_protocols_ieee80211_wire::block_ack::frame::BufferSize::Window(
                oer_protocols_ieee80211_wire::block_ack::frame::BlockAckWindow::constant::<64>(),
            ),
        ),
    };
    assert!(!tx.on_action(request, at(2)));
}

#[test]
fn a_stale_response_changes_nothing() {
    let mut tx = operational();
    assert!(tx.on_action(response(99, 0, 0, 32), at(2)));
    assert!(tx.operational(WmmUserPriority::UP0).is_some());
    assert_eq!(tx.poll(at(2), SendRoom::Free), None);
}

#[test]
fn a_dropped_frame_owes_a_block_ack_request_at_the_planes_head() {
    let mut tx = StaTxBlockAck::new(POLICY).unwrap();
    tx.associate(BlockAckAdmission::Admitted);
    // Without an operational agreement the plane's head owes nothing.
    tx.request(WmmUserPriority::UP0, SequenceNumber::new(4).unwrap());
    assert_eq!(tx.poll(at(2), SendRoom::Free), None);
    let mut tx = operational();
    let start = SequenceNumber::new(9).unwrap();
    tx.request(WmmUserPriority::UP0, start);
    assert_eq!(tx.deadline(SendRoom::Free), Some(Instant::EPOCH));
    assert_eq!(
        tx.poll(at(3), SendRoom::Free),
        Some(TxBlockAckOwed::Request {
            tid: WmmUserPriority::UP0,
            start
        })
    );
    tx.request_sent(WmmUserPriority::UP0);
    assert_eq!(tx.poll(at(3), SendRoom::Free), None);
    // Unanswered, it is owed again at the plane's last head.
    tx.request_unanswered(WmmUserPriority::UP0);
    assert_eq!(
        tx.poll(at(4), SendRoom::Free),
        Some(TxBlockAckOwed::Request {
            tid: WmmUserPriority::UP0,
            start
        })
    );
}

#[test]
fn traffic_changes_the_agreements_only_when_it_queues_a_negotiation() {
    let mut tx = StaTxBlockAck::new(POLICY).unwrap();
    assert!(
        !tx.traffic(WmmUserPriority::UP0, at(0)),
        "before admission it changes nothing"
    );
    tx.associate(BlockAckAdmission::Admitted);
    assert!(
        tx.traffic(WmmUserPriority::UP0, at(0)),
        "an idle TID queues its negotiation"
    );
    assert!(
        !tx.traffic(WmmUserPriority::UP0, at(1)),
        "a queued one is only recorded"
    );
    let mut tx = operational();
    let deadline = tx.deadline(SendRoom::Free);
    assert!(deadline.is_some());
    assert!(
        !tx.traffic(WmmUserPriority::UP0, at(1_000)),
        "an operational one keeps its earlier inactivity end"
    );
    assert_eq!(tx.deadline(SendRoom::Free), deadline);
    assert!(!tx.may_owe(at(1_000)));
}
