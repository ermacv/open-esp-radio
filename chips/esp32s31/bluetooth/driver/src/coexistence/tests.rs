use oer_bluetooth_le_port::{AdvertisingEventKind, ConnectionEventKind};

use super::{
    CoexistenceProfile, advertising_priorities, connection_priorities, connection_protection,
    passive_scan_priorities,
};

fn values<const N: usize>(
    lanes: [oer_esp32s31_hal::bluetooth::engine::SchedulerItemCoexistencePriority; N],
) -> [u8; N] {
    lanes.map(|lane| lane.value())
}

const ADVERTISING_KINDS: [AdvertisingEventKind; 2] =
    [AdvertisingEventKind::Ordinary, AdvertisingEventKind::Raised];

const CONNECTION_KINDS: [ConnectionEventKind; 5] = [
    ConnectionEventKind::Ordinary,
    ConnectionEventKind::Opening,
    ConnectionEventKind::MissedReceptions,
    ConnectionEventKind::ControlProcedure,
    ConnectionEventKind::CentralFailures,
];

#[test]
fn alone_every_lane_requests_the_highest_priority_and_nothing_is_protected() {
    for kind in ADVERTISING_KINDS {
        let advertising = advertising_priorities(CoexistenceProfile::Standalone, kind);
        assert_eq!(values(advertising.lanes), [15; 4]);
    }
    for kind in CONNECTION_KINDS {
        let connection = connection_priorities(CoexistenceProfile::Standalone, kind);
        assert_eq!(values([connection.event, connection.base]), [15, 15]);
    }
    assert_eq!(connection_protection(CoexistenceProfile::Standalone), None);
}

#[test]
fn shared_the_event_lane_follows_the_reason() {
    let shared = CoexistenceProfile::Shared;
    assert_eq!(
        values(advertising_priorities(shared, AdvertisingEventKind::Ordinary).lanes),
        [4, 0, 13, 13]
    );
    assert_eq!(
        values(advertising_priorities(shared, AdvertisingEventKind::Raised).lanes),
        [9, 0, 13, 13]
    );
    for (kind, event) in [
        (ConnectionEventKind::Ordinary, 4),
        (ConnectionEventKind::Opening, 9),
        (ConnectionEventKind::MissedReceptions, 9),
        (ConnectionEventKind::ControlProcedure, 9),
        (ConnectionEventKind::CentralFailures, 11),
    ] {
        let connection = connection_priorities(shared, kind);
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
