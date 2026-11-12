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
    Block, BrandRefusal, CommandState, Core, CoreBrand, Eviction, GROUPS, Group, GroupsFull,
    Interface, InterfaceError, LoansRefused, NetEvent, NetworkOwner, NotLent, OpenRefused, Outcome,
    Pool, Remote, SendRefusal, Unsent, claim,
};
use oer_protocols_network_port::{Class, FramePort, Unpublished};

const SIZE: usize = 1536;
const BLOCKS: usize = 8;
const LOCAL: [u8; 6] = [2, 0, 0, 0, 0, 1];
const PEER: [u8; 6] = [2, 0, 0, 0, 0, 2];
const MOVED: [u8; 6] = [2, 0, 0, 0, 0, 3];
const OUR_IP: [u8; 4] = [192, 0, 2, 1];
const PEER_IP: [u8; 4] = [192, 0, 2, 2];
const GATEWAY: [u8; 4] = [192, 0, 2, 254];

/// A declared work's tag in these tests: a number each test chooses.
type Stack<B> = NetworkOwner<'static, B, SIZE, 2, 2, 2, 2, BLOCKS, Port, u32>;
type Small<B> = NetworkOwner<'static, B, SIZE, 1, 1, 1, 2, BLOCKS, Port, u32>;
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
    /// Refuses every frame for good.
    rejects: bool,
    /// Gives less room than was asked for.
    room: Option<usize>,
    /// Answers a failed fill with a refusal of its own.
    misreports: bool,
    /// Refuses a frame once it is written.
    after_fill: Option<&'static str>,
    sent: Vec<Vec<u8>>,
    /// The link's epoch while it is up.
    up: Option<u64>,
    /// Frames to take, each held as said.
    received: std::collections::VecDeque<(Vec<u8>, oer_protocols_network_port::Holding)>,
    /// Writes every frame into one buffer it keeps, and the run of one octet
    /// that buffer holds, as a port that keeps its buffers' runs does.
    keeps_buffer: bool,
    buffer: Option<(Vec<u8>, Option<oer_protocols_network_port::Held>)>,
}

/// A frame the tests' port lent, in its own octets.
#[derive(Debug)]
struct Held(Vec<u8>);

impl oer_protocols_network_port::EthernetFrame for Held {
    fn view(
        &mut self,
    ) -> Result<
        oer_protocols_network_port::EthernetFrameView<'_>,
        oer_protocols_network_port::FrameLengthError,
    > {
        let (header, payload) = EthernetHeader::parse(&self.0)
            .map_err(|_| oer_protocols_network_port::FrameLengthError::OutsideStorage)?;
        Ok(oer_protocols_network_port::EthernetFrameView {
            destination: header.destination,
            source: header.source,
            ether_type: header.ether_type.value(),
            payload,
        })
    }
}

impl oer_protocols_network_port::FrameSource for Port {
    type Notice = ();
    type Held = Held;

    fn take_frames(
        &mut self,
        limit: usize,
        mut take: impl FnMut(oer_protocols_network_port::Incoming<'_, Held>),
    ) -> oer_protocols_network_port::ReceivedBatch {
        use oer_protocols_network_port::{
            BatchEnd, EthernetFrame, Holding, Incoming, ReceivedBatch,
        };
        let mut taken = 0;
        let end = loop {
            if taken >= limit {
                break BatchEnd::Limit;
            }
            let Some((bytes, holding)) = self.received.pop_front() else {
                break BatchEnd::Drained;
            };
            taken += 1;
            let mut held = Held(bytes);
            match holding {
                Holding::Lendable => take(Incoming::Lent(held)),
                Holding::Copied(reason) => take(Incoming::Copied {
                    frame: held.view(),
                    reason,
                }),
            }
        };
        ReceivedBatch { taken, end }
    }

    fn take_notice(&mut self) -> Option<()> {
        None
    }

    fn link(&self) -> oer_protocols_network_port::LinkSnapshot {
        self.up
            .map_or(oer_protocols_network_port::LinkSnapshot::DOWN, |epoch| {
                oer_protocols_network_port::LinkSnapshot {
                    state: oer_protocols_network_port::LinkState::Up,
                    epoch,
                }
            })
    }

    fn register(
        &mut self,
        _: &mut core::task::Context<'_>,
    ) -> oer_protocols_network_port::PortReady {
        oer_protocols_network_port::PortReady {
            received: !self.received.is_empty(),
            other: false,
        }
    }
}

impl FramePort for Port {
    type Refusal = &'static str;
    type Ticket = ();

    fn is_final(refusal: &&'static str) -> bool {
        *refusal == "rejected"
    }

    fn is_fault(refusal: &&'static str) -> bool {
        *refusal == "lost"
    }

