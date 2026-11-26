use oer_portable_time::{Duration, Instant};
use oer_protocols_ieee802_wire::Eui48;
use oer_protocols_ieee80211_wire::qos::Tid;

use super::*;
use oer_protocols_ieee80211_wire::sequence::SequenceNumber;
use {
    crate::block_ack::{OperationalTxBlockAck, TxBlockAckResponse},
    oer_protocols_ieee80211_wire::block_ack::BlockAckAction,
};

/// Three TIDs and a modulo-63 token sequence, as the Espressif station's.
const POLICY: TxBlockAckOriginatorPolicy = TxBlockAckOriginatorPolicy {
    tids: &[
        WmmUserPriority::UP0,
        WmmUserPriority::UP7,
        WmmUserPriority::UP5,
    ],
    dialog_tokens: DialogTokenSequence {
        first: DialogToken::new(1),
        lowest: DialogToken::new(0),
        highest: DialogToken::new(62),
    },
};

fn originator(
    window: u16,
    amsdu_tids: &'static [WmmUserPriority],
) -> TxBlockAckOriginator<TX_BLOCK_ACK_MAX_TIDS> {
    TxBlockAckOriginator::new(
        POLICY,
        TxBlockAckOriginatorConfig {
            window: oer_protocols_ieee80211_wire::block_ack::frame::BlockAckWindow::new(window)
                .unwrap(),
            negotiation_timeout: Duration::from_micros(100_000),
            amsdu_tids,
        },
    )
    .unwrap()
}

fn sequence(value: u16) -> SequenceNumber {
    SequenceNumber::new(value).unwrap()
}

fn response(dialog_token: u8, tid: u8, status: u16, window: u16) -> BlockAckAction {
    BlockAckAction::AddbaResponse {
        dialog_token: DialogToken::new(dialog_token),
        status: oer_protocols_ieee80211_wire::status::StatusCode::new(status),
        timeout: oer_protocols_ieee80211_wire::tsf::TimeUnits::new(7),
        parameters: oer_protocols_ieee80211_wire::block_ack::frame::AddbaParameters::new(
            oer_protocols_ieee80211_wire::block_ack::frame::BlockAckPolicy::Immediate,
            oer_protocols_ieee80211_wire::block_ack::frame::AmsduSupport::Supported,
            Tid::new(tid).unwrap(),
            oer_protocols_ieee80211_wire::block_ack::frame::BlockAckWindow::new(window).map_or(
                oer_protocols_ieee80211_wire::block_ack::frame::BufferSize::Unnamed,
                oer_protocols_ieee80211_wire::block_ack::frame::BufferSize::Window,
            ),
        ),
    }
}

#[test]
fn the_policy_s_tids_share_one_token_sequence_and_route_responses() {
    let mut originator = originator(32, &[WmmUserPriority::UP0]);
    let at = Instant::from_micros(0);
    let tid0 = originator
        .begin(WmmUserPriority::UP0, sequence(0x100), at)
        .unwrap();
    let tid7 = originator
        .begin(WmmUserPriority::UP7, sequence(0x200), at)
        .unwrap();
    let tid5 = originator
        .begin(WmmUserPriority::UP5, sequence(0x300), at)
        .unwrap();
    assert_eq!(
        [tid0.dialog_token, tid7.dialog_token, tid5.dialog_token],
        [1, 2, 3].map(DialogToken::new)
    );
    assert_eq!(originator.alarm(WmmUserPriority::UP7), Some(tid7.alarm));
    assert_eq!(
        originator.on_response_action(response(tid7.dialog_token.value(), 7, 0, 16)),
        Ok(TxBlockAckResponseDisposition::Matched(
            TxBlockAckOriginatorResponse {
                tid: WmmUserPriority::UP7,
                response: TxBlockAckResponse::Operational(OperationalTxBlockAck {
                    tid: WmmUserPriority::UP7,
                    window:
                        oer_protocols_ieee80211_wire::block_ack::frame::BlockAckWindow::constant::<16>(
                        ),
                    timeout: oer_protocols_ieee80211_wire::tsf::TimeUnits::new(7),
                    starting_sequence: sequence(0x200),
                    amsdu: false,
                }),
            }
        ))
    );
    assert_eq!(originator.alarm(WmmUserPriority::UP7), None);
    // TID 0 may carry A-MSDUs, as the configuration allows.
    assert!(matches!(
        originator.on_response_action(response(tid0.dialog_token.value(), 0, 0, 16)),
        Ok(TxBlockAckResponseDisposition::Matched(
            TxBlockAckOriginatorResponse {
                response: TxBlockAckResponse::Operational(OperationalTxBlockAck {
                    amsdu: true,
                    ..
                }),
                ..
            }
        ))
    ));
    assert_eq!(
        originator.expire_next(Instant::from_micros(100_000)),
        Some(WmmUserPriority::UP5)
    );
    assert_eq!(originator.expire_next(Instant::from_micros(100_000)), None);
    assert!(originator.operational(WmmUserPriority::UP7).is_some());
}

