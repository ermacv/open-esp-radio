use oer_bluetooth_radio::RadioDuration;

use super::{LE_1M_EVENT_MICROS, connection_allowances, connection_event_span};

#[test]
fn a_connection_event_may_run_past_its_reservation_within_the_interval() {
    // Up to 7,499 us the event may take the whole interval.
    assert_eq!(
        connection_event_span(
            RadioDuration::from_micros(7_499),
            RadioDuration::from_micros(40)
        ),
        RadioDuration::from_micros(7_499)
    );
    assert_eq!(
        connection_event_span(
            RadioDuration::from_micros(7_500),
            RadioDuration::from_micros(40)
        ),
        RadioDuration::from_micros(7_500 - 2_000 + 40)
    );
    assert_eq!(
        connection_event_span(
            RadioDuration::from_micros(30_000),
            RadioDuration::from_micros(16)
        ),
        RadioDuration::from_micros(30_000 - 2_000 + 16)
    );
    // The span never depends on the reserved air window.
    assert!(
        connection_event_span(
            RadioDuration::from_micros(30_000),
            RadioDuration::from_micros(16)
        ) > RadioDuration::from_micros(u64::from(LE_1M_EVENT_MICROS))
    );
}

#[test]
fn allowances_keep_the_event_length_after_the_preparation_lead() {
    let allowances = connection_allowances(137, 500).unwrap();
    assert_eq!(allowances.local_sleep_clock_ppm, 500);
    assert_eq!(
        allowances.event_length,
        RadioDuration::from_micros(1_074 - 137)
    );
    assert_eq!(
        allowances.first_event_length,
        RadioDuration::from_micros(1_075)
    );
}

#[test]
fn a_preparation_lead_that_leaves_no_event_time_has_no_allowances() {
    assert!(connection_allowances(LE_1M_EVENT_MICROS - 1, 500).is_some());
    assert!(connection_allowances(LE_1M_EVENT_MICROS, 500).is_none());
    assert!(connection_allowances(LE_1M_EVENT_MICROS + 1, 500).is_none());
    assert!(connection_allowances(u32::MAX, 500).is_none());
}
