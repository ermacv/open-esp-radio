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
