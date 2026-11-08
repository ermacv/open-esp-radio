//! Work a local producer declares: a place and no block, a frame its producer
//! fills in the port's slot, and the ends of that work.

use std::cell::Cell;
use std::vec::Vec;

use super::*;
use crate::{Declined, WorkId};

/// The data a producer writes for `work`: its place, then a count.
fn pattern(length: usize) -> Vec<u8> {
    (0..length).map(|at| at as u8 ^ 0xa5).collect()
}

/// A fill that writes [`pattern`] and counts its calls.
fn writes(calls: &Cell<usize>) -> impl FnOnce(WorkId, &mut [u8]) -> Result<(), Declined> + '_ {
    move |_, data| {
        calls.set(calls.get() + 1);
        data.copy_from_slice(&pattern(data.len()));
        Ok(())
    }
}

fn declines(calls: &Cell<usize>) -> impl FnOnce(WorkId, &mut [u8]) -> Result<(), Declined> + '_ {
    move |_, _| {
        calls.set(calls.get() + 1);
        Err(Declined::Stale)
    }
}

fn broadcast() -> Remote {
    Remote {
        address: [192, 0, 2, 255],
        port: 7,
    }
}

/// Transmit the next ready work with `fill`.
fn produce_next<B>(
    stack: &mut Stack<B>,
    now: Instant,
    fill: impl FnOnce(WorkId, &mut [u8]) -> Result<(), Declined>,
) -> Result<(), Unsent<&'static str>> {
    let demand = stack.poll_demand(now).unwrap();
    stack.transmit(now, demand.grant(), fill)
}

#[test]
fn a_declared_work_holds_a_place_and_no_block() {
    world!(|stack| {
        let dscp = Dscp::from_field(oer_portable_wire::U8::constant::<46>());
        let socket = stack.open_local(6000, dscp).unwrap();
        let work = stack.declare(at(0), &socket, broadcast(), 100).unwrap();
        assert_eq!(stack.pool().free(), BLOCKS, "no block");
        let demand = stack.poll_demand(at(0)).unwrap();
        assert_eq!(demand.ready.class, Class::Data);
        assert_eq!(demand.ready.length, 14 + 20 + 8 + 100);
        assert_eq!(demand.ready.flow, Some(socket.flow()));
        assert_eq!(demand.ready.dscp, dscp);
        assert_eq!(demand.ready.peer, [0xFF; 6]);
        assert!(
            stack.queued().next().is_none(),
            "nothing to evict: no block"
        );
        let _running = running(stack.take_result(work));
    });
}

#[test]
fn a_declaration_is_refused_as_a_send_is() {
    world_on!(interface(OUR_IP, 24, None), |stack| {
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let far = Remote {
            address: [198, 51, 100, 1],
            port: 7,
        };
        assert_eq!(
            stack.declare(at(0), &socket, peer(), 1473).err(),
            Some(SendRefusal::TooLong)
        );
        assert_eq!(
            stack.declare(at(0), &socket, far, 10).err(),
            Some(SendRefusal::NoRoute)
        );
        let _one = stack.declare(at(0), &socket, broadcast(), 10).unwrap();
        let _two = stack.declare(at(0), &socket, broadcast(), 10).unwrap();
        assert_eq!(
            stack.declare(at(0), &socket, broadcast(), 10).err(),
            Some(SendRefusal::NoPlace)
        );
    });
    world!(|stack| {
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let other = stack.open_local(6001, best_effort()).unwrap();
        for (on, last) in [(&socket, 3), (&other, 4)] {
            let remote = Remote {
                address: [192, 0, 2, last],
                port: 7,
            };
            assert!(stack.declare(at(0), on, remote, 10).is_ok());
        }
        let third = Remote {
            address: [192, 0, 2, 5],
            port: 7,
        };
        assert_eq!(
            stack.declare(at(0), &socket, third, 10).err(),
            Some(SendRefusal::NeighborsFull)
        );
        let close = stack.close(other).unwrap();
        assert_eq!(finished(stack.take_result(close)), Outcome::SocketClosed);
        assert!(stack.declare(at(0), &socket, third, 10).is_ok());
    });
}

