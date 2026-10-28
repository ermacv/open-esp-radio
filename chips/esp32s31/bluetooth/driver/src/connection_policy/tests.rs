use oer_bluetooth_le_port::RadioDuration;

use super::{LE_1M_EVENT_MICROS, connection_allowances};

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
