use super::*;
use crate::seq;

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

fn active(start: u16, window: u16) -> TxBlockAckSession {
    let mut session = TxBlockAckSession::new(TxBlockAckConfig { window, ..CONFIG }).unwrap();
    let request = session.begin(seq(start), at(0)).unwrap();
    session
        .on_response_action(BlockAckAction::AddbaResponse {
            dialog_token: request.dialog_token,
            status: 0,
            tid: CONFIG.tid,
            immediate: true,
            amsdu: false,
            window,
            timeout_tu: 0,
        })
        .unwrap();
    session
}

#[test]
fn a_full_64_member_expiry_report_advances_once_without_a_shift_overflow() {
    let mut session = active(4090, 64);
    assert_eq!(session.on_expired(seq(4090), u64::MAX), Some(seq(58)));
    assert_eq!(session.operational().unwrap().starting_sequence, seq(58));
    assert_eq!(session.on_delivered(seq(58), u64::MAX), None);
    assert_eq!(session.operational().unwrap().starting_sequence, seq(122));
    assert_eq!(session.on_expired(seq(122), 1), Some(seq(123)));
}

#[test]
fn stale_and_out_of_window_reports_do_not_mark_a_new_head() {
    let mut session = active(100, 32);
    for sn in [99, 132, 2048, 4095] {
        assert_eq!(session.on_delivered(seq(sn), 1), None);
        assert_eq!(session.on_expired(seq(sn), 1), None);
    }
    // The stale bit is ignored, but the next bit still names the head.
    assert_eq!(session.on_delivered(seq(99), 3), None);
    assert_eq!(session.operational().unwrap().starting_sequence, seq(101));
    assert_eq!(session.on_expired(seq(101), 1), Some(seq(102)));
}

#[test]
fn terminal_marks_are_idempotent_and_reset_with_the_agreement() {
    let mut session = active(100, 32);
    assert_eq!(session.on_expired(seq(101), 1), None);
    assert_eq!(session.on_delivered(seq(101), 1), None);
    assert_eq!(session.on_delivered(seq(100), 1), Some(seq(102)));
    session.stop();
    let request = session.begin(seq(100), at(0)).unwrap();
    session
        .on_response_action(BlockAckAction::AddbaResponse {
            dialog_token: request.dialog_token,
            status: 0,
            tid: CONFIG.tid,
            immediate: true,
            amsdu: false,
            window: 32,
            timeout_tu: 0,
        })
        .unwrap();
    assert_eq!(session.on_delivered(seq(100), 1), None);
    assert_eq!(session.operational().unwrap().starting_sequence, seq(101));
}

#[test]
fn a_window_larger_than_the_originator_marks_is_refused() {
    assert_eq!(
        TxBlockAckConfig {
            window: 65,
            ..CONFIG
        }
        .validate(),
        Err(TxBlockAckError::InvalidWindow(65))
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

fn responses(count: usize) {
    let parameters = encode_ba_parameters(7, 32, true).to_le_bytes();
    let accepted = [3, 1, 1, 0, 0, parameters[0], parameters[1], 0, 0];
    let mut extended = std::vec::Vec::from(accepted);
    // An HE peer's ADDBA Extension element after the fixed fields.
    extended.extend_from_slice(&[159, 1, 0]);
    let rejected = [3, 1, 1, 37, 0, 0, 0, 0, 0];
    oer_ieee80211_wire::fuzz::inputs(&[&accepted, &extended, &rejected], count, 32, |bytes| {
        let mut session = TxBlockAckSession::new(CONFIG).unwrap();
        let request = session.begin(seq(0x100), at(0)).unwrap();
        assert_eq!(request.dialog_token, 1);
        if let Ok(TxBlockAckResponse::Operational(agreement)) = session.on_response(bytes) {
            assert!(agreement.window != 0 && agreement.window <= CONFIG.window);
            assert_eq!(session.operational(), Some(agreement));
        }
    });
}

#[test]
fn response_handling_never_panics() {
    responses(oer_ieee80211_wire::fuzz::QUICK);
}

#[test]
#[ignore = "the full run"]
fn response_handling_never_panics_full() {
    responses(oer_ieee80211_wire::fuzz::FULL);
}

// Fixture encoding for independently constructed peer responses.
const fn encode_ba_parameters(tid: u8, window: u16, amsdu: bool) -> u16 {
    (amsdu as u16) | 1 << 1 | (tid as u16) << 2 | window << 6
}
