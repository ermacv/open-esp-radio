use oer_hil_link::{Received, test_support::received};
use oer_hil_protocol::{Envelope, wifi::StationLifecycleEvent};

use super::{beacon_loss_count_in, station_unchanged_since_in};

/// Boot `boot_id`'s `Hello` as the host receives it.
fn hello(boot_id: u64, message_sequence: u32) -> Received {
    received(oer_hil_link::test_support::hello(boot_id, message_sequence))
}

/// A device event as the host receives it.
fn event<M: oer_hil_protocol::Message>(
    boot_id: u64,
    message_sequence: u32,
    session_id: u64,
    request_id: u32,
    body: M,
) -> Received {
    received(Envelope::new(
        boot_id,
        message_sequence,
        session_id,
        request_id,
        body,
    ))
}

#[test]
fn beacon_loss_qualification_ignores_previous_boots() {
    let mut messages = vec![
        hello(7, 0),
        event(
            7,
            1,
            0,
            0,
            oer_hil_protocol::wifi::StationLifecycle(StationLifecycleEvent::Disconnected {
                generation: 0,
                reason: oer_hil_protocol::wifi::StationDisconnectReason::BeaconLoss,
            }),
        ),
        hello(8, 0),
        event(
            8,
            1,
            0,
            0,
            oer_hil_protocol::wifi::StationLifecycle(StationLifecycleEvent::Connected {
                generation: 0,
                association_bandwidth_mhz: None,
                security: None,
            }),
        ),
    ];
    assert_eq!(beacon_loss_count_in(&messages), 0);

    messages.push(event(
        8,
        2,
        0,
        0,
        oer_hil_protocol::wifi::StationLifecycle(StationLifecycleEvent::Disconnected {
            generation: 0,
            reason: oer_hil_protocol::wifi::StationDisconnectReason::BeaconLoss,
        }),
    ));
    assert_eq!(beacon_loss_count_in(&messages), 1);
}

#[test]
fn pause_requires_same_connection_even_after_fast_reconnect_or_reboot() {
    let connected = |sequence, generation| {
        event(
            7,
            sequence,
            0,
            0,
            oer_hil_protocol::wifi::StationLifecycle(StationLifecycleEvent::Connected {
                generation,
                association_bandwidth_mhz: None,
                security: None,
            }),
        )
    };
    let mut events = vec![hello(7, 0), connected(1, 3)];
    let cursor = events.len();
    events.push(event(7, 2, 1, 0, oer_hil_protocol::base::Accepted));
    assert!(station_unchanged_since_in(&events, cursor).is_ok());
    events.push(connected(3, 4));
    assert!(station_unchanged_since_in(&events, cursor).is_err());
    events.pop();
    events.push(hello(8, 0));
    assert!(station_unchanged_since_in(&events, cursor).is_err());
    events.pop();
    events.push(event(
        7,
        3,
        0,
        0,
        oer_hil_protocol::wifi::StationLifecycle(StationLifecycleEvent::Disconnected {
            generation: 3,
            reason: oer_hil_protocol::wifi::StationDisconnectReason::BeaconLoss,
        }),
    ));
    assert!(station_unchanged_since_in(&events, cursor).is_err());
    assert!(station_unchanged_since_in(&events, events.len() + 1).is_err());
}

#[test]
fn pause_accepts_image_key_reply_only_in_the_established_boot() {
    let mut reply = hello(7, 2);
    reply.request_id = 11;
    let mut events = vec![hello(7, 0), reply];
    assert!(station_unchanged_since_in(&events, 1).is_ok());
    events[1].boot_id = 8;
    assert!(station_unchanged_since_in(&events, 1).is_err());
    events[1] = hello(7, 2);
    assert!(station_unchanged_since_in(&events, 1).is_err());
    events[1] = event(8, 2, 1, 11, oer_hil_protocol::base::Accepted);
    assert!(station_unchanged_since_in(&events, 1).is_err());
    assert!(station_unchanged_since_in(&events, 0).is_err());
}