    fn publish<E>(
        &mut self,
        meta: oer_protocols_network_port::FrameMeta,
        fill: impl FnOnce(&mut oer_protocols_network_port::Slot<'_>) -> Result<(), E>,
    ) -> Result<(), Unpublished<&'static str, E>> {
        let length = meta.length;
        if self.full {
            return Err(Unpublished::Port("full"));
        }
        if self.rejects {
            return Err(Unpublished::Port("rejected"));
        }
        if self.lazy {
            return Ok(());
        }
        let room = self.room.unwrap_or(length);
        let (mut slot, held) = match self.buffer.take() {
            Some((mut buffer, held)) => {
                buffer.resize(room, 0);
                (buffer, held)
            }
            None => (vec![0; room], None),
        };
        let mut room = oer_protocols_network_port::Slot::new(&mut slot, held);
        let written = fill(&mut room);
        let held = room.held();
        if self.keeps_buffer {
            self.buffer = Some((slot.clone(), held));
        }
        if self.careless {
            return Ok(());
        }
        if let Err(error) = written {
            return Err(if self.misreports {
                Unpublished::Port("misreported")
            } else {
                Unpublished::Fill(error)
            });
        }
        if let Some(refusal) = self.after_fill {
            return Err(Unpublished::Port(refusal));
        }
        // What the port was told is what was written: every frame any test
        // publishes checks it.
        let frame = slot.get(..length).expect("the frame fits its room");
        let (ethernet, rest) = oer_protocols_network_wire::ethernet::EthernetHeader::parse(frame)
            .expect("a written frame starts with its Ethernet header");
        let written = (ethernet.ether_type
            == oer_protocols_network_wire::ethernet::EtherType::IPV4)
            .then(|| {
                oer_protocols_network_wire::ip::Ipv4Header::parse(rest)
                    .expect("an IPv4 frame's header parses")
                    .dscp()
            });
        assert_eq!(
            meta.dscp, written,
            "the DSCP the port was told is the one written"
        );
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

/// The fill of a test whose work no producer declared: only a declared
/// work's frame calls it.
fn unproduced<T>(
    _: crate::WorkId,
    _: T,
    _: &mut crate::Payload<'_>,
) -> Result<(), crate::Declined> {
    panic!("no work of this test is a producer's")
}

/// Send the next ready work.
fn send_next<B>(stack: &mut Stack<B>, now: Instant) -> Result<(), Unsent<&'static str>> {
    let demand = stack.poll_demand(now).unwrap();
    // No declared work: no producer's outcome comes with the frame.
    stack
        .transmit(now, demand.grant(), unproduced, &())
        .map(|_| ())
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
        assert_eq!(demand.ready().class, Class::Control);
        stack
            .transmit(at(0), demand.grant(), unproduced, &())
            .unwrap();
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
        assert_eq!(demand.ready().class, Class::Data);
        assert_eq!(demand.ready().peer, PEER);
        assert_eq!(demand.ready().flow, Some(0));
        assert_eq!(demand.ready().dscp, dscp);
        stack
            .transmit(at(1), demand.grant(), unproduced, &())
            .unwrap();
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
fn a_failed_fill_is_its_own_cause_and_the_port_goes_on() {
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"hello");
        let command = stack.send(at(1), &socket, peer(), block).unwrap();
        stack.port_mut().room = Some(10);
        assert_eq!(send_next(stack, at(1)), Err(Unsent::SlotTooShort));
        assert_eq!(finished(stack.take_result(command)), Outcome::Rejected);
        assert!(stack.poll_demand(at(1)).is_none(), "not offered again");
        assert_eq!(stack.pool().free(), BLOCKS);
        assert!(!stack.port_broken());
        stack.port_mut().room = None;
        let block = filled(stack.pool(), b"after");
        let command = stack.send(at(1), &socket, peer(), block).unwrap();
        send_next(stack, at(1)).unwrap();
        assert_eq!(finished(stack.take_result(command)), Outcome::Accepted);
    });
}

#[test]
fn a_port_refusal_after_a_failed_fill_is_the_port_s_fault() {
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"hello");
        let command = stack.send(at(1), &socket, peer(), block).unwrap();
        stack.port_mut().room = Some(10);
        stack.port_mut().misreports = true;
        assert_eq!(
            send_next(stack, at(1)),
            Err(Unsent::FillMisreported("misreported"))
        );
        assert_eq!(
            finished(stack.take_result(command)),
            Outcome::Rejected,
            "the fill's cause: the port says it took nothing"
        );
        assert!(stack.port_broken());
        assert_eq!(stack.pool().free(), BLOCKS);
        stack.port_mut().room = None;
        stack.port_mut().misreports = false;
        let block = filled(stack.pool(), b"after");
        let _command = stack.send(at(1), &socket, peer(), block).unwrap();
        assert_eq!(send_next(stack, at(1)), Err(Unsent::PortContract));
        assert!(stack.port().sent.is_empty());
    });
}

#[test]
fn a_yes_after_a_failed_fill_wins_over_the_fill_s_cause() {
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"hello");
        let command = stack.send(at(1), &socket, peer(), block).unwrap();
        stack.port_mut().room = Some(10);
        stack.port_mut().misreports = true;
        stack.port_mut().careless = true;
        assert_eq!(send_next(stack, at(1)), Err(Unsent::PortContract));
        assert_eq!(finished(stack.take_result(command)), Outcome::Unknown);
        assert!(stack.port_broken());
    });
}

