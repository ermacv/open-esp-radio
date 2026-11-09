//! The load producer keeps each datagram at its own place: offers go oldest
//! first, a refused offer comes back first, a freed place takes the next
//! datagram, and a tag of the datagram that left it is no longer current.
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
use oer_portable_time::Instant;
use wifi::{Produce, SendRefusal};

/// The datagrams the source holds at once.
const QUEUE: u64 = 32;

#[test]
fn each_datagram_keeps_its_place_until_it_leaves() {
    let port = Port::default();
    // The service stays: its end would end the producer's sends.
    let wifi::Built {
        service: _service,
        net,
        source,
        ..
    } = wifi::network(port, LOCAL, OURS, 5001, 5002);
    let mut run = udp::open(&net, &start()).unwrap();
    let (receiving, mut sending) = run.split();
    for sequence in 0..QUEUE {
        assert!(matches!(
            sending.try_send(64, PEER_PORT, load(1, sequence)),
            udp::Attempt::Taken
        ));
    }
    assert!(
        matches!(
            sending.try_send(64, PEER_PORT, load(1, QUEUE)),
            udp::Attempt::NoRoom
        ),
        "every place holds a datagram"
    );
    let mut producer = udp::LoadProducer::new(source);
    let now = Instant::from_micros(0);
    let offers: Vec<_> = (0..QUEUE)
        .map(|_| producer.offer(now).ok().unwrap())
        .collect();
    assert!(producer.offer(now).is_err(), "each offered once");
    assert!(offers.iter().all(|offer| producer.current(offer.tag)));
    // A refusal for now: that datagram is offered again, first.
    producer.refused(offers[5].tag, SendRefusal::NoPlace);
    assert_eq!(
        producer.offer(now).ok().map(|offer| offer.tag),
        Some(offers[5].tag)
    );
    // A final refusal frees its place, which takes the next datagram.
    producer.refused(offers[7].tag, SendRefusal::TooLong);
    assert!(!producer.current(offers[7].tag), "it left");
    assert!(matches!(
        sending.try_send(64, PEER_PORT, load(1, QUEUE)),
        udp::Attempt::Taken
    ));
    let next = producer.offer(now).ok().unwrap();
    assert!(producer.current(next.tag));
    assert!(
        !producer.current(offers[7].tag),
        "the earlier tag of that place names the datagram that left"
    );
    assert!(
        offers
            .iter()
            .enumerate()
            .all(|(at, offer)| at == 7 || producer.current(offer.tag))
    );
    drop((receiving, sending));
    drop(run);
    // A new run: no tag of the earlier one is current.
    let _run = udp::open(&net, &start()).unwrap();
    assert!(!producer.current(next.tag));
    assert!(offers.iter().all(|offer| !producer.current(offer.tag)));
    assert!(producer.offer(now).is_err(), "nothing left to offer");
}