#[test]
fn a_produced_frame_is_written_in_the_slot_and_its_checksum_verifies() {
    world!(|stack| {
        knows_peer(stack);
        let dscp = Dscp::from_field(oer_portable_wire::U8::constant::<10>());
        let socket = stack.open_local(6000, dscp).unwrap();
        let work = stack.declare(at(1), &socket, peer(), 37).unwrap();
        let calls = Cell::new(0);
        produce_next(stack, at(1), writes(&calls)).unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(finished(stack.take_result(work)), Outcome::Accepted);
        let frame = stack.port_mut().sent.pop().unwrap();
        let (ethernet, packet) = EthernetHeader::parse(&frame).unwrap();
        assert_eq!(ethernet.destination, PEER);
        assert_eq!(ethernet.source, LOCAL);
        assert_eq!(ethernet.ether_type, EtherType::IPV4);
        assert!(Checksum::new().with(&packet[..20]).verifies());
        let ip = Ipv4Header::parse(packet).unwrap();
        assert_eq!(ip.addresses()[..4], OUR_IP);
        assert_eq!(ip.addresses()[4..], PEER_IP);
        assert_eq!(packet[1] >> 2, 10, "the socket's DSCP");
        let datagram = ip.payload();
        let (udp, data) = UdpHeader::parse(datagram).unwrap();
        assert_eq!(udp.ports.source, 6000);
        assert_eq!(udp.ports.destination, 7);
        assert_eq!(usize::from(udp.length), 8 + 37);
        assert_eq!(data, pattern(37));
        assert_ne!(udp.checksum, 0);
        assert!(
            Checksum::new()
                .with(&ipv4_pseudo_header(OUR_IP, PEER_IP, PROTOCOL_UDP, udp.length))
                .with(datagram)
                .verifies()
        );
    });
}

#[test]
fn a_declared_work_waits_for_its_neighbor_without_a_slot() {
    world!(|stack| {
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let work = stack.declare(at(0), &socket, peer(), 8).unwrap();
        let demand = stack.poll_demand(at(0)).unwrap();
        assert_eq!(demand.ready.class, Class::Control, "its neighbor first");
        let calls = Cell::new(0);
        stack.transmit(at(0), demand.grant(), writes(&calls)).unwrap();
        assert_eq!(calls.get(), 0, "an ARP request fills no data");
        assert!(stack.poll_demand(at(0)).is_none(), "no slot during ARP");
        let work = running(stack.take_result(work));
        let reply = arp_frame(Operation::Reply, (PEER, PEER_IP), OUR_IP);
        stack.input(at(1), &reply, &mut ());
        produce_next(stack, at(1), writes(&calls)).unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(finished(stack.take_result(work)), Outcome::Accepted);
    });
}

#[test]
fn a_refusal_before_the_fill_never_calls_it_and_the_work_stays() {
    world!(|stack| {
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let work = stack.declare(at(0), &socket, broadcast(), 8).unwrap();
        stack.port_mut().full = true;
        let calls = Cell::new(0);
        assert_eq!(
            produce_next(stack, at(0), writes(&calls)),
            Err(Unsent::Port("full"))
        );
        assert_eq!(calls.get(), 0);
        let work = running(stack.take_result(work));
        stack.port_mut().full = false;
        produce_next(stack, at(0), writes(&calls)).unwrap();
        assert_eq!(calls.get(), 1, "one fill");
        assert_eq!(finished(stack.take_result(work)), Outcome::Accepted);
    });
}

/// A declining fill ends its work `Declined`, never `Rejected`, and the next
/// socket's work goes first.
#[test]
fn a_declining_fill_ends_its_work_declined_and_passes_the_turn() {
    world!(|stack| {
        let local = stack.open_local(6000, best_effort()).unwrap();
        let channel = stack.open(5000, best_effort()).unwrap();
        let first = stack.declare(at(0), &local, broadcast(), 8).unwrap();
        let second = stack.declare(at(0), &local, broadcast(), 8).unwrap();
        let block = filled(stack.pool(), b"other");
        let other = stack.send(at(0), &channel, broadcast(), block).unwrap();
        let calls = Cell::new(0);
        assert_eq!(
            produce_next(stack, at(0), declines(&calls)),
            Err(Unsent::Declined)
        );
        assert_eq!(calls.get(), 1);
        assert_eq!(finished(stack.take_result(first)), Outcome::Declined);
        assert!(!stack.port_broken());
        let next = stack.poll_demand(at(0)).unwrap();
        assert_eq!(
            next.ready.flow,
            Some(channel.flow()),
            "the other socket goes first"
        );
        stack.transmit(at(0), next.grant(), unproduced).unwrap();
        assert_eq!(finished(stack.take_result(other)), Outcome::Accepted);
        produce_next(stack, at(0), writes(&calls)).unwrap();
        assert_eq!(finished(stack.take_result(second)), Outcome::Accepted);
        assert_eq!(stack.port().sent.len(), 2);
    });
}

