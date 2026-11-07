extern crate std;

use std::vec;
use std::vec::Vec;

use oer_portable_events::EventSink;
use oer_portable_time::{Duration, Instant};
use oer_protocols_network_wire::arp::{self, Arp, Operation};
use oer_protocols_network_wire::checksum::{Checksum, ipv4_pseudo_header, udp_field};
use oer_protocols_network_wire::ethernet::{EtherType, EthernetHeader};
use oer_protocols_network_wire::ip::{DsField, Dscp, Ipv4Fields, Ipv4Header};
use oer_protocols_network_wire::transport::{PROTOCOL_UDP, Ports, UdpHeader};

use crate::{
    Block, Class, Core, FramePort, Interface, InterfaceError, NetEvent, OpenRefused, Outcome, Pool,
    Remote, SendRefusal, SlotTooShort, Unsent, scope,
};

const SIZE: usize = 1536;
const BLOCKS: usize = 8;
const LOCAL: [u8; 6] = [2, 0, 0, 0, 0, 1];
const PEER: [u8; 6] = [2, 0, 0, 0, 0, 2];
const MOVED: [u8; 6] = [2, 0, 0, 0, 0, 3];
const OUR_IP: [u8; 4] = [192, 0, 2, 1];
const PEER_IP: [u8; 4] = [192, 0, 2, 2];
const GATEWAY: [u8; 4] = [192, 0, 2, 254];

type Stack<'id> = Core<'id, SIZE, 2, 2, 2, 2>;
type Blocks<'id> = Pool<'id, SIZE, BLOCKS>;

#[derive(Default)]
struct Dropped(Vec<NetEvent>);

impl EventSink<NetEvent> for Dropped {
    fn event(&mut self, event: NetEvent) {
        self.0.push(event);
    }
}

/// The tests' port: what it accepted, and how it misbehaves on demand.
#[derive(Default)]
struct Port {
    full: bool,
    /// Says `Ok` without writing.
    lazy: bool,
    /// Says `Ok` whatever the writing said.
    careless: bool,
    /// Gives less room than was asked for.
    room: Option<usize>,
    sent: Vec<Vec<u8>>,
}

impl FramePort for Port {
    type Refusal = &'static str;

    fn publish(
        &mut self,
        length: usize,
        fill: impl FnOnce(&mut [u8]) -> Result<(), SlotTooShort>,
    ) -> Result<(), &'static str> {
        if self.full {
            return Err("full");
        }
        if self.lazy {
            return Ok(());
        }
        let mut slot = vec![0; self.room.unwrap_or(length)];
        let written = fill(&mut slot);
        if self.careless {
            return Ok(());
        }
        written.map_err(|_| "short")?;
        self.sent.push(slot);
        Ok(())
    }
}

fn at(millis: u32) -> Instant {
    Instant::from_micros(u64::from(millis) * 1000)
}

fn interface(address: [u8; 4], prefix: u8, gateway: Option<[u8; 4]>) -> Interface {
    Interface::new(LOCAL, address, prefix, gateway, 1500).unwrap()
}

fn best_effort() -> Dscp {
    Dscp::default()
}

/// A world of one pool, one core and one port on `interface`.
fn world_on(
    interface: Interface,
    test: impl for<'id> FnOnce(&mut Stack<'id>, &mut Blocks<'id>, &mut Port),
) {
    let mut memory = std::boxed::Box::new([[0; SIZE]; BLOCKS]);
    scope(&mut memory, |mut pool, brand| {
        let mut stack = Core::new(brand, interface);
        test(&mut stack, &mut pool, &mut Port::default());
    });
}

fn world(test: impl for<'id> FnOnce(&mut Stack<'id>, &mut Blocks<'id>, &mut Port)) {
    world_on(interface(OUR_IP, 24, Some(GATEWAY)), test);
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

fn filled<'id>(pool: &mut Blocks<'id>, data: &[u8]) -> Block<'id, SIZE> {
    let mut block = pool.take().unwrap();
    block.fill(data).unwrap();
    block
}

