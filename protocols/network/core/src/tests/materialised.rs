//! A stored datagram's frame: its data copied from the block into the slot
//! and summed in that one pass, and what its demand found of the next hop
//! trusted while it holds, looked up again once anything known moved.

use super::*;

/// The data of a test datagram of `length` octets, none alike in a row.
fn octets(length: usize) -> Vec<u8> {
    (0..length)
        .map(|at| (at * 37 + 11 + (at >> 8)) as u8)
        .collect()
}

/// The UDP segment and data of `frame`, a datagram to the peer from us,
/// with the checksum its octets give, computed apart from the core.
fn segment(frame: &[u8]) -> (UdpHeader, Vec<u8>, u16) {
    let (_, payload) = EthernetHeader::parse(frame).unwrap();
    let packet = Ipv4Header::parse(payload).unwrap();
    let (udp, data) = UdpHeader::parse(packet.payload()).unwrap();
    let pseudo = ipv4_pseudo_header(OUR_IP, PEER_IP, PROTOCOL_UDP, udp.length);
    let unfilled = UdpHeader { checksum: 0, ..udp };
    let expected = udp_field(
        Checksum::new()
            .with(&pseudo)
            .with(&unfilled.to_bytes())
            .with(data)
            .finish(),
    );
    assert!(
        Checksum::new()
            .with(&pseudo)
            .with(packet.payload())
            .verifies()
    );
    (udp, data.to_vec(), expected)
}

/// Every length, odd ones and the largest the interface takes included: the frame holds the
/// block's octets and the checksum of exactly them.
#[test]
fn a_stored_datagram_is_copied_and_summed_at_every_length() {
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        for length in (0..=41).chain([1471, 1472]) {
            let data = octets(length);
            let block = filled(stack.pool(), &data);
            let command = stack.send(at(1), &socket, peer(), block).unwrap();
            send_next(stack, at(1)).unwrap();
            assert_eq!(finished(stack.take_result(command)), Outcome::Accepted);
            let frame = stack.port_mut().sent.pop().unwrap();
            assert_eq!(frame.len(), 14 + 20 + 8 + length, "{length}");
            let (udp, sent, expected) = segment(&frame);
            assert_eq!(sent, data, "{length}");
            assert_eq!(udp.checksum, expected, "{length}");
        }
        assert_eq!(stack.pool().free(), BLOCKS);
    });
}

/// A slot one octet short of a stored datagram ends it `Rejected`, with
/// its block home, as a short slot did before the copy was summed.
#[test]
fn a_slot_short_of_a_stored_datagram_rejects_it() {
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), &octets(9));
        let command = stack.send(at(1), &socket, peer(), block).unwrap();
        stack.port_mut().room = Some(14 + 20 + 8 + 8);
        assert_eq!(send_next(stack, at(1)), Err(Unsent::SlotTooShort));
        assert_eq!(finished(stack.take_result(command)), Outcome::Rejected);
        assert_eq!(stack.pool().free(), BLOCKS);
    });
}

/// Another address learned between the choice and the frame moves the
/// table: the frame looks again and still finds its peer where the demand
/// did, so it goes.
#[test]
fn a_table_that_moved_elsewhere_is_looked_at_again_and_the_frame_goes() {
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"chosen");
        let command = stack.send(at(1), &socket, peer(), block).unwrap();
        let chosen = stack.poll_demand(at(1)).unwrap().grant();
        let other = arp_frame(Operation::Reply, (MOVED, [192, 0, 2, 3]), OUR_IP);
        stack.input(at(1), &other, &mut ());
        stack.transmit(at(1), chosen, unproduced).unwrap();
        let (header, _) = EthernetHeader::parse(&stack.port_mut().sent[0]).unwrap();
        assert_eq!(header.destination, PEER);
        assert_eq!(finished(stack.take_result(command)), Outcome::Accepted);
    });
}

/// The peer heard again at the same address: the table moved, the look
/// finds the same station, and the frame goes there.
#[test]
fn a_neighbor_heard_again_keeps_its_grant() {
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"chosen");
        let command = stack.send(at(1), &socket, peer(), block).unwrap();
        let chosen = stack.poll_demand(at(1)).unwrap().grant();
        let again = arp_frame(Operation::Reply, (PEER, PEER_IP), OUR_IP);
        stack.input(at(2), &again, &mut ());
        stack.transmit(at(2), chosen, unproduced).unwrap();
        assert_eq!(finished(stack.take_result(command)), Outcome::Accepted);
    });
}