#[test]
fn a_refusal_after_a_written_frame_is_the_port_s_own() {
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"hello");
        let command = stack.send(at(1), &socket, peer(), block).unwrap();
        stack.port_mut().after_fill = Some("full");
        assert_eq!(send_next(stack, at(1)), Err(Unsent::Port("full")));
        let command = running(stack.take_result(command));
        assert_eq!(stack.poll_demand(at(1)).unwrap().ready().class, Class::Data);
        stack.port_mut().after_fill = Some("rejected");
        assert_eq!(send_next(stack, at(1)), Err(Unsent::Rejected("rejected")));
        assert_eq!(finished(stack.take_result(command)), Outcome::Rejected);
        assert!(!stack.port_broken());
        assert_eq!(stack.pool().free(), BLOCKS);
    });
}

/// A port that fails after the fill: the frame did not go, so the work
/// ends rejected, with no repeat, its block home; the port is not trusted
/// again, every later frame refused as the broken port's, and the owner
/// learns it was a fault, not a contradiction.
#[test]
fn a_port_fault_after_the_fill_ends_the_work_unrepeated_and_breaks_the_port() {
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"hello");
        let command = stack.send(at(1), &socket, peer(), block).unwrap();
        stack.port_mut().after_fill = Some("lost");
        assert_eq!(send_next(stack, at(1)), Err(Unsent::PortFault("lost")));
        assert_eq!(finished(stack.take_result(command)), Outcome::Rejected);
        assert!(stack.poll_demand(at(1)).is_none(), "not sent again");
        assert_eq!(stack.broken(), Some(crate::Broken::Fault));
        let block = filled(stack.pool(), b"later");
        let _later = stack.send(at(2), &socket, peer(), block).unwrap();
        stack.port_mut().after_fill = None;
        assert_eq!(send_next(stack, at(2)), Err(Unsent::PortContract));
        assert_eq!(stack.port_mut().sent.len(), 0, "nothing went");
    });
}

#[test]
fn a_grant_of_a_reply_cannot_answer_a_later_asker() {
    world!(|stack| {
        let first = arp_frame(Operation::Request, (PEER, PEER_IP), OUR_IP);
        stack.input(at(0), &first, &mut ());
        let stale = stack.poll_demand(at(0)).unwrap().grant();
        let again = stack.poll_demand(at(0)).unwrap();
        stack
            .transmit(at(0), again.grant(), unproduced, &())
            .unwrap();
        let second = arp_frame(Operation::Request, (MOVED, [192, 0, 2, 3]), OUR_IP);
        stack.input(at(0), &second, &mut ());
        assert_eq!(
            stack.transmit(at(0), stale, unproduced, &()),
            Err(Unsent::Stale)
        );
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
        let stale = stack.demands(at(1), &()).next().unwrap().grant();
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
        assert_eq!(
            stack.transmit(at(1), stale, unproduced, &()),
            Err(Unsent::Stale)
        );
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
        assert_eq!(demand.ready().peer, MOVED);
        stack
            .transmit(at(2), demand.grant(), unproduced, &())
            .unwrap();
        let (header, _) = EthernetHeader::parse(&stack.port_mut().sent[0]).unwrap();
        assert_eq!(header.destination, MOVED);
    });
}

/// An expired neighbor that no work needs is not asked again: a turn asks
/// only for what some work goes through.
#[test]
fn a_neighbor_that_expires_with_no_work_is_not_asked_again() {
    world!(|stack| {
        knows_peer(stack);
        let _socket = stack.open(5000, best_effort()).unwrap();
        let later = at(0).saturating_add(Duration::from_secs(61));
        stack.tick(later, &mut ());
        assert!(stack.poll_demand(later).is_none(), "no request is owed");
        assert!(stack.port().sent.is_empty());
    });
}

#[test]
fn a_neighbor_that_expires_under_a_datagram_is_asked_again() {
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"x");
        let command = stack.send(at(1), &socket, peer(), block).unwrap();
        let stale = stack.demands(at(1), &()).next().unwrap().grant();
        let later = at(0).saturating_add(Duration::from_secs(61));
        assert_eq!(
            stack.transmit(later, stale, unproduced, &()),
            Err(Unsent::Stale),
            "no frame to an address that expired"
        );
        stack.tick(later, &mut ());
        assert_eq!(
            stack.poll_demand(later).unwrap().ready().class,
            Class::Control
        );
        let command = running(stack.take_result(command));
        let mut now = later;
        while let Some(demand) = stack.poll_demand(now) {
            stack
                .transmit(now, demand.grant(), unproduced, &())
                .unwrap();
            now = stack.poll_deadline().unwrap();
            stack.tick(now, &mut ());
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
            .demands(at(1), &())
            .map(|demand| {
                (
                    demand.ready().flow,
                    demand.ready().peer,
                    demand.ready().dscp.value(),
                )
            })
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
        stack
            .transmit(at(0), demand.grant(), unproduced, &())
            .unwrap();
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = stack.pool().take().unwrap();
        let command = stack.send(at(1), &socket, peer(), block).unwrap();
        stack.drop_observer(command);
        let second = stack.pool().take().unwrap();
        let refused = stack.send(at(1), &socket, peer(), second).unwrap_err();
        assert_eq!(refused.reason, SendRefusal::NoPlace);
        let demand = stack.poll_demand(at(1)).unwrap();
        stack
            .transmit(at(1), demand.grant(), unproduced, &())
            .unwrap();
        let command = stack.send(at(2), &socket, peer(), refused.block).unwrap();
        let demand = stack.poll_demand(at(2)).unwrap();
        stack
            .transmit(at(2), demand.grant(), unproduced, &())
            .unwrap();
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
        stack.tick(at(2), &mut ());
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
        let stale = stack.demands(at(0), &()).next().unwrap().grant();
        let mut now = at(0);
        while let Some(demand) = stack.poll_demand(now) {
            stack
                .transmit(now, demand.grant(), unproduced, &())
                .unwrap();
            now = stack.poll_deadline().unwrap();
            stack.tick(now, &mut ());
        }
        assert_eq!(finished(stack.take_result(first)), Outcome::Unreachable);
        let block = filled(stack.pool(), b"y");
        let _second = stack.send(now, &socket, peer(), block).unwrap();
        stack.port_mut().sent.clear();
        assert_eq!(
            stack.transmit(now, stale, unproduced, &()),
            Err(Unsent::Stale)
        );
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
        assert_eq!(
            stack.transmit(at(2), chosen, unproduced, &()),
            Err(Unsent::Stale)
        );
        assert!(stack.port_mut().sent.is_empty());
        assert_eq!(stack.poll_demand(at(2)).unwrap().ready().peer, MOVED);
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
        assert!(stack.demands(expiry, &()).next().is_none(), "past its life");
        stack.tick(expiry, &mut ());
        assert_eq!(
            stack.demands(expiry, &()).next().unwrap().ready().class,
            Class::Control
        );
    });
}

