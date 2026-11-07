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
    Block, BrandRefusal, Class, CommandState, CoreBrand, FramePort, Interface, InterfaceError,
    NetEvent, NetworkOwner, OpenRefused, Outcome, Pool, Remote, SendRefusal, SlotTooShort, Unsent,
    claim,
};

const SIZE: usize = 1536;
const BLOCKS: usize = 8;
const LOCAL: [u8; 6] = [2, 0, 0, 0, 0, 1];
const PEER: [u8; 6] = [2, 0, 0, 0, 0, 2];
const MOVED: [u8; 6] = [2, 0, 0, 0, 0, 3];
const OUR_IP: [u8; 4] = [192, 0, 2, 1];
const PEER_IP: [u8; 4] = [192, 0, 2, 2];
const GATEWAY: [u8; 4] = [192, 0, 2, 254];

type Stack<B> = NetworkOwner<'static, B, SIZE, 2, 2, 2, 2, BLOCKS, Port>;
type Small<B> = NetworkOwner<'static, B, SIZE, 1, 1, 1, 2, BLOCKS, Port>;
type Blocks<B> = Pool<'static, B, SIZE, BLOCKS>;

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

/// A brand of the test's own.
macro_rules! brand {
    ($name:ident) => {
        #[derive(Debug, Eq, PartialEq)]
        struct $name;
    };
}

/// The pool and the core's right of brand `B`, over storage of its own.
fn claimed<B: 'static>() -> (Blocks<B>, CoreBrand<B>) {
    let memory = std::boxed::Box::leak(std::boxed::Box::new([[0; SIZE]; BLOCKS]));
    claim(memory).unwrap()
}

/// A world of one owner of brand `B` on `interface`.
fn run<B: 'static>(interface: Interface, test: impl FnOnce(&mut Stack<B>)) {
    let (pool, brand) = claimed::<B>();
    test(&mut NetworkOwner::new(
        brand,
        interface,
        pool,
        Port::default(),
    ));
}

/// A world of one socket, one place, one receive slot and two neighbors.
fn run_small<B: 'static>(test: impl FnOnce(&mut Small<B>)) {
    let (pool, brand) = claimed::<B>();
    test(&mut NetworkOwner::new(
        brand,
        interface(OUR_IP, 24, None),
        pool,
        Port::default(),
    ));
}

/// Each use has a brand of its own, as a brand claims once.
macro_rules! world {
    ($test:expr) => {{
        brand!(Net);
        run::<Net>(interface(OUR_IP, 24, Some(GATEWAY)), $test);
    }};
}

macro_rules! world_on {
    ($interface:expr, $test:expr) => {{
        brand!(Net);
        run::<Net>($interface, $test);
    }};
}

macro_rules! small {
    ($test:expr) => {{
        brand!(Net);
        run_small::<Net>($test);
    }};
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

fn filled<B>(pool: &mut Blocks<B>, data: &[u8]) -> Block<'static, B, SIZE> {
    let mut block = pool.take().unwrap();
    block.fill(data).unwrap();
    block
}

/// Send the next ready work.
fn send_next<B>(stack: &mut Stack<B>, now: Instant) -> Result<(), Unsent<&'static str>> {
    let demand = stack.poll_demand(now).unwrap();
    stack.transmit(now, demand.grant())
}

/// Learn the peer through its own request for us.
fn knows_peer<B>(stack: &mut Stack<B>) {
    let request = arp_frame(Operation::Request, (PEER, PEER_IP), OUR_IP);
    stack.input(at(0), &request, &mut ());
    send_next(stack, at(0)).unwrap();
    stack.port_mut().sent.clear();
}

fn finished<B: core::fmt::Debug>(state: CommandState<B>) -> Outcome {
    match state {
        CommandState::Finished(outcome) => outcome,
        other => panic!("{other:?}"),
    }
}

