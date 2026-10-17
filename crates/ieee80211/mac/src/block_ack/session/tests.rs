use super::*;
use crate::sequence::seq;

const CONFIG: TxBlockAckConfig = TxBlockAckConfig {
    tid: 7,
    window: 32,
    timeout_tu: 0,
    negotiation_timeout: Duration::from_micros(100_000),
    amsdu: true,
};

fn at(micros: u64) -> Instant {
    Instant::from_micros(micros)
}

#[test]
fn request_encoding_is_exact_and_bounded() {
    let mut session = TxBlockAckSession::new(CONFIG).unwrap();
    let request = session.begin(seq(0x0abc), at(50)).unwrap();
    assert_eq!(request.starting_sequence, seq(0x0abc));
    assert_eq!(request.alarm.deadline, at(100_050));
    assert_eq!(request.body, [3, 0, 1, 0x1f, 0x08, 0, 0, 0xc0, 0xab]);
}

#[test]
fn a_deadline_past_the_time_range_refuses_the_negotiation() {
    let mut session = TxBlockAckSession::new(CONFIG).unwrap();
    assert_eq!(
        session.begin(seq(0), at(u64::MAX - 1)),
        Err(TxBlockAckError::DeadlineOverflow)
    );
    assert!(!session.is_awaiting());
}

#[test]
fn a_zero_negotiation_timeout_is_refused() {
    let config = TxBlockAckConfig {
        negotiation_timeout: Duration::ZERO,
        ..CONFIG
    };
    assert_eq!(config.validate(), Err(TxBlockAckError::ZeroTimeout));
}

#[test]
fn matching_response_commits_only_the_static_window() {
    let mut session = TxBlockAckSession::new(CONFIG).unwrap();
    let request = session.begin(seq(0x123), at(0)).unwrap();
    let response = [3, 1, request.dialog_token, 0, 0, 0x1f, 0x08, 0, 0];
    let agreement = OperationalTxBlockAck {
        tid: 7,
        window: 32,
        timeout_tu: 0,
        starting_sequence: seq(0x123),
        amsdu: true,
    };
    assert_eq!(
        session.on_response(&response),
        Ok(TxBlockAckResponse::Operational(agreement))
    );
    assert_eq!(session.operational(), Some(agreement));
    assert!(!session.on_alarm(request.alarm));
}

#[test]
fn matching_response_accepts_an_addba_extension_ie() {
    let mut session = TxBlockAckSession::new(CONFIG).unwrap();
    let request = session.begin(seq(0x123), at(0)).unwrap();
    let response = [
        3,
        1,
        request.dialog_token,
        0,
        0,
        0x1f,
        0x08,
        0,
        0,
        159,
        1,
        0,
    ];
    assert!(matches!(
        session.on_response(&response),
        Ok(TxBlockAckResponse::Operational(_))
    ));
}

#[test]
fn stale_alarm_cannot_cancel_a_new_generation() {
    let mut session = TxBlockAckSession::new(CONFIG).unwrap();
    let stale = session.begin(seq(1), at(0)).unwrap().alarm;
    let current = session.begin(seq(2), at(10)).unwrap().alarm;
    assert!(!session.on_alarm(stale));
    assert!(session.on_alarm(current));
    assert_eq!(session.operational(), None);
}

#[test]
fn response_cannot_expand_the_static_capacity() {
    let mut session = TxBlockAckSession::new(CONFIG).unwrap();
    let request = session.begin(seq(0), at(0)).unwrap();
    let parameters = encode_ba_parameters(7, 64, false).to_le_bytes();
    let response = [
        3,
        1,
        request.dialog_token,
        0,
        0,
        parameters[0],
        parameters[1],
        0,
        0,
    ];
    assert_eq!(
        session.on_response(&response),
        Err(TxBlockAckError::WindowExceedsCapacity(64))
    );
}

#[test]
fn rejected_response_returns_to_idle_without_a_timer_retry() {
    let mut session = TxBlockAckSession::new(CONFIG).unwrap();
    let request = session.begin(seq(0), at(0)).unwrap();
    let response = [3, 1, request.dialog_token, 37, 0, 0, 0, 0, 0];
    assert_eq!(
        session.on_response(&response),
        Ok(TxBlockAckResponse::Rejected(37))
    );
    assert_eq!(session.operational(), None);
    assert!(!session.on_alarm(request.alarm));
}