/// Send the next ready work through `port`.
fn send_next<'id>(
    stack: &mut Stack<'id>,
    pool: &mut Blocks<'id>,
    port: &mut Port,
    now: Instant,
) -> Result<(), Unsent<&'static str>> {
    let demand = stack.poll_demand(now).unwrap();
    stack.transmit(now, demand.grant(), port, pool)
}

/// Learn the peer through its own request for us.
fn knows_peer<'id>(stack: &mut Stack<'id>, pool: &mut Blocks<'id>, port: &mut Port) {
    let request = arp_frame(Operation::Request, (PEER, PEER_IP), OUR_IP);
    stack.input(at(0), &request, pool, &mut ());
    send_next(stack, pool, port, at(0)).unwrap();
    port.sent.clear();
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
fn a_datagram_to_an_unknown_neighbor_holds_a_block_and_no_port_room() {
    world(|stack, pool, port| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(pool, b"hello");
        let _command = stack.send(at(0), &socket, peer(), block).unwrap();
        assert_eq!(pool.free(), BLOCKS - 1);
        let demand = stack.poll_demand(at(0)).unwrap();
        assert_eq!(demand.class, Class::Control);
        stack.transmit(at(0), demand.grant(), port, pool).unwrap();
        let (header, payload) = EthernetHeader::parse(&port.sent[0]).unwrap();
        assert_eq!(header.destination, [0xFF; 6]);
        let asked = Arp::parse(payload).unwrap().unwrap();
        assert_eq!(asked.operation, Operation::Request);
        assert_eq!(asked.target_protocol, PEER_IP);
        assert!(stack.poll_demand(at(0)).is_none(), "nothing to send yet");
    });
}

#[test]
fn a_reply_releases_the_datagram_and_its_frame_is_whole() {
    world(|stack, pool, port| {
        let dscp = Dscp::new(46).unwrap();
        let socket = stack.open(5000, dscp).unwrap();
        let block = filled(pool, b"hello");
        let command = stack.send(at(0), &socket, peer(), block).unwrap();
        send_next(stack, pool, port, at(0)).unwrap();
        let reply = arp_frame(Operation::Reply, (PEER, PEER_IP), OUR_IP);
        stack.input(at(1), &reply, pool, &mut ());
        let demand = stack.poll_demand(at(1)).unwrap();
        assert_eq!(demand.class, Class::Data);
        assert_eq!(demand.peer, PEER);
        assert_eq!(demand.flow, Some(0));
        assert_eq!(demand.dscp, dscp);
        stack.transmit(at(1), demand.grant(), port, pool).unwrap();
        let frame = &port.sent[1];
        let (ethernet, payload) = EthernetHeader::parse(frame).unwrap();
        assert_eq!((ethernet.destination, ethernet.source), (PEER, LOCAL));
        let packet = Ipv4Header::parse(payload).unwrap();
        assert!(Checksum::new().with(&payload[..20]).verifies());
        assert_eq!(packet.addresses()[4..], PEER_IP);
        assert_eq!(packet.dscp(), dscp);
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
    });
}

#[test]
fn a_datagram_beyond_the_network_goes_through_the_gateway() {
    world(|stack, pool, port| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(pool, b"far");
        let far = Remote {
            address: [198, 51, 100, 1],
            port: 7,
        };
        let _command = stack.send(at(0), &socket, far, block).unwrap();
        send_next(stack, pool, port, at(0)).unwrap();
        let (_, payload) = EthernetHeader::parse(&port.sent[0]).unwrap();
        assert_eq!(
            Arp::parse(payload).unwrap().unwrap().target_protocol,
            GATEWAY
        );
    });
}

#[test]
fn no_route_refuses_and_returns_the_block() {
    world_on(interface(OUR_IP, 24, None), |stack, pool, _| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(pool, b"far");
        let far = Remote {
            address: [198, 51, 100, 1],
            port: 7,
        };
        let refused = stack.send(at(0), &socket, far, block).unwrap_err();
        assert_eq!(refused.reason, SendRefusal::NoRoute);
        assert_eq!(refused.block.octets(), b"far");
    });
}