fn running<B: core::fmt::Debug>(state: CommandState<B>) -> crate::CommandId<B> {
    match state {
        CommandState::Running(command) => command,
        other => panic!("{other:?}"),
    }
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
    world!(|stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"hello");
        let _command = stack.send(at(0), &socket, peer(), block).unwrap();
        assert_eq!(stack.pool().free(), BLOCKS - 1);
        let demand = stack.poll_demand(at(0)).unwrap();
        assert_eq!(demand.class, Class::Control);
        stack.transmit(at(0), demand.grant()).unwrap();
        let (header, payload) = EthernetHeader::parse(&stack.port_mut().sent[0]).unwrap();
        assert_eq!(header.destination, [0xFF; 6]);
        let asked = Arp::parse(payload).unwrap().unwrap();
        assert_eq!(asked.operation, Operation::Request);
        assert_eq!(asked.target_protocol, PEER_IP);
        assert!(stack.poll_demand(at(0)).is_none(), "nothing to send yet");
    });
}

#[test]
fn a_reply_releases_the_datagram_and_its_frame_is_whole() {
    world!(|stack| {
        let dscp = Dscp::new(46).unwrap();
        let socket = stack.open(5000, dscp).unwrap();
        let block = filled(stack.pool(), b"hello");
        let command = stack.send(at(0), &socket, peer(), block).unwrap();
        send_next(stack, at(0)).unwrap();
        let reply = arp_frame(Operation::Reply, (PEER, PEER_IP), OUR_IP);
        stack.input(at(1), &reply, &mut ());
        let demand = stack.poll_demand(at(1)).unwrap();
        assert_eq!(demand.class, Class::Data);
        assert_eq!(demand.peer, PEER);
        assert_eq!(demand.flow, Some(0));
        assert_eq!(demand.dscp, dscp);
        stack.transmit(at(1), demand.grant()).unwrap();
        let frame = &stack.port_mut().sent[1];
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
        assert_eq!(finished(stack.take_result(command)), Outcome::Accepted);
        assert_eq!(stack.pool().free(), BLOCKS);
    });
}

#[test]
fn a_datagram_beyond_the_network_goes_through_the_gateway() {
    world!(|stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"far");
        let far = Remote {
            address: [198, 51, 100, 1],
            port: 7,
        };
        let _command = stack.send(at(0), &socket, far, block).unwrap();
        send_next(stack, at(0)).unwrap();
        let (_, payload) = EthernetHeader::parse(&stack.port_mut().sent[0]).unwrap();
        assert_eq!(
            Arp::parse(payload).unwrap().unwrap().target_protocol,
            GATEWAY
        );
    });
}

#[test]
fn no_route_refuses_and_returns_the_block() {
    world_on!(interface(OUR_IP, 24, None), |stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"far");
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
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"hello");
        let command = stack.send(at(1), &socket, peer(), block).unwrap();
        stack.port_mut().full = true;
        assert_eq!(send_next(stack, at(1)), Err(Unsent::Port("full")));
        assert_eq!(
            stack.pool().free(),
            BLOCKS - 1,
            "the data stays with the core"
        );
        let command = running(stack.take_result(command));
        stack.port_mut().full = false;
        send_next(stack, at(2)).unwrap();
        assert_eq!(finished(stack.take_result(command)), Outcome::Accepted);
    });
}

#[test]
fn an_accepted_frame_is_never_offered_again() {
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"once");
        let _command = stack.send(at(1), &socket, peer(), block).unwrap();
        send_next(stack, at(1)).unwrap();
        assert!(stack.poll_demand(at(1)).is_none());
        assert_eq!(stack.port_mut().sent.len(), 1);
    });
}

fn contradicted<B: core::fmt::Debug>(stack: &mut Stack<B>, lazy: bool, careless: bool) {
    knows_peer(stack);
    let socket = stack.open(5000, best_effort()).unwrap();
    let block = filled(stack.pool(), b"x");
    let command = stack.send(at(1), &socket, peer(), block).unwrap();
    stack.port_mut().lazy = lazy;
    stack.port_mut().careless = careless;
    stack.port_mut().room = Some(10);
    assert_eq!(send_next(stack, at(1)), Err(Unsent::PortContract));
    assert_eq!(finished(stack.take_result(command)), Outcome::Unknown);
    assert!(stack.poll_demand(at(1)).is_none(), "not sent again");
    assert_eq!(stack.pool().free(), BLOCKS);
}

#[test]
fn a_port_that_says_yes_without_writing_ends_the_work_unrepeated() {
    world!(|stack| contradicted(stack, true, false));
}

#[test]
fn a_port_that_says_yes_after_a_failed_write_ends_the_work_unrepeated() {
    world!(|stack| contradicted(stack, false, true));
}

