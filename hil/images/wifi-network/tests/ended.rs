//! The network's end shows to the load's sender.
//! `oer` and the load service stand in for their chip-side names, so only
//! start and the port are substituted.
extern crate self as oer;
extern crate self as oer_hil_services_load;

#[path = "support/load.rs"]
mod names;
pub use names::{LoadError, Start, StaticAddress};

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

/// The service's end shows to the sender: one waiting for room is woken by
/// a dropped service and sees the end, and what it hands over then is lost.
#[test]
fn a_sender_after_the_network_ended_loses_its_datagram() {
    let port = Port::default();
    let wifi::Built {
        service, net, end, ..
    } = wifi::network(port, LOCAL, OURS, 5001, 5002);
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
    {
        let mut waiting = core::pin::pin!(sending.send(64, PEER_PORT, load(1, 32)));
        assert!(waiting.as_mut().poll(cx).is_pending(), "no room");
        count.take();
        drop(service);
        assert!(end.is_ended());
        assert!(count.take() > 0, "the end wakes the waiting sender");
        assert_eq!(waiting.as_mut().poll(cx), core::task::Poll::Ready(false));
    }
    assert!(matches!(
        sending.try_send(64, PEER_PORT, load(1, 0)),
        udp::Attempt::Lost
    ));
}