#[test]
fn a_refused_frame_moves_nothing_and_its_outcome_waits_for_acceptance() {
    world(|stack, pool, port| {
        knows_peer(stack, pool, port);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(pool, b"hello");
        let command = stack.send(at(1), &socket, peer(), block).unwrap();
        port.full = true;
        assert_eq!(
            send_next(stack, pool, port, at(1)),
            Err(Unsent::Port("full"))
        );
        assert_eq!(pool.free(), BLOCKS - 1, "the data stays with the core");
        let command = stack.take_result(command).unwrap_err();
        port.full = false;
        send_next(stack, pool, port, at(2)).unwrap();
        assert_eq!(stack.take_result(command), Ok(Outcome::Accepted));
    });
}

#[test]
fn an_accepted_frame_is_never_offered_again() {
    world(|stack, pool, port| {
        knows_peer(stack, pool, port);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(pool, b"once");
        let _command = stack.send(at(1), &socket, peer(), block).unwrap();
        send_next(stack, pool, port, at(1)).unwrap();
        assert!(stack.poll_demand(at(1)).is_none());
        assert_eq!(port.sent.len(), 1);
    });
}

#[test]
fn a_port_that_says_yes_without_writing_is_reported_and_moves_nothing() {
    world(|stack, pool, port| {
        knows_peer(stack, pool, port);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(pool, b"x");
        let command = stack.send(at(1), &socket, peer(), block).unwrap();
        port.lazy = true;
        assert_eq!(send_next(stack, pool, port, at(1)), Err(Unsent::NotFilled));
        assert_eq!(pool.free(), BLOCKS - 1);
        assert!(stack.take_result(command).is_err());
    });
}

#[test]
fn a_port_that_says_yes_after_a_failed_write_is_reported_too() {
    world(|stack, pool, port| {
        knows_peer(stack, pool, port);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(pool, b"x");
        let _command = stack.send(at(1), &socket, peer(), block).unwrap();
        port.careless = true;
        port.room = Some(10);
        assert_eq!(send_next(stack, pool, port, at(1)), Err(Unsent::NotFilled));
        assert_eq!(pool.free(), BLOCKS - 1);
    });
}

#[test]
fn room_that_is_too_short_sends_nothing() {
    world(|stack, pool, port| {
        knows_peer(stack, pool, port);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(pool, b"hello");
        let _command = stack.send(at(1), &socket, peer(), block).unwrap();
        port.room = Some(10);
        assert_eq!(
            send_next(stack, pool, port, at(1)),
            Err(Unsent::Port("short"))
        );
        assert_eq!(stack.poll_demand(at(1)).unwrap().class, Class::Data);
    });
}

#[test]
fn a_grant_of_a_reply_cannot_answer_a_later_asker() {
    world(|stack, pool, port| {
        let first = arp_frame(Operation::Request, (PEER, PEER_IP), OUR_IP);
        stack.input(at(0), &first, pool, &mut ());
        let stale = stack.poll_demand(at(0)).unwrap().grant();
        let again = stack.poll_demand(at(0)).unwrap();
        stack.transmit(at(0), again.grant(), port, pool).unwrap();
        let second = arp_frame(Operation::Request, (MOVED, [192, 0, 2, 3]), OUR_IP);
        stack.input(at(0), &second, pool, &mut ());
        assert_eq!(stack.transmit(at(0), stale, port, pool), Err(Unsent::Stale));
        assert_eq!(port.sent.len(), 1);
    });
}