#[test]
fn room_that_is_too_short_sends_nothing() {
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"hello");
        let _command = stack.send(at(1), &socket, peer(), block).unwrap();
        stack.port_mut().room = Some(10);
        assert_eq!(send_next(stack, at(1)), Err(Unsent::Port("short")));
        assert_eq!(stack.poll_demand(at(1)).unwrap().class, Class::Data);
    });
}

#[test]
fn a_grant_of_a_reply_cannot_answer_a_later_asker() {
    world!(|stack| {
        let first = arp_frame(Operation::Request, (PEER, PEER_IP), OUR_IP);
        stack.input(at(0), &first, &mut ());
        let stale = stack.poll_demand(at(0)).unwrap().grant();
        let again = stack.poll_demand(at(0)).unwrap();
        stack.transmit(at(0), again.grant()).unwrap();
        let second = arp_frame(Operation::Request, (MOVED, [192, 0, 2, 3]), OUR_IP);
        stack.input(at(0), &second, &mut ());
        assert_eq!(stack.transmit(at(0), stale), Err(Unsent::Stale));
        assert_eq!(stack.port_mut().sent.len(), 1);
    });
}

#[test]
fn a_grant_stays_stale_however_often_its_place_is_used() {
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"first");
        let command = stack.send(at(1), &socket, peer(), block).unwrap();
        let stale = stack.demands(at(1)).next().unwrap().grant();
        send_next(stack, at(1)).unwrap();
        assert_eq!(finished(stack.take_result(command)), Outcome::Accepted);
        // Past any 16-bit generation.
        for _ in 0..70_000 {
            let block = filled(stack.pool(), b"later");
            let command = stack.send(at(1), &socket, peer(), block).unwrap();
            send_next(stack, at(1)).unwrap();
            assert_eq!(finished(stack.take_result(command)), Outcome::Accepted);
            stack.port_mut().sent.clear();
        }
        let block = filled(stack.pool(), b"waiting");
        let _command = stack.send(at(1), &socket, peer(), block).unwrap();
        assert_eq!(stack.transmit(at(1), stale), Err(Unsent::Stale));
        assert!(stack.port_mut().sent.is_empty());
    });
}

#[test]
fn a_changed_neighbor_reaches_a_datagram_that_waits() {
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"x");
        let _command = stack.send(at(1), &socket, peer(), block).unwrap();
        let moved = arp_frame(Operation::Reply, (MOVED, PEER_IP), OUR_IP);
        stack.input(at(2), &moved, &mut ());
        let demand = stack.poll_demand(at(2)).unwrap();
        assert_eq!(demand.peer, MOVED);
        stack.transmit(at(2), demand.grant()).unwrap();
        let (header, _) = EthernetHeader::parse(&stack.port_mut().sent[0]).unwrap();
        assert_eq!(header.destination, MOVED);
    });
}

#[test]
fn a_neighbor_that_expires_under_a_datagram_is_asked_again() {
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"x");
        let command = stack.send(at(1), &socket, peer(), block).unwrap();
        let stale = stack.demands(at(1)).next().unwrap().grant();
        let later = at(0).saturating_add(Duration::from_secs(61));
        assert_eq!(
            stack.transmit(later, stale),
            Err(Unsent::Stale),
            "no frame to an address that expired"
        );
        stack.tick(later);
        assert_eq!(stack.poll_demand(later).unwrap().class, Class::Control);
        let command = running(stack.take_result(command));
        let mut now = later;
        while let Some(demand) = stack.poll_demand(now) {
            stack.transmit(now, demand.grant()).unwrap();
            now = stack.poll_deadline().unwrap();
            stack.tick(now);
        }
        assert_eq!(finished(stack.take_result(command)), Outcome::Unreachable);
        assert_eq!(stack.pool().free(), BLOCKS);
    });
}

