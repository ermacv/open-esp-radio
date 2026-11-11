//! A socket's next datagrams to the remote its last one went to take what
//! that one prepared: the route, the neighbor's address and the headers'
//! fixed octets. Each frame is checked here as a frame, whole, against the
//! fields it carries; and each dependency the preparation holds on, the
//! neighbor's address, its expiry, the remote and the socket, moved, takes
//! the next datagram through the whole path again.

use super::*;

/// What a datagram's frame carries, read back from its octets: its station
/// address, its ports, its marking and its data, every checksum verified.
fn read(frame: &[u8]) -> ([u8; 6], (u16, u16), Dscp, Vec<u8>) {
    let (ethernet, payload) = EthernetHeader::parse(frame).unwrap();
    assert_eq!(ethernet.source, LOCAL);
    let packet = Ipv4Header::parse(payload).unwrap();
    assert!(Checksum::new().with(&payload[..20]).verifies(), "IPv4 sum");
    let (udp, data) = UdpHeader::parse(packet.payload()).unwrap();
    let pseudo = ipv4_pseudo_header(OUR_IP, PEER_IP, PROTOCOL_UDP, udp.length);
    assert!(
        Checksum::new()
            .with(&pseudo)
            .with(packet.payload())
            .verifies(),
        "UDP sum"
    );
    (
        ethernet.destination,
        (udp.ports.source, udp.ports.destination),
        packet.dscp(),
        data.to_vec(),
    )
}

/// Send `data` from `socket` to `remote` and take its frame.
fn sent<B: core::fmt::Debug>(
    stack: &mut Stack<B>,
    now: Instant,
    socket: &crate::SocketId<B>,
    remote: Remote,
    data: &[u8],
) -> Vec<u8> {
    let block = filled(stack.pool(), data);
    let command = stack.send(now, socket, remote, block).unwrap();
    send_next(stack, now).unwrap();
    assert_eq!(finished(stack.take_result(command)), Outcome::Accepted);
    stack.port_mut().sent.pop().unwrap()
}

/// The first datagram prepares; the ones after it carry the same headers
/// but their own lengths and data, each checksum right.
#[test]
fn datagrams_after_the_first_are_whole_from_what_it_prepared() {
    world!(|stack| {
        knows_peer(stack);
        let dscp = Dscp::new(46).unwrap();
        let socket = stack.open(5000, dscp).unwrap();
        for (now, data) in [(1, &b"first"[..]), (2, b"a longer second"), (3, b"3")] {
            let frame = sent(stack, at(now), &socket, peer(), data);
            assert_eq!(read(&frame), (PEER, (5000, 7), dscp, data.to_vec()));
        }
        assert_eq!(stack.pool().free(), BLOCKS);
    });
}

/// A neighbor whose station address moves takes the next datagram to its
/// new address, not to what was prepared.
#[test]
fn a_neighbor_that_moves_takes_the_next_datagram_to_its_new_address() {
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        let frame = sent(stack, at(1), &socket, peer(), b"before");
        assert_eq!(read(&frame).0, PEER);
        let moved = arp_frame(Operation::Reply, (MOVED, PEER_IP), OUR_IP);
        stack.input(at(2), &moved, &mut ());
        let frame = sent(stack, at(3), &socket, peer(), b"after");
        assert_eq!(read(&frame).0, MOVED);
    });
}

/// A neighbor that expired is asked for again before the next datagram
/// goes: what was prepared through it holds no more.
#[test]
fn a_neighbor_that_expired_is_asked_again_before_the_next_datagram() {
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        let _frame = sent(stack, at(1), &socket, peer(), b"before");
        let later = at(0).saturating_add(Duration::from_secs(61));
        let block = filled(stack.pool(), b"after");
        let _command = stack.send(later, &socket, peer(), block).unwrap();
        assert_eq!(
            stack.poll_demand(later).unwrap().ready().class,
            Class::Control,
            "an ARP request goes first"
        );
    });
}

/// Another remote takes the whole path and is prepared for in its turn;
/// the first is served again after it.
#[test]
fn another_remote_prepares_for_itself() {
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        let other = Remote { port: 9, ..peer() };
        for (now, remote, port) in [(1, peer(), 7), (2, other, 9), (3, other, 9), (4, peer(), 7)] {
            let frame = sent(stack, at(now), &socket, remote, b"x");
            assert_eq!(read(&frame).1, (5000, port));
        }
    });
}

/// A socket opened again in the place of a closed one sends with its own
/// port and marking, nothing the closed one prepared.
#[test]
fn a_socket_opened_again_sends_with_its_own_port_and_marking() {
    world!(|stack| {
        knows_peer(stack);
        let dscp = Dscp::new(46).unwrap();
        let first = stack.open(5000, dscp).unwrap();
        let _frame = sent(stack, at(1), &first, peer(), b"first");
        let close = stack.close(first).unwrap();
        let _closed = finished(stack.take_result(close));
        let second = stack.open(5001, best_effort()).unwrap();
        let frame = sent(stack, at(2), &second, peer(), b"second");
        assert_eq!(read(&frame).1, (5001, 7));
        assert_eq!(read(&frame).2, best_effort());
    });
}