#[test]
fn a_grant_stays_stale_however_often_its_place_is_used() {
    world(|stack, pool, port| {
        knows_peer(stack, pool, port);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(pool, b"first");
        let command = stack.send(at(1), &socket, peer(), block).unwrap();
        let stale = stack.demands(at(1)).next().unwrap().grant();
        send_next(stack, pool, port, at(1)).unwrap();
        assert_eq!(stack.take_result(command), Ok(Outcome::Accepted));
        // Past any 16-bit generation.
        for _ in 0..70_000 {
            let block = filled(pool, b"later");
            let command = stack.send(at(1), &socket, peer(), block).unwrap();
            send_next(stack, pool, port, at(1)).unwrap();
            assert_eq!(stack.take_result(command), Ok(Outcome::Accepted));
            port.sent.clear();
        }
        let block = filled(pool, b"waiting");
        let _command = stack.send(at(1), &socket, peer(), block).unwrap();
        assert_eq!(stack.transmit(at(1), stale, port, pool), Err(Unsent::Stale));
        assert!(port.sent.is_empty());
    });
}

#[test]
fn a_changed_neighbor_reaches_a_datagram_that_waits() {
    world(|stack, pool, port| {
        knows_peer(stack, pool, port);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(pool, b"x");
        let _command = stack.send(at(1), &socket, peer(), block).unwrap();
        let moved = arp_frame(Operation::Reply, (MOVED, PEER_IP), OUR_IP);
        stack.input(at(2), &moved, pool, &mut ());
        let demand = stack.poll_demand(at(2)).unwrap();
        assert_eq!(demand.peer, MOVED);
        stack.transmit(at(2), demand.grant(), port, pool).unwrap();
        let (header, _) = EthernetHeader::parse(&port.sent[0]).unwrap();
        assert_eq!(header.destination, MOVED);
    });
}

#[test]
fn a_neighbor_that_expires_under_a_datagram_is_asked_again() {
    world(|stack, pool, port| {
        knows_peer(stack, pool, port);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(pool, b"x");
        let command = stack.send(at(1), &socket, peer(), block).unwrap();
        let stale = stack.demands(at(1)).next().unwrap().grant();
        let later = at(0).saturating_add(Duration::from_secs(61));
        assert_eq!(
            stack.transmit(later, stale, port, pool),
            Err(Unsent::Stale),
            "no frame to an address that expired"
        );
        assert_eq!(stack.poll_demand(later).unwrap().class, Class::Control);
        let command = stack.take_result(command).unwrap_err();
        let mut now = later;
        while let Some(demand) = stack.poll_demand(now) {
            stack.transmit(now, demand.grant(), port, pool).unwrap();
            now = stack.poll_deadline().unwrap();
            stack.tick(now, pool);
        }
        assert_eq!(stack.take_result(command), Ok(Outcome::Unreachable));
        assert_eq!(pool.free(), BLOCKS);
    });
}

#[test]
fn every_ready_piece_of_work_is_listed_with_who_it_is_for() {
    world(|stack, pool, port| {
        knows_peer(stack, pool, port);
        let first = stack.open(5000, Dscp::new(46).unwrap()).unwrap();
        let second = stack.open(5001, best_effort()).unwrap();
        for socket in [&first, &second] {
            let block = filled(pool, b"x");
            let _command = stack.send(at(1), socket, peer(), block).unwrap();
        }
        let listed: Vec<_> = stack
            .demands(at(1))
            .map(|demand| (demand.flow, demand.peer, demand.dscp.value()))
            .collect();
        assert_eq!(listed, [(Some(0), PEER, 46), (Some(1), PEER, 0)]);
    });
}

#[test]
fn a_dropped_observer_keeps_a_running_commands_place() {
    let mut memory = std::boxed::Box::new([[0; SIZE]; BLOCKS]);
    scope(&mut memory, |mut pool, brand| {
        let mut stack: Core<'_, SIZE, 1, 1, 1, 2> = Core::new(brand, interface(OUR_IP, 24, None));
        let mut port = Port::default();
        let request = arp_frame(Operation::Request, (PEER, PEER_IP), OUR_IP);
        stack.input(at(0), &request, &mut pool, &mut ());
        let demand = stack.poll_demand(at(0)).unwrap();
        stack
            .transmit(at(0), demand.grant(), &mut port, &mut pool)
            .unwrap();
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = pool.take().unwrap();
        let command = stack.send(at(1), &socket, peer(), block).unwrap();
        stack.drop_observer(command);
        let second = pool.take().unwrap();
        let refused = stack.send(at(1), &socket, peer(), second).unwrap_err();
        assert_eq!(refused.reason, SendRefusal::NoPlace);
        let demand = stack.poll_demand(at(1)).unwrap();
        stack
            .transmit(at(1), demand.grant(), &mut port, &mut pool)
            .unwrap();
        let command = stack.send(at(2), &socket, peer(), refused.block).unwrap();
        let demand = stack.poll_demand(at(2)).unwrap();
        stack
            .transmit(at(2), demand.grant(), &mut port, &mut pool)
            .unwrap();
        assert_eq!(stack.take_result(command), Ok(Outcome::Accepted));
    });
}

