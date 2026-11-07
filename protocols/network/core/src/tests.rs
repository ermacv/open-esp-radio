extern crate std;

use std::vec::Vec;

use oer_portable_events::EventSink;
use oer_portable_time::{Duration, Instant};
use oer_protocols_network_wire::arp::{self, Arp, Operation};
use oer_protocols_network_wire::checksum::{Checksum, ipv4_pseudo_header, udp_field};
use oer_protocols_network_wire::ethernet::{EtherType, EthernetHeader};
use oer_protocols_network_wire::ip::{DsField, Ipv4Fields, Ipv4Header};
use oer_protocols_network_wire::transport::{PROTOCOL_UDP, Ports, UdpHeader};

use crate::{
    Class, Core, Demand, Interface, InterfaceError, NetEvent, OpenRefused, Outcome, Pool, Remote,
    SendRefusal, Unmaterialized,
};

const SIZE: usize = 1536;
const BLOCKS: usize = 8;
const LOCAL: [u8; 6] = [2, 0, 0, 0, 0, 1];
const PEER: [u8; 6] = [2, 0, 0, 0, 0, 2];
const OUR_IP: [u8; 4] = [192, 0, 2, 1];
const PEER_IP: [u8; 4] = [192, 0, 2, 2];
const GATEWAY: [u8; 4] = [192, 0, 2, 254];

type Stack<'m> = Core<'m, SIZE, 2, 2, 2, 2>;

#[derive(Default)]
struct Dropped(Vec<NetEvent>);

impl EventSink<NetEvent> for Dropped {
    fn event(&mut self, event: NetEvent) {
        self.0.push(event);
    }
}

fn at(millis: u32) -> Instant {
    Instant::from_micros(u64::from(millis) * 1000)
}

fn stack<'m>() -> Stack<'m> {
    Core::new(Interface::new(LOCAL, OUR_IP, 24, Some(GATEWAY), 1500).unwrap())
}

fn peer() -> Remote {
    Remote {
        address: PEER_IP,
        port: 7,
    }
}

fn arp_frame(operation: Operation, sender: ([u8; 6], [u8; 4]), target: [u8; 4]) -> Vec<u8> {
    let mut frame = EthernetHeader {
        destination: if operation == Operation::Request {
            [0xFF; 6]
        } else {
            LOCAL
        },
        source: sender.0,
        ether_type: EtherType::ARP,
    }
    .to_bytes()
    .to_vec();
    let mut packet = [0; arp::PACKET_LEN];
    Arp {
        operation,
        sender_hardware: sender.0,
        sender_protocol: sender.1,
        target_hardware: LOCAL,
        target_protocol: target,
    }
    .write(&mut packet)
    .unwrap();
    frame.extend_from_slice(&packet);
    frame
}

fn udp_frame(source_port: u16, destination_port: u16, data: &[u8]) -> Vec<u8> {
    let length = u16::try_from(8 + data.len()).unwrap();
    let unfilled = UdpHeader {
        ports: Ports {
            source: source_port,
            destination: destination_port,
        },
        length,
        checksum: 0,
    };
    let pseudo = ipv4_pseudo_header(PEER_IP, OUR_IP, PROTOCOL_UDP, length);
    let checksum = udp_field(
        Checksum::new()
            .with(&pseudo)
            .with(&unfilled.to_bytes())
            .with(data)
            .finish(),
    );
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
            total_length: length + 20,
            identification: 1,
            time_to_live: 64,
            protocol: PROTOCOL_UDP,
            source: PEER_IP,
            destination: OUR_IP,
        }
        .to_bytes(),
    );
    frame.extend_from_slice(
        &UdpHeader {
            checksum,
            ..unfilled
        }
        .to_bytes(),
    );
    frame.extend_from_slice(data);
    frame
}

/// What a storage for the tests takes.
fn storage() -> std::boxed::Box<[[u8; SIZE]; BLOCKS]> {
    std::boxed::Box::new([[0; SIZE]; BLOCKS])
}