#[test]
fn the_earliest_deadline_follows_the_live_negotiations() {
    let mut originator = originator(16, &[]);
    assert_eq!(originator.earliest_alarm_deadline(), None);
    originator
        .begin(WmmUserPriority::UP0, sequence(0), Instant::from_micros(75))
        .unwrap();
    originator
        .begin(WmmUserPriority::UP7, sequence(0), Instant::from_micros(25))
        .unwrap();
    originator
        .begin(WmmUserPriority::UP5, sequence(0), Instant::from_micros(50))
        .unwrap();
    assert_eq!(
        originator.earliest_alarm_deadline(),
        Some(Instant::from_micros(100_025))
    );
    assert!(originator.stop(WmmUserPriority::UP7));
    assert_eq!(
        originator.earliest_alarm_deadline(),
        Some(Instant::from_micros(100_050))
    );
}

#[test]
fn an_unowned_tid_and_a_stale_token_change_nothing() {
    let mut originator = originator(32, &[]);
    assert_eq!(
        originator.begin(WmmUserPriority::UP3, sequence(0), Instant::from_micros(0)),
        Err(TxBlockAckOriginatorError::UnsupportedTid(
            WmmUserPriority::UP3
        ))
    );
    assert_eq!(
        originator.on_response(&[3, 1, 42, 0, 0, 0, 0, 0, 0]),
        Ok(TxBlockAckResponseDisposition::StaleDialogToken(
            DialogToken::new(42)
        ))
    );
    assert!(!originator.stop(WmmUserPriority::UP3));
}

#[test]
fn attempts_requeue_until_the_peer_answers_or_they_run_out() {
    let mut originator = originator(32, &[]);
    let at = Instant::from_micros(0);
    assert!(!originator.has_pending());
    originator.queue_initial(TxBlockAckRetry {
        attempts: 2,
        interval: Duration::ZERO,
    });
    assert_eq!(originator.take_pending(at), Some(WmmUserPriority::UP0));
    assert_eq!(originator.take_pending(at), Some(WmmUserPriority::UP7));
    assert_eq!(originator.take_pending(at), Some(WmmUserPriority::UP5));
    assert_eq!(originator.take_pending(at), None);

    // TID 0's request did not leave: its second attempt is queued.
    originator.transmit_failed(WmmUserPriority::UP0, at);
    assert_eq!(originator.take_pending(at), Some(WmmUserPriority::UP0));
    let tid0 = originator
        .begin(WmmUserPriority::UP0, sequence(0), Instant::from_micros(0))
        .unwrap();
    // Its second negotiation times out with no attempt left.
    assert_eq!(
        originator.expire_next(Instant::from_micros(100_000)),
        Some(WmmUserPriority::UP0)
    );
    assert!(!originator.has_pending());
    assert_ne!(tid0.dialog_token, DialogToken::ZERO);

    // TID 7 is refused: the peer's answer ends its attempts.
    let tid7 = originator
        .begin(WmmUserPriority::UP7, sequence(0), Instant::from_micros(0))
        .unwrap();
    assert_eq!(
        originator.on_response_action(response(tid7.dialog_token.value(), 7, 37, 0)),
        Ok(TxBlockAckResponseDisposition::Matched(
            TxBlockAckOriginatorResponse {
                tid: WmmUserPriority::UP7,
                response: TxBlockAckResponse::Rejected(
                    oer_protocols_ieee80211_wire::status::StatusCode::REQUEST_DECLINED
                ),
            }
        ))
    );
    assert!(!originator.has_pending());

    // Leaving ends everything and counts what was live.
    originator
        .begin(WmmUserPriority::UP5, sequence(0), Instant::from_micros(0))
        .unwrap();
    assert_eq!(originator.stop_all(), 1);
    assert_eq!(originator.earliest_alarm_deadline(), None);
}