#[test]
fn a_close_has_its_own_place_when_every_command_place_is_taken() {
    world(|stack, pool, port| {
        knows_peer(stack, pool, port);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(pool, b"one");
        let first = stack.send(at(1), &socket, peer(), block).unwrap();
        let block = filled(pool, b"two");
        let second = stack.send(at(1), &socket, peer(), block).unwrap();
        let close = stack.close(socket, pool).unwrap();
        assert_eq!(stack.take_result(first), Ok(Outcome::Closed));
        assert_eq!(stack.take_result(second), Ok(Outcome::Closed));
        assert_eq!(stack.take_result(close), Ok(Outcome::SocketClosed));
        assert_eq!(pool.free(), BLOCKS);
    });
}

#[test]
fn a_reused_socket_waits_for_the_old_ones_results() {
    let mut memory = std::boxed::Box::new([[0; SIZE]; BLOCKS]);
    scope(&mut memory, |mut pool, brand| {
        let mut stack: Core<'_, SIZE, 1, 1, 1, 2> = Core::new(brand, interface(OUR_IP, 24, None));
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = pool.take().unwrap();
        let old = stack.send(at(0), &socket, peer(), block).unwrap();
        let close = stack.close(socket, &mut pool).unwrap();
        assert_eq!(
            stack.open(5000, best_effort()).err(),
            Some(OpenRefused::NoSocket),
            "results unread"
        );
        stack.drop_observer(old);
        assert_eq!(stack.take_result(close), Ok(Outcome::SocketClosed));
        let reused = stack.open(5000, best_effort()).unwrap();
        assert_eq!(pool.free(), BLOCKS);
        assert!(stack.close(reused, &mut pool).is_ok());
    });
}

#[test]
fn a_neighbor_that_never_answers_ends_its_datagrams_after_its_requests() {
    world(|stack, pool, port| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(pool, b"lost");
        let mut command = stack.send(at(0), &socket, peer(), block).unwrap();
        let mut now = at(0);
        let mut requests = 0;
        while let Some(demand) = stack.poll_demand(now) {
            stack.transmit(now, demand.grant(), port, pool).unwrap();
            requests += 1;
            command = stack.take_result(command).unwrap_err();
            now = stack.poll_deadline().unwrap();
            stack.tick(now, pool);
        }
        assert_eq!(requests, 3);
        assert_eq!(now, at(3000), "one request per second");
        assert_eq!(stack.take_result(command), Ok(Outcome::Unreachable));
        assert_eq!(pool.free(), BLOCKS);
    });
}

#[test]
fn a_request_refused_by_the_port_counts_for_nothing() {
    world(|stack, pool, port| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(pool, b"x");
        let _command = stack.send(at(0), &socket, peer(), block).unwrap();
        port.full = true;
        assert!(send_next(stack, pool, port, at(0)).is_err());
        assert!(stack.poll_demand(at(0)).is_some());
        assert_eq!(
            stack.poll_deadline(),
            None,
            "no retry runs while one is owed"
        );
    });
}