/// Serve the head demand as the port would, answering `accept`.
fn serve<'m>(
    stack: &mut Stack<'m>,
    pool: &mut Pool<'m, SIZE, BLOCKS>,
    now: Instant,
    demand: Demand,
    accept: bool,
) -> Vec<u8> {
    let mut slot = [0; SIZE];
    let wanted = demand.length;
    let done = stack.materialize(demand.grant(), &mut slot, pool).unwrap();
    assert_eq!(done.length, wanted);
    let length = done.length;
    if accept {
        done.accepted(now);
    } else {
        done.refused();
    }
    slot[..length].to_vec()
}

fn filled<'m>(pool: &mut Pool<'m, SIZE, BLOCKS>, data: &[u8]) -> crate::Block<'m, SIZE> {
    let mut block = pool.take().unwrap();
    block.fill(data).unwrap();
    block
}

/// Learn the peer through its own request for us.
fn knows_peer<'m>(stack: &mut Stack<'m>, pool: &mut Pool<'m, SIZE, BLOCKS>) {
    let request = arp_frame(Operation::Request, (PEER, PEER_IP), OUR_IP);
    stack.input(at(0), &request, pool, &mut ());
    let demand = stack.poll_demand().unwrap();
    serve(stack, pool, at(0), demand, true);
}

#[test]
fn an_interface_refuses_what_cannot_be() {
    assert_eq!(
        Interface::new(LOCAL, OUR_IP, 33, None, 1500),
        Err(InterfaceError::Prefix(33))
    );
    assert_eq!(
        Interface::new(LOCAL, OUR_IP, 24, None, 67),
        Err(InterfaceError::Mtu(67))
    );
    assert_eq!(
        Interface::new(LOCAL, OUR_IP, 24, Some([198, 51, 100, 1]), 1500),
        Err(InterfaceError::Gateway)
    );
}

#[test]
fn a_datagram_to_an_unknown_neighbor_holds_a_block_and_no_port_slot() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack = stack();
    let socket = stack.open(5000).unwrap();
    let block = filled(&mut pool, b"hello");
    let _command = stack.send(at(0), &socket, peer(), block).unwrap();
    assert_eq!(pool.free(), BLOCKS - 1);
    let request = stack.poll_demand().unwrap();
    assert_eq!(request.class, Class::Control);
    let frame = serve(&mut stack, &mut pool, at(0), request, true);
    let (header, payload) = EthernetHeader::parse(&frame).unwrap();
    assert_eq!(header.destination, [0xFF; 6]);
    let asked = Arp::parse(payload).unwrap().unwrap();
    assert_eq!(asked.operation, Operation::Request);
    assert_eq!(asked.target_protocol, PEER_IP);
    assert!(stack.poll_demand().is_none(), "nothing to materialize yet");
}

#[test]
fn a_reply_releases_the_datagram_and_its_frame_is_whole() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack = stack();
    let socket = stack.open(5000).unwrap();
    let block = filled(&mut pool, b"hello");
    let command = stack.send(at(0), &socket, peer(), block).unwrap();
    let request = stack.poll_demand().unwrap();
    serve(&mut stack, &mut pool, at(0), request, true);
    let reply = arp_frame(Operation::Reply, (PEER, PEER_IP), OUR_IP);
    stack.input(at(1), &reply, &mut pool, &mut ());
    let demand = stack.poll_demand().unwrap();
    assert_eq!(demand.class, Class::Data);
    let frame = serve(&mut stack, &mut pool, at(1), demand, true);
    let (ethernet, payload) = EthernetHeader::parse(&frame).unwrap();
    assert_eq!((ethernet.destination, ethernet.source), (PEER, LOCAL));
    let packet = Ipv4Header::parse(payload).unwrap();
    assert!(Checksum::new().with(&payload[..20]).verifies());
    assert_eq!(packet.addresses()[4..], PEER_IP);
    let (udp, data) = UdpHeader::parse(packet.payload()).unwrap();
    assert_eq!(data, b"hello");
    assert_eq!((udp.ports.source, udp.ports.destination), (5000, 7));
    let pseudo = ipv4_pseudo_header(OUR_IP, PEER_IP, PROTOCOL_UDP, udp.length);
    assert!(
        Checksum::new()
            .with(&pseudo)
            .with(packet.payload())
            .verifies()
    );
    assert_eq!(stack.take_result(command), Ok(Outcome::Accepted));
    assert_eq!(pool.free(), BLOCKS);
}

