use oer_bluetooth_radio::{CoexistenceLevel, RadioDuration};

use super::{advertising_level, advertising_period, peripheral_connection_level};

#[test]
fn shorter_advertising_intervals_raise_fewer_events() {
    for (millis, period) in [(20, 3), (25, 3), (26, 2), (50, 2), (51, 1), (1_000, 1)] {
        assert_eq!(
            advertising_period(RadioDuration::from_micros(millis * 1_000)),
            period,
            "{millis} ms"
        );
    }
}

#[test]
fn every_period_th_advertising_event_is_raised_after_the_start() {
    let levels: [CoexistenceLevel; 8] =
        core::array::from_fn(|ended| advertising_level(3, ended as u16));
    use CoexistenceLevel::{Baseline as B, Elevated as E};
    // Events 1 and 2 keep the start level; then counts 1, 2, 3, ... decide.
    assert_eq!(levels, [E, E, B, B, E, B, B, E]);
    assert!((0..8).all(|ended| advertising_level(1, ended) == E));
}

#[test]
fn a_new_connection_is_raised_for_its_first_six_events() {
    for event in 0..6 {
        assert_eq!(
            peripheral_connection_level(event, Some(event), 7_500, false),
            CoexistenceLevel::Elevated
        );
    }
    assert_eq!(
        peripheral_connection_level(6, Some(6), 7_500, false),
        CoexistenceLevel::Baseline
    );
}

#[test]
fn missed_receptions_raise_the_level_by_interval() {
    use CoexistenceLevel::{Baseline as B, Elevated as E};
    // Interval in microseconds, the last missed count that stays baseline.
    for (interval, allowed) in [
        (7_500, 3),
        (12_500, 3),
        (13_750, 2),
        (25_000, 2),
        (50_000, 1),
        (51_250, 0),
    ] {
        let quiet = peripheral_connection_level(100, Some(100 - allowed), interval, false);
        let missed = peripheral_connection_level(100, Some(99 - allowed), interval, false);
        assert_eq!((quiet, missed), (B, E), "{interval} us");
    }
    // Without any reception the count starts at event zero.
    assert_eq!(peripheral_connection_level(9, None, 7_500, false), E);
    // The count wraps with the event counter.
    assert_eq!(
        peripheral_connection_level(6, Some(u16::MAX), 7_500, false),
        E
    );
}

#[test]
fn a_local_procedure_raises_the_level() {
    assert_eq!(
        peripheral_connection_level(100, Some(100), 7_500, true),
        CoexistenceLevel::Elevated
    );
}