#[test]
fn every_reply_owed_is_listed_and_any_can_be_chosen() {
    world!(|stack| {
        let first = arp_frame(Operation::Request, (PEER, PEER_IP), OUR_IP);
        let second = arp_frame(Operation::Request, (MOVED, [192, 0, 2, 3]), OUR_IP);
        stack.input(at(0), &first, &mut ());
        stack.input(at(0), &second, &mut ());
        let chosen = stack.demands(at(0), &()).nth(1).unwrap();
        assert_eq!(chosen.ready().peer, MOVED);
        stack
            .transmit(at(0), chosen.grant(), unproduced, &())
            .unwrap();
        let left: Vec<_> = stack
            .demands(at(0), &())
            .map(|demand| demand.ready().peer)
            .collect();
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
    let stale = stack.demands(at(0), &()).next().unwrap().grant();
    let mut retired = stack.retire();
    assert_eq!(retired.pool.outstanding(), 1, "the application's block");
    assert_eq!(retired.results.received_dropped, 1);
    retired.pool.give(held);
    assert_eq!(retired.pool.outstanding(), 0);
    let mut next: Stack<Net> = NetworkOwner::new(retired.brand, local, retired.pool, retired.port);
    assert_eq!(next.take_result(command), CommandState::Gone);
    assert_eq!(next.receive(&socket).err(), Some(crate::StaleSocket));
    assert_eq!(
        next.transmit(at(0), stale, unproduced, &()),
        Err(Unsent::Stale)
    );
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
    stack
        .transmit(at(0), demand.grant(), unproduced, &())
        .unwrap();
    let first = stack.open(5000, best_effort()).unwrap();
    let block = filled(stack.pool(), b"one");
    let read = stack.send(at(1), &first, peer(), block).unwrap();
    let block = filled(stack.pool(), b"two");
    let unread = stack.send(at(1), &first, peer(), block).unwrap();
    for _ in 0..2 {
        let demand = stack.poll_demand(at(1)).unwrap();
        stack
            .transmit(at(1), demand.grant(), unproduced, &())
            .unwrap();
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
    stack.tick(now, &mut ());
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
            stack
                .transmit(now, demand.grant(), unproduced, &())
                .unwrap();
            requests += 1;
            command = running(stack.take_result(command));
            now = stack.poll_deadline().unwrap();
            stack.tick(now, &mut ());
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
        assert_eq!(demand.ready().class, Class::Control);
        stack
            .transmit(at(1), demand.grant(), unproduced, &())
            .unwrap();
        let (header, payload) = EthernetHeader::parse(&stack.port_mut().sent[0]).unwrap();
        assert_eq!(header.destination, PEER);
        let reply = Arp::parse(payload).unwrap().unwrap();
        assert_eq!(reply.operation, Operation::Reply);
        assert_eq!(reply.sender_protocol, OUR_IP);
        assert_eq!(reply.target_protocol, PEER_IP);
        assert_eq!(stack.poll_demand(at(1)).unwrap().ready().class, Class::Data);
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
        assert_eq!(
            stack.poll_demand(at(0)).unwrap().ready().class,
            Class::Control
        );
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
        assert_eq!(
            stack.poll_demand(at(0)).unwrap().ready().class,
            Class::Control
        );
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
        while stack.demands(at(1), &()).next().is_some() {
            send_next(stack, at(1)).unwrap();
        }
        assert_eq!(senders(stack), [5000, 5001, 5000, 5001]);
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
        assert_eq!(
            stack.poll_demand(later).unwrap().ready().class,
            Class::Control
        );
    });
}

/// The source port of each datagram the port took, in order.
fn senders<B>(stack: &Stack<B>) -> Vec<u16> {
    stack
        .port()
        .sent
        .iter()
        .map(|frame| {
            let (_, payload) = EthernetHeader::parse(frame).unwrap();
            let packet = Ipv4Header::parse(payload).unwrap();
            UdpHeader::parse(packet.payload()).unwrap().0.ports.source
        })
        .collect()
}

/// Two datagrams of the first socket and one of the second are ready, with
/// the first's turn; the first frame meets a final refusal.
fn refused_first<B: core::fmt::Debug>(stack: &mut Stack<B>, port: bool) {
    knows_peer(stack);
    let first = stack.open(5000, best_effort()).unwrap();
    let second = stack.open(5001, best_effort()).unwrap();
    for socket in [&first, &first, &second] {
        let block = filled(stack.pool(), b"x");
        let _command = stack.send(at(1), socket, peer(), block).unwrap();
    }
    stack.port_mut().rejects = port;
    stack.port_mut().room = (!port).then_some(10);
    let expected = if port {
        Unsent::Rejected("rejected")
    } else {
        Unsent::SlotTooShort
    };
    assert_eq!(send_next(stack, at(1)), Err(expected));
    stack.port_mut().rejects = false;
    stack.port_mut().room = None;
    while stack.demands(at(1), &()).next().is_some() {
        send_next(stack, at(1)).unwrap();
    }
    assert_eq!(senders(stack), [5001, 5000], "the second socket's turn");
}

#[test]
fn a_final_refusal_by_the_port_passes_the_turn_to_the_next_socket() {
    world!(|stack| refused_first(stack, true));
}

#[test]
fn a_failed_fill_passes_the_turn_to_the_next_socket() {
    world!(|stack| refused_first(stack, false));
}

#[test]
fn a_refused_datagram_keeps_its_turn() {
    world!(|stack| {
        let first = stack.open(1, best_effort()).unwrap();
        let second = stack.open(2, best_effort()).unwrap();
        let block = filled(stack.pool(), b"resolve");
        stack.send(at(0), &first, peer(), block).unwrap();
        send_next(stack, at(0)).unwrap();
        let reply = arp_frame(Operation::Reply, (PEER, PEER_IP), OUR_IP);
        stack.input(at(1), &reply, &mut ());
        send_next(stack, at(1)).unwrap();
        for socket in [&first, &second] {
            let block = filled(stack.pool(), b"x");
            stack.send(at(1), socket, peer(), block).unwrap();
        }
        stack.port_mut().full = true;
        let demand = stack.poll_demand(at(1)).unwrap();
        let flow = demand.ready().flow;
        assert!(
            stack
                .transmit(at(1), demand.grant(), unproduced, &())
                .is_err()
        );
        stack.port_mut().full = false;
        let demand = stack.poll_demand(at(1)).unwrap();
        assert_eq!(demand.ready().flow, flow, "refused work stays first");
        stack
            .transmit(at(1), demand.grant(), unproduced, &())
            .unwrap();
        assert_ne!(stack.poll_demand(at(1)).unwrap().ready().flow, flow);
    });
}

#[test]
fn an_evicted_datagram_returns_its_block_and_names_its_end() {
    world!(|stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"waits");
        let command = stack.send(at(3), &socket, peer(), block).unwrap();
        let held = stack.pool().free();
        let queued = stack.queued().next().unwrap();
        assert_eq!((queued.flow, queued.since), (socket.flow(), at(3)));
        assert!(stack.evict(queued, Eviction::MemoryPressure));
        assert_eq!(stack.pool().free(), held + 1);
        assert!(stack.queued().next().is_none());
        assert_eq!(
            stack.take_result(command),
            CommandState::Finished(Outcome::Evicted(Eviction::MemoryPressure))
        );
        assert!(stack.poll_deadline().is_none(), "no neighbor is asked for");
    });
}