#[test]
fn a_yes_after_a_declined_fill_is_unknown_and_the_port_s_fault() {
    world!(|stack| {
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let work = stack.declare(at(0), &socket, broadcast(), 8).unwrap();
        stack.port_mut().careless = true;
        let calls = Cell::new(0);
        assert_eq!(
            produce_next(stack, at(0), declines(&calls)),
            Err(Unsent::PortContract)
        );
        assert_eq!(finished(stack.take_result(work)), Outcome::Unknown);
        assert!(stack.port_broken());
        assert!(stack.poll_demand(at(0)).is_none(), "never again");
    });
}

#[test]
fn a_port_refusal_after_a_declined_fill_ends_it_declined_and_is_the_port_s_fault() {
    world!(|stack| {
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let work = stack.declare(at(0), &socket, broadcast(), 8).unwrap();
        stack.port_mut().misreports = true;
        let calls = Cell::new(0);
        assert_eq!(
            produce_next(stack, at(0), declines(&calls)),
            Err(Unsent::FillMisreported("misreported"))
        );
        assert_eq!(
            finished(stack.take_result(work)),
            Outcome::Declined,
            "the fill's recorded cause"
        );
        assert!(stack.port_broken());
    });
}

#[test]
fn a_slot_too_short_for_a_produced_frame_ends_it_rejected_unfilled() {
    world!(|stack| {
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let work = stack.declare(at(0), &socket, broadcast(), 64).unwrap();
        stack.port_mut().room = Some(30);
        let calls = Cell::new(0);
        assert_eq!(
            produce_next(stack, at(0), writes(&calls)),
            Err(Unsent::SlotTooShort)
        );
        assert_eq!(calls.get(), 0, "no room for its headers: no fill");
        assert_eq!(finished(stack.take_result(work)), Outcome::Rejected);
        assert!(!stack.port_broken());
    });
}

#[test]
fn withdrawal_ends_a_declared_work_declined_without_the_port() {
    world!(|stack| {
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let work = stack.declare(at(0), &socket, broadcast(), 8).unwrap();
        assert!(stack.withdraw(&work));
        assert!(!stack.withdraw(&work), "once");
        assert!(stack.poll_demand(at(0)).is_none());
        assert_eq!(finished(stack.take_result(work)), Outcome::Declined);
        assert!(stack.port().sent.is_empty());
    });
}

#[test]
fn a_close_ends_declared_work_and_its_stale_grant_fills_nothing() {
    world!(|stack| {
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let work = stack.declare(at(0), &socket, broadcast(), 8).unwrap();
        let stale = stack.poll_demand(at(0)).unwrap().grant();
        let close = stack.close(socket).unwrap();
        let calls = Cell::new(0);
        assert_eq!(
            stack.transmit(at(0), stale, writes(&calls)),
            Err(Unsent::Stale)
        );
        assert_eq!(calls.get(), 0, "no fill");
        assert_eq!(finished(stack.take_result(work)), Outcome::Closed);
        assert_eq!(finished(stack.take_result(close)), Outcome::SocketClosed);
        assert!(stack.port().sent.is_empty());
    });
}

#[test]
fn a_retired_core_ends_declared_work_retired() {
    brand!(Net);
    let (pool, brand) = claimed::<Net>();
    let mut stack: Stack<Net> =
        NetworkOwner::new(brand, interface(OUR_IP, 24, None), pool, Port::default());
    let socket = stack.open_local(6000, best_effort()).unwrap();
    let work = stack.declare(at(0), &socket, broadcast(), 8).unwrap();
    let mut retired = stack.retire();
    assert_eq!(finished(retired.results.take(work)), Outcome::Retired);
    assert_eq!(retired.pool.outstanding(), 0);
}

#[test]
fn two_open_works_never_share_an_identity() {
    world!(|stack| {
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let first = stack.declare(at(0), &socket, broadcast(), 8).unwrap();
        let second = stack.declare(at(0), &socket, broadcast(), 8).unwrap();
        assert_ne!(first.work(), second.work());
        let old = first.work();
        assert!(stack.withdraw(&first));
        let _ = stack.take_result(first);
        let reused = stack.declare(at(0), &socket, broadcast(), 8).unwrap();
        assert_eq!(reused.work().place(), old.place(), "the same place");
        assert_ne!(reused.work(), old, "another work");
        assert_ne!(reused.work(), second.work());
    });
}

