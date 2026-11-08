//! ICMP echo through the owner's public calls. Ported from Xarxa fbebde5's
//! echo tests (`tests/echo_backpressure.rs`, `stack.rs`) where they fit:
//! here a reply holds a block while it waits, and its bounds are counted.

use super::*;
use crate::{ECHO_RATE, ECHOES, IcmpDrop};
use oer_protocols_network_wire::icmp::{Echo, MessageType, PROTOCOL_ICMP};

/// An ICMP message from the peer to `to`, in an Ethernet frame.
pub(super) fn icmp_frame(to: [u8; 4], message: &[u8]) -> Vec<u8> {
    let mut frame = EthernetHeader {
        destination: LOCAL,
        source: PEER,
        ether_type: EtherType::IPV4,
    }
    .to_bytes()
    .to_vec();
    frame.extend_from_slice(
        &Ipv4Fields {
            services: DsField::from_bytes([0]),
            total_length: u16::try_from(20 + message.len()).unwrap(),
            identification: 9,
            time_to_live: 64,
            protocol: PROTOCOL_ICMP,
            source: PEER_IP,
            destination: to,
        }
        .to_bytes(),
    );
    frame.extend_from_slice(message);
    frame
}

/// An echo of `sequence` with `data`, its checksum right.
pub(super) fn echo(sequence: u16, data: &[u8]) -> Vec<u8> {
    let request = Echo {
        kind: MessageType::Echo,
        code: 0,
        checksum: 0,
        identifier: 17,
        sequence,
        data,
    };
    let mut message = request.header(MessageType::Echo).to_vec();
    message.extend_from_slice(data);
    message
}

fn ask<B>(stack: &mut Stack<B>, now: Instant, message: &[u8]) -> Vec<NetEvent> {
    let mut dropped = Dropped::default();
    stack.input(now, &icmp_frame(OUR_IP, message), &mut dropped);
    dropped.0
}

/// Every ready work sent; what went.
fn flush<B>(stack: &mut Stack<B>, now: Instant) -> Vec<Vec<u8>> {
    let before = stack.port().sent.len();
    while let Some(demand) = stack.poll_demand(now) {
        stack.transmit(now, demand.grant(), unproduced).unwrap();
    }
    stack.port().sent[before..].to_vec()
}

#[test]
fn an_echo_is_answered_from_the_address_it_was_sent_to_with_all_its_data() {
    world!(|stack| {
        knows_peer(stack);
        let free = stack.pool().free();
        let data: Vec<u8> = (0..=255).collect();
        assert!(ask(stack, at(0), &echo(7, &data)).is_empty());
        assert_eq!(stack.pool().free(), free - 1, "the reply holds a block");
        let sent = flush(stack, at(0));
        assert_eq!(sent.len(), 1);
        let frame = &sent[0];
        let (ethernet, payload) = EthernetHeader::parse(frame).unwrap();
        assert_eq!((ethernet.destination, ethernet.source), (PEER, LOCAL));
        let ip = Ipv4Header::parse(payload).unwrap();
        assert!(Checksum::new().with(&payload[..20]).verifies());
        assert_eq!(ip.protocol(), PROTOCOL_ICMP);
        assert_eq!(ip.addresses(), [192, 0, 2, 1, 192, 0, 2, 2]);
        assert!(Checksum::new().with(ip.payload()).verifies());
        let reply = Echo::parse(ip.payload()).unwrap();
        assert_eq!(reply.kind, MessageType::EchoReply);
        assert_eq!(reply.code, 0);
        assert_eq!((reply.identifier, reply.sequence), (17, 7));
        assert_eq!(reply.data, &data[..]);
        assert_eq!(stack.pool().free(), free, "its block is home");
    });
}

#[test]
fn an_echo_to_an_unknown_peer_waits_for_its_neighbor() {
    world!(|stack| {
        assert!(ask(stack, at(0), &echo(1, b"ping")).is_empty());
        let sent = flush(stack, at(0));
        assert_eq!(sent.len(), 1);
        assert_eq!(&sent[0][12..14], &[8, 6], "an ARP request first");
        let reply = arp_frame(Operation::Reply, (PEER, PEER_IP), OUR_IP);
        stack.input(at(1), &reply, &mut ());
        let sent = flush(stack, at(1));
        assert_eq!(sent.len(), 1);
        assert_eq!(Echo::parse(&sent[0][34..]).unwrap().sequence, 1);
    });
}

#[test]
fn an_unreachable_asker_gets_nothing_and_the_block_comes_home() {
    world!(|stack| {
        let free = stack.pool().free();
        assert!(ask(stack, at(0), &echo(1, b"ping")).is_empty());
        let mut dropped = Dropped::default();
        for second in 0..5 {
            stack.tick(secs_of(second), &mut dropped);
            flush(stack, secs_of(second));
        }
        assert!(dropped.0.contains(&NetEvent::Icmp(IcmpDrop::Unreachable)));
        assert_eq!(stack.pool().free(), free);
        assert_eq!(stack.poll_demand(secs_of(5)), None);
    });
}

fn secs_of(seconds: u32) -> Instant {
    at(seconds * 1000)
}

