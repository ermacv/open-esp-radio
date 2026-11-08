//! A run is a generation of the load producer's source: an earlier run's
//! datagram never goes with a later run's data.
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
use oer_portable_time_virtual::VirtualClock;

/// A datagram an earlier run left waiting ends `Declined` when the next run
/// begins, and no frame carries the later run's data under it.
#[test]
fn an_earlier_run_s_datagram_never_goes_with_a_later_run_s_data() {
    let port = Port::default();
    port.0.borrow_mut().incoming.push_back(peer_asks());
    let wifi::Built {
        mut service, net, ..
    } = wifi::network(port.clone(), LOCAL, OURS, 5001, 5002);
    let clock = VirtualClock::new();
    let start = start();
    {
        let mut run = udp::open(&net, &start).unwrap();
        let (_receiving, mut sending) = run.split();
        assert!(matches!(
            sending.try_send(64, PEER_PORT, load(1, 0)),
            udp::Attempt::Taken
        ));
    }
    // Declared, and the port has no room yet.
    turn(&mut service, &clock);
    turn(&mut service, &clock);
    {
        let mut run = udp::open(&net, &start).unwrap();
        let (_receiving, mut sending) = run.split();
        assert!(matches!(
            sending.try_send(64, PEER_PORT, load(2, 0)),
            udp::Attempt::Taken
        ));
    }
    port.0.borrow_mut().open = true;
    for _ in 0..4 {
        turn(&mut service, &clock);
    }
    let payloads = sent(&port);
    assert_eq!(payloads.len(), 1, "only the later run's datagram");
    assert_eq!(header::Header::read(&payloads[0]), Some(load(2, 0)));
    let _ended = service.retire();
}
