//! Work a local producer declares: a place and no block, a frame its producer
//! fills in the port's slot, and the ends of that work.

use std::cell::Cell;
use std::vec::Vec;

use super::*;
use crate::{Declined, Ends, Payload, WorkId};
use oer_protocols_network_port::Held;

/// The data a producer writes for `work`: its place, then a count.
fn pattern(length: usize) -> Vec<u8> {
    (0..length).map(|at| at as u8 ^ 0xa5).collect()
}

/// A fill that writes [`pattern`] and counts its calls.
fn writes(
    calls: &Cell<usize>,
) -> impl FnOnce(WorkId, u32, &mut Payload<'_>) -> Result<(), Declined> + '_ {
    move |_, _, data| {
        calls.set(calls.get() + 1);
        data.put(&pattern(data.remaining()))
            .map_err(|_| Declined::Unwritable)
    }
}

fn declines(
    calls: &Cell<usize>,
) -> impl FnOnce(WorkId, u32, &mut Payload<'_>) -> Result<(), Declined> + '_ {
    move |_, _, _| {
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

/// Transmit the next ready work with `fill`: the declared work it ended,
/// with its tag, when the port took it.
fn produce_next<B>(
    stack: &mut Stack<B>,
    now: Instant,
    fill: impl FnOnce(WorkId, u32, &mut Payload<'_>) -> Result<(), Declined>,
) -> Result<Option<(WorkId, u32)>, Unsent<&'static str>> {
    let demand = stack.poll_demand(now).unwrap();
    stack
        .transmit(now, demand.grant(), fill, &())
        .map(|sent| sent.ended)
}

/// The next declared work of `ends`' socket to end, with its tag and
/// outcome.
fn end_of<B>(stack: &mut Stack<B>, ends: &Ends<B>) -> Option<(WorkId, u32, Outcome)> {
    stack.next_declared_end(ends, &())
}

#[test]
fn a_declared_work_holds_a_place_and_no_block() {
    world!(|stack| {
        let dscp = Dscp::from_field(oer_portable_wire::U8::constant::<46>());
        let socket = stack.open_local(6000, dscp).unwrap();
        let work = stack.declare(at(0), &socket, broadcast(), 100, 1).unwrap();
        assert_eq!(stack.pool().free(), BLOCKS, "no block");
        let demand = stack.poll_demand(at(0)).unwrap();
        assert_eq!(demand.ready().class, Class::Data);
        assert_eq!(demand.ready().length, 14 + 20 + 8 + 100);
        assert_eq!(demand.ready().flow, Some(socket.flow()));
        assert_eq!(demand.ready().dscp, dscp);
        assert_eq!(demand.ready().peer, [0xFF; 6]);
        assert!(
            stack.queued().next().is_none(),
            "nothing to evict: no block"
        );
        assert_eq!(end_of(stack, &socket.ends()), None, "it runs");
        let _ = work;
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
            stack.declare(at(0), &socket, peer(), 1473, 2).err(),
            Some(SendRefusal::TooLong)
        );
        assert_eq!(
            stack.declare(at(0), &socket, far, 10, 3).err(),
            Some(SendRefusal::NoRoute)
        );
        let _one = stack.declare(at(0), &socket, broadcast(), 10, 4).unwrap();
        let _two = stack.declare(at(0), &socket, broadcast(), 10, 5).unwrap();
        assert_eq!(
            stack.declare(at(0), &socket, broadcast(), 10, 6).err(),
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
            assert!(stack.declare(at(0), on, remote, 10, 7).is_ok());
        }
        let third = Remote {
            address: [192, 0, 2, 5],
            port: 7,
        };
        assert_eq!(
            stack.declare(at(0), &socket, third, 10, 8).err(),
            Some(SendRefusal::NeighborsFull)
        );
        let close = stack.close(other).unwrap();
        assert_eq!(finished(stack.take_result(close)), Outcome::SocketClosed);
        assert!(stack.declare(at(0), &socket, third, 10, 9).is_ok());
    });
}

#[test]
fn a_produced_frame_is_written_in_the_slot_and_its_checksum_verifies() {
    world!(|stack| {
        knows_peer(stack);
        let dscp = Dscp::from_field(oer_portable_wire::U8::constant::<10>());
        let socket = stack.open_local(6000, dscp).unwrap();
        let work = stack.declare(at(1), &socket, peer(), 37, 10).unwrap();
        let calls = Cell::new(0);
        let given = produce_next(stack, at(1), writes(&calls)).unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(given, Some((work, 10)), "its one outcome, with its tag");
        assert_eq!(end_of(stack, &socket.ends()), None, "told once");
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
                .with(&ipv4_pseudo_header(
                    OUR_IP,
                    PEER_IP,
                    PROTOCOL_UDP,
                    udp.length
                ))
                .with(datagram)
                .verifies()
        );
    });
}

/// A producer's header and its filler, written as one pass with no read of
/// the filler, go out as written, under a checksum that verifies.
#[test]
fn a_header_and_filler_go_out_whole_under_a_checksum_that_verifies() {
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open_local(6000, best_effort()).unwrap();
        for length in [1, 2, 28, 29, 1472] {
            let work = stack.declare(at(1), &socket, peer(), length, 11).unwrap();
            let header: Vec<u8> = pattern(length.min(28));
            let fill = |_, _, data: &mut Payload<'_>| {
                data.put(&header).map_err(|_| Declined::Unwritable)?;
                data.repeat(0x5a, data.remaining())
                    .map_err(|_| Declined::Unwritable)
            };
            assert_eq!(produce_next(stack, at(1), fill).unwrap(), Some((work, 11)));
            let frame = stack.port_mut().sent.pop().unwrap();
            let (_, packet) = EthernetHeader::parse(&frame).unwrap();
            let ip = Ipv4Header::parse(packet).unwrap();
            let datagram = ip.payload();
            let (udp, data) = UdpHeader::parse(datagram).unwrap();
            let mut expected = header.clone();
            expected.resize(length, 0x5a);
            assert_eq!(data, expected, "{length} octets");
            assert!(
                Checksum::new()
                    .with(&ipv4_pseudo_header(
                        OUR_IP,
                        PEER_IP,
                        PROTOCOL_UDP,
                        udp.length
                    ))
                    .with(datagram)
                    .verifies(),
                "{length} octets"
            );
        }
    });
}