#[test]
fn a_datagram_beyond_the_network_goes_through_the_gateway() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack = stack();
    let socket = stack.open(5000).unwrap();
    let block = filled(&mut pool, b"far");
    let far = Remote {
        address: [198, 51, 100, 1],
        port: 7,
    };
    let _command = stack.send(at(0), &socket, far, block).unwrap();
    let request = stack.poll_demand().unwrap();
    let frame = serve(&mut stack, &mut pool, at(0), request, true);
    let (_, payload) = EthernetHeader::parse(&frame).unwrap();
    assert_eq!(
        Arp::parse(payload).unwrap().unwrap().target_protocol,
        GATEWAY
    );
}

#[test]
fn no_route_refuses_and_returns_the_block() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack: Stack<'_> = Core::new(Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap());
    let socket = stack.open(5000).unwrap();
    let block = filled(&mut pool, b"far");
    let far = Remote {
        address: [198, 51, 100, 1],
        port: 7,
    };
    let refused = stack.send(at(0), &socket, far, block).unwrap_err();
    assert_eq!(refused.reason, SendRefusal::NoRoute);
    assert_eq!(refused.block.octets(), b"far");
}

#[test]
fn a_refused_frame_moves_nothing_and_its_outcome_waits_for_acceptance() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack = stack();
    knows_peer(&mut stack, &mut pool);
    let socket = stack.open(5000).unwrap();
    let block = filled(&mut pool, b"hello");
    let command = stack.send(at(1), &socket, peer(), block).unwrap();
    let demand = stack.poll_demand().unwrap();
    serve(&mut stack, &mut pool, at(1), demand, false);
    assert_eq!(pool.free(), BLOCKS - 1, "the data stays with the core");
    let command = stack.take_result(command).unwrap_err();
    let again = stack.poll_demand().unwrap();
    assert_eq!(again.class, Class::Data);
    serve(&mut stack, &mut pool, at(2), again, true);
    assert_eq!(stack.take_result(command), Ok(Outcome::Accepted));
}

#[test]
fn a_frame_dropped_unanswered_leaves_its_work_ready() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack = stack();
    knows_peer(&mut stack, &mut pool);
    let socket = stack.open(5000).unwrap();
    let block = filled(&mut pool, b"x");
    let _command = stack.send(at(1), &socket, peer(), block).unwrap();
    let demand = stack.poll_demand().unwrap();
    let mut slot = [0; SIZE];
    drop(
        stack
            .materialize(demand.grant(), &mut slot, &mut pool)
            .unwrap(),
    );
    assert_eq!(stack.poll_demand().unwrap().class, Class::Data);
    assert_eq!(pool.free(), BLOCKS - 1);
}

#[test]
fn a_slot_that_is_too_short_writes_nothing() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack = stack();
    knows_peer(&mut stack, &mut pool);
    let socket = stack.open(5000).unwrap();
    let block = filled(&mut pool, b"hello");
    let _command = stack.send(at(1), &socket, peer(), block).unwrap();
    let demand = stack.poll_demand().unwrap();
    assert_eq!(
        stack
            .materialize(demand.grant(), &mut [0; 10], &mut pool)
            .err()
            .unwrap(),
        Unmaterialized::Slot
    );
    assert_eq!(stack.poll_demand().unwrap().class, Class::Data);
}

