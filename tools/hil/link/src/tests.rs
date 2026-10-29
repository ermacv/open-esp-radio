use std::{thread, time::Duration};

use oer_hil_protocol::{
    Endpoint, Message, WireKind,
    base::{GetHello, Hello, RejectReason, Rejected},
};
use serde::{Deserialize, Serialize};

use crate::test_support::{Device, HELLO};
use crate::{CallError, LinkError};

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

const SHORT: Duration = Duration::from_secs(5);

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
    assert_eq!(link.health().missing, 0);
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
fn missing_messages_are_counted_and_a_reordered_one_fails_the_link() {
    let (mut device, link) = Device::new(3);
    device.send(0, HELLO);
    let boot = link.boot(SHORT).unwrap();
    device.sequence = 4;
    device.send(0, Pong(1));
    assert!(link.take(&boot, |message| message.is::<Pong>(), SHORT).unwrap().is_some());
    assert_eq!(link.health().missing, 3);
    device.sequence = 2;
    device.send(0, Pong(2));
    let failed = link.take(&boot, |_| false, SHORT);
    assert!(matches!(failed, Err(CallError::Link(LinkError::Protocol(_)))), "{failed:?}");
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