#[test]
fn a_policy_names_each_tid_once() {
    const TWICE: TxBlockAckOriginatorPolicy = TxBlockAckOriginatorPolicy {
        tids: &[WmmUserPriority::UP0, WmmUserPriority::UP0],
        ..POLICY
    };
    assert!(matches!(
        TxBlockAckOriginator::<TX_BLOCK_ACK_MAX_TIDS>::new(
            TWICE,
            TxBlockAckOriginatorConfig {
                window: oer_protocols_ieee80211_wire::block_ack::frame::BlockAckWindow::constant::<8>(
                ),
                negotiation_timeout: Duration::from_micros(1),
                amsdu_tids: &[],
            },
        ),
        Err(TxBlockAckOriginatorError::UnsupportedTid(
            WmmUserPriority::UP0
        ))
    ));
}

#[test]
fn a_failed_attempt_waits_the_interval_before_the_next_is_due() {
    let mut originator = originator(32, &[]);
    originator.queue_initial(TxBlockAckRetry {
        attempts: 3,
        interval: Duration::from_micros(1_000_000),
    });
    let at = Instant::from_micros(500);
    assert_eq!(originator.next_pending_deadline(), Some(Instant::EPOCH));
    for tid in [0, 7, 5] {
        assert_eq!(
            originator.take_pending(at),
            Some(WmmUserPriority::new(tid).unwrap())
        );
    }
    originator
        .begin(WmmUserPriority::UP0, sequence(0), at)
        .unwrap();
    // The response does not come: the next attempt is due one interval
    // after the timeout, not before.
    let timeout = Instant::from_micros(100_500);
    assert_eq!(originator.expire_next(timeout), Some(WmmUserPriority::UP0));
    let due = Instant::from_micros(1_100_500);
    assert_eq!(originator.next_pending_deadline(), Some(due));
    assert_eq!(
        originator.take_pending(Instant::from_micros(1_100_499)),
        None
    );
    assert_eq!(originator.take_pending(due), Some(WmmUserPriority::UP0));
    // The third and last attempt's request does not leave.
    originator
        .begin(WmmUserPriority::UP0, sequence(0), due)
        .unwrap();
    originator.transmit_failed(WmmUserPriority::UP0, due);
    assert_eq!(
        originator.take_pending(Instant::from_micros(2_100_500)),
        Some(WmmUserPriority::UP0)
    );
    originator.transmit_failed(WmmUserPriority::UP0, Instant::from_micros(2_100_500));
    assert!(!originator.has_pending());
    assert_eq!(originator.generation(WmmUserPriority::UP3), 0);
}

#[test]
fn no_attempt_queues_nothing() {
    let mut originator = originator(32, &[]);
    originator.queue_initial(TxBlockAckRetry {
        attempts: 0,
        interval: Duration::ZERO,
    });
    assert!(!originator.has_pending());
    assert_eq!(DialogToken::new(255).next_nonzero(), DialogToken::new(1));
}

