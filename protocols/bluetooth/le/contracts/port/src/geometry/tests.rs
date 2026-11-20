#![allow(clippy::disallowed_macros)]

use super::{Counts, Misfit, misfit};
use crate::{
    LeConnectionCapabilities, LeLimits, LePhys, LeRadioCapabilities, LegacyAdvertisingTypes,
    LinkAcknowledgement, RadioDuration,
};

/// Every role, and room for the largest geometry an index names.
const ALL: LeRadioCapabilities = LeRadioCapabilities {
    legacy_advertising: LegacyAdvertisingTypes {
        nonconnectable: true,
        responsive: true,
    },
    passive_scanning: true,
    peripheral_connection: Some(LeConnectionCapabilities {
        max_data_payload: 27,
        link_acknowledgement: LinkAcknowledgement::Hardware,
        max_first_receive_wait: RadioDuration::from_micros(0),
        max_receive_wait: RadioDuration::from_micros(0),
    }),
    direct_test_mode: true,
    phys: LePhys::LE_1M,
    limits: LeLimits {
        advertising_sets: 4,
        connections: 3,
        connection_events: 4,
        transmit_places: 5,
        receive_nodes: 6,
        accept_list: 0,
    },
    ..LeRadioCapabilities::NONE
};

/// One of every role at the capabilities' limits.
const FULL: Counts = Counts {
    advertising: 4,
    scanners: 1,
    connections: 3,
    events: 4,
    transmit: 5,
    receive: 6,
    tests: 1,
};

#[test]
fn a_geometry_inside_the_capabilities_fits() {
    assert_eq!(misfit(&ALL, FULL), None);
    let none = Counts {
        advertising: 0,
        scanners: 0,
        connections: 0,
        events: 2,
        transmit: 0,
        receive: 0,
        tests: 0,
    };
    assert_eq!(misfit(&LeRadioCapabilities::NONE, none), None);
}

#[test]
fn each_count_one_past_its_limit_is_named() {
    for (counts, expected) in [
        (Counts { events: 1, ..FULL }, Misfit::EventsBelowTwo),
        (
            Counts {
                scanners: 2,
                ..FULL
            },
            Misfit::Scanners,
        ),
        (Counts { tests: 2, ..FULL }, Misfit::Tests),
        (
            Counts {
                advertising: 5,
                ..FULL
            },
            Misfit::AdvertisingSets,
        ),
        (
            Counts {
                connections: 4,
                ..FULL
            },
            Misfit::Connections,
        ),
        (Counts { events: 5, ..FULL }, Misfit::ConnectionEvents),
        (
            Counts {
                transmit: 6,
                ..FULL
            },
            Misfit::TransmitPlaces,
        ),
        (Counts { receive: 7, ..FULL }, Misfit::ReceiveNodes),
    ] {
        assert_eq!(misfit(&ALL, counts), Some(expected), "{counts:?}");
    }
}

#[test]
fn a_connection_event_or_transmit_count_needs_no_room_without_connections() {
    let counts = Counts {
        connections: 0,
        events: 9,
        transmit: 9,
        ..FULL
    };
    assert_eq!(misfit(&ALL, counts), None);
}

#[test]
fn an_index_is_one_octet_and_a_node_two() {
    let wide = LeRadioCapabilities {
        limits: LeLimits {
            advertising_sets: usize::MAX,
            connections: usize::MAX,
            connection_events: usize::MAX,
            transmit_places: usize::MAX,
            receive_nodes: usize::MAX,
            accept_list: usize::MAX,
        },
        ..ALL
    };
    let at = |count| Counts {
        advertising: count,
        connections: count,
        events: count,
        transmit: count,
        ..FULL
    };
    assert_eq!(misfit(&wide, at(256)), None);
    for one in [
        Counts {
            advertising: 257,
            ..at(2)
        },
        Counts {
            connections: 257,
            ..at(2)
        },
        Counts {
            events: 257,
            ..at(2)
        },
        Counts {
            transmit: 257,
            ..at(2)
        },
    ] {
        assert_eq!(
            misfit(&wide, one),
            Some(Misfit::IndexBeyondAnOctet),
            "{one:?}"
        );
    }
    assert_eq!(
        misfit(
            &wide,
            Counts {
                receive: 65_535,
                ..FULL
            }
        ),
        None
    );
    assert_eq!(
        misfit(
            &wide,
            Counts {
                receive: 65_536,
                ..FULL
            }
        ),
        Some(Misfit::NodeBeyondTwoOctets)
    );
}

#[test]
fn a_role_the_capabilities_do_not_serve_has_no_instance() {
    let without = |caps: LeRadioCapabilities, counts| misfit(&caps, counts);
    let none = Counts {
        advertising: 0,
        scanners: 0,
        connections: 0,
        tests: 0,
        ..FULL
    };
    let no_advertising = LeRadioCapabilities {
        legacy_advertising: LegacyAdvertisingTypes::NONE,
        ..ALL
    };
    assert_eq!(
        without(
            no_advertising,
            Counts {
                advertising: 1,
                ..none
            }
        ),
        Some(Misfit::NoAdvertising)
    );
    assert_eq!(without(no_advertising, none), None);
    let no_scanning = LeRadioCapabilities {
        passive_scanning: false,
        active_scanning: false,
        ..ALL
    };
    assert_eq!(
        without(
            no_scanning,
            Counts {
                scanners: 1,
                ..none
            }
        ),
        Some(Misfit::NoScanning)
    );
    assert_eq!(without(no_scanning, none), None);
    let no_connections = LeRadioCapabilities {
        peripheral_connection: None,
        ..ALL
    };
    assert_eq!(
        without(
            no_connections,
            Counts {
                connections: 1,
                ..none
            }
        ),
        Some(Misfit::NoConnections)
    );
    assert_eq!(without(no_connections, none), None);
    let no_test = LeRadioCapabilities {
        direct_test_mode: false,
        ..ALL
    };
    assert_eq!(
        without(no_test, Counts { tests: 1, ..none }),
        Some(Misfit::NoTest)
    );
    assert_eq!(without(no_test, none), None);
}