/// The load's datagram: a header of 28 octets, filler behind it.
fn header_and_filler(_: WorkId, _: u32, data: &mut Payload<'_>) -> Result<(), Declined> {
    data.put(&[0xa1; 28]).map_err(|_| Declined::Unwritable)?;
    data.repeat(0x5a, data.remaining())
        .map_err(|_| Declined::Unwritable)
}

/// The UDP datagram of `frame`, and whether its checksum verifies.
fn datagram_of(frame: &[u8]) -> (Vec<u8>, bool) {
    let (_, packet) = EthernetHeader::parse(frame).unwrap();
    let datagram = Ipv4Header::parse(packet).unwrap().payload();
    let (udp, _) = UdpHeader::parse(datagram).unwrap();
    let verifies = Checksum::new()
        .with(&ipv4_pseudo_header(
            OUR_IP,
            PEER_IP,
            PROTOCOL_UDP,
            udp.length,
        ))
        .with(datagram)
        .verifies();
    (datagram.to_vec(), verifies)
}

/// The filler a buffer holds from its last frame is kept, not written again:
/// the next frame of the same length goes whole under a checksum that
/// verifies, and the buffer holds the run again. What the buffer holds is
/// what goes: an octet changed there behind the port's back goes out as it
/// is, which the checksum, summed for the run, then fails.
#[test]
fn a_filler_run_the_buffer_holds_is_kept_and_not_written_again() {
    world!(|stack| {
        knows_peer(stack);
        stack.port_mut().keeps_buffer = true;
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let run = Held {
            at: 42 + 28,
            length: 72,
            octet: 0x5a,
        };
        let first_work = stack.declare(at(1), &socket, peer(), 100, 12).unwrap();
        assert_eq!(
            produce_next(stack, at(1), header_and_filler).unwrap(),
            Some((first_work, 12))
        );
        let (first, verifies) = datagram_of(&stack.port_mut().sent.pop().unwrap());
        assert!(verifies);
        assert_eq!(stack.port().buffer.as_ref().unwrap().1, Some(run));
        let second_work = stack.declare(at(1), &socket, peer(), 100, 13).unwrap();
        assert_eq!(
            produce_next(stack, at(1), header_and_filler).unwrap(),
            Some((second_work, 13))
        );
        let (second, verifies) = datagram_of(&stack.port_mut().sent.pop().unwrap());
        assert!(verifies, "the kept run goes as written");
        assert_eq!(second, first);
        assert_eq!(stack.port().buffer.as_ref().unwrap().1, Some(run));
        // The buffer changed behind the port's back, which still says it
        // holds the run: the run is not written again.
        stack.port_mut().buffer.as_mut().unwrap().0[100] = 0;
        let third_work = stack.declare(at(1), &socket, peer(), 100, 14).unwrap();
        assert_eq!(
            produce_next(stack, at(1), header_and_filler).unwrap(),
            Some((third_work, 14))
        );
        let (third, verifies) = datagram_of(&stack.port_mut().sent.pop().unwrap());
        assert_eq!(third[100 - 34], 0, "the octet goes as the buffer held it");
        assert!(!verifies);
    });
}