#[test]
fn work_the_port_took_cannot_be_evicted() {
    world!(|stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"goes");
        let command = stack.send(at(0), &socket, peer(), block).unwrap();
        let reply = arp_frame(Operation::Reply, (PEER, PEER_IP), OUR_IP);
        send_next(stack, at(0)).unwrap();
        stack.input(at(1), &reply, &mut ());
        let queued = stack.queued().next().unwrap();
        send_next(stack, at(1)).unwrap();
        assert!(!stack.evict(queued, Eviction::MemoryPressure));
        assert_eq!(
            stack.take_result(command),
            CommandState::Finished(Outcome::Accepted)
        );
    });
}

#[test]
fn a_group_counts_a_block_until_the_pool_has_it_back() {
    brand!(Net);
    let (mut pool, _) = claimed::<Net>();
    let group = pool.group(1).unwrap();
    let block = pool.take_as(group).unwrap();
    assert_eq!(block.group(), group);
    assert_eq!(pool.held(group), 1);
    assert!(
        pool.take_as(group).is_none(),
        "free blocks do not lift a limit"
    );
    assert!(pool.take().is_some(), "the shared group has its own count");
    pool.give(block);
    assert_eq!(pool.held(group), 0);
    assert!(pool.take_as(group).is_some());
}

#[test]
fn the_heaviest_group_is_found_with_no_socket_in_it() {
    brand!(Net);
    let (mut pool, _) = claimed::<Net>();
    assert!(pool.heaviest().is_none(), "nothing is out");
    let first = pool.group(usize::MAX).unwrap();
    let second = pool.group(usize::MAX).unwrap();
    let _one = pool.take_as(first).unwrap();
    let _two = pool.take_as(second).unwrap();
    let _three = pool.take_as(second).unwrap();
    assert_eq!(pool.heaviest(), Some((second, 2)));
}