#[test]
fn an_originator_owns_at_most_its_tids() {
    let config = TxBlockAckOriginatorConfig {
        window: oer_protocols_ieee80211_wire::block_ack::frame::BlockAckWindow::constant::<8>(),
        negotiation_timeout: Duration::from_micros(1),
        amsdu_tids: &[],
    };
    assert!(matches!(
        TxBlockAckOriginator::<2>::new(POLICY, config),
        Err(TxBlockAckOriginatorError::TooManyTids)
    ));
    let one = TxBlockAckOriginatorPolicy {
        tids: &[WmmUserPriority::UP0],
        ..POLICY
    };
    let originator = TxBlockAckOriginator::<1>::new(one, config).unwrap();
    let mut tids = originator.tids();
    assert_eq!(
        (tids.next(), tids.next()),
        (Some(WmmUserPriority::UP0), None)
    );
}

fn responses(count: usize) {
    let mut seeds = std::vec::Vec::new();
    for (dialog_token, tid, status) in [(1, 0_u8, 0), (2, 7, 0), (3, 5, 37), (42, 0, 0)] {
        let parameters = (1_u16 << 1 | u16::from(tid) << 2 | 16 << 6).to_le_bytes();
        let status = u16::to_le_bytes(status);
        seeds.push([
            3,
            1,
            dialog_token,
            status[0],
            status[1],
            parameters[0],
            parameters[1],
            7,
            0,
        ]);
    }
    let seeds: std::vec::Vec<&[u8]> = seeds.iter().map(|seed| seed.as_slice()).collect();
    oer_protocols_ieee80211_wire::fuzz::inputs(&seeds, count, 32, |bytes| {
        let mut originator = originator(32, &[WmmUserPriority::UP0]);
        let at = Instant::from_micros(0);
        for tid in [0, 7, 5] {
            originator
                .begin(WmmUserPriority::new(tid).unwrap(), sequence(0x100), at)
                .unwrap();
        }
        if let Ok(TxBlockAckResponseDisposition::Matched(matched)) = originator.on_response(bytes) {
            let _ = originator.operational(matched.tid);
        }
    });
}

#[test]
fn response_routing_never_panics() {
    responses(oer_protocols_ieee80211_wire::fuzz::QUICK);
}

#[test]
#[ignore = "the full run"]
fn response_routing_never_panics_full() {
    responses(oer_protocols_ieee80211_wire::fuzz::FULL);
}

fn active_originator(start: u16) -> TxBlockAckOriginator<TX_BLOCK_ACK_MAX_TIDS> {
    let mut core = originator(32, &[]);
    let request = core
        .begin(WmmUserPriority::UP0, sequence(start), Instant::EPOCH)
        .unwrap();
    core.on_response_action(response(request.dialog_token.value(), 0, 0, 32))
        .unwrap();
    core
}

const PEER: [u8; 6] = [2, 0, 0, 0, 0, 2];
const LOCAL: [u8; 6] = [2, 0, 0, 0, 0, 1];
fn bar(start: u16) -> [u8; oer_protocols_ieee80211_wire::block_ack::BLOCK_ACK_REQUEST_LEN] {
    oer_protocols_ieee80211_wire::block_ack::encode_block_ack_request(
        Eui48::new(PEER),
        Eui48::new(LOCAL),
        Tid::constant::<0>(),
        sequence(start),
    )
}

#[test]
fn expiry_after_deliveries_uses_the_current_originator_head() {
    let mut core = active_originator(100);
    assert_eq!(
        core.on_delivered(
            WmmUserPriority::UP0,
            sequence(100),
            0b11,
            Eui48::new(PEER),
            Eui48::new(LOCAL)
        ),
        None
    );
    assert_eq!(
        core.operational(WmmUserPriority::UP0)
            .unwrap()
            .starting_sequence,
        sequence(102)
    );
    assert_eq!(
        core.on_expired(
            WmmUserPriority::UP0,
            sequence(102),
            1,
            Eui48::new(PEER),
            Eui48::new(LOCAL)
        ),
        Some(bar(103))
    );
    assert_eq!(
        core.operational(WmmUserPriority::UP0)
            .unwrap()
            .starting_sequence,
        sequence(103)
    );
}