/// A longer frame writes what the run did not cover; any other frame in the
/// buffer leaves it holding no run.
#[test]
fn a_longer_frame_writes_past_the_run_and_another_frame_ends_it() {
    world!(|stack| {
        knows_peer(stack);
        stack.port_mut().keeps_buffer = true;
        let local = stack.open_local(6000, best_effort()).unwrap();
        let channel = stack.open(5000, best_effort()).unwrap();
        let short = stack.declare(at(1), &local, peer(), 40, 15).unwrap();
        assert_eq!(
            produce_next(stack, at(1), header_and_filler).unwrap(),
            Some((short, 15))
        );
        let _ = stack.port_mut().sent.pop();
        let long = stack.declare(at(1), &local, peer(), 300, 16).unwrap();
        assert_eq!(
            produce_next(stack, at(1), header_and_filler).unwrap(),
            Some((long, 16))
        );
        let (long_datagram, verifies) = datagram_of(&stack.port_mut().sent.pop().unwrap());
        assert!(verifies);
        assert!(long_datagram[8 + 28..].iter().all(|octet| *octet == 0x5a));
        let block = filled(stack.pool(), b"other");
        let _sent = stack.send(at(1), &channel, peer(), block).unwrap();
        let demand = stack.poll_demand(at(1)).unwrap();
        stack
            .transmit(at(1), demand.grant(), unproduced, &())
            .unwrap();
        assert_eq!(stack.port().buffer.as_ref().unwrap().1, None);
    });
}

/// A fill that leaves part of its data unwritten is declined: its work ends
/// declined and no frame goes, so nothing an earlier use of the slot left
/// goes out.
#[test]
fn a_fill_that_writes_part_of_its_data_is_declined_and_sends_nothing() {
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let work = stack.declare(at(1), &socket, peer(), 37, 17).unwrap();
        let short =
            |_, _, data: &mut Payload<'_>| data.put(&[1, 2, 3]).map_err(|_| Declined::Unwritable);
        assert_eq!(produce_next(stack, at(1), short), Err(Unsent::Declined));
        assert_eq!(
            end_of(stack, &socket.ends()),
            Some((work, 17, Outcome::Declined))
        );
        assert!(stack.port().sent.is_empty());
        assert!(!stack.port_broken());
    });
}

#[test]
fn a_declared_work_waits_for_its_neighbor_without_a_slot() {
    world!(|stack| {
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let work = stack.declare(at(0), &socket, peer(), 8, 18).unwrap();
        let demand = stack.poll_demand(at(0)).unwrap();
        assert_eq!(demand.ready().class, Class::Control, "its neighbor first");
        let calls = Cell::new(0);
        stack
            .transmit(at(0), demand.grant(), writes(&calls), &())
            .unwrap();
        assert_eq!(calls.get(), 0, "an ARP request fills no data");
        assert!(stack.poll_demand(at(0)).is_none(), "no slot during ARP");
        assert_eq!(end_of(stack, &socket.ends()), None, "it runs");
        let reply = arp_frame(Operation::Reply, (PEER, PEER_IP), OUR_IP);
        stack.input(at(1), &reply, &mut ());
        let given = produce_next(stack, at(1), writes(&calls)).unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(given, Some((work, 18)));
    });
}

#[test]
fn a_refusal_before_the_fill_never_calls_it_and_the_work_stays() {
    world!(|stack| {
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let work = stack.declare(at(0), &socket, broadcast(), 8, 19).unwrap();
        stack.port_mut().full = true;
        let calls = Cell::new(0);
        assert_eq!(
            produce_next(stack, at(0), writes(&calls)),
            Err(Unsent::Port("full"))
        );
        assert_eq!(calls.get(), 0);
        assert_eq!(end_of(stack, &socket.ends()), None, "it stays");
        stack.port_mut().full = false;
        let given = produce_next(stack, at(0), writes(&calls)).unwrap();
        assert_eq!(calls.get(), 1, "one fill");
        assert_eq!(given, Some((work, 19)));
    });
}