#[test]
fn every_ready_piece_of_work_is_listed_with_who_it_is_for() {
    world!(|stack| {
        knows_peer(stack);
        let first = stack.open(5000, Dscp::new(46).unwrap()).unwrap();
        let second = stack.open(5001, best_effort()).unwrap();
        for socket in [&first, &second] {
            let block = filled(stack.pool(), b"x");
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
    small!(|stack| {
        let request = arp_frame(Operation::Request, (PEER, PEER_IP), OUR_IP);
        stack.input(at(0), &request, &mut ());
        let demand = stack.poll_demand(at(0)).unwrap();
        stack.transmit(at(0), demand.grant()).unwrap();
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = stack.pool().take().unwrap();
        let command = stack.send(at(1), &socket, peer(), block).unwrap();
        stack.drop_observer(command);
        let second = stack.pool().take().unwrap();
        let refused = stack.send(at(1), &socket, peer(), second).unwrap_err();
        assert_eq!(refused.reason, SendRefusal::NoPlace);
        let demand = stack.poll_demand(at(1)).unwrap();
        stack.transmit(at(1), demand.grant()).unwrap();
        let command = stack.send(at(2), &socket, peer(), refused.block).unwrap();
        let demand = stack.poll_demand(at(2)).unwrap();
        stack.transmit(at(2), demand.grant()).unwrap();
        assert_eq!(finished(stack.take_result(command)), Outcome::Accepted);
    });
}

#[test]
fn a_close_has_its_own_place_when_every_command_place_is_taken() {
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"one");
        let first = stack.send(at(1), &socket, peer(), block).unwrap();
        let block = filled(stack.pool(), b"two");
        let second = stack.send(at(1), &socket, peer(), block).unwrap();
        let close = stack.close(socket).unwrap();
        assert_eq!(finished(stack.take_result(first)), Outcome::Closed);
        assert_eq!(finished(stack.take_result(second)), Outcome::Closed);
        assert_eq!(finished(stack.take_result(close)), Outcome::SocketClosed);
        assert_eq!(stack.pool().free(), BLOCKS);
    });
}

#[test]
fn a_reused_socket_waits_for_the_old_ones_results() {
    small!(|stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = stack.pool().take().unwrap();
        let old = stack.send(at(0), &socket, peer(), block).unwrap();
        let close = stack.close(socket).unwrap();
        assert_eq!(
            stack.open(5000, best_effort()).err(),
            Some(OpenRefused::NoSocket),
            "results unread"
        );
        stack.drop_observer(old);
        assert_eq!(finished(stack.take_result(close)), Outcome::SocketClosed);
        let reused = stack.open(5000, best_effort()).unwrap();
        assert_eq!(stack.pool().free(), BLOCKS);
        assert!(stack.close(reused).is_ok());
    });
}

/// Three datagrams to three neighbors, over a table of two: the third waits
/// outside, and no datagram asks another's neighbor away.
#[test]
fn a_datagram_never_asks_anothers_neighbor_away() {
    world!(|stack| {
        let hops = [[192, 0, 2, 2], [192, 0, 2, 3], [192, 0, 2, 4]];
        let first = stack.open(5000, best_effort()).unwrap();
        let second = stack.open(5001, best_effort()).unwrap();
        let remote = |address| Remote { address, port: 7 };
        let block = filled(stack.pool(), b"one");
        let one = stack.send(at(0), &first, remote(hops[0]), block).unwrap();
        let block = filled(stack.pool(), b"two");
        let two = stack.send(at(0), &second, remote(hops[1]), block).unwrap();
        for hop in &hops[..2] {
            send_next(stack, at(0)).unwrap();
            let reply = arp_frame(Operation::Reply, (PEER, *hop), OUR_IP);
            stack.input(at(1), &reply, &mut ());
        }
        // Both neighbors are known and needed: neither is evicted for a third.
        let block = filled(stack.pool(), b"three");
        let refused = stack
            .send(at(2), &second, remote(hops[2]), block)
            .unwrap_err();
        assert_eq!(refused.reason, SendRefusal::NeighborsFull);
        stack.tick(at(2));
        send_next(stack, at(2)).unwrap();
        send_next(stack, at(2)).unwrap();
        assert_eq!(finished(stack.take_result(one)), Outcome::Accepted);
        assert_eq!(finished(stack.take_result(two)), Outcome::Accepted);
        let three = stack
            .send(at(3), &second, remote(hops[2]), refused.block)
            .unwrap();
        send_next(stack, at(3)).unwrap();
        let reply = arp_frame(Operation::Reply, (PEER, hops[2]), OUR_IP);
        stack.input(at(4), &reply, &mut ());
        send_next(stack, at(4)).unwrap();
        assert_eq!(finished(stack.take_result(three)), Outcome::Accepted);
    });
}

