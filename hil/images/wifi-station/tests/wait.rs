//! The load producer wakes the network only for what the network can act on:
//! new datagrams while it can declare them, and a new run's generation
//! always; never for datagrams it already knows and cannot declare yet.
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
use core::task::Context;
use oer_portable_time_virtual::VirtualClock;

/// Nine datagrams over a port with no room: eight are declared, the
/// ninth waits for a place. With no new data, time or port event the
/// service comes to wait, with no wake of its own, and a tenth datagram it
/// cannot declare wakes nothing. A new run then wakes it though no datagram
/// is pushed, and its old works end unsent.
#[test]
fn a_full_port_and_a_ninth_datagram_let_the_service_wait() {
    let port = Port::default();
    port.0.borrow_mut().incoming.push_back(peer_asks());
    let wifi::Built {
        mut service,
        net,
        source,
        ..
    } = wifi::network(port.clone(), LOCAL, OURS, 5001, 5002);
    let clock = VirtualClock::new();
    turn(&mut service, &clock);
    let start = start();
    let mut first = udp::open(&net, &start).unwrap();
    {
        let (_receiving, mut sending) = first.split();
        for sequence in 0..9 {
            assert!(matches!(
                sending.try_send(64, PEER_PORT, load(1, sequence)),
                udp::Attempt::Taken
            ));
        }
    }
    let (count, waker) = Count::waker();
    let mut events = ();
    let mut run = core::pin::pin!(service.run(&clock, &mut events));
    assert!(
        quiesce(&mut run, &count, &waker, 32).is_some(),
        "no new data, timer or port event: the service waits"
    );
    assert!(port.0.borrow().sent.is_empty());
    let wakes = source.wakes();
    assert!(quiesce(&mut run, &count, &waker, 1).is_some());
    assert_eq!(
        source.wakes(),
        wakes,
        "the producer gives no wake of its own"
    );
    // A tenth datagram the network cannot declare yet wakes nothing either.
    {
        let (_receiving, mut sending) = first.split();
        assert!(matches!(
            sending.try_send(64, PEER_PORT, load(1, 9)),
            udp::Attempt::Taken
        ));
    }
    assert_eq!(
        count.take(),
        0,
        "new data alone does not wake a blocked network"
    );
    assert_eq!(source.wakes(), wakes);
    assert!(quiesce(&mut run, &count, &waker, 1).is_some());
    drop(first);
    // A new run, with no datagram pushed: its generation alone wakes.
    let _next = udp::open(&net, &start).unwrap();
    assert!(count.take() > 0, "a new generation wakes the network");
    assert_eq!(source.wakes(), wakes + 1);
    assert!(quiesce(&mut run, &count, &waker, 32).is_some());
    port.0.borrow_mut().open = true;
    let cx = &mut Context::from_waker(&waker);
    for _ in 0..4 {
        assert!(run.as_mut().poll(cx).is_pending());
    }
    assert!(
        sent(&port).is_empty(),
        "the earlier run's datagrams never go"
    );
}