#[test]
fn malformed_foreign_and_broadcast_icmp_is_counted_and_never_answered() {
    world!(|stack| {
        knows_peer(stack);
        let mut bad_checksum = echo(1, b"ping");
        bad_checksum[2] ^= 1;
        let mut coded = echo(1, b"ping");
        coded[1] = 1;
        // Recompute: a right checksum over a wrong code.
        coded[2..4].copy_from_slice(&[0, 0]);
        let sum = Checksum::new().with(&coded).finish();
        coded[2..4].copy_from_slice(&sum.to_be_bytes());
        let reply = {
            let mut reply = Echo::parse(&echo(1, b"ping"))
                .unwrap()
                .header(MessageType::EchoReply)
                .to_vec();
            reply.extend_from_slice(b"ping");
            reply
        };
        let cases: [(&[u8], IcmpDrop); 4] = [
            (&[8, 0, 0], IcmpDrop::Malformed),
            (&bad_checksum, IcmpDrop::Checksum),
            (&coded, IcmpDrop::Malformed),
            (&reply, IcmpDrop::Other),
        ];
        for (message, drop) in cases {
            assert_eq!(
                ask(stack, at(0), message),
                [NetEvent::Icmp(drop)],
                "{drop:?}"
            );
        }
        for broadcast in [[255; 4], [192, 0, 2, 255]] {
            let mut dropped = Dropped::default();
            stack.input(
                at(0),
                &icmp_frame(broadcast, &echo(1, b"ping")),
                &mut dropped,
            );
            assert_eq!(dropped.0, [NetEvent::Icmp(IcmpDrop::Broadcast)]);
        }
        // To another host: not ours, not counted.
        let mut dropped = Dropped::default();
        stack.input(
            at(0),
            &icmp_frame([192, 0, 2, 9], &echo(1, b"ping")),
            &mut dropped,
        );
        assert!(dropped.0.is_empty());
        assert_eq!(stack.poll_demand(at(0)), None);
    });
}

#[test]
fn an_echo_longer_than_a_block_is_dropped() {
    world!(|stack| {
        knows_peer(stack);
        let data = vec![0x5A; SIZE - 8 + 1];
        assert_eq!(
            ask(stack, at(0), &echo(1, &data)),
            [NetEvent::Icmp(IcmpDrop::TooLong)]
        );
        let data = vec![0x5A; SIZE - 8];
        assert!(ask(stack, at(0), &echo(1, &data)).is_empty());
        let sent = flush(stack, at(0));
        assert_eq!(Echo::parse(&sent[0][34..]).unwrap().data, &data[..]);
    });
}

#[test]
fn at_most_echoes_replies_are_owed_and_the_rest_are_busy() {
    world!(|stack| {
        // Unknown peer: replies wait.
        for sequence in 0..u16::try_from(ECHOES).unwrap() {
            assert!(ask(stack, at(0), &echo(sequence, b"ping")).is_empty());
        }
        assert_eq!(
            ask(stack, at(0), &echo(99, b"ping")),
            [NetEvent::Icmp(IcmpDrop::Busy)]
        );
        let reply = arp_frame(Operation::Reply, (PEER, PEER_IP), OUR_IP);
        stack.input(at(1), &reply, &mut ());
        let sequences: Vec<u16> = flush(stack, at(1))
            .iter()
            .filter(|frame| frame.len() > 42)
            .map(|frame| Echo::parse(&frame[34..]).unwrap().sequence)
            .collect();
        assert_eq!(sequences, [0, 1, 2, 3], "oldest first");
    });
}

#[test]
fn at_most_echo_rate_replies_a_second() {
    world!(|stack| {
        knows_peer(stack);
        let mut answered = 0;
        for sequence in 0..u16::from(ECHO_RATE) + 4 {
            let dropped = ask(stack, at(100), &echo(sequence, b"ping"));
            if dropped.is_empty() {
                answered += 1;
            } else {
                assert_eq!(dropped, [NetEvent::Icmp(IcmpDrop::Limited)]);
            }
            flush(stack, at(100));
        }
        assert_eq!(answered, ECHO_RATE);
        // A second later, the window is new.
        assert!(ask(stack, at(1100), &echo(1, b"ping")).is_empty());
    });
}

#[test]
fn an_echo_never_takes_the_pools_last_free_block() {
    world!(|stack| {
        knows_peer(stack);
        let mut held = Vec::new();
        while stack.pool().free() > 1 {
            held.push(stack.pool().take().unwrap());
        }
        assert_eq!(
            ask(stack, at(0), &echo(1, b"ping")),
            [NetEvent::Icmp(IcmpDrop::NoBlock)]
        );
        assert_eq!(stack.pool().free(), 1);
        for block in held {
            stack.pool().give(block);
        }
    });
}

#[test]
fn a_reply_the_port_refuses_waits_and_retirement_brings_its_block_home() {
    world!(|stack| {
        knows_peer(stack);
        assert!(ask(stack, at(0), &echo(1, b"ping")).is_empty());
        stack.port_mut().full = true;
        let demand = stack.poll_demand(at(0)).unwrap();
        assert!(stack.transmit(at(0), demand.grant(), unproduced).is_err());
        assert!(stack.poll_demand(at(0)).is_some(), "it stays ready");
    });
    // Retirement: the owed reply's block goes home with the rest.
    brand!(Net);
    let (pool, brand) = claimed::<Net>();
    let mut stack: Stack<Net> =
        NetworkOwner::new(brand, interface(OUR_IP, 24, None), pool, Port::default());
    knows_peer(&mut stack);
    assert!(ask(&mut stack, at(0), &echo(1, b"ping")).is_empty());
    let retired = stack.retire();
    assert_eq!(retired.pool.free(), BLOCKS);
}

#[test]
fn an_interface_without_an_address_answers_no_echo() {
    brand!(Net);
    run::<Net>(Interface::dhcp(LOCAL, 1500, 1).unwrap(), |stack| {
        // No address yet: an echo to any address is no one's.
        let mut dropped = Dropped::default();
        stack.input(at(0), &icmp_frame(OUR_IP, &echo(1, b"ping")), &mut dropped);
        assert!(dropped.0.is_empty());
        assert_eq!(stack.poll_demand(at(0)), None);
    });
}