/// A declining fill ends its work `Declined`, never `Rejected`, and the next
/// socket's work goes first.
#[test]
fn a_declining_fill_ends_its_work_declined_and_passes_the_turn() {
    world!(|stack| {
        let local = stack.open_local(6000, best_effort()).unwrap();
        let channel = stack.open(5000, best_effort()).unwrap();
        let first = stack.declare(at(0), &local, broadcast(), 8, 20).unwrap();
        let second = stack.declare(at(0), &local, broadcast(), 8, 21).unwrap();
        let block = filled(stack.pool(), b"other");
        let other = stack.send(at(0), &channel, broadcast(), block).unwrap();
        let calls = Cell::new(0);
        assert_eq!(
            produce_next(stack, at(0), declines(&calls)),
            Err(Unsent::Declined)
        );
        assert_eq!(calls.get(), 1);
        assert_eq!(
            end_of(stack, &local.ends()),
            Some((first, 20, Outcome::Declined))
        );
        assert!(!stack.port_broken());
        let next = stack.poll_demand(at(0)).unwrap();
        assert_eq!(
            next.ready().flow,
            Some(channel.flow()),
            "the other socket goes first"
        );
        stack
            .transmit(at(0), next.grant(), unproduced, &())
            .unwrap();
        assert_eq!(finished(stack.take_result(other)), Outcome::Accepted);
        assert_eq!(
            produce_next(stack, at(0), writes(&calls)).unwrap(),
            Some((second, 21))
        );
        assert_eq!(stack.port().sent.len(), 2);
    });
}

#[test]
fn a_yes_after_a_declined_fill_is_unknown_and_the_port_s_fault() {
    world!(|stack| {
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let work = stack.declare(at(0), &socket, broadcast(), 8, 22).unwrap();
        stack.port_mut().careless = true;
        let calls = Cell::new(0);
        assert_eq!(
            produce_next(stack, at(0), declines(&calls)),
            Err(Unsent::PortContract)
        );
        assert_eq!(
            end_of(stack, &socket.ends()),
            Some((work, 22, Outcome::Unknown)),
            "an unwritten frame the port took is no accepted work"
        );
        assert!(stack.port_broken());
        assert!(stack.poll_demand(at(0)).is_none(), "never again");
    });
}

#[test]
fn a_port_refusal_after_a_declined_fill_ends_it_declined_and_is_the_port_s_fault() {
    world!(|stack| {
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let work = stack.declare(at(0), &socket, broadcast(), 8, 23).unwrap();
        stack.port_mut().misreports = true;
        let calls = Cell::new(0);
        assert_eq!(
            produce_next(stack, at(0), declines(&calls)),
            Err(Unsent::FillMisreported("misreported"))
        );
        assert_eq!(
            end_of(stack, &socket.ends()),
            Some((work, 23, Outcome::Declined)),
            "the fill's recorded cause"
        );
        assert!(stack.port_broken());
    });
}

#[test]
fn a_slot_too_short_for_a_produced_frame_ends_it_rejected_unfilled() {
    world!(|stack| {
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let work = stack.declare(at(0), &socket, broadcast(), 64, 24).unwrap();
        stack.port_mut().room = Some(30);
        let calls = Cell::new(0);
        assert_eq!(
            produce_next(stack, at(0), writes(&calls)),
            Err(Unsent::SlotTooShort)
        );
        assert_eq!(calls.get(), 0, "no room for its headers: no fill");
        assert_eq!(
            end_of(stack, &socket.ends()),
            Some((work, 24, Outcome::Rejected))
        );
        assert!(!stack.port_broken());
    });
}

#[test]
fn withdrawal_ends_a_declared_work_declined_without_the_port() {
    world!(|stack| {
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let work = stack.declare(at(0), &socket, broadcast(), 8, 25).unwrap();
        let keep_others = |tag: &u32| *tag != 25;
        assert_eq!(stack.withdraw_declared(&socket, keep_others), 1);
        assert_eq!(stack.withdraw_declared(&socket, keep_others), 0, "once");
        assert!(stack.poll_demand(at(0)).is_none());
        assert_eq!(
            end_of(stack, &socket.ends()),
            Some((work, 25, Outcome::Declined))
        );
        assert!(stack.port().sent.is_empty());
    });
}