#[test]
fn a_grant_of_a_request_cannot_serve_a_later_resolution() {
    world!(|stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"x");
        let first = stack.send(at(0), &socket, peer(), block).unwrap();
        let stale = stack.demands(at(0)).next().unwrap().grant();
        let mut now = at(0);
        while let Some(demand) = stack.poll_demand(now) {
            stack.transmit(now, demand.grant()).unwrap();
            now = stack.poll_deadline().unwrap();
            stack.tick(now);
        }
        assert_eq!(finished(stack.take_result(first)), Outcome::Unreachable);
        let block = filled(stack.pool(), b"y");
        let _second = stack.send(now, &socket, peer(), block).unwrap();
        stack.port_mut().sent.clear();
        assert_eq!(stack.transmit(now, stale), Err(Unsent::Stale));
        assert!(stack.port_mut().sent.is_empty());
    });
}

#[test]
fn a_grant_chosen_for_one_address_does_not_go_to_another() {
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"x");
        let _command = stack.send(at(1), &socket, peer(), block).unwrap();
        let chosen = stack.poll_demand(at(1)).unwrap().grant();
        let moved = arp_frame(Operation::Reply, (MOVED, PEER_IP), OUR_IP);
        stack.input(at(2), &moved, &mut ());
        assert_eq!(stack.transmit(at(2), chosen), Err(Unsent::Stale));
        assert!(stack.port_mut().sent.is_empty());
        assert_eq!(stack.poll_demand(at(2)).unwrap().peer, MOVED);
    });
}

#[test]
fn a_neighbor_expiring_under_a_datagram_names_its_time() {
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"x");
        let _command = stack.send(at(1), &socket, peer(), block).unwrap();
        let expiry = at(0).saturating_add(Duration::from_secs(60));
        assert_eq!(stack.poll_deadline(), Some(expiry));
        assert!(stack.demands(expiry).next().is_none(), "past its life");
        stack.tick(expiry);
        assert_eq!(stack.demands(expiry).next().unwrap().class, Class::Control);
    });
}

#[test]
fn every_reply_owed_is_listed_and_any_can_be_chosen() {
    world!(|stack| {
        let first = arp_frame(Operation::Request, (PEER, PEER_IP), OUR_IP);
        let second = arp_frame(Operation::Request, (MOVED, [192, 0, 2, 3]), OUR_IP);
        stack.input(at(0), &first, &mut ());
        stack.input(at(0), &second, &mut ());
        let chosen = stack.demands(at(0)).nth(1).unwrap();
        assert_eq!(chosen.peer, MOVED);
        stack.transmit(at(0), chosen.grant()).unwrap();
        let left: Vec<_> = stack.demands(at(0)).map(|demand| demand.peer).collect();
        assert_eq!(left, [PEER]);
    });
}

#[test]
fn a_brand_claims_once() {
    brand!(Once);
    let _first = claimed::<Once>();
    let memory = std::boxed::Box::leak(std::boxed::Box::new([[0; SIZE]; BLOCKS]));
    let refused = claim::<Once, SIZE, BLOCKS>(memory).err().unwrap();
    assert_eq!(refused.reason, BrandRefusal::Claimed);
}

#[test]
fn a_retired_core_returns_its_blocks_and_the_next_finds_old_handles_gone() {
    brand!(Net);
    let (pool, brand) = claimed::<Net>();
    let local = interface(OUR_IP, 24, None);
    let mut stack: Stack<Net> = NetworkOwner::new(brand, local, pool, Port::default());
    let socket = stack.open(5000, best_effort()).unwrap();
    let block = filled(stack.pool(), b"waits");
    let command = stack.send(at(0), &socket, peer(), block).unwrap();
    stack.input(at(0), &udp_frame(7, 5000, b"unread"), &mut ());
    let held = stack.pool().take().unwrap();
    let stale = stack.demands(at(0)).next().unwrap().grant();
    let mut retired = stack.retire();
    assert_eq!(retired.pool.outstanding(), 1, "the application's block");
    assert_eq!(retired.results.received_dropped, 1);
    retired.pool.give(held);
    assert_eq!(retired.pool.outstanding(), 0);
    let mut next: Stack<Net> = NetworkOwner::new(retired.brand, local, retired.pool, retired.port);
    assert_eq!(next.take_result(command), CommandState::Gone);
    assert_eq!(next.receive(&socket).err(), Some(crate::StaleSocket));
    assert_eq!(next.transmit(at(0), stale), Err(Unsent::Stale));
    let reused = next.open(5000, best_effort()).unwrap();
    assert_ne!(reused, socket);
}