#[test]
fn a_pool_keeps_as_many_groups_as_it_can() {
    brand!(Net);
    let (mut pool, _) = claimed::<Net>();
    for _ in 1..GROUPS {
        pool.group(1).unwrap();
    }
    assert_eq!(pool.group(1), Err(GroupsFull));
}

/// A generator of no consequence: the walk below only needs to be repeatable.
struct Walk(u64);

impl Walk {
    fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        usize::try_from(self.0 % u64::try_from(bound).unwrap()).unwrap()
    }
}

/// What the test itself knows is out of the pool, apart from the pool's count.
struct Ledger<B: 'static> {
    /// In the application's hands, with the group each was taken for.
    held: Vec<(Block<'static, B, SIZE>, usize)>,
    /// Commands running, with their blocks' groups.
    running: Vec<(crate::CommandId<B>, usize)>,
    /// Datagrams waiting in each socket, by group.
    received: [Vec<usize>; 2],
}

impl<B: core::fmt::Debug + 'static> Ledger<B> {
    fn count(&self, group: usize) -> usize {
        self.held.iter().filter(|(_, of)| *of == group).count()
            + self.running.iter().filter(|(_, of)| *of == group).count()
            + self
                .received
                .iter()
                .flatten()
                .filter(|of| **of == group)
                .count()
    }

    /// Commands that ended have returned their blocks.
    fn harvest(&mut self, stack: &mut Stack<B>) {
        let running = core::mem::take(&mut self.running);
        for (command, group) in running {
            if let CommandState::Running(command) = stack.take_result(command) {
                self.running.push((command, group));
            }
        }
    }
}

/// Every way a block moves, in a repeatable order, with the pool's count held to
/// the test's own ledger after each step.
fn walk_blocks<B: core::fmt::Debug + 'static>(seed: u64) {
    const LIMITS: [usize; 3] = [usize::MAX, 2, 3];
    let (mut pool, brand) = claimed::<B>();
    let groups = [
        Group::shared(),
        pool.group(LIMITS[1]).unwrap(),
        pool.group(LIMITS[2]).unwrap(),
    ];
    let of_socket = [1, 2];
    let local = interface(OUR_IP, 24, None);
    let mut stack: Stack<B> = NetworkOwner::new(brand, local, pool, Port::default());
    let open = |stack: &mut Stack<B>, index: usize| {
        let port = 5000 + u16::try_from(index).unwrap();
        let group = groups[of_socket[index]];
        stack.open_in(port, best_effort(), group).unwrap()
    };
    let mut sockets = [Some(open(&mut stack, 0)), Some(open(&mut stack, 1))];
    let mut ledger = Ledger {
        held: Vec::new(),
        running: Vec::new(),
        received: [Vec::new(), Vec::new()],
    };
    let mut walk = Walk(seed.wrapping_mul(0x9e37_79b9).wrapping_add(1));
    for step in 0..2_000_u32 {
        let now = at(step);
        let socket = walk.below(2);
        match walk.below(10) {
            0 => {
                let group = walk.below(3);
                let wanted = stack.pool().free() > 0 && ledger.count(group) < LIMITS[group];
                let block = stack.pool().take_as(groups[group]);
                assert_eq!(block.is_some(), wanted, "step {step}");
                if let Some(block) = block {
                    ledger.held.push((block, group));
                }
            }
            1 if !ledger.held.is_empty() => {
                let at_random = walk.below(ledger.held.len());
                let (block, _) = ledger.held.swap_remove(at_random);
                stack.pool().give(block);
            }
            2 if !ledger.held.is_empty() => {
                let at_random = walk.below(ledger.held.len());
                let (block, group) = ledger.held.swap_remove(at_random);
                let id = sockets[socket].as_ref().unwrap();
                match stack.send(now, id, peer(), block) {
                    Ok(command) => ledger.running.push((command, group)),
                    Err(refused) => ledger.held.push((refused.block, group)),
                }
            }
            3 => {
                stack.port_mut().full = walk.below(3) == 0;
                if let Some(demand) = stack.poll_demand(now) {
                    let _taken = stack.transmit(now, demand.grant(), unproduced, &());
                }
            }
            4 => {
                let group = of_socket[socket];
                let wanted = ledger.received[socket].len() < 2
                    && stack.pool().free() > 0
                    && ledger.count(group) < LIMITS[group];
                let port = 5000 + u16::try_from(socket).unwrap();
                stack.input(now, &udp_frame(7, port, b"in"), &mut ());
                if wanted {
                    ledger.received[socket].push(group);
                }
            }
            5 => {
                let id = sockets[socket].as_ref().unwrap();
                if let Some(datagram) = stack.receive(id).unwrap() {
                    let group = ledger.received[socket].remove(0);
                    ledger.held.push((datagram.block, group));
                }
            }
            6 => {
                let queued: Vec<_> = stack.queued().collect();
                let count = queued.len();
                if count != 0 {
                    let chosen = walk.below(count);
                    let queued = queued.into_iter().nth(chosen).unwrap();
                    assert!(stack.evict(queued, Eviction::MemoryPressure));
                }
            }
            7 => {
                let id = sockets[socket].take().unwrap();
                let close = stack.close(id).unwrap();
                ledger.received[socket].clear();
                ledger.harvest(&mut stack);
                stack.take_result(close);
                sockets[socket] = Some(open(&mut stack, socket));
            }
            8 => {
                let reply = arp_frame(Operation::Reply, (PEER, PEER_IP), OUR_IP);
                stack.input(now, &reply, &mut ());
            }
            9 if walk.below(20) == 0 => {
                let retired = stack.retire();
                ledger.running.clear();
                ledger.received = [Vec::new(), Vec::new()];
                let local = interface(OUR_IP, 24, None);
                stack = NetworkOwner::new(retired.brand, local, retired.pool, retired.port);
                sockets = [Some(open(&mut stack, 0)), Some(open(&mut stack, 1))];
            }
            _ => stack.tick(now, &mut ()),
        }
        ledger.harvest(&mut stack);
        let mut sum = 0;
        for group in 0..3 {
            let held = stack.pool().held(groups[group]);
            assert_eq!(held, ledger.count(group), "step {step}, group {group}");
            assert!(held <= LIMITS[group], "step {step}, group {group}");
            sum += held;
        }
        assert_eq!(sum, stack.pool().outstanding(), "step {step}");
    }
    for (block, group) in ledger.held.drain(..) {
        assert_eq!(block.group(), groups[group]);
        stack.pool().give(block);
    }
}