#[test]
fn a_dropped_observer_keeps_a_running_commands_place() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack: Core<'_, SIZE, 1, 1, 1, 2> =
        Core::new(Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap());
    let request = arp_frame(Operation::Request, (PEER, PEER_IP), OUR_IP);
    stack.input(at(0), &request, &mut pool, &mut ());
    let demand = stack.poll_demand().unwrap();
    let mut slot = [0; SIZE];
    stack
        .materialize(demand.grant(), &mut slot, &mut pool)
        .unwrap()
        .accepted(at(0));
    let socket = stack.open(5000).unwrap();
    let block = filled(&mut pool, b"one");
    let command = stack.send(at(1), &socket, peer(), block).unwrap();
    stack.drop_observer(command);
    let second = filled(&mut pool, b"two");
    let refused = stack.send(at(1), &socket, peer(), second).unwrap_err();
    assert_eq!(
        refused.reason,
        SendRefusal::NoPlace,
        "the place is still held"
    );
    let demand = stack.poll_demand().unwrap();
    stack
        .materialize(demand.grant(), &mut slot, &mut pool)
        .unwrap()
        .accepted(at(1));
    let command = stack.send(at(2), &socket, peer(), refused.block).unwrap();
    let demand = stack.poll_demand().unwrap();
    stack
        .materialize(demand.grant(), &mut slot, &mut pool)
        .unwrap()
        .accepted(at(2));
    assert_eq!(stack.take_result(command), Ok(Outcome::Accepted));
}

#[test]
fn a_close_has_its_own_place_when_every_command_place_is_taken() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack = stack();
    knows_peer(&mut stack, &mut pool);
    let socket = stack.open(5000).unwrap();
    let block = filled(&mut pool, b"one");
    let first = stack.send(at(1), &socket, peer(), block).unwrap();
    let block = filled(&mut pool, b"two");
    let second = stack.send(at(1), &socket, peer(), block).unwrap();
    let close = stack.close(socket, &mut pool).unwrap();
    assert_eq!(stack.take_result(first), Ok(Outcome::Closed));
    assert_eq!(stack.take_result(second), Ok(Outcome::Closed));
    assert_eq!(stack.take_result(close), Ok(Outcome::SocketClosed));
    assert_eq!(pool.free(), BLOCKS);
}

#[test]
fn a_reused_socket_never_sees_the_old_ones_commands() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack: Core<'_, SIZE, 1, 1, 1, 2> =
        Core::new(Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap());
    let socket = stack.open(5000).unwrap();
    let block = filled(&mut pool, b"one");
    let old = stack.send(at(0), &socket, peer(), block).unwrap();
    let close = stack.close(socket, &mut pool).unwrap();
    assert_eq!(
        stack.open(5000),
        Err(OpenRefused::NoSocket),
        "results unread"
    );
    stack.drop_observer(old);
    assert_eq!(stack.take_result(close), Ok(Outcome::SocketClosed));
    let reused = stack.open(5000).unwrap();
    assert_eq!(pool.free(), BLOCKS);
    assert!(stack.close(reused, &mut pool).is_ok());
}

#[test]
fn a_neighbor_that_never_answers_ends_its_datagrams_after_its_requests() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack = stack();
    let socket = stack.open(5000).unwrap();
    let block = filled(&mut pool, b"lost");
    let mut command = stack.send(at(0), &socket, peer(), block).unwrap();
    let mut now = at(0);
    let mut requests = 0;
    while let Some(demand) = stack.poll_demand() {
        serve(&mut stack, &mut pool, now, demand, true);
        requests += 1;
        command = stack.take_result(command).unwrap_err();
        now = stack.poll_deadline().unwrap();
        stack.tick(now, &mut pool);
    }
    assert_eq!(requests, 3);
    assert_eq!(now, at(3000), "one request per second");
    assert_eq!(stack.take_result(command), Ok(Outcome::Unreachable));
    assert_eq!(pool.free(), BLOCKS);
}

#[test]
fn a_request_refused_by_the_port_counts_for_nothing() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack = stack();
    let socket = stack.open(5000).unwrap();
    let block = filled(&mut pool, b"x");
    let _command = stack.send(at(0), &socket, peer(), block).unwrap();
    let demand = stack.poll_demand().unwrap();
    serve(&mut stack, &mut pool, at(0), demand, false);
    assert!(stack.poll_demand().is_some());
    assert_eq!(
        stack.poll_deadline(),
        None,
        "no retry runs while one is owed"
    );
}