#[test]
fn a_close_ends_declared_work_and_its_stale_grant_fills_nothing() {
    world!(|stack| {
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let work = stack.declare(at(0), &socket, broadcast(), 8, 26).unwrap();
        let stale = stack.poll_demand(at(0)).unwrap().grant();
        let ends = socket.ends();
        let close = stack.close(socket).unwrap();
        let calls = Cell::new(0);
        assert_eq!(
            stack.transmit(at(0), stale, writes(&calls), &()),
            Err(Unsent::Stale)
        );
        assert_eq!(calls.get(), 0, "no fill");
        assert_eq!(end_of(stack, &ends), Some((work, 26, Outcome::Closed)));
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
    let work = stack.declare(at(0), &socket, broadcast(), 8, 27).unwrap();
    let mut retired = stack.retire();
    let results: Vec<_> = retired.results.take_declared(&socket).collect();
    assert_eq!(results, [(work, 27, Outcome::Retired)]);
    assert_eq!(retired.results.take_declared(&socket).count(), 0, "once");
    assert_eq!(retired.pool.outstanding(), 0);
}

/// A work its port took ended in its publication: a retirement after it
/// keeps only the works still open, each once.
#[test]
fn a_retired_core_keeps_only_the_works_no_publication_ended() {
    brand!(Net);
    let (pool, brand) = claimed::<Net>();
    let mut stack: Stack<Net> =
        NetworkOwner::new(brand, interface(OUR_IP, 24, None), pool, Port::default());
    let socket = stack.open_local(6000, best_effort()).unwrap();
    let sent = stack.declare(at(0), &socket, broadcast(), 8, 40).unwrap();
    let open = stack.declare(at(0), &socket, broadcast(), 8, 41).unwrap();
    let calls = Cell::new(0);
    assert_eq!(
        produce_next(&mut stack, at(0), writes(&calls)).unwrap(),
        Some((sent, 40))
    );
    let mut retired = stack.retire();
    let results: Vec<_> = retired.results.take_declared(&socket).collect();
    assert_eq!(results, [(open, 41, Outcome::Retired)]);
}

#[test]
fn two_open_works_never_share_an_identity() {
    world!(|stack| {
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let first = stack.declare(at(0), &socket, broadcast(), 8, 28).unwrap();
        let second = stack.declare(at(0), &socket, broadcast(), 8, 29).unwrap();
        assert_ne!(first, second);
        assert_eq!(stack.withdraw_declared(&socket, |tag| *tag != 28), 1);
        assert_eq!(
            end_of(stack, &socket.ends()),
            Some((first, 28, Outcome::Declined))
        );
        let reused = stack.declare(at(0), &socket, broadcast(), 8, 30).unwrap();
        assert_eq!(reused.place(), first.place(), "the same place");
        assert_ne!(reused, first, "another work");
        assert_ne!(reused, second);
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
        let work = stack.declare(at(0), &socket, peer(), 8, 31).unwrap();
        send_next(stack, at(0)).unwrap();
        let reply = arp_frame(Operation::Reply, (PEER, PEER_IP), OUR_IP);
        stack.input(at(1), &reply, &mut ());
        moved(stack, "an answer");
        // A commit.
        let calls = Cell::new(0);
        produce_next(stack, at(1), writes(&calls)).unwrap();
        moved(stack, "a commit");
        let _ = work;
        // A decline.
        let work = stack.declare(at(1), &socket, peer(), 8, 32).unwrap();
        assert_eq!(
            produce_next(stack, at(1), declines(&calls)),
            Err(Unsent::Declined)
        );
        moved(stack, "a decline");
        assert_eq!(end_of(stack, &socket.ends()).map(|end| end.0), Some(work));
        // A withdrawal.
        let work = stack.declare(at(1), &socket, peer(), 8, 33).unwrap();
        assert_eq!(stack.withdraw_declared(&socket, |tag| *tag != 33), 1);
        moved(stack, "a withdrawal");
        assert_eq!(end_of(stack, &socket.ends()).map(|end| end.0), Some(work));
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
        let work = stack.declare(at(1), &socket, peer(), 8, 34).unwrap();
        let ends = socket.ends();
        let close = stack.close(socket).unwrap();
        moved(stack, "a close");
        assert_eq!(end_of(stack, &ends).map(|end| end.0), Some(work));
        let _ = stack.take_result(close);
        // A resolution given up.
        let socket = stack.open_local(6001, best_effort()).unwrap();
        let far = Remote {
            address: [192, 0, 2, 9],
            port: 7,
        };
        let work = stack.declare(at(2), &socket, far, 8, 35).unwrap();
        let mut now = at(2);
        while let Some(demand) = stack.poll_demand(now) {
            stack
                .transmit(now, demand.grant(), unproduced, &())
                .unwrap();
            now = stack.poll_deadline().unwrap();
            stack.tick(now, &mut ());
        }
        assert_eq!(
            end_of(stack, &socket.ends()),
            Some((work, 35, Outcome::Unreachable))
        );
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
        let work = stack.declare(at(0), &socket, broadcast(), 8, 36).unwrap();
        let calls = Cell::new(0);
        assert_eq!(
            produce_next(stack, at(0), writes(&calls)).unwrap(),
            Some((work, 36))
        );
        assert_eq!(stack.neighbors_changed(), before, "broadcast held no entry");
    });
}