#[test]
fn a_non_head_expiry_is_remembered_until_delivery_reaches_it() {
    let mut core = active_originator(100);
    assert_eq!(
        core.on_expired(
            WmmUserPriority::UP0,
            sequence(102),
            1,
            Eui48::new(PEER),
            Eui48::new(LOCAL)
        ),
        None
    );
    assert_eq!(
        core.on_delivered(
            WmmUserPriority::UP0,
            sequence(103),
            1,
            Eui48::new(PEER),
            Eui48::new(LOCAL)
        ),
        None
    );
    assert_eq!(
        core.on_delivered(
            WmmUserPriority::UP0,
            sequence(100),
            1,
            Eui48::new(PEER),
            Eui48::new(LOCAL)
        ),
        None
    );
    assert_eq!(
        core.operational(WmmUserPriority::UP0)
            .unwrap()
            .starting_sequence,
        sequence(101)
    );
    assert_eq!(
        core.on_delivered(
            WmmUserPriority::UP0,
            sequence(101),
            1,
            Eui48::new(PEER),
            Eui48::new(LOCAL)
        ),
        Some(bar(104))
    );
    assert_eq!(
        core.operational(WmmUserPriority::UP0)
            .unwrap()
            .starting_sequence,
        sequence(104)
    );
    assert_eq!(
        core.on_expired(
            WmmUserPriority::UP0,
            sequence(102),
            1,
            Eui48::new(PEER),
            Eui48::new(LOCAL)
        ),
        None
    );
    assert_eq!(
        core.on_delivered(
            WmmUserPriority::UP0,
            sequence(101),
            1,
            Eui48::new(PEER),
            Eui48::new(LOCAL)
        ),
        None
    );
}

#[test]
fn consecutive_expiries_emit_one_bar_at_the_final_window_start() {
    let mut core = active_originator(100);
    assert_eq!(
        core.on_expired(
            WmmUserPriority::UP0,
            sequence(101),
            0b111,
            Eui48::new(PEER),
            Eui48::new(LOCAL)
        ),
        None
    );
    assert_eq!(
        core.on_expired(
            WmmUserPriority::UP0,
            sequence(100),
            1,
            Eui48::new(PEER),
            Eui48::new(LOCAL)
        ),
        Some(bar(104))
    );
    assert_eq!(
        core.on_expired(
            WmmUserPriority::UP0,
            sequence(104),
            0b111,
            Eui48::new(PEER),
            Eui48::new(LOCAL)
        ),
        Some(bar(107))
    );
    assert_eq!(
        core.on_expired(
            WmmUserPriority::UP0,
            sequence(100),
            0b1111,
            Eui48::new(PEER),
            Eui48::new(LOCAL)
        ),
        None
    );
    assert_eq!(
        core.operational(WmmUserPriority::UP0)
            .unwrap()
            .starting_sequence,
        sequence(107)
    );
}

#[test]
fn delivery_and_deferred_expiry_advance_across_sequence_zero() {
    let mut core = active_originator(4094);
    assert_eq!(
        core.on_expired(
            WmmUserPriority::UP0,
            sequence(4095),
            0b11,
            Eui48::new(PEER),
            Eui48::new(LOCAL)
        ),
        None
    );
    assert_eq!(
        core.on_delivered(
            WmmUserPriority::UP0,
            sequence(1),
            1,
            Eui48::new(PEER),
            Eui48::new(LOCAL)
        ),
        None
    );
    assert_eq!(
        core.on_delivered(
            WmmUserPriority::UP0,
            sequence(4094),
            1,
            Eui48::new(PEER),
            Eui48::new(LOCAL)
        ),
        Some(bar(2))
    );
    assert_eq!(
        core.operational(WmmUserPriority::UP0)
            .unwrap()
            .starting_sequence,
        sequence(2)
    );
    assert_eq!(
        core.on_expired(
            WmmUserPriority::UP0,
            sequence(4095),
            0b11,
            Eui48::new(PEER),
            Eui48::new(LOCAL)
        ),
        None
    );
}