#[test]
fn an_arp_request_for_us_is_answered_and_control_goes_before_data() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack = stack();
    knows_peer(&mut stack, &mut pool);
    let socket = stack.open(5000).unwrap();
    let block = filled(&mut pool, b"x");
    let _command = stack.send(at(1), &socket, peer(), block).unwrap();
    let request = arp_frame(Operation::Request, (PEER, PEER_IP), OUR_IP);
    stack.input(at(1), &request, &mut pool, &mut ());
    let demand = stack.poll_demand().unwrap();
    assert_eq!(demand.class, Class::Control);
    let frame = serve(&mut stack, &mut pool, at(1), demand, true);
    let (header, payload) = EthernetHeader::parse(&frame).unwrap();
    assert_eq!(header.destination, PEER);
    let reply = Arp::parse(payload).unwrap().unwrap();
    assert_eq!(reply.operation, Operation::Reply);
    assert_eq!(reply.sender_protocol, OUR_IP);
    assert_eq!(reply.target_protocol, PEER_IP);
    assert_eq!(stack.poll_demand().unwrap().class, Class::Data);
}

#[test]
fn replies_past_their_bound_are_dropped_and_counted() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack = stack();
    let mut dropped = Dropped::default();
    let request = arp_frame(Operation::Request, (PEER, PEER_IP), OUR_IP);
    for _ in 0..3 {
        stack.input(at(0), &request, &mut pool, &mut dropped);
    }
    assert_eq!(dropped.0, [NetEvent::ReplyDropped]);
}

#[test]
fn an_arp_packet_for_another_makes_no_entry() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack = stack();
    let other = arp_frame(Operation::Request, (PEER, PEER_IP), [192, 0, 2, 9]);
    stack.input(at(0), &other, &mut pool, &mut ());
    assert!(stack.poll_demand().is_none());
    let socket = stack.open(5000).unwrap();
    let block = filled(&mut pool, b"x");
    let _command = stack.send(at(0), &socket, peer(), block).unwrap();
    assert_eq!(stack.poll_demand().unwrap().class, Class::Control);
}

#[test]
fn a_datagram_reaches_its_socket_and_its_block_is_the_applications() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack = stack();
    let socket = stack.open(5000).unwrap();
    let frame = udp_frame(7, 5000, b"payload");
    stack.input(at(0), &frame, &mut pool, &mut ());
    let received = stack.receive(&socket).unwrap().unwrap();
    assert_eq!(received.block.octets(), b"payload");
    assert_eq!(
        received.remote,
        Remote {
            address: PEER_IP,
            port: 7
        }
    );
    assert_eq!(pool.free(), BLOCKS - 1);
    assert!(pool.give(received.block).is_ok());
    assert!(stack.receive(&socket).unwrap().is_none());
}

#[test]
fn received_datagrams_are_refused_with_a_counted_reason() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack = stack();
    let mut dropped = Dropped::default();
    let socket = stack.open(5000).unwrap();
    stack.input(at(0), &udp_frame(7, 6000, b"x"), &mut pool, &mut dropped);
    let mut broken = udp_frame(7, 5000, b"x");
    *broken.last_mut().unwrap() ^= 1;
    stack.input(at(0), &broken, &mut pool, &mut dropped);
    for _ in 0..3 {
        stack.input(at(0), &udp_frame(7, 5000, b"x"), &mut pool, &mut dropped);
    }
    assert_eq!(
        dropped.0,
        [
            NetEvent::NoSocket,
            NetEvent::Checksum,
            NetEvent::ReceiveFull
        ]
    );
    assert_eq!(pool.free(), BLOCKS - 2);
    let _first = stack.receive(&socket).unwrap().unwrap();
}

#[test]
fn no_block_drops_a_received_datagram_and_counts_it() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack = stack();
    let mut dropped = Dropped::default();
    let _socket = stack.open(5000).unwrap();
    let mut held = Vec::new();
    while let Some(block) = pool.take() {
        held.push(block);
    }
    stack.input(at(0), &udp_frame(7, 5000, b"x"), &mut pool, &mut dropped);
    assert_eq!(dropped.0, [NetEvent::NoBlock]);
}

#[test]
fn a_frame_cut_at_any_length_is_handled() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack = stack();
    let _socket = stack.open(5000).unwrap();
    let udp = udp_frame(7, 5000, b"payload");
    let arp = arp_frame(Operation::Request, (PEER, PEER_IP), OUR_IP);
    for frame in [&udp, &arp] {
        for length in 0..frame.len() {
            stack.input(at(0), &frame[..length], &mut pool, &mut ());
        }
    }
}