#[test]
fn a_block_counts_toward_its_group_through_every_move() {
    brand!(First);
    brand!(Second);
    brand!(Third);
    walk_blocks::<First>(1);
    walk_blocks::<Second>(2);
    walk_blocks::<Third>(3);
}

/// A core with a pool of one driver buffer, lent to `socket` 0.
fn lending<B: 'static>() -> (Blocks<B>, CoreBrand<B>) {
    let (mut pool, brand) = claimed::<B>();
    pool.loans().pool(1, 0).unwrap();
    pool.loans().grant(0, 1).unwrap();
    (pool, brand)
}

type LoanCore<B> = Core<'static, B, SIZE, 1, 1, 1, 1>;

fn next_core<B: 'static>(mut pool: Blocks<B>, brand: CoreBrand<B>) -> (Blocks<B>, CoreBrand<B>) {
    let core: LoanCore<B> = Core::new(brand, interface(OUR_IP, 24, None));
    let (_results, brand) = core.retire(&mut pool);
    (pool, brand)
}

#[test]
fn a_loan_stays_owed_across_a_core() {
    brand!(Net);
    let (mut pool, brand) = lending::<Net>();
    let held = pool.loans().lend(0).unwrap();
    let (mut pool, _brand) = next_core(pool, brand);
    assert_eq!(pool.loans().outstanding(), 1);
    assert_eq!(
        pool.loans().grant(0, 1),
        Err(LoansRefused::Pool { room: 0 }),
        "the old loan still takes its buffer"
    );
    pool.loans().give_back(held);
    assert!(pool.loans().released());
    pool.loans().grant(0, 1).unwrap();
}

#[test]
fn an_old_loan_pays_nothing_of_a_new_one() {
    brand!(Net);
    let (mut pool, brand) = lending::<Net>();
    pool.loans().pool(2, 0).unwrap();
    pool.loans().grant(0, 2).unwrap();
    let old = pool.loans().lend(0).unwrap();
    let (mut pool, _brand) = next_core(pool, brand);
    pool.loans().grant(0, 1).unwrap();
    let new = pool.loans().lend(0).unwrap();
    pool.loans().give_back(old);
    assert_eq!(pool.loans().outstanding(), 1, "the new loan is still out");
    assert!(!pool.loans().released());
    assert_eq!(pool.loans().lend(0).err(), Some(NotLent::NoCredit));
    pool.loans().give_back(new);
    assert!(pool.loans().released());
}

#[test]
fn a_datagram_the_port_rejects_for_good_ends_named_and_goes_no_more() {
    world!(|stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        knows_peer(stack);
        let block = filled(stack.pool(), b"never");
        let free = stack.pool().free();
        let command = stack.send(at(0), &socket, peer(), block).unwrap();
        stack.port_mut().rejects = true;
        let demand = stack.poll_demand(at(0)).unwrap();
        let refused = stack.transmit(at(0), demand.grant(), unproduced, &());
        assert!(matches!(refused, Err(Unsent::Rejected("rejected"))));
        assert!(
            stack.poll_demand(at(0)).is_none(),
            "it is not offered again"
        );
        assert_eq!(stack.pool().free(), free + 1);
        assert_eq!(
            stack.take_result(command),
            CommandState::Finished(Outcome::Rejected)
        );
    });
}

