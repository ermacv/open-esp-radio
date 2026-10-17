use super::*;

fn at(micros: u64) -> Instant {
    Instant::from_micros(micros)
}

#[test]
fn a_matching_response_ends_the_procedure() {
    let mut query = StationSaQuery::new();
    assert_eq!(query.start(at(1_000), 65_525 + 7), Ok(Some([7, 0])));
    assert_eq!(query.start(at(1_100), 3), Ok(None));
    assert!(!query.response([6, 0]));
    assert!(query.is_active());
    assert!(query.response([7, 0]));
    assert!(!query.is_active());
    assert_eq!(query.step(at(2_000_000)), SaQueryStep::Idle);
}

#[test]
fn retries_every_200_ms_and_accepts_any_request_of_the_procedure() {
    let mut query = StationSaQuery::new();
    query.start(at(0), 100).unwrap();
    assert_eq!(query.next_deadline(), Some(at(200_000)));
    assert_eq!(query.step(at(199_999)), SaQueryStep::Idle);
    assert_eq!(query.step(at(200_000)), SaQueryStep::Request([101, 0]));
    assert_eq!(query.step(at(400_000)), SaQueryStep::Request([102, 0]));
    assert!(!query.response([99, 0]));
    assert!(!query.response([103, 0]));
    assert!(query.response([100, 0]));
}

#[test]
fn no_response_within_1024_ms_times_out() {
    let mut query = StationSaQuery::new();
    query.start(at(0), 0).unwrap();
    for expected in 1..=5_u8 {
        assert_eq!(
            query.step(at(u64::from(expected) * 200_000)),
            SaQueryStep::Request([expected, 0])
        );
    }
    assert_eq!(query.next_deadline(), Some(at(1_024_000)));
    assert_eq!(query.step(at(1_024_000)), SaQueryStep::TimedOut);
    assert!(!query.is_active());
    assert_eq!(query.start(at(1_100_000), 9), Ok(Some([9, 0])));
}

#[test]
fn overflow_refuses_without_starting_and_the_last_representable_window_finishes() {
    let mut query = StationSaQuery::new();
    let boundary = u64::MAX - TIMEOUT.as_micros();
    assert_eq!(
        query.start(at(boundary + 1), 9),
        Err(SaQueryDeadlineOverflow)
    );
    assert!(!query.is_active());
    assert!(query.next_deadline().is_none());
    assert_eq!(query.start(at(boundary), 9), Ok(Some([9, 0])));
    for ordinal in 1..=5_u64 {
        assert_eq!(
            query.step(at(boundary + ordinal * 200_000)),
            SaQueryStep::Request([9 + ordinal as u8, 0])
        );
    }
    assert_eq!(query.next_deadline(), Some(at(u64::MAX)));
    assert_eq!(query.step(at(u64::MAX - 1)), SaQueryStep::Idle);
    assert_eq!(query.step(at(u64::MAX)), SaQueryStep::TimedOut);
    assert!(!query.is_active());
}
