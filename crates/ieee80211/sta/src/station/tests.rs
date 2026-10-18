use super::*;

#[test]
fn retry_policy_is_bounded_and_saturates_exponential_delay() {
    let policy = StaReconnectPolicy::new(4, 10, 25, 5).unwrap();
    assert_eq!(policy.retry_backoff_millis(1), 10);
    assert_eq!(policy.retry_backoff_millis(2), 20);
    assert_eq!(policy.retry_backoff_millis(3), 25);
    assert_eq!(policy.retry_backoff_millis(u16::MAX), 25);
    assert_eq!(
        StaReconnectPolicy::new(0, 1, 1, 1),
        Err(StaReconnectPolicyError::ZeroAttemptLimit)
    );
}
