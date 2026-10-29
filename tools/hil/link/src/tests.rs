use super::Received;
use super::capture::test_support::received;
use oer_hil_protocol::{DecodeCounters, Envelope};

/// Boot `boot_id`'s `Hello` as the host receives it.
fn hello(boot_id: u64, message_sequence: u32) -> Received {
    received(super::capture::test_support::hello(
        boot_id,
        message_sequence,
    ))
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

use crate::{ProtocolHealth, command_response_matches};

#[test]
fn wifi_credentials_are_redacted_in_host_debug_and_persisted_protocol_records() {
    use oer_hil_protocol::Message;
    use oer_ieee80211_control::*;
    let command = StartStation(StationStart {
        credentials: NetworkCredentials::try_new(b"private-test-network", b"private-test-password")
            .unwrap(),
        power_save: StationPowerSave::Off,
    });
    // Even an accidental decoded echo must not disclose the command body.
    let echo = event(7, 11, 13, 17, command);
    let debug = format!("{echo:?}");
    assert!(debug.contains("payload_length:"));
    assert!(!debug.contains("payload: ["));
    assert_eq!(echo.to_json()["body"]["credentials"], "<redacted>");
    let state = super::ProtocolState {
        sent: vec![super::SentCommand {
            request_id: 17,
            session_id: 13,
            path: StartStation::PATH,
            host_sent_unix_micros: 23,
        }],
        messages: vec![
            echo,
            event(
                7,
                12,
                13,
                17,
                StationLink {
                    epoch: 3,
                    state: StationLinkState::Authorized {
                        bssid: [2; 6],
                        channel: 6,
                    },
                },
            ),
        ],
        received_unix_micros: vec![29, 31],
        ..Default::default()
    };
    let log = super::capture::protocol_log(&state, None, true, false).unwrap();
    let text = std::str::from_utf8(&log).unwrap();
    assert!(!text.contains("private-test"));
    let records: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        records[0],
        serde_json::json!({
            "record": "host-command",
            "command": {"request_id":17, "session_id":13, "path": StartStation::PATH, "host_sent_unix_micros":23},
        })
    );
    assert_eq!(
        records[1]["message"]["body"],
        serde_json::json!({"credentials":"<redacted>", "power_save":"Off"})
    );
    assert_eq!(
        records[2]["message"]["body"],
        serde_json::json!({"epoch":3, "state":{"Authorized":{"bssid":[2,2,2,2,2,2],"channel":6}}})
    );
    assert_eq!(records[4]["finalized"], true);
    assert_eq!(records[4]["cancelled"], false);
}

#[test]
fn attachment_accepts_an_existing_sequence_but_still_detects_a_gap() {
    let mut boot = ProtocolHealth::default();
    boot.observe(&hello(7, 87), DecodeCounters::default());
    assert!(boot.failure.is_some());
    let mut attached = ProtocolHealth {
        origin: super::CaptureOrigin::Attachment,
        ..Default::default()
    };
    attached.observe(&hello(7, 87), DecodeCounters::default());
    assert!(attached.failure.is_none());
    attached.observe(&hello(7, 89), DecodeCounters::default());
    assert!(attached.failure.unwrap().contains("discontinuity"));
}

#[test]
fn command_response_requires_boot_session_and_request_identity() {
    let response = event(7, 13, 17, 19, oer_hil_protocol::base::Accepted);
    assert!(command_response_matches(&response, 7, 17, 19));
    assert!(!command_response_matches(&response, 8, 17, 19));
    assert!(!command_response_matches(&response, 7, 18, 19));
    assert!(!command_response_matches(&response, 7, 17, 20));
}

#[test]
fn target_sequence_discontinuity_is_a_fatal_protocol_error() {
    let mut health = ProtocolHealth::default();
    health.observe(&hello(7, 0), DecodeCounters::default());
    health.observe(
        &event(7, 2, 0, 0, oer_hil_protocol::base::Accepted),
        DecodeCounters::default(),
    );
    assert!(
        health
            .failure
            .as_deref()
            .is_some_and(|failure| { failure.contains("expected 1, observed 2") })
    );
}

#[test]
fn a_new_boot_must_restart_its_target_sequence() {
    let mut health = ProtocolHealth::default();
    health.observe(&hello(7, 0), DecodeCounters::default());
    health.observe(
        &event(8, 3, 0, 0, oer_hil_protocol::base::Accepted),
        DecodeCounters::default(),
    );
    assert!(
        health
            .failure
            .as_deref()
            .is_some_and(|failure| { failure.contains("boot 8") })
    );
}

#[test]
fn a_valid_new_boot_clears_previous_boot_failure() {
    let mut health = ProtocolHealth::default();
    health.observe(&hello(7, 0), DecodeCounters::default());
    health.observe(
        &event(7, 2, 0, 0, oer_hil_protocol::base::Accepted),
        DecodeCounters::default(),
    );
    assert!(health.failure.is_some());
    health.observe(&hello(8, 0), DecodeCounters::default());
    assert_eq!(health.boot_id, Some(8));
    assert_eq!(health.failure, None);
}

#[test]
fn a_stream_ends_without_failure_only_after_an_expected_detach() {
    let events = super::ProtocolEvents::default();
    events.close(Some(super::LinkError::transport(
        "serial reader reached end of stream",
    )));
    assert!(events.state.lock().unwrap().failure.is_some());
    let events = super::ProtocolEvents::default();
    events.state.lock().unwrap().expected_detach = true;
    events.close(Some(super::LinkError::transport(
        "serial reader reached end of stream",
    )));
    let state = events.state.lock().unwrap();
    assert!(state.failure.is_none() && state.closed);
    assert!(
        state.check().is_ok(),
        "the reply received before it still counts"
    );
}
