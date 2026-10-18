use oer_bluetooth_radio::CoexistenceLevel;

use super::{
    CoexistenceProfile, advertising_priorities, connection_priorities, connection_protection,
    passive_scan_priorities,
};

fn values<const N: usize>(
    lanes: [oer_esp32s31_bluetooth_memory::SchedulerItemCoexistencePriority; N],
) -> [u8; N] {
    lanes.map(|lane| lane.value())
}

#[test]
fn alone_every_lane_requests_the_highest_priority_and_nothing_is_protected() {
    for level in [
        CoexistenceLevel::Baseline,
        CoexistenceLevel::Elevated,
        CoexistenceLevel::Critical,
    ] {
        let advertising = advertising_priorities(CoexistenceProfile::Standalone, level);
        assert_eq!(values(advertising.lanes), [15; 4]);
        let connection = connection_priorities(CoexistenceProfile::Standalone, level);
        assert_eq!(values([connection.event, connection.base]), [15, 15]);
    }
    assert_eq!(connection_protection(CoexistenceProfile::Standalone), None);
}

#[test]
fn shared_the_event_lane_follows_the_level() {
    let shared = CoexistenceProfile::Shared;
    assert_eq!(
        values(advertising_priorities(shared, CoexistenceLevel::Baseline).lanes),
        [4, 0, 13, 13]
    );
    assert_eq!(
        values(advertising_priorities(shared, CoexistenceLevel::Elevated).lanes),
        [9, 0, 13, 13]
    );
    for (level, event) in [
        (CoexistenceLevel::Baseline, 4),
        (CoexistenceLevel::Elevated, 9),
        (CoexistenceLevel::Critical, 11),
    ] {
        let connection = connection_priorities(shared, level);
        assert_eq!(values([connection.event, connection.base]), [event, 4]);
    }
}

#[test]
fn a_shared_connection_is_protected_for_eight_slots_in_256_us_units() {
    assert_eq!(
        connection_protection(CoexistenceProfile::Shared).map(|protection| protection.value()),
        Some(20)
    );
}

#[test]
fn a_passive_scan_window_is_equal_alone_and_takes_the_vendor_lanes_when_shared() {
    assert_eq!(
        values(passive_scan_priorities(CoexistenceProfile::Standalone).lanes),
        [15; 4]
    );
    assert_eq!(
        values(passive_scan_priorities(CoexistenceProfile::Shared).lanes),
        [4, 11, 0, 0]
    );
}
