//! A retired network wakes a load sender waiting for room.
//! `oer` and the load service stand in for their chip-side names.
extern crate self as oer;
extern crate self as oer_hil_services_load;

#[path = "support/load.rs"]
mod names;
pub use names::{LoadError, Start};

#[path = "support/wifi.rs"]
pub mod wifi;

#[path = "../../../services/load/src/header.rs"]
pub mod header;

#[path = "../src/udp.rs"]
mod udp;

#[path = "support/bench.rs"]
mod bench;

use bench::*;
use core::future::Future;
use core::task::Context;

/// Retirement, as a drop, ends a sender's wait for room: it is woken and
/// sees the end.
#[test]
fn a_retired_network_wakes_a_sender_waiting_for_room() {
    let wifi::Built {
        service, net, end, ..
    } = wifi::network(Port::default(), LOCAL, OURS, 5001, 5002);
    let start = start();
    let mut run = udp::open(&net, &start).unwrap();
    let (_receiving, mut sending) = run.split();
    for sequence in 0..32 {
        assert!(matches!(
            sending.try_send(64, PEER_PORT, load(1, sequence)),
            udp::Attempt::Taken
        ));
    }
    let (count, waker) = Count::waker();
    let cx = &mut Context::from_waker(&waker);
    let mut waiting = core::pin::pin!(sending.send(64, PEER_PORT, load(1, 32)));
    assert!(waiting.as_mut().poll(cx).is_pending(), "no room");
    count.take();
    let _ended = service.retire();
    assert!(end.is_ended());
    assert!(count.take() > 0, "the end wakes the waiting sender");
    assert_eq!(waiting.as_mut().poll(cx), core::task::Poll::Ready(false));
}