#[test]
fn a_broken_ipv4_header_is_counted_not_delivered() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack = stack();
    let mut dropped = Dropped::default();
    let socket = stack.open(5000).unwrap();
    let mut frame = udp_frame(7, 5000, b"x");
    frame[14 + 10] ^= 1;
    stack.input(at(0), &frame, &mut pool, &mut dropped);
    assert_eq!(dropped.0, [NetEvent::Checksum]);
    assert!(stack.receive(&socket).unwrap().is_none());
}

#[test]
fn a_fragment_is_counted_not_delivered() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack = stack();
    let mut dropped = Dropped::default();
    let socket = stack.open(5000).unwrap();
    let mut frame = udp_frame(7, 5000, b"x");
    // More Fragments, then the header's checksum again.
    frame[14 + 6] |= 0x20;
    frame[14 + 10..14 + 12].copy_from_slice(&[0, 0]);
    let field = Checksum::new().with(&frame[14..34]).finish();
    frame[14 + 10..14 + 12].copy_from_slice(&field.to_be_bytes());
    stack.input(at(0), &frame, &mut pool, &mut dropped);
    assert_eq!(dropped.0, [NetEvent::Fragment]);
    assert!(stack.receive(&socket).unwrap().is_none());
}

#[test]
fn a_frame_for_another_station_is_ignored() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack = stack();
    let socket = stack.open(5000).unwrap();
    let mut frame = udp_frame(7, 5000, b"x");
    frame[5] = 9;
    stack.input(at(0), &frame, &mut pool, &mut ());
    assert!(stack.receive(&socket).unwrap().is_none());
}

#[test]
fn a_host_prefix_has_no_broadcast_of_its_own() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack: Stack<'_> = Core::new(Interface::new(LOCAL, PEER_IP, 31, None, 1500).unwrap());
    let socket = stack.open(5000).unwrap();
    let block = filled(&mut pool, b"x");
    let other = Remote {
        address: [192, 0, 2, 3],
        port: 7,
    };
    let _command = stack.send(at(0), &socket, other, block).unwrap();
    assert_eq!(stack.poll_demand().unwrap().class, Class::Control);
}

#[test]
fn sockets_take_turns() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack = stack();
    knows_peer(&mut stack, &mut pool);
    let first = stack.open(5000).unwrap();
    let second = stack.open(5001).unwrap();
    for socket in [&first, &second, &first, &second] {
        let block = filled(&mut pool, b"x");
        let _command = stack.send(at(1), socket, peer(), block).unwrap();
    }
    let mut order = Vec::new();
    while let Some(demand) = stack.poll_demand() {
        let frame = serve(&mut stack, &mut pool, at(1), demand, true);
        let (_, payload) = EthernetHeader::parse(&frame).unwrap();
        let packet = Ipv4Header::parse(payload).unwrap();
        order.push(UdpHeader::parse(packet.payload()).unwrap().0.ports.source);
    }
    assert_eq!(order, [5000, 5001, 5000, 5001]);
}

#[test]
fn a_port_in_use_and_a_full_table_refuse_to_open() {
    let mut stack = stack();
    let _first = stack.open(5000).unwrap();
    assert_eq!(stack.open(5000), Err(OpenRefused::PortInUse));
    let _second = stack.open(5001).unwrap();
    assert_eq!(stack.open(5002), Err(OpenRefused::NoSocket));
}

#[test]
fn a_time_past_a_neighbors_life_asks_again() {
    let mut memory = storage();
    let mut pool = Pool::new(&mut memory);
    let mut stack = stack();
    knows_peer(&mut stack, &mut pool);
    let socket = stack.open(5000).unwrap();
    let block = filled(&mut pool, b"x");
    let later = at(0).saturating_add(Duration::from_secs(61));
    let _command = stack.send(later, &socket, peer(), block).unwrap();
    assert_eq!(stack.poll_demand().unwrap().class, Class::Control);
}
