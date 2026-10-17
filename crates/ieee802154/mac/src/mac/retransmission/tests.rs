//! Expectations follow OpenThread `SubMac::HandleTransmitDone` and the
//! `mac.h` defaults ESP-IDF builds it with.

use super::{AttemptFailure, FrameRetries, RetryStart};

/// A frame is retried `max_frame_retries` times, then the failure ends it.
#[test]
fn a_frame_is_retried_up_to_its_limit() {
    let mut retries = FrameRetries::new(2);
    assert_eq!(
        retries.retry(AttemptFailure::ChannelAccess),
        Some(RetryStart::Now)
    );
    assert!(retries.retry(AttemptFailure::ChannelAccess).is_some());
    assert_eq!(retries.retries(), 2);
    assert_eq!(retries.retry(AttemptFailure::NoAcknowledgement), None);
    assert_eq!(
        FrameRetries::new(0).retry(AttemptFailure::NoAcknowledgement),
        None
    );
}

/// Each retry after a missing acknowledgement waits `random % 2^BE` unit
/// periods, BE growing from 0 to 5; channel-access retries leave it.
#[test]
fn no_ack_retries_wait_a_growing_random_delay() {
    let mut retries = FrameRetries::new(10);
    let mut exponents = std::vec::Vec::new();
    for failure in [
        AttemptFailure::NoAcknowledgement,
        AttemptFailure::ChannelAccess,
        AttemptFailure::NoAcknowledgement,
        AttemptFailure::NoAcknowledgement,
        AttemptFailure::NoAcknowledgement,
        AttemptFailure::NoAcknowledgement,
        AttemptFailure::NoAcknowledgement,
        AttemptFailure::NoAcknowledgement,
    ] {
        if let Some(RetryStart::AfterDelay { exponent }) = retries.retry(failure) {
            exponents.push(exponent);
        }
    }
    assert_eq!(exponents, [0, 1, 2, 3, 4, 5, 5]);

    assert_eq!(
        RetryStart::AfterDelay { exponent: 0 }.delay_micros(u32::MAX),
        0
    );
    assert_eq!(
        RetryStart::AfterDelay { exponent: 3 }.delay_micros(13),
        5 * 320
    );
    assert_eq!(RetryStart::Now.delay_micros(13), 0);
}