#[test]
fn a_confirmed_result_outlives_its_core_unread() {
    brand!(Net);
    let (pool, brand) = claimed::<Net>();
    let local = interface(OUR_IP, 24, Some(GATEWAY));
    let mut stack: NetworkOwner<'static, Net, SIZE, 3, 2, 2, 2, BLOCKS, Port> =
        NetworkOwner::new(brand, local, pool, Port::default());
    let request = arp_frame(Operation::Request, (PEER, PEER_IP), OUR_IP);
    stack.input(at(0), &request, &mut ());
    let demand = stack.poll_demand(at(0)).unwrap();
    stack.transmit(at(0), demand.grant()).unwrap();
    let first = stack.open(5000, best_effort()).unwrap();
    let block = filled(stack.pool(), b"one");
    let read = stack.send(at(1), &first, peer(), block).unwrap();
    let block = filled(stack.pool(), b"two");
    let unread = stack.send(at(1), &first, peer(), block).unwrap();
    for _ in 0..2 {
        let demand = stack.poll_demand(at(1)).unwrap();
        stack.transmit(at(1), demand.grant()).unwrap();
    }
    assert_eq!(finished(stack.take_result(read)), Outcome::Accepted);
    let second = stack.open(5001, best_effort()).unwrap();
    let block = filled(stack.pool(), b"cancelled");
    let cancelled = stack.send(at(1), &second, peer(), block).unwrap();
    let close = stack.close(second).unwrap();
    let block = filled(stack.pool(), b"running");
    let running = stack.send(at(1), &first, peer(), block).unwrap();
    let third = stack.open(5002, best_effort()).unwrap();
    let block = filled(stack.pool(), b"unobserved");
    let unobserved = stack.send(at(1), &third, peer(), block).unwrap();
    stack.drop_observer(unobserved);
    let mut retired = stack.retire();
    assert_eq!(retired.pool.free(), BLOCKS);
    assert_eq!(retired.results.unread(), 4);
    let results = &mut retired.results;
    assert_eq!(finished(results.take(unread)), Outcome::Accepted);
    assert_eq!(finished(results.take(cancelled)), Outcome::Closed);
    assert_eq!(finished(results.take(close)), Outcome::SocketClosed);
    assert_eq!(finished(results.take(running)), Outcome::Retired);
    assert_eq!(results.unread(), 0);
}

#[test]
fn a_network_task_holds_its_core_across_an_await() {
    use core::task::{Context, Poll, Waker};
    brand!(Net);
    async fn service(pool: Blocks<Net>, brand: CoreBrand<Net>, interface: Interface) -> bool {
        let mut stack: Stack<Net> = NetworkOwner::new(brand, interface, pool, Port::default());
        let socket = stack.open(5000, best_effort()).unwrap();
        embassy_futures::yield_now().await;
        stack.close(socket).is_ok()
    }
    fn task<F: Future + 'static>(future: F) -> F {
        future
    }
    let (pool, brand) = claimed::<Net>();
    let run = task(service(pool, brand, interface(OUR_IP, 24, None)));
    let mut run = core::pin::pin!(run);
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(run.as_mut().poll(&mut cx), Poll::Pending);
    assert_eq!(run.as_mut().poll(&mut cx), Poll::Ready(true));
}

#[test]
fn a_closed_socket_leaves_no_neighbor_asked_for() {
    world!(|stack| {
        stack.port_mut().full = true;
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"x");
        let gone = stack.send(at(0), &socket, peer(), block).unwrap();
        assert!(send_next(stack, at(0)).is_err());
        let close = stack.close(socket).unwrap();
        assert_eq!(finished(stack.take_result(gone)), Outcome::Closed);
        assert_eq!(finished(stack.take_result(close)), Outcome::SocketClosed);
        assert!(
            stack.poll_demand(at(0)).is_none(),
            "nobody needs the answer"
        );
        let socket = stack.open(5000, best_effort()).unwrap();
        for last in [3, 4] {
            let block = filled(stack.pool(), b"y");
            let remote = Remote {
                address: [192, 0, 2, last],
                port: 7,
            };
            assert!(stack.send(at(0), &socket, remote, block).is_ok());
        }
    });
}

