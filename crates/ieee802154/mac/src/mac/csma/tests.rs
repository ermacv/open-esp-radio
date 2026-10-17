//! Expectations follow OpenThread `SubMac::StartCsmaBackoff`,
//! `StartTimerForBackoff` and `HandleTransmitDone`.

use super::CsmaCa;

/// One unit backoff period: twenty 16 µs symbols.
const UNIT: u32 = 320;

/// The exponent starts at 3 and grows by one per busy channel up to 5; the
/// backoff is `random % 2^BE` unit periods.
#[test]
fn the_backoff_exponent_grows_to_the_maximum() {
    let mut csma = CsmaCa::new(4);
    assert_eq!(csma.backoff_micros(0), Some(0));
    assert_eq!(csma.backoff_micros(7), Some(7 * UNIT));
    assert_eq!(csma.backoff_micros(8), Some(0));
    assert_eq!(csma.backoff_micros(u32::MAX), Some(7 * UNIT));
    for exponent in [4, 5, 5] {
        assert!(csma.channel_busy());
        let largest = (1 << exponent) - 1;
        assert_eq!(csma.backoff_micros(u32::MAX), Some(largest * UNIT));
        assert_eq!(csma.backoff_micros(1 << exponent), Some(0));
    }
}

/// `max_backoffs` busy channels are retried; the next one ends the
/// transmission, so a request makes `max_backoffs + 1` CCA attempts.
#[test]
fn busy_channels_are_retried_up_to_the_maximum() {
    let mut csma = CsmaCa::new(2);
    assert!(csma.channel_busy());
    assert!(csma.channel_busy());
    assert_eq!(csma.backoffs(), 2);
    assert!(!csma.channel_busy());
    assert_eq!(csma.backoffs(), 2);
}

/// Without backoffs the frame goes out at once with one CCA attempt.
#[test]
fn no_backoffs_transmit_at_once() {
    let mut csma = CsmaCa::new(0);
    assert_eq!(csma.backoff_micros(5), None);
    assert!(!csma.channel_busy());
}
