//! The load producer writes each datagram whole in the slot the network
//! grants it.
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

/// The producer writes each datagram whole in the port's slot: the header
/// the sender handed over and the filler behind it, at the size it asked.
#[test]
fn a_run_s_datagrams_go_whole_from_the_producer() {
    let port = Port::default();
    {
        let mut wire = port.0.borrow_mut();
        wire.open = true;
        wire.incoming.push_back(peer_asks());
    }
    let wifi::Built {
        mut service, net, ..
    } = wifi::network(port.clone(), LOCAL, OURS, 5001, 5002);
    let clock = VirtualClock::new();
    turn(&mut service, &clock);
    let start = start();
    let mut run = udp::open(&net, &start).unwrap();
    {
        let (_receiving, mut sending) = run.split();
        for (sequence, size) in [(0, 28), (1, 100), (2, 1472)] {
            assert!(matches!(
                sending.try_send(size, PEER_PORT, load(3, sequence)),
                udp::Attempt::Taken
            ));
        }
        assert!(
            matches!(
                sending.try_send(27, PEER_PORT, load(3, 9)),
                udp::Attempt::Lost
            ),
            "no header fits"
        );
        assert!(
            matches!(
                sending.try_send(1473, PEER_PORT, load(3, 9)),
                udp::Attempt::Lost
            ),
            "no frame carries it"
        );
    }
    for _ in 0..4 {
        turn(&mut service, &clock);
    }
    let payloads = sent(&port);
    assert_eq!(payloads.len(), 3);
    for (payload, (sequence, size)) in payloads.iter().zip([(0, 28), (1, 100), (2, 1472)]) {
        assert_eq!(payload.len(), size);
        assert_eq!(header::Header::read(payload), Some(load(3, sequence)));
        assert!(
            payload[header::LENGTH..]
                .iter()
                .all(|octet| *octet == header::FILLER)
        );
    }
    drop(run);
    let _ended = service.retire();
}