#[test]
fn inactive_or_other_tid_progress_never_emits_a_bar() {
    let mut core = originator(32, &[]);
    assert_eq!(
        core.on_expired(
            WmmUserPriority::UP0,
            sequence(0),
            1,
            Eui48::new(PEER),
            Eui48::new(LOCAL)
        ),
        None
    );
    assert_eq!(
        core.on_delivered(
            WmmUserPriority::UP0,
            sequence(0),
            1,
            Eui48::new(PEER),
            Eui48::new(LOCAL)
        ),
        None
    );
    core = active_originator(0);
    assert_eq!(
        core.on_expired(
            WmmUserPriority::UP1,
            sequence(0),
            1,
            Eui48::new(PEER),
            Eui48::new(LOCAL)
        ),
        None
    );
    assert_eq!(
        core.on_expired(
            WmmUserPriority::UP7,
            sequence(0),
            1,
            Eui48::new(PEER),
            Eui48::new(LOCAL)
        ),
        None
    );
    assert_eq!(
        core.on_expired(
            WmmUserPriority::UP0,
            sequence(0),
            1,
            Eui48::new(PEER),
            Eui48::new(LOCAL)
        ),
        Some(bar(1))
    );
}

#[test]
fn policy_tid_validation_rejects_duplicates() {
    let config = TxBlockAckOriginatorConfig {
        window: oer_protocols_ieee80211_wire::block_ack::frame::BlockAckWindow::constant::<8>(),
        negotiation_timeout: Duration::from_micros(1),
        amsdu_tids: &WmmUserPriority::ALL,
    };
    let policy = TxBlockAckOriginatorPolicy {
        tids: &[
            WmmUserPriority::UP7,
            WmmUserPriority::UP0,
            WmmUserPriority::UP7,
        ],
        ..POLICY
    };
    assert!(matches!(
        TxBlockAckOriginator::<3>::new(policy, config),
        Err(TxBlockAckOriginatorError::UnsupportedTid(observed))
            if observed == WmmUserPriority::UP7
    ));
    let policy = TxBlockAckOriginatorPolicy {
        tids: &[
            WmmUserPriority::UP7,
            WmmUserPriority::UP0,
            WmmUserPriority::UP5,
        ],
        ..POLICY
    };
    let originator = TxBlockAckOriginator::<3>::new(policy, config).unwrap();
    assert!(originator.tids().eq([
        WmmUserPriority::UP7,
        WmmUserPriority::UP0,
        WmmUserPriority::UP5
    ]));
}

const RETRY: TxBlockAckRetry = TxBlockAckRetry {
    attempts: 2,
    interval: Duration::from_micros(1_000),
};

#[test]
fn queue_offers_one_idle_tid_and_never_a_live_one() {
    let mut originator = originator(32, &[]);
    let at = Instant::from_micros(0);
    assert!(
        !originator.queue(WmmUserPriority::UP3, RETRY),
        "a TID outside the policy"
    );
    assert!(originator.queue(WmmUserPriority::UP7, RETRY));
    assert!(
        !originator.queue(WmmUserPriority::UP7, RETRY),
        "already queued"
    );
    assert_eq!(originator.take_pending(at), Some(WmmUserPriority::UP7));
    assert_eq!(originator.take_pending(at), None, "only TID 7 was queued");
    let request = originator
        .begin(WmmUserPriority::UP7, sequence(9), at)
        .unwrap();
    assert!(
        !originator.queue(WmmUserPriority::UP7, RETRY),
        "a negotiation is live"
    );
    originator
        .on_response_action(response(request.dialog_token.value(), 7, 0, 32))
        .unwrap();
    assert!(originator.operational(WmmUserPriority::UP7).is_some());
    assert!(
        !originator.queue(WmmUserPriority::UP7, RETRY),
        "an agreement is live"
    );
    assert!(originator.stop(WmmUserPriority::UP7));
    assert!(
        originator.queue(WmmUserPriority::UP7, RETRY),
        "a stopped TID may negotiate again"
    );
}