#[test]
fn a_send_only_socket_drops_input_before_a_block() {
    world!(|stack| {
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let mut dropped = Dropped::default();
        stack.input(at(0), &udp_frame(7, 6000, b"no"), &mut dropped);
        assert_eq!(dropped.0, [NetEvent::SendOnly]);
        assert_eq!(stack.pool().free(), BLOCKS, "no block was taken");
        assert!(stack.receive(&socket).unwrap().is_none());
    });
}

/// The counter moves whenever a neighbor entry may have become available to
/// a new send: each end of a datagram or declared work that held one, an ARP
/// answer and a resolution given up.
#[test]
fn the_neighbor_counter_moves_on_every_release_of_protection() {
    world!(|stack| {
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let channel = stack.open(5000, best_effort()).unwrap();
        let mut seen = stack.neighbors_changed();
        let mut moved = |stack: &mut Stack<_>, what: &str| {
            let now = stack.neighbors_changed();
            assert_ne!(now, seen, "{what}");
            seen = now;
        };
        // An ARP answer.
        let work = stack.declare(at(0), &socket, peer(), 8).unwrap();
        send_next(stack, at(0)).unwrap();
        let reply = arp_frame(Operation::Reply, (PEER, PEER_IP), OUR_IP);
        stack.input(at(1), &reply, &mut ());
        moved(stack, "an answer");
        // A commit.
        let calls = Cell::new(0);
        produce_next(stack, at(1), writes(&calls)).unwrap();
        moved(stack, "a commit");
        let _ = stack.take_result(work);
        // A decline.
        let work = stack.declare(at(1), &socket, peer(), 8).unwrap();
        assert_eq!(
            produce_next(stack, at(1), declines(&calls)),
            Err(Unsent::Declined)
        );
        moved(stack, "a decline");
        let _ = stack.take_result(work);
        // A withdrawal.
        let work = stack.declare(at(1), &socket, peer(), 8).unwrap();
        assert!(stack.withdraw(&work));
        moved(stack, "a withdrawal");
        let _ = stack.take_result(work);
        // A final refusal of the port.
        let block = filled(stack.pool(), b"x");
        let command = stack.send(at(1), &channel, peer(), block).unwrap();
        stack.port_mut().rejects = true;
        assert!(send_next(stack, at(1)).is_err());
        stack.port_mut().rejects = false;
        moved(stack, "a reject");
        let _ = stack.take_result(command);
        // An eviction.
        let block = filled(stack.pool(), b"x");
        let command = stack.send(at(1), &channel, peer(), block).unwrap();
        let queued = stack.queued().next().unwrap();
        assert!(stack.evict(queued, Eviction::MemoryPressure));
        moved(stack, "an eviction");
        let _ = stack.take_result(command);
        // A close.
        let work = stack.declare(at(1), &socket, peer(), 8).unwrap();
        let close = stack.close(socket).unwrap();
        moved(stack, "a close");
        let _ = (stack.take_result(work), stack.take_result(close));
        // A resolution given up.
        let socket = stack.open_local(6001, best_effort()).unwrap();
        let far = Remote {
            address: [192, 0, 2, 9],
            port: 7,
        };
        let work = stack.declare(at(2), &socket, far, 8).unwrap();
        let mut now = at(2);
        while let Some(demand) = stack.poll_demand(now) {
            stack.transmit(now, demand.grant(), unproduced).unwrap();
            now = stack.poll_deadline().unwrap();
            stack.tick(now);
        }
        assert_eq!(finished(stack.take_result(work)), Outcome::Unreachable);
        moved(stack, "a resolution given up");
    });
}

/// Nothing a neighbor holds changes when a datagram to broadcast ends.
#[test]
fn reading_the_neighbor_counter_resets_nothing() {
    world!(|stack| {
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let before = stack.neighbors_changed();
        assert_eq!(stack.neighbors_changed(), before);
        let work = stack.declare(at(0), &socket, broadcast(), 8).unwrap();
        let calls = Cell::new(0);
        produce_next(stack, at(0), writes(&calls)).unwrap();
        assert_eq!(finished(stack.take_result(work)), Outcome::Accepted);
        assert_eq!(
            stack.neighbors_changed(),
            before,
            "broadcast held no entry"
        );
    });
}
