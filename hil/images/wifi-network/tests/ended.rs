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

/// The service's end shows to the sender: what it hands over then is lost.
#[test]
fn a_sender_after_the_network_ended_loses_its_datagram() {
    let port = Port::default();
    let wifi::Built {
        service, net, end, ..
    } = wifi::network(port, LOCAL, OURS, 5001, 5002);
    let start = start();
    let mut run = udp::open(&net, &start).unwrap();
    drop(service);
    assert!(end.is_ended());
    let (_receiving, mut sending) = run.split();
    assert!(matches!(
        sending.try_send(64, PEER_PORT, load(1, 0)),
        udp::Attempt::Lost
    ));
}