#[test]
fn a_refusal_or_spent_attempts_close_the_tid_until_stop_all() {
    let mut originator = originator(32, &[]);
    let mut at = Instant::from_micros(0);
    assert!(originator.queue(WmmUserPriority::UP0, RETRY));
    assert_eq!(originator.take_pending(at), Some(WmmUserPriority::UP0));
    let request = originator
        .begin(WmmUserPriority::UP0, sequence(1), at)
        .unwrap();
    originator
        .on_response_action(response(request.dialog_token.value(), 0, 37, 32))
        .unwrap();
    assert!(originator.is_closed(WmmUserPriority::UP0));
    assert!(!originator.queue(WmmUserPriority::UP0, RETRY));

    // Two unanswered attempts close TID 5.
    assert!(originator.queue(WmmUserPriority::UP5, RETRY));
    for _ in 0..2 {
        assert_eq!(originator.take_pending(at), Some(WmmUserPriority::UP5));
        originator
            .begin(WmmUserPriority::UP5, sequence(1), at)
            .unwrap();
        at = at.saturating_add(Duration::from_micros(100_000));
        assert_eq!(originator.expire_next(at), Some(WmmUserPriority::UP5));
        at = at.saturating_add(Duration::from_micros(1_000));
    }
    assert!(!originator.has_pending());
    assert!(originator.is_closed(WmmUserPriority::UP5));
    assert!(!originator.queue(WmmUserPriority::UP5, RETRY));

    assert!(originator.close(WmmUserPriority::UP7));
    assert!(!originator.queue(WmmUserPriority::UP7, RETRY));
    originator.stop_all();
    assert!(
        originator.queue(WmmUserPriority::UP0, RETRY)
            && originator.queue(WmmUserPriority::UP5, RETRY)
    );
    assert!(originator.queue(WmmUserPriority::UP7, RETRY));
}

#[test]
fn a_dialog_token_sequence_wraps_to_its_lowest_token() {
    let modulo_63 = POLICY.dialog_tokens;
    assert_eq!(modulo_63.next(DialogToken::new(1)), DialogToken::new(2));
    assert_eq!(modulo_63.next(DialogToken::new(61)), DialogToken::new(62));
    assert_eq!(modulo_63.next(DialogToken::new(62)), DialogToken::new(0));
    assert_eq!(modulo_63.next(DialogToken::new(0)), DialogToken::new(1));
    for current in 0..=u8::MAX {
        assert_eq!(
            DialogTokenSequence::NONZERO.next(DialogToken::new(current)),
            DialogToken::new(current).next_nonzero()
        );
    }
}

#[test]
fn the_requests_carry_the_sequence_in_turn() {
    let mut originator = TxBlockAckOriginator::<TX_BLOCK_ACK_MAX_TIDS>::new(
        TxBlockAckOriginatorPolicy {
            dialog_tokens: DialogTokenSequence {
                first: DialogToken::new(62),
                lowest: DialogToken::new(0),
                highest: DialogToken::new(62),
            },
            ..POLICY
        },
        TxBlockAckOriginatorConfig {
            window: oer_protocols_ieee80211_wire::block_ack::frame::BlockAckWindow::constant::<8>(),
            negotiation_timeout: Duration::from_micros(100_000),
            amsdu_tids: &[],
        },
    )
    .unwrap();
    let tokens = [0_u8, 7, 5].map(|tid| {
        originator
            .begin(
                WmmUserPriority::new(tid).unwrap(),
                sequence(0),
                Instant::from_micros(0),
            )
            .unwrap()
            .dialog_token
    });
    assert_eq!(tokens, [62, 0, 1].map(DialogToken::new));
}

