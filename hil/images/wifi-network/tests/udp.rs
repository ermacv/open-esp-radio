//! The image's UDP facade on real sockets: ending a run returns every block.
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

use core::future::Future;
use core::task::{Context, Waker};

use bench::*;
use oer_portable_time_virtual::VirtualClock;

#[test]
fn ending_a_load_run_returns_every_block_its_adapters_held() {
    let port = Port::default();
    let wifi::Built {
        mut service, net, ..
    } = wifi::network(port.clone(), LOCAL, OURS, 5001, 5002);
    let start = start();
    let mut run = udp::open(&net, &start).unwrap();
    let clock = VirtualClock::new();
    // A run's end finds a datagram its reader had taken and not read.
    port.0.borrow_mut().incoming.push_back(datagram());
    turn(&mut service, &clock);
    {
        let (mut receiving, _sending) = run.split();
        let ready = core::pin::pin!(receiving.ready());
        let waker = Waker::noop();
        assert!(ready.poll(&mut Context::from_waker(waker)).is_ready());
    }
    {
        let (_receiving, mut sending) = run.split();
        let mut taken = 0;
        for sequence in 0..1000 {
            match sending.try_send(64, PEER_PORT, load(1, sequence)) {
                udp::Attempt::Taken => taken += 1,
                udp::Attempt::NoRoom => turn(&mut service, &clock),
                udp::Attempt::Lost => panic!("unexpected loss"),
            }
            if taken == 32 {
                break;
            }
        }
        assert_eq!(taken, 32, "the producer holds 32 datagrams");
        // The declared works wait for ARP and hold no block; the producer's
        // room is what pushes back.
        for sequence in 0..8 {
            turn(&mut service, &clock);
            assert!(matches!(
                sending.try_send(64, PEER_PORT, load(1, 100 + sequence)),
                udp::Attempt::NoRoom
            ));
        }
    }
    drop(run);
    let mut ended = service.retire();
    drop(net);
    ended.collect();
    assert_eq!(
        ended.retired.pool.outstanding(),
        0,
        "ending a run lost a block"
    );
}