#[test]
fn a_port_that_contradicted_itself_publishes_nothing_more() {
    brand!(Net);
    let (pool, brand) = claimed::<Net>();
    let local = interface(OUR_IP, 24, None);
    let mut stack: Stack<Net> = NetworkOwner::new(brand, local, pool, Port::default());
    let socket = stack.open(5000, best_effort()).unwrap();
    let block = filled(stack.pool(), b"x");
    let _command = stack.send(at(0), &socket, peer(), block).unwrap();
    stack.port_mut().lazy = true;
    assert_eq!(send_next(&mut stack, at(0)), Err(Unsent::PortContract));
    assert!(stack.port_broken());
    stack.port_mut().lazy = false;
    let now = stack.poll_deadline().unwrap();
    stack.tick(now);
    assert_eq!(send_next(&mut stack, now), Err(Unsent::PortContract));
    assert!(stack.port().sent.is_empty(), "no later ARP request");
    let retired = stack.retire();
    let next: Stack<Net> = NetworkOwner::new(retired.brand, local, retired.pool, retired.port);
    assert!(!next.port_broken());
}

#[test]
fn a_neighbor_that_never_answers_ends_its_datagrams_after_its_requests() {
    world!(|stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"lost");
        let mut command = stack.send(at(0), &socket, peer(), block).unwrap();
        let mut now = at(0);
        let mut requests = 0;
        while let Some(demand) = stack.poll_demand(now) {
            stack.transmit(now, demand.grant()).unwrap();
            requests += 1;
            command = running(stack.take_result(command));
            now = stack.poll_deadline().unwrap();
            stack.tick(now);
        }
        assert_eq!(requests, 3);
        assert_eq!(now, at(3000), "one request per second");
        assert_eq!(finished(stack.take_result(command)), Outcome::Unreachable);
        assert_eq!(stack.pool().free(), BLOCKS);
    });
}

#[test]
fn a_request_refused_by_the_port_counts_for_nothing() {
    world!(|stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"x");
        let _command = stack.send(at(0), &socket, peer(), block).unwrap();
        stack.port_mut().full = true;
        assert!(send_next(stack, at(0)).is_err());
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
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"x");
        let _command = stack.send(at(1), &socket, peer(), block).unwrap();
        let request = arp_frame(Operation::Request, (PEER, PEER_IP), OUR_IP);
        stack.input(at(1), &request, &mut ());
        let demand = stack.poll_demand(at(1)).unwrap();
        assert_eq!(demand.class, Class::Control);
        stack.transmit(at(1), demand.grant()).unwrap();
        let (header, payload) = EthernetHeader::parse(&stack.port_mut().sent[0]).unwrap();
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
    world!(|stack| {
        let mut dropped = Dropped::default();
        let request = arp_frame(Operation::Request, (PEER, PEER_IP), OUR_IP);
        for _ in 0..3 {
            stack.input(at(0), &request, &mut dropped);
        }
        assert_eq!(dropped.0, [NetEvent::ReplyDropped]);
    });
}

#[test]
fn an_arp_packet_for_another_makes_no_entry() {
    world!(|stack| {
        let other = arp_frame(Operation::Request, (PEER, PEER_IP), [192, 0, 2, 9]);
        stack.input(at(0), &other, &mut ());
        assert!(stack.poll_demand(at(0)).is_none());
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"x");
        let _command = stack.send(at(0), &socket, peer(), block).unwrap();
        assert_eq!(stack.poll_demand(at(0)).unwrap().class, Class::Control);
    });
}

#[test]
fn a_datagram_reaches_its_socket_and_its_block_is_the_applications() {
    world!(|stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        stack.input(at(0), &udp_frame(7, 5000, b"payload"), &mut ());
        let received = stack.receive(&socket).unwrap().unwrap();
        assert_eq!(received.block.octets(), b"payload");
        assert_eq!(received.remote, peer());
        assert_eq!(stack.pool().free(), BLOCKS - 1);
        stack.pool().give(received.block);
        assert!(stack.receive(&socket).unwrap().is_none());
    });
}