#[test]
fn a_first_dialog_token_outside_its_sequence_is_refused() {
    let tokens = DialogTokenSequence {
        first: DialogToken::new(63),
        lowest: DialogToken::new(0),
        highest: DialogToken::new(62),
    };
    assert_eq!(
        TxBlockAckOriginator::<TX_BLOCK_ACK_MAX_TIDS>::new(
            TxBlockAckOriginatorPolicy {
                dialog_tokens: tokens,
                ..POLICY
            },
            TxBlockAckOriginatorConfig {
                window: oer_protocols_ieee80211_wire::block_ack::frame::BlockAckWindow::constant::<8>(),
                negotiation_timeout: Duration::from_micros(1),
                amsdu_tids: &[],
            },
        )
        .err(),
        Some(TxBlockAckOriginatorError::DialogTokens(tokens))
    );
}

/// The negotiation's states as the originator answers them: open and
/// queued, no attempt to queue, its alarm not before its deadline.
#[test]
fn a_negotiation_is_queued_awaited_and_expires_only_at_its_deadline() {
    let mut originator = originator(32, &[]);
    assert!(
        !originator.is_closed(WmmUserPriority::UP7),
        "a new TID is open"
    );
    assert!(
        !originator.queue(
            WmmUserPriority::UP7,
            TxBlockAckRetry {
                attempts: 0,
                interval: Duration::ZERO,
            }
        ),
        "no attempt queues nothing"
    );
    assert!(!originator.has_pending());
    assert!(originator.queue(WmmUserPriority::UP7, RETRY));
    assert!(originator.has_pending());
    let at = Instant::from_micros(1_000);
    assert_eq!(originator.take_pending(at), Some(WmmUserPriority::UP7));
    originator
        .begin(WmmUserPriority::UP7, sequence(0), at)
        .unwrap();
    let deadline = originator.alarm(WmmUserPriority::UP7).unwrap().deadline;
    assert_eq!(
        originator.expire_next(Instant::from_micros(deadline.as_micros() - 1)),
        None,
        "not before its deadline"
    );
    assert_eq!(originator.expire_next(deadline), Some(WmmUserPriority::UP7));
}

/// A second `queue_initial` leaves a negotiation already out alone: its
/// alarm stays, nothing for its TID is due, and the other TIDs are queued.
/// Its attempts are the new policy's, so its expiry queues a retry.
#[test]
fn queue_initial_keeps_a_negotiation_already_out() {
    let mut originator = originator(32, &[]);
    originator.queue_initial(TxBlockAckRetry {
        attempts: 1,
        interval: RETRY.interval,
    });
    let at = Instant::from_micros(1_000);
    assert_eq!(originator.take_pending(at), Some(WmmUserPriority::UP0));
    originator
        .begin(WmmUserPriority::UP0, sequence(0), at)
        .unwrap();
    let alarm = originator.alarm(WmmUserPriority::UP0).unwrap();
    originator.queue_initial(RETRY);
    assert_eq!(
        originator.alarm(WmmUserPriority::UP0),
        Some(alarm),
        "its alarm stays"
    );
    assert_eq!(originator.take_pending(at), Some(WmmUserPriority::UP7));
    assert_eq!(originator.take_pending(at), Some(WmmUserPriority::UP5));
    assert_eq!(originator.take_pending(at), None, "nothing due for TID 0");
    assert_eq!(
        originator.expire_next(alarm.deadline),
        Some(WmmUserPriority::UP0)
    );
    assert!(
        !originator.is_closed(WmmUserPriority::UP0),
        "the new attempts remain"
    );
    assert_eq!(
        originator.next_pending_deadline(),
        Some(alarm.deadline.saturating_add(RETRY.interval))
    );
}