#[test]
fn an_arp_request_for_us_is_answered_and_control_goes_before_data() {
    world(|stack, pool, port| {
        knows_peer(stack, pool, port);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(pool, b"x");
        let _command = stack.send(at(1), &socket, peer(), block).unwrap();
        let request = arp_frame(Operation::Request, (PEER, PEER_IP), OUR_IP);
        stack.input(at(1), &request, pool, &mut ());
        let demand = stack.poll_demand(at(1)).unwrap();
        assert_eq!(demand.class, Class::Control);
        stack.transmit(at(1), demand.grant(), port, pool).unwrap();
        let (header, payload) = EthernetHeader::parse(&port.sent[0]).unwrap();
        assert_eq!(header.destination, PEER);
        let reply = Arp::parse(payload).unwrap().unwrap();
        assert_eq!(reply.operation, Operation::Reply);
        assert_eq!(reply.sender_protocol, OUR_IP);
        assert_eq!(reply.target_protocol, PEER_IP);
        assert_eq!(stack.poll_demand(at(1)).unwrap().class, Class::Data);
    });
}

#[test]
fn replies_past_their_bound_are_dropped_and_counted() {
    world(|stack, pool, _| {
        let mut dropped = Dropped::default();
        let request = arp_frame(Operation::Request, (PEER, PEER_IP), OUR_IP);
        for _ in 0..3 {
            stack.input(at(0), &request, pool, &mut dropped);
        }
        assert_eq!(dropped.0, [NetEvent::ReplyDropped]);
    });
}

#[test]
fn an_arp_packet_for_another_makes_no_entry() {
    world(|stack, pool, _| {
        let other = arp_frame(Operation::Request, (PEER, PEER_IP), [192, 0, 2, 9]);
        stack.input(at(0), &other, pool, &mut ());
        assert!(stack.poll_demand(at(0)).is_none());
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(pool, b"x");
        let _command = stack.send(at(0), &socket, peer(), block).unwrap();
        assert_eq!(stack.poll_demand(at(0)).unwrap().class, Class::Control);
    });
}

#[test]
fn a_datagram_reaches_its_socket_and_its_block_is_the_applications() {
    world(|stack, pool, _| {
        let socket = stack.open(5000, best_effort()).unwrap();
        stack.input(at(0), &udp_frame(7, 5000, b"payload"), pool, &mut ());
        let received = stack.receive(&socket).unwrap().unwrap();
        assert_eq!(received.block.octets(), b"payload");
        assert_eq!(received.remote, peer());
        assert_eq!(pool.free(), BLOCKS - 1);
        pool.give(received.block);
        assert!(stack.receive(&socket).unwrap().is_none());
    });
}

