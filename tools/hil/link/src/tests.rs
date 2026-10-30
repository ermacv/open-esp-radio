use std::{thread, time::Duration};

use oer_hil_protocol::{
    Endpoint, Message, WireKind,
    base::{GetHello, Hello, RejectReason, Rejected},
};
use serde::{Deserialize, Serialize};

use crate::test_support::{Device, HELLO};
use crate::{
    CallError, Completeness, EVENT_CAPACITY, LinkError, NOTES_CAPACITY, NotSent, OUTBOUND_CAPACITY,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, postcard_schema::Schema)]
struct Ping(u32);
impl Message for Ping {
    const PATH: &'static str = "test/ping";
    const WIRE_KIND: WireKind = WireKind::Command;
}
impl Endpoint for Ping {
    type Response = Pong;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, postcard_schema::Schema)]
struct Pong(u32);
impl Message for Pong {
    const PATH: &'static str = "test/pong";
}

/// An event of a request, or of no request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, postcard_schema::Schema)]
struct Progress(u32);
impl Message for Progress {
    const PATH: &'static str = "test/progress";
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, postcard_schema::Schema)]
struct Blob([[u8; 25]; 8]);
impl Message for Blob {
    const PATH: &'static str = "test/blob";
    const WIRE_KIND: WireKind = WireKind::Command;
}
impl Endpoint for Blob {
    type Response = Pong;
}

const SHORT: Duration = Duration::from_secs(5);

