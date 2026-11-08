//! The load producer counts a datagram whose end is not known, and never
//! sends it again.
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
use oer_portable_time_virtual::VirtualClock;

/// A port that says it took a frame it never filled: the datagram ends
/// `Unknown`, the producer counts it, and nothing is sent again.
#[test]
fn an_unknown_end_is_counted_and_not_sent_again() {
    let port = Port::default();
    {
        let mut wire = port.0.borrow_mut();
        wire.incoming.push_back(peer_asks());
        wire.open = true;
    }
    let wifi::Built {
        mut service,
        net,
        source,
        ..
    } = wifi::network(port.clone(), LOCAL, OURS, 5001, 5002);
    let clock = VirtualClock::new();
    // The peer is known, and the port answers our reply to it; from here on
    // it says it took what it never filled.
    turn(&mut service, &clock);
    port.0.borrow_mut().lazy = true;
    let start = start();
    let mut run = udp::open(&net, &start).unwrap();
    {
        let (_receiving, mut sending) = run.split();
        assert!(matches!(
            sending.try_send(64, PEER_PORT, load(1, 0)),
            udp::Attempt::Taken
        ));
    }
    let mut events = ();
    let mut service_run = core::pin::pin!(service.run(&clock, &mut events));
    let cx = &mut core::task::Context::from_waker(core::task::Waker::noop());
    assert!(matches!(
        core::future::Future::poll(service_run.as_mut(), cx),
        core::task::Poll::Ready(_)
    ));
    assert_eq!(source.unknown(), 1);
    assert!(sent(&port).is_empty(), "no load datagram went whole");
    drop(run);
}