#[test]
fn received_datagrams_are_refused_with_a_counted_reason() {
    world(|stack, pool, _| {
        let mut dropped = Dropped::default();
        let socket = stack.open(5000, best_effort()).unwrap();
        stack.input(at(0), &udp_frame(7, 6000, b"x"), pool, &mut dropped);
        let mut broken = udp_frame(7, 5000, b"x");
        *broken.last_mut().unwrap() ^= 1;
        stack.input(at(0), &broken, pool, &mut dropped);
        for _ in 0..3 {
            stack.input(at(0), &udp_frame(7, 5000, b"x"), pool, &mut dropped);
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
    });
}

#[test]
fn a_broken_ipv4_header_is_counted_not_delivered() {
    world(|stack, pool, _| {
        let mut dropped = Dropped::default();
        let socket = stack.open(5000, best_effort()).unwrap();
        let mut frame = udp_frame(7, 5000, b"x");
        frame[14 + 10] ^= 1;
        stack.input(at(0), &frame, pool, &mut dropped);
        assert_eq!(dropped.0, [NetEvent::Checksum]);
        assert!(stack.receive(&socket).unwrap().is_none());
    });
}

#[test]
fn a_fragment_is_counted_not_delivered() {
    world(|stack, pool, _| {
        let mut dropped = Dropped::default();
        let socket = stack.open(5000, best_effort()).unwrap();
        let mut frame = udp_frame(7, 5000, b"x");
        // More Fragments, then the header's checksum again.
        frame[14 + 6] |= 0x20;
        frame[14 + 10..14 + 12].copy_from_slice(&[0, 0]);
        let field = Checksum::new().with(&frame[14..34]).finish();
        frame[14 + 10..14 + 12].copy_from_slice(&field.to_be_bytes());
        stack.input(at(0), &frame, pool, &mut dropped);
        assert_eq!(dropped.0, [NetEvent::Fragment]);
        assert!(stack.receive(&socket).unwrap().is_none());
    });
}

#[test]
fn no_block_drops_a_received_datagram_and_counts_it() {
    world(|stack, pool, _| {
        let mut dropped = Dropped::default();
        let _socket = stack.open(5000, best_effort()).unwrap();
        let mut held = Vec::new();
        while let Some(block) = pool.take() {
            held.push(block);
        }
        stack.input(at(0), &udp_frame(7, 5000, b"x"), pool, &mut dropped);
        assert_eq!(dropped.0, [NetEvent::NoBlock]);
        for block in held {
            pool.give(block);
        }
    });
}

#[test]
fn a_frame_cut_at_any_length_is_handled() {
    world(|stack, pool, _| {
        let _socket = stack.open(5000, best_effort()).unwrap();
        let udp = udp_frame(7, 5000, b"payload");
        let arp = arp_frame(Operation::Request, (PEER, PEER_IP), OUR_IP);
        for frame in [&udp, &arp] {
            for length in 0..frame.len() {
                stack.input(at(0), &frame[..length], pool, &mut ());
            }
        }
    });
}

#[test]
fn a_frame_for_another_station_is_ignored() {
    world(|stack, pool, _| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let mut frame = udp_frame(7, 5000, b"x");
        frame[5] = 9;
        stack.input(at(0), &frame, pool, &mut ());
        assert!(stack.receive(&socket).unwrap().is_none());
    });
}

#[test]
fn a_host_prefix_has_no_broadcast_of_its_own() {
    world_on(interface(PEER_IP, 31, None), |stack, pool, _| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(pool, b"x");
        let other = Remote {
            address: [192, 0, 2, 3],
            port: 7,
        };
        let _command = stack.send(at(0), &socket, other, block).unwrap();
        assert_eq!(stack.poll_demand(at(0)).unwrap().class, Class::Control);
    });
}

#[test]
fn sockets_take_turns() {
    world(|stack, pool, port| {
        knows_peer(stack, pool, port);
        let first = stack.open(5000, best_effort()).unwrap();
        let second = stack.open(5001, best_effort()).unwrap();
        for socket in [&first, &second, &first, &second] {
            let block = filled(pool, b"x");
            let _command = stack.send(at(1), socket, peer(), block).unwrap();
        }
        while stack.demands(at(1)).next().is_some() {
            send_next(stack, pool, port, at(1)).unwrap();
        }
        let order: Vec<_> = port
            .sent
            .iter()
            .map(|frame| {
                let (_, payload) = EthernetHeader::parse(frame).unwrap();
                let packet = Ipv4Header::parse(payload).unwrap();
                UdpHeader::parse(packet.payload()).unwrap().0.ports.source
            })
            .collect();
        assert_eq!(order, [5000, 5001, 5000, 5001]);
    });
}

#[test]
fn a_port_in_use_and_a_full_table_refuse_to_open() {
    world(|stack, _, _| {
        let _first = stack.open(5000, best_effort()).unwrap();
        assert_eq!(
            stack.open(5000, best_effort()).err(),
            Some(OpenRefused::PortInUse)
        );
        let _second = stack.open(5001, best_effort()).unwrap();
        assert_eq!(
            stack.open(5002, best_effort()).err(),
            Some(OpenRefused::NoSocket)
        );
    });
}

#[test]
fn a_time_past_a_neighbors_life_asks_again() {
    world(|stack, pool, port| {
        knows_peer(stack, pool, port);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(pool, b"x");
        let later = at(0).saturating_add(Duration::from_secs(61));
        let _command = stack.send(later, &socket, peer(), block).unwrap();
        assert_eq!(stack.poll_demand(later).unwrap().class, Class::Control);
    });
}