/// Wait up to [`SHORT`] for `condition`.
fn until(condition: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + SHORT;
    while !condition() {
        assert!(
            std::time::Instant::now() < deadline,
            "the condition never held"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_call_returns_its_typed_response_and_a_refusal_returns_its_reason() {
    let (mut device, link) = Device::new(7);
    device.send(0, HELLO);
    let boot = link.boot(SHORT).unwrap();
    assert_eq!((boot.boot_id, boot.hello), (7, HELLO));
    let server = thread::spawn(move || {
        let request = device.request().unwrap();
        assert_eq!((request.boot_id, request.key), (7, Ping::KEY));
        let Ping(value) = postcard::from_bytes(&request.payload).unwrap();
        // An unrelated event first: it does not answer the request.
        device.send(0, Pong(0));
        device.send(request.request_id, Pong(value + 1));
        let request = device.request().unwrap();
        device.send(request.request_id, Rejected(RejectReason::Busy));
        device
    });
    assert_eq!(link.call(&boot, Ping(41), SHORT), Ok(Pong(42)));
    assert_eq!(
        link.call(&boot, Ping(1), SHORT),
        Err(CallError::Rejected(RejectReason::Busy))
    );
    let _device = server.join().unwrap();
    assert_eq!(link.health().running.unwrap().missing, 0);
}

#[test]
fn a_missed_hello_is_asked_again_and_names_the_running_boot() {
    let (mut device, link) = Device::new(9);
    // The boot's own Hello was before the link opened.
    device.sequence = 3;
    let server = thread::spawn(move || {
        let request = device.request().unwrap();
        assert_eq!((request.boot_id, request.key), (0, GetHello::KEY));
        device.send(request.request_id, HELLO);
        device
    });
    let boot = link.boot(Duration::from_millis(400)).unwrap();
    assert_eq!(boot.boot_id, 9);
    let _device = server.join().unwrap();
}

#[test]
fn no_response_leaves_the_request_unconfirmed_and_a_late_one_answers_nothing_else() {
    let (mut device, link) = Device::new(5);
    device.send(0, HELLO);
    let boot = link.boot(SHORT).unwrap();
    let first = link.call(&boot, Ping(1), Duration::from_millis(50));
    let Err(CallError::Unconfirmed { request_id }) = first else {
        panic!("{first:?}");
    };
    let late = device.request().unwrap();
    assert_eq!(late.request_id, request_id);
    device.send(request_id, Pong(100));
    let server = thread::spawn(move || {
        let request = device.request().unwrap();
        device.send(request.request_id, Pong(2));
        device
    });
    assert_eq!(link.call(&boot, Ping(1), SHORT), Ok(Pong(2)));
    let _device = server.join().unwrap();
}

#[test]
fn another_boot_ends_the_requests_of_the_earlier_one() {
    let (mut device, link) = Device::new(5);
    device.send(0, HELLO);
    let boot = link.boot(SHORT).unwrap();
    let server = thread::spawn(move || {
        let _request = device.request().unwrap();
        device.reboot(6, HELLO);
        device
    });
    assert_eq!(
        link.call(&boot, Ping(1), SHORT),
        Err(CallError::Rebooted { boot_id: 6 })
    );
    let _device = server.join().unwrap();
    let rebooted = link.boot(SHORT).unwrap();
    assert_eq!(rebooted.boot_id, 6);
    assert_eq!(link.health().boots, 2);
}

#[test]
fn a_stream_of_events_to_the_limit_cannot_displace_a_response() {
    let (mut device, link) = Device::new(8);
    device.send(0, HELLO);
    let boot = link.boot(SHORT).unwrap();
    let cursor = link.cursor(&boot);
    let server = thread::spawn(move || {
        let request = device.request().unwrap();
        // A related event before the response, then a flood past the ring.
        device.send(request.request_id, Progress(1));
        for value in 0..EVENT_CAPACITY as u32 + 10 {
            device.send(0, Progress(value));
        }
        device.send(request.request_id, Pong(7));
        device
    });
    assert_eq!(link.call(&boot, Ping(6), SHORT), Ok(Pong(7)));
    let _device = server.join().unwrap();
    let health = link.health().running.unwrap();
    assert_eq!((health.displaced, health.missing), (11, 0));
    assert_eq!(link.completeness(&cursor), Completeness::Lossy);
    // The displaced messages were the oldest: the related event among them.
    let kept = link
        .take(&cursor, |message| message.is::<Progress>(), SHORT)
        .unwrap()
        .unwrap();
    assert_eq!(kept.decode::<Progress>(), Some(Ok(Progress(10))));
}

#[test]
fn a_cursor_made_before_a_request_sees_its_related_event_before_the_response() {
    let (mut device, link) = Device::new(8);
    device.send(0, HELLO);
    let boot = link.boot(SHORT).unwrap();
    // An event before the cursor is not the cursor's.
    device.send(0, Pong(99));
    until(|| link.health().running.is_some_and(|boot| boot.unread == 1));
    let cursor = link.cursor(&boot);
    let server = thread::spawn(move || {
        let request = device.request().unwrap();
        device.send(request.request_id, Progress(1));
        device.send(request.request_id, Pong(2));
        device
    });
    assert_eq!(link.call(&boot, Ping(0), SHORT), Ok(Pong(2)));
    let related = link
        .take(&cursor, |message| message.is::<Progress>(), SHORT)
        .unwrap()
        .unwrap();
    assert_eq!(related.request_id, 1);
    assert_eq!(related.decode::<Progress>(), Some(Ok(Progress(1))));
    assert!(
        link.take(&cursor, |message| message.is::<Pong>(), Duration::ZERO)
            .unwrap()
            .is_none(),
        "the event before the cursor"
    );
    assert_eq!(link.completeness(&cursor), Completeness::Complete);
    let _device = server.join().unwrap();
}

#[test]
fn a_new_boot_inherits_no_backlog_and_ends_the_old_boots_cursors() {
    let (mut device, link) = Device::new(1);
    device.send(0, HELLO);
    let boot = link.boot(SHORT).unwrap();
    let cursor = link.cursor(&boot);
    for value in 0..5 {
        device.send(0, Pong(value));
    }
    device.reboot(2, HELLO);
    device.send(0, Pong(50));
    let rebooted = loop {
        let boot = link.boot(SHORT).unwrap();
        if boot.boot_id == 2 {
            break boot;
        }
    };
    assert_eq!(
        link.completeness(&cursor),
        Completeness::Ended { boot_id: 2 }
    );
    assert!(matches!(
        link.take(&cursor, |_| true, SHORT),
        Err(CallError::Rebooted { boot_id: 2 })
    ));
    let health = link.health();
    assert_eq!(health.past.len(), 1);
    assert_eq!((health.past[0].boot_id, health.past[0].unread), (1, 5));
    // The new boot's cursor starts at its own messages; Pong(50) came before.
    let fresh = link.cursor(&rebooted);
    device.send(0, Pong(51));
    let first = link.take(&fresh, |_| true, SHORT).unwrap().unwrap();
    assert_eq!(first.decode::<Pong>(), Some(Ok(Pong(51))));
    assert_eq!(health.running.unwrap().unread, 1);
}

#[test]
fn a_device_gap_spoils_only_the_cursors_that_span_it() {
    let (mut device, link) = Device::new(4);
    device.send(0, HELLO);
    let boot = link.boot(SHORT).unwrap();
    device.sequence = 5;
    device.send(0, Pong(1));
    until(|| link.health().running.is_some_and(|boot| boot.unread == 1));
    let after = link.cursor(&boot);
    device.send(0, Pong(2));
    assert!(link.take(&after, |_| true, SHORT).unwrap().is_some());
    assert_eq!(link.completeness(&after), Completeness::Complete);
    let spanning = after;
    device.sequence += 2;
    device.send(0, Pong(3));
    assert!(link.take(&spanning, |_| true, SHORT).unwrap().is_some());
    assert_eq!(link.completeness(&spanning), Completeness::Lossy);
    assert_eq!(
        link.completeness(&link.cursor(&boot)),
        Completeness::Complete
    );
    assert_eq!(link.health().running.unwrap().missing, 6);
}

#[test]
fn a_full_outbound_queue_refuses_a_request_before_it_leaves() {
    let (mut device, link) = Device::new(3);
    device.send(0, HELLO);
    let boot = link.boot(SHORT).unwrap();
    // The device reads nothing: the socket fills, then the queue.
    let mut refused = None;
    for attempt in 0..100_000 {
        match link.call(&boot, Blob([[0xa5; 25]; 8]), Duration::ZERO) {
            Err(CallError::Unconfirmed { .. }) => {}
            Err(CallError::NotSent(reason)) => {
                refused = Some((attempt, reason));
                break;
            }
            other => panic!("{other:?}"),
        }
    }
    let (attempt, reason) = refused.expect("the queue never filled");
    assert_eq!(reason, NotSent::QueueFull);
    assert!(attempt >= OUTBOUND_CAPACITY, "{attempt}");
    drop(device);
}

#[test]
fn diagnostic_lines_beyond_the_ring_are_counted_as_lost() {
    let (mut device, link) = Device::new(3);
    device.send(0, HELLO);
    link.boot(SHORT).unwrap();
    for _ in 0..NOTES_CAPACITY + 5 {
        device.write(b"OER-DIAG:garbled\n");
    }
    device.send(0, HELLO);
    until(|| link.health().notes_lost == 5);
    let notes = link.take_notes();
    assert_eq!((notes.lines.len(), notes.lost), (NOTES_CAPACITY, 5));
    assert!(
        notes
            .lines
            .iter()
            .all(|line| line.starts_with("OER-DIAG-ERROR"))
    );
    assert_eq!(link.take_notes().lost, 0);
}

#[test]
fn missing_messages_are_counted_and_a_reordered_one_fails_the_link() {
    let (mut device, link) = Device::new(3);
    device.send(0, HELLO);
    let boot = link.boot(SHORT).unwrap();
    let cursor = link.cursor(&boot);
    device.sequence = 4;
    device.send(0, Pong(1));
    assert!(
        link.take(&cursor, |message| message.is::<Pong>(), SHORT)
            .unwrap()
            .is_some()
    );
    assert_eq!(link.health().running.unwrap().missing, 3);
    device.sequence = 2;
    device.send(0, Pong(2));
    let failed = link.take(&cursor, |_| false, SHORT);
    assert!(
        matches!(failed, Err(CallError::Link(LinkError::Protocol(_)))),
        "{failed:?}"
    );
}

#[test]
fn a_payload_that_does_not_decode_as_its_key_is_an_error_not_a_silent_skip() {
    let malformed = crate::Received {
        boot_id: 4,
        message_sequence: 9,
        session_id: 0,
        request_id: 0,
        key: Pong::KEY,
        payload: vec![0xff; 6],
    };
    assert_eq!(
        malformed.decode::<Pong>(),
        Some(Err(CallError::Payload { path: "test/pong" }))
    );
    assert!(malformed.decode::<Hello>().is_none());
}

#[test]
fn the_reserved_boot_identity_fails_the_link() {
    let (mut device, link) = Device::new(0);
    device.send(0, HELLO);
    assert!(matches!(link.boot(SHORT), Err(LinkError::Protocol(_))));
}