#[test]
fn an_arp_request_rejected_for_good_ends_what_waits_with_that_cause() {
    world!(|stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let one = filled(stack.pool(), b"one");
        let two = filled(stack.pool(), b"two");
        let free = stack.pool().free();
        let first = stack.send(at(0), &socket, peer(), one).unwrap();
        let second = stack.send(at(0), &socket, peer(), two).unwrap();
        stack.port_mut().rejects = true;
        let demand = stack.poll_demand(at(0)).unwrap();
        let refused = stack.transmit(at(0), demand.grant(), unproduced, &());
        assert!(matches!(refused, Err(Unsent::Rejected("rejected"))));
        assert_eq!(finished(stack.take_result(first)), Outcome::Rejected);
        assert_eq!(finished(stack.take_result(second)), Outcome::Rejected);
        assert_eq!(stack.pool().free(), free + 2);
        assert!(stack.poll_demand(at(0)).is_none(), "nothing is asked again");
        assert_eq!(stack.poll_deadline(), None, "and no time is owed to it");
    });
}

#[test]
fn a_rejected_arp_request_leaves_another_neighbor_waiting() {
    world!(|stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let near = filled(stack.pool(), b"near");
        let far = filled(stack.pool(), b"far");
        let other = Remote {
            address: [192, 0, 2, 77],
            ..peer()
        };
        let first = stack.send(at(0), &socket, peer(), near).unwrap();
        let waiting = stack.send(at(0), &socket, other, far).unwrap();
        stack.port_mut().rejects = true;
        let demand = stack.poll_demand(at(0)).unwrap();
        assert!(
            stack
                .transmit(at(0), demand.grant(), unproduced, &())
                .is_err()
        );
        assert_eq!(finished(stack.take_result(first)), Outcome::Rejected);
        stack.port_mut().rejects = false;
        assert!(send_next(stack, at(0)).is_ok(), "the other is still asked");
        let waiting = running(stack.take_result(waiting));
        stack.tick(at(4000), &mut ());
        stack.tick(at(5000), &mut ());
        send_next(stack, at(5000)).ok();
        stack.tick(at(6000), &mut ());
        send_next(stack, at(6000)).ok();
        stack.tick(at(7000), &mut ());
        assert_eq!(finished(stack.take_result(waiting)), Outcome::Unreachable);
    });
}

#[test]
fn a_request_refused_after_one_went_out_ends_rejected() {
    world!(|stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"waits");
        let command = stack.send(at(0), &socket, peer(), block).unwrap();
        send_next(stack, at(0)).unwrap();
        stack.tick(at(1000), &mut ());
        stack.port_mut().rejects = true;
        let demand = stack.poll_demand(at(1000)).unwrap();
        assert!(
            stack
                .transmit(at(1000), demand.grant(), unproduced, &())
                .is_err()
        );
        assert_eq!(finished(stack.take_result(command)), Outcome::Rejected);
    });
}

#[test]
fn a_new_datagram_after_a_rejected_request_asks_anew() {
    world!(|stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"never");
        let first = stack.send(at(0), &socket, peer(), block).unwrap();
        stack.port_mut().rejects = true;
        let demand = stack.poll_demand(at(0)).unwrap();
        assert!(
            stack
                .transmit(at(0), demand.grant(), unproduced, &())
                .is_err()
        );
        assert_eq!(finished(stack.take_result(first)), Outcome::Rejected);
        stack.port_mut().rejects = false;
        let block = filled(stack.pool(), b"later");
        let later = stack.send(at(0), &socket, peer(), block).unwrap();
        assert!(send_next(stack, at(0)).is_ok());
        assert!(matches!(stack.take_result(later), CommandState::Running(_)));
    });
}

mod counted;
mod demands;
mod dhcp;
mod echo;
mod ends;
mod idle;
mod lending;
mod materialised;
mod prepared;
mod produced;

/// What one frame of an intake batch of one came to, for the tests that
/// take frames one at a time.
#[derive(Debug)]
pub(crate) enum Took<B, H> {
    Empty,
    Read,
    Lent(crate::LentDatagram<B, H>),
    Broken(oer_protocols_network_port::ReceiveFault),
}

impl<
    'm,
    B,
    const SIZE: usize,
    const SOCKETS: usize,
    const CREDITS: usize,
    const RECEIVED: usize,
    const NEIGHBORS: usize,
    const BLOCKS: usize,
    P: FramePort + oer_protocols_network_port::FrameSource,
    T: Copy,
> NetworkOwner<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, P, T>
{
    /// One batch of at most one frame, a lent datagram kept by the test.
    pub(crate) fn take_one<C: crate::Count + ?Sized>(
        &mut self,
        now: Instant,
        events: &mut impl EventSink<NetEvent>,
        count: &C,
    ) -> Took<B, P::Held> {
        let mut lent = None;
        let intake = self.take_frames(now, 1, events, count, |datagram| {
            lent = Some(datagram);
            Ok(())
        });
        match (intake.batch.end, lent) {
            (oer_protocols_network_port::BatchEnd::Fault(fault), _) => Took::Broken(fault),
            (_, Some(datagram)) => Took::Lent(datagram),
            _ if intake.batch.taken == 0 => Took::Empty,
            _ => Took::Read,
        }
    }
}
