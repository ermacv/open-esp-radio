use oer_time::{Duration, Instant};

use super::*;
use crate::block_ack::{BlockAckAction, OperationalTxBlockAck, TxBlockAckResponse};
use crate::sequence::SequenceNumber;

/// Three TIDs and a modulo-63 token sequence, as the Espressif station's.
const POLICY: TxBlockAckOriginatorPolicy = TxBlockAckOriginatorPolicy {
    tids: &[0, 7, 5],
    first_dialog_token: 1,
    next_dialog_token,
};

const fn next_dialog_token(current: u8) -> u8 {
    if current >= 62 { 0 } else { current + 1 }
}

fn originator(window: u16, amsdu_tids: u8) -> TxBlockAckOriginator<TX_BLOCK_ACK_MAX_TIDS> {
    TxBlockAckOriginator::new(
        POLICY,
        TxBlockAckOriginatorConfig {
            window,
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
        dialog_token,
        status,
        tid,
        immediate: true,
        amsdu: true,
        window,
        timeout_tu: 7,
    }
}

#[test]
fn the_policy_s_tids_share_one_token_sequence_and_route_responses() {
    let mut originator = originator(32, 1);
    let at = Instant::from_micros(0);
    let tid0 = originator.begin(0, sequence(0x100), at).unwrap();
    let tid7 = originator.begin(7, sequence(0x200), at).unwrap();
    let tid5 = originator.begin(5, sequence(0x300), at).unwrap();
    assert_eq!(
        [tid0.dialog_token, tid7.dialog_token, tid5.dialog_token],
        [1, 2, 3]
    );
    assert_eq!(originator.alarm(7), Some(tid7.alarm));
    assert_eq!(
        originator.on_response_action(response(tid7.dialog_token, 7, 0, 16)),
        Ok(TxBlockAckResponseDisposition::Matched(
            TxBlockAckOriginatorResponse {
                tid: 7,
                response: TxBlockAckResponse::Operational(OperationalTxBlockAck {
                    tid: 7,
                    window: 16,
                    timeout_tu: 7,
                    starting_sequence: sequence(0x200),
                    amsdu: false,
                }),
            }
        ))
    );
    assert_eq!(originator.alarm(7), None);
    // TID 0 may carry A-MSDUs, as the configuration allows.
    assert!(matches!(
        originator.on_response_action(response(tid0.dialog_token, 0, 0, 16)),
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
        Some(5)
    );
    assert_eq!(originator.expire_next(Instant::from_micros(100_000)), None);
    assert!(originator.operational(7).is_some());
}

#[test]
fn the_earliest_deadline_follows_the_live_negotiations() {
    let mut originator = originator(16, 0);
    assert_eq!(originator.earliest_alarm_deadline(), None);
    originator
        .begin(0, sequence(0), Instant::from_micros(75))
        .unwrap();
    originator
        .begin(7, sequence(0), Instant::from_micros(25))
        .unwrap();
    originator
        .begin(5, sequence(0), Instant::from_micros(50))
        .unwrap();
    assert_eq!(
        originator.earliest_alarm_deadline(),
        Some(Instant::from_micros(100_025))
    );
    assert!(originator.stop(7));
    assert_eq!(
        originator.earliest_alarm_deadline(),
        Some(Instant::from_micros(100_050))
    );
}

#[test]
fn an_unowned_tid_and_a_stale_token_change_nothing() {
    let mut originator = originator(32, 0);
    assert_eq!(
        originator.begin(3, sequence(0), Instant::from_micros(0)),
        Err(TxBlockAckOriginatorError::UnsupportedTid(3))
    );
    assert_eq!(
        originator.on_response(&[3, 1, 42, 0, 0, 0, 0, 0, 0]),
        Ok(TxBlockAckResponseDisposition::StaleDialogToken(42))
    );
    assert!(!originator.stop(3));
}

#[test]
fn attempts_requeue_until_the_peer_answers_or_they_run_out() {
    let mut originator = originator(32, 0);
    let at = Instant::from_micros(0);
    assert!(!originator.has_pending());
    originator.queue_initial(TxBlockAckRetry {
        attempts: 2,
        interval: Duration::ZERO,
    });
    assert_eq!(originator.take_pending(at), Some(0));
    assert_eq!(originator.take_pending(at), Some(7));
    assert_eq!(originator.take_pending(at), Some(5));
    assert_eq!(originator.take_pending(at), None);

    // TID 0's request did not leave: its second attempt is queued.
    originator.transmit_failed(0, at);
    assert_eq!(originator.take_pending(at), Some(0));
    let tid0 = originator
        .begin(0, sequence(0), Instant::from_micros(0))
        .unwrap();
    // Its second negotiation times out with no attempt left.
    assert_eq!(
        originator.expire_next(Instant::from_micros(100_000)),
        Some(0)
    );
    assert!(!originator.has_pending());
    assert_ne!(tid0.dialog_token, 0);

    // TID 7 is refused: the peer's answer ends its attempts.
    let tid7 = originator
        .begin(7, sequence(0), Instant::from_micros(0))
        .unwrap();
    assert_eq!(
        originator.on_response_action(response(tid7.dialog_token, 7, 37, 0)),
        Ok(TxBlockAckResponseDisposition::Matched(
            TxBlockAckOriginatorResponse {
                tid: 7,
                response: TxBlockAckResponse::Rejected(37),
            }
        ))
    );
    assert!(!originator.has_pending());

    // Leaving ends everything and counts what was live.
    originator
        .begin(5, sequence(0), Instant::from_micros(0))
        .unwrap();
    assert_eq!(originator.stop_all(), 1);
    assert_eq!(originator.earliest_alarm_deadline(), None);
}

#[test]
fn a_policy_names_each_tid_once() {
    const TWICE: TxBlockAckOriginatorPolicy = TxBlockAckOriginatorPolicy {
        tids: &[0, 0],
        ..POLICY
    };
    assert!(matches!(
        TxBlockAckOriginator::<TX_BLOCK_ACK_MAX_TIDS>::new(
            TWICE,
            TxBlockAckOriginatorConfig {
                window: 8,
                negotiation_timeout: Duration::from_micros(1),
                amsdu_tids: 0,
            },
        ),
        Err(TxBlockAckOriginatorError::UnsupportedTid(0))
    ));
}

#[test]
fn a_failed_attempt_waits_the_interval_before_the_next_is_due() {
    let mut originator = originator(32, 0);
    originator.queue_initial(TxBlockAckRetry {
        attempts: 3,
        interval: Duration::from_micros(1_000_000),
    });
    let at = Instant::from_micros(500);
    assert_eq!(originator.next_pending_deadline(), Some(Instant::EPOCH));
    for tid in [0, 7, 5] {
        assert_eq!(originator.take_pending(at), Some(tid));
    }
    originator.begin(0, sequence(0), at).unwrap();
    // The response does not come: the next attempt is due one interval
    // after the timeout, not before.
    let timeout = Instant::from_micros(100_500);
    assert_eq!(originator.expire_next(timeout), Some(0));
    let due = Instant::from_micros(1_100_500);
    assert_eq!(originator.next_pending_deadline(), Some(due));
    assert_eq!(
        originator.take_pending(Instant::from_micros(1_100_499)),
        None
    );
    assert_eq!(originator.take_pending(due), Some(0));
    // The third and last attempt's request does not leave.
    originator.begin(0, sequence(0), due).unwrap();
    originator.transmit_failed(0, due);
    assert_eq!(
        originator.take_pending(Instant::from_micros(2_100_500)),
        Some(0)
    );
    originator.transmit_failed(0, Instant::from_micros(2_100_500));
    assert!(!originator.has_pending());
    assert_eq!(originator.generation(3), 0);
}

#[test]
fn no_attempt_queues_nothing() {
    let mut originator = originator(32, 0);
    originator.queue_initial(TxBlockAckRetry {
        attempts: 0,
        interval: Duration::ZERO,
    });
    assert!(!originator.has_pending());
    assert_eq!(next_nonzero_dialog_token(255), 1);
}

#[test]
fn an_originator_owns_at_most_its_tids() {
    let config = TxBlockAckOriginatorConfig {
        window: 8,
        negotiation_timeout: Duration::from_micros(1),
        amsdu_tids: 0,
    };
    assert!(matches!(
        TxBlockAckOriginator::<2>::new(POLICY, config),
        Err(TxBlockAckOriginatorError::TooManyTids)
    ));
    let one = TxBlockAckOriginatorPolicy {
        tids: &[0],
        ..POLICY
    };
    let originator = TxBlockAckOriginator::<1>::new(one, config).unwrap();
    let mut tids = originator.tids();
    assert_eq!((tids.next(), tids.next()), (Some(0), None));
}