#[test]
fn received_datagrams_are_refused_with_a_counted_reason() {
    world!(|stack| {
        let mut dropped = Dropped::default();
        let socket = stack.open(5000, best_effort()).unwrap();
        stack.input(at(0), &udp_frame(7, 6000, b"x"), &mut dropped);
        let mut broken = udp_frame(7, 5000, b"x");
        *broken.last_mut().unwrap() ^= 1;
        stack.input(at(0), &broken, &mut dropped);
        for _ in 0..3 {
            stack.input(at(0), &udp_frame(7, 5000, b"x"), &mut dropped);
        }
        assert_eq!(
            dropped.0,
            [
                NetEvent::NoSocket,
                NetEvent::Checksum,
                NetEvent::ReceiveFull
            ]
        );
        assert_eq!(stack.pool().free(), BLOCKS - 2);
        let _first = stack.receive(&socket).unwrap().unwrap();
    });
}

#[test]
fn a_broken_ipv4_header_is_counted_not_delivered() {
    world!(|stack| {
        let mut dropped = Dropped::default();
        let socket = stack.open(5000, best_effort()).unwrap();
        let mut frame = udp_frame(7, 5000, b"x");
        frame[14 + 10] ^= 1;
        stack.input(at(0), &frame, &mut dropped);
        assert_eq!(dropped.0, [NetEvent::Checksum]);
        assert!(stack.receive(&socket).unwrap().is_none());
    });
}

#[test]
fn a_fragment_is_counted_not_delivered() {
    world!(|stack| {
        let mut dropped = Dropped::default();
        let socket = stack.open(5000, best_effort()).unwrap();
        let mut frame = udp_frame(7, 5000, b"x");
        // More Fragments, then the header's checksum again.
        frame[14 + 6] |= 0x20;
        frame[14 + 10..14 + 12].copy_from_slice(&[0, 0]);
        let field = Checksum::new().with(&frame[14..34]).finish();
        frame[14 + 10..14 + 12].copy_from_slice(&field.to_be_bytes());
        stack.input(at(0), &frame, &mut dropped);
        assert_eq!(dropped.0, [NetEvent::Fragment]);
        assert!(stack.receive(&socket).unwrap().is_none());
    });
}

#[test]
fn no_block_drops_a_received_datagram_and_counts_it() {
    world!(|stack| {
        let mut dropped = Dropped::default();
        let _socket = stack.open(5000, best_effort()).unwrap();
        let mut held = Vec::new();
        while let Some(block) = stack.pool().take() {
            held.push(block);
        }
        stack.input(at(0), &udp_frame(7, 5000, b"x"), &mut dropped);
        assert_eq!(dropped.0, [NetEvent::NoBlock]);
        for block in held {
            stack.pool().give(block);
        }
    });
}

#[test]
fn a_frame_cut_at_any_length_is_handled() {
    world!(|stack| {
        let _socket = stack.open(5000, best_effort()).unwrap();
        let udp = udp_frame(7, 5000, b"payload");
        let arp = arp_frame(Operation::Request, (PEER, PEER_IP), OUR_IP);
        for frame in [&udp, &arp] {
            for length in 0..frame.len() {
                stack.input(at(0), &frame[..length], &mut ());
            }
        }
    });
}

#[test]
fn a_frame_for_another_station_is_ignored() {
    world!(|stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let mut frame = udp_frame(7, 5000, b"x");
        frame[5] = 9;
        stack.input(at(0), &frame, &mut ());
        assert!(stack.receive(&socket).unwrap().is_none());
    });
}

#[test]
fn a_host_prefix_has_no_broadcast_of_its_own() {
    world_on!(interface(PEER_IP, 31, None), |stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"x");
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
    world!(|stack| {
        knows_peer(stack);
        let first = stack.open(5000, best_effort()).unwrap();
        let second = stack.open(5001, best_effort()).unwrap();
        for socket in [&first, &second, &first, &second] {
            let block = filled(stack.pool(), b"x");
            let _command = stack.send(at(1), socket, peer(), block).unwrap();
        }
        while stack.demands(at(1)).next().is_some() {
            send_next(stack, at(1)).unwrap();
        }
        let order: Vec<_> = stack
            .port()
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
    world!(|stack| {
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
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"x");
        let later = at(0).saturating_add(Duration::from_secs(61));
        let _command = stack.send(later, &socket, peer(), block).unwrap();
        assert_eq!(stack.poll_demand(later).unwrap().class, Class::Control);
    });
}
