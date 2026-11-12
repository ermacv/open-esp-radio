extern crate std;

use std::boxed::Box;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::Wake;
use std::vec;
use std::vec::Vec;

use core::pin::pin;
use core::task::{Context, Poll, Waker};

use embassy_futures::yield_now;
use oer_portable_memory::WakingSpscSender;
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_network_core::{
    Block, Eviction, Group, Interface, NetworkOwner, OpenRefused, Outcome, Remote, SendRefusal,
    claim,
};
use oer_protocols_network_port::{
    BatchEnd, CopyReason, EthernetFrame, EthernetFrameView, FrameLengthError, FramePort,
    FrameSource, Holding, Incoming, LinkSnapshot, LinkState, PortReady, ReceivedBatch, Unpublished,
};
use oer_protocols_network_wire::arp::{self, Arp, Operation};
use oer_protocols_network_wire::checksum::{Checksum, ipv4_pseudo_header, udp_field};
use oer_protocols_network_wire::ethernet::{EtherType, EthernetHeader};
use oer_protocols_network_wire::ip::{DsField, Dscp, Ipv4Fields, Ipv4Header};
use oer_protocols_network_wire::transport::{PROTOCOL_UDP, Ports, UdpHeader};

use oer_portable_wire::Writer;
use oer_protocols_network_wire::dhcp::{self, DhcpMessage, Fixed, Flags, MessageType, Op, option};

use crate::{
    ConfigurationWatch, EndCause, Ended, MemoryPolicy, Observer, RESERVE_AHEAD, Refusal,
    ReturnLane, ReturnRefusal, Returner, Sent, Service, Socket, SocketClosed, SocketLanes,
    StopLane,
};

/// A brand of the test's own.
macro_rules! brand {
    ($name:ident) => {
        struct $name;
    };
}

/// Passes of a test before it counts as spinning.
const POLLS: usize = 100_000;
const SIZE: usize = 256;
const BLOCKS: usize = 4;
const CREDITS: usize = 2;
const RECEIVED: usize = 2;
const DEPTH: usize = 5;
const LOCAL: [u8; 6] = [2, 0, 0, 0, 0, 1];
const PEER: [u8; 6] = [2, 0, 0, 0, 0, 2];
const OUR_IP: [u8; 4] = [192, 0, 2, 1];
const PEER_IP: [u8; 4] = [192, 0, 2, 2];

/// The wire between the stack and the test, which plays the peer.
#[derive(Default)]
struct Wire {
    /// Calls a scenario counts: frames and notices taken, empty or not, and registrations.
    takes: u64,
    empty_takes: u64,
    notice_takes: u64,
    empty_notices: u64,
    registers: u64,
    incoming: VecDeque<Vec<u8>>,
    sent: Vec<Vec<u8>>,
    full: bool,
    /// The peer answers ARP.
    answers: bool,
    /// The port says it took a frame it never filled.
    contradicts: bool,
    /// The port refuses every frame for good.
    rejects: bool,
    /// The port loses the buffer of every frame written.
    loses: bool,
    /// The port's receive queue is broken: every take answers it.
    receive_fault: Option<oer_protocols_network_port::ReceiveFault>,
    /// The next frames the port refuses for good.
    rejections: usize,
    /// The port gives less room than a frame needs.
    room: Option<usize>,
    /// What the port answers to a fill that failed.
    after_failed_fill: AfterFailedFill,
    /// Tickets given.
    tickets: u32,
    /// Ended frames, by ticket.
    ended: VecDeque<u32>,
    waker: Option<Waker>,
    /// The last frame was refused for room.
    refused: bool,
    /// Frames refused for room.
    refusals: usize,
    /// Frames the port takes before it is full; `None` for no such bound.
    accepts: Option<usize>,
    /// How the port holds what it shows: lendable buffers, or its copies.
    holding: Option<Holding>,
    /// Buffers the port lent and has not got back.
    lent_out: Rc<std::cell::Cell<usize>>,
}

/// A buffer the wire lent: it counts itself back when dropped.
struct WireHeld {
    frame: Vec<u8>,
    out: Rc<std::cell::Cell<usize>>,
}

impl Drop for WireHeld {
    fn drop(&mut self) {
        self.out.set(self.out.get() - 1);
    }
}

impl EthernetFrame for WireHeld {
    fn view(&mut self) -> Result<EthernetFrameView<'_>, FrameLengthError> {
        let (header, payload) =
            EthernetHeader::parse(&self.frame).map_err(|_| FrameLengthError::OutsideStorage)?;
        Ok(EthernetFrameView {
            destination: header.destination,
            source: header.source,
            ether_type: header.ether_type.value(),
            payload,
        })
    }
}

impl Wire {
    fn wake(&mut self) {
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }
}

/// A port's answer to a fill that failed.
#[derive(Clone, Copy, Default)]
enum AfterFailedFill {
    /// The fill's own error, as the contract says.
    #[default]
    Fill,
    /// A refusal of its own.
    Refusal,
    /// `Ok`.
    Yes,
}

#[derive(Clone, Default)]
struct WirePort(Rc<RefCell<Wire>>);

/// Why the wire took no frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Refused {
    /// No room: the work waits.
    Room,
    /// For good.
    Final,
    /// The port lost the buffer the frame was written in: its fault.
    Lost,
}

impl FramePort for WirePort {
    type Refusal = Refused;
    type Ticket = u32;

    fn is_final(refusal: &Refused) -> bool {
        *refusal == Refused::Final
    }

    fn is_fault(refusal: &Refused) -> bool {
        *refusal == Refused::Lost
    }

    fn publish<E>(
        &mut self,
        meta: oer_protocols_network_port::FrameMeta,
        fill: impl FnOnce(&mut oer_protocols_network_port::Slot<'_>) -> Result<(), E>,
    ) -> Result<u32, Unpublished<Refused, E>> {
        let length = meta.length;
        let mut wire = self.0.borrow_mut();
        wire.refused = wire.full;
        if wire.full {
            wire.refusals += 1;
            return Err(Unpublished::Port(Refused::Room));
        }
        if wire.rejects {
            return Err(Unpublished::Port(Refused::Final));
        }
        if wire.rejections > 0 {
            wire.rejections -= 1;
            return Err(Unpublished::Port(Refused::Final));
        }
        if wire.contradicts {
            return Ok(0);
        }
        let mut frame = std::vec![0; wire.room.unwrap_or(length)];
        if let Err(error) = fill(&mut oer_protocols_network_port::Slot::new(&mut frame, None)) {
            return match wire.after_failed_fill {
                AfterFailedFill::Fill => Err(Unpublished::Fill(error)),
                AfterFailedFill::Refusal => Err(Unpublished::Port(Refused::Room)),
                AfterFailedFill::Yes => Ok(0),
            };
        }
        if wire.loses {
            // Written, then lost with its buffer: it never went.
            return Err(Unpublished::Port(Refused::Lost));
        }
        let reply = wire.answers.then(|| answer_of(&frame)).flatten();
        wire.sent.push(frame);
        wire.incoming.extend(reply);
        wire.tickets += 1;
        if let Some(accepts) = &mut wire.accepts {
            *accepts -= 1;
            if *accepts == 0 {
                wire.accepts = None;
                wire.full = true;
            }
        }
        Ok(wire.tickets)
    }
}

impl FrameSource for WirePort {
    type Notice = u32;
    type Held = WireHeld;

    fn take_notice(&mut self) -> Option<u32> {
        let mut wire = self.0.borrow_mut();
        wire.notice_takes += 1;
        let notice = wire.ended.pop_front();
        if notice.is_none() {
            wire.empty_notices += 1;
        }
        notice
    }

    /// A batch is one take: `takes` counts batches, `empty_takes` those
    /// that found no frame.
    fn take_frames(
        &mut self,
        limit: usize,
        mut take: impl FnMut(Incoming<'_, WireHeld>),
    ) -> ReceivedBatch {
        {
            let mut wire = self.0.borrow_mut();
            wire.takes += 1;
            if let Some(fault) = wire.receive_fault {
                return ReceivedBatch {
                    taken: 0,
                    end: BatchEnd::Fault(fault),
                };
            }
        }
        let mut taken = 0;
        let end = loop {
            if taken >= limit {
                break BatchEnd::Limit;
            }
            let (frame, holding, out) = {
                let mut wire = self.0.borrow_mut();
                let Some(frame) = wire.incoming.pop_front() else {
                    break BatchEnd::Drained;
                };
                let holding = wire.holding.unwrap_or(Holding::Copied(CopyReason::Format));
                (frame, holding, wire.lent_out.clone())
            };
            taken += 1;
            out.set(out.get() + 1);
            let mut held = WireHeld { frame, out };
            match holding {
                Holding::Lendable => take(Incoming::Lent(&mut Some(held))),
                Holding::Copied(reason) => take(Incoming::Copied {
                    frame: held.view(),
                    reason,
                }),
            }
        };
        if taken == 0 {
            self.0.borrow_mut().empty_takes += 1;
        }
        ReceivedBatch { taken, end }
    }

    fn link(&self) -> LinkSnapshot {
        LinkSnapshot {
            state: LinkState::Up,
            epoch: 1,
        }
    }

    fn register(&mut self, cx: &mut Context<'_>) -> PortReady {
        let mut wire = self.0.borrow_mut();
        wire.waker = Some(cx.waker().clone());
        wire.registers += 1;
        PortReady {
            received: !wire.incoming.is_empty(),
            other: !wire.ended.is_empty() || (wire.refused && !wire.full),
        }
    }
}

/// What the peer says to `frame`: its ARP reply.
fn answer_of(frame: &[u8]) -> Option<Vec<u8>> {
    let (header, payload) = EthernetHeader::parse(frame).ok()?;
    if header.ether_type != EtherType::ARP {
        return None;
    }
    let request = Arp::parse(payload).ok()??;
    (request.operation == Operation::Request).then(|| {
        let mut reply = EthernetHeader {
            destination: LOCAL,
            source: PEER,
            ether_type: EtherType::ARP,
        }
        .to_bytes()
        .to_vec();
        let mut packet = [0; arp::PACKET_LEN];
        Arp {
            operation: Operation::Reply,
            sender_hardware: PEER,
            sender_protocol: PEER_IP,
            target_hardware: LOCAL,
            target_protocol: OUR_IP,
        }
        .write(&mut packet)
        .unwrap();
        reply.extend_from_slice(&packet);
        reply
    })
}

fn udp_frame(destination_port: u16, data: &[u8]) -> Vec<u8> {
    let length = u16::try_from(8 + data.len()).unwrap();
    let unfilled = UdpHeader {
        ports: Ports {
            source: 7,
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

fn peer() -> Remote {
    Remote {
        address: PEER_IP,
        port: 7,
    }
}

/// The payloads the port accepted.
fn datagrams(wire: &Wire) -> Vec<Vec<u8>> {
    wire.sent
        .iter()
        .filter_map(|frame| {
            let (header, payload) = EthernetHeader::parse(frame).ok()?;
            (header.ether_type == EtherType::IPV4).then(|| {
                let packet = Ipv4Header::parse(payload).unwrap();
                UdpHeader::parse(packet.payload()).unwrap().1.to_vec()
            })
        })
        .collect()
}

/// A server's reply to our station: `kind` of `xid`, leasing [`OUR_IP`] for
/// `lease` seconds.
fn dhcp_reply(kind: MessageType, xid: u32, lease: u32) -> Vec<u8> {
    let mut message = vec![0; 400];
    let length = {
        let mut writer = Writer::new(&mut message);
        let [c0, c1, c2, c3, c4, c5] = LOCAL;
        Fixed {
            op: Op::Reply,
            htype: 1,
            hlen: 6,
            hops: 0,
            xid,
            secs: 0,
            flags: Flags::from_bytes([0; 2]),
            ciaddr: [0; 4],
            yiaddr: OUR_IP,
            siaddr: PEER_IP,
            giaddr: [0; 4],
            chaddr: [c0, c1, c2, c3, c4, c5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        }
        .write(&mut writer)
        .unwrap();
        let options: [(u8, &[u8]); 4] = [
            (option::MESSAGE_TYPE, &[kind.encoding()]),
            (option::SERVER_IDENTIFIER, &PEER_IP),
            (option::SUBNET_MASK, &[255, 255, 255, 0]),
            (option::LEASE_TIME, &lease.to_be_bytes()),
        ];
        for (code, data) in options {
            dhcp::write_option(&mut writer, code, data).unwrap();
        }
        dhcp::write_end(&mut writer).unwrap();
        writer.written()
    };
    message.truncate(length);
    let mut frame = udp_frame(dhcp::CLIENT_PORT, &message);
    // From the server's port, to the offered address.
    frame[34..36].copy_from_slice(&dhcp::SERVER_PORT.to_be_bytes());
    let udp_length = u16::try_from(8 + message.len()).unwrap();
    frame[40..42].copy_from_slice(&[0, 0]);
    let sum = Checksum::new()
        .with(&ipv4_pseudo_header(
            PEER_IP,
            OUR_IP,
            PROTOCOL_UDP,
            udp_length,
        ))
        .with(&frame[34..])
        .finish();
    frame[40..42]
        .copy_from_slice(&oer_protocols_network_wire::checksum::udp_field(sum).to_be_bytes());
    frame
}

/// The DHCP messages among `sent` frames, by type and xid.
fn dhcp_messages(sent: &[Vec<u8>]) -> Vec<(MessageType, u32)> {
    sent.iter()
        .filter_map(|frame| {
            let (header, payload) = EthernetHeader::parse(frame).ok()?;
            (header.ether_type == EtherType::IPV4).then_some(())?;
            let packet = Ipv4Header::parse(payload).ok()?;
            let (udp, data) = UdpHeader::parse(packet.payload()).ok()?;
            (udp.ports.destination == dhcp::SERVER_PORT).then_some(())?;
            let message = DhcpMessage::parse(data).ok()?;
            Some((message.message_type()?, message.fixed.xid))
        })
        .collect()
}

type TestSocket<B> = Socket<'static, 'static, B, SIZE, DEPTH, RECEIVED, CREDITS>;
type TestService<B> =
    Service<'static, 'static, B, SIZE, 1, CREDITS, RECEIVED, 2, BLOCKS, DEPTH, RECEIVED, WirePort>;
type TestEnded<B> = Ended<'static, 'static, B, SIZE, 1, CREDITS, BLOCKS, DEPTH, RECEIVED, WirePort>;

/// A stack of brand `B`, its one socket, and the test's handles.
struct Bench<B: 'static> {
    service: TestService<B>,
    socket: TestSocket<B>,
    wire: WirePort,
    stop: WakingSpscSender<'static, (), 1>,
    clock: &'static VirtualClock,
}

fn bench<B: 'static>() -> Bench<B> {
    let memory = Box::leak(Box::new([[0; SIZE]; BLOCKS]));
    let (pool, brand) = claim::<B, SIZE, BLOCKS>(memory).unwrap();
    let wire = WirePort::default();
    wire.0.borrow_mut().answers = true;
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let owner = NetworkOwner::new(brand, interface, pool, wire.clone());
    bench_on(owner, wire)
}

fn storage<B>() -> &'static mut SocketLanes<'static, 'static, B, SIZE, DEPTH, RECEIVED, CREDITS> {
    Box::leak(Box::new(SocketLanes::new()))
}

fn bench_on<B: 'static>(
    owner: NetworkOwner<'static, B, SIZE, 1, CREDITS, RECEIVED, 2, BLOCKS, WirePort>,
    wire: WirePort,
) -> Bench<B> {
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut service = Service::new(owner, MemoryPolicy::KEEP, stopping);
    let socket = service
        .bind(5000, Dscp::default(), Group::shared(), storage::<B>())
        .ok()
        .unwrap();
    Bench {
        service,
        socket,
        wire,
        stop,
        clock: Box::leak(Box::new(VirtualClock::new())),
    }
}

/// A task's "poll me".
struct Woken(AtomicBool);

impl Wake for Woken {
    fn wake(self: Arc<Self>) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Run two tasks as an executor would: a task is polled only once woken, and
/// with nothing woken, time moves on. A test fails when nothing is woken and no
/// time is due, so where only one wake can come, losing it fails the test.
fn run_tasks<T, U>(
    clock: &VirtualClock,
    first: impl Future<Output = T>,
    second: impl Future<Output = U>,
) -> (T, U) {
    let mut first = pin!(first);
    let mut second = pin!(second);
    let (mut first_end, mut second_end) = (None, None);
    let flags = [Woken(AtomicBool::new(true)), Woken(AtomicBool::new(true))].map(Arc::new);
    let wakers = flags.clone().map(Waker::from);
    for _ in 0..POLLS {
        if first_end.is_none() && flags[0].0.swap(false, Ordering::SeqCst) {
            let mut cx = Context::from_waker(&wakers[0]);
            if let Poll::Ready(end) = first.as_mut().poll(&mut cx) {
                first_end = Some(end);
            }
        }
        if second_end.is_none() && flags[1].0.swap(false, Ordering::SeqCst) {
            let mut cx = Context::from_waker(&wakers[1]);
            if let Poll::Ready(end) = second.as_mut().poll(&mut cx) {
                second_end = Some(end);
            }
        }
        if let (true, true) = (first_end.is_some(), second_end.is_some()) {
            return (first_end.unwrap(), second_end.unwrap());
        }
        let first_woken = first_end.is_none() && flags[0].0.load(Ordering::SeqCst);
        let second_woken = second_end.is_none() && flags[1].0.load(Ordering::SeqCst);
        if !first_woken && !second_woken {
            clock
                .advance_to_next_deadline()
                .expect("no task is woken and no time is due: a wake was lost");
        }
    }
    panic!("{POLLS} passes without an end: a task spins");
}

/// A service's task: drive it, then hand every owner on.
async fn serve<B>(
    mut service: TestService<B>,
    clock: &VirtualClock,
    events: &mut (),
) -> TestEnded<B> {
    let _cause = service.run(clock, events).await;
    service.retire()
}

/// Run `client` beside the service until it ends, then stop the service.
fn run<B: 'static, R>(
    bench: Bench<B>,
    client: impl AsyncFnOnce(&mut TestSocket<B>, &WirePort) -> R,
) -> (TestEnded<B>, TestSocket<B>, R) {
    let Bench {
        service,
        mut socket,
        wire,
        stop,
        clock,
    } = bench;
    let mut events = ();
    let drive = async {
        let result = client(&mut socket, &wire).await;
        stop.try_send(()).ok().unwrap();
        result
    };
    let (ended, result) = run_tasks(clock, serve(service, clock, &mut events), drive);
    (ended, socket, result)
}

/// As [`run`], for a client that owns its socket and may drop it.
fn run_owning<B: 'static, R>(
    bench: Bench<B>,
    client: impl AsyncFnOnce(TestSocket<B>, &WirePort) -> R,
) -> (TestEnded<B>, R) {
    let Bench {
        service,
        socket,
        wire,
        stop,
        clock,
    } = bench;
    let mut events = ();
    let drive = async {
        let result = client(socket, &wire).await;
        stop.try_send(()).ok().unwrap();
        result
    };
    run_tasks(clock, serve(service, clock, &mut events), drive)
}

async fn fill<B>(socket: &mut TestSocket<B>, data: &[u8]) -> Block<'static, B, SIZE> {
    let mut block = socket.reserve().await.unwrap();
    block.fill(data).unwrap();
    block
}

/// Let the service run out of work.
async fn settle() {
    for _ in 0..16 {
        yield_now().await;
    }
}

fn open_port(wire: &WirePort, full: bool) {
    let mut wire = wire.0.borrow_mut();
    wire.full = full;
    wire.wake();
}

fn arrive(wire: &WirePort, count: usize) {
    let mut wire = wire.0.borrow_mut();
    wire.incoming
        .extend((0..count).map(|_| udp_frame(5000, b"many")));
    wire.wake();
}

async fn result_of(mut observer: Observer<'_>) -> Sent {
    observer.finished().await;
    observer.take().unwrap()
}

async fn send<B>(socket: &mut TestSocket<B>, data: &[u8]) -> Observer<'static> {
    let block = fill(socket, data).await;
    socket.credit().await.unwrap();
    socket.send(peer(), block).ok().unwrap()
}

async fn send_one<B>(socket: &mut TestSocket<B>, data: &[u8]) -> Sent {
    result_of(send(socket, data).await).await
}

/// Every block the service did not see again comes back from the socket.
fn returned<B>(ended: &mut TestEnded<B>, socket: &mut TestSocket<B>) {
    while let Some(block) = socket.reclaim() {
        ended.retired.pool.give(block);
    }
    assert_eq!(ended.retired.pool.outstanding(), 0);
}

/// A waker that counts, for one named notification.
struct Count(AtomicUsize);

impl Wake for Count {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn counter() -> (Arc<Count>, Waker) {
    let count = Arc::new(Count(AtomicUsize::new(0)));
    let waker = Waker::from(count.clone());
    (count, waker)
}

#[test]
fn a_datagram_goes_from_a_socket_through_the_service_to_the_port() {
    brand!(Net);
    let (_, _, sent) = run(bench::<Net>(), async |socket, wire| {
        let sent = send_one(socket, b"hello").await;
        assert_eq!(datagrams(&wire.0.borrow()), [b"hello".to_vec()]);
        sent
    });
    assert_eq!(sent, Sent::Ended(Outcome::Accepted));
}

#[test]
fn a_received_datagram_reaches_the_socket_and_its_block_returns() {
    brand!(Net);
    let (mut ended, mut socket, ()) = run(bench::<Net>(), async |socket, wire| {
        {
            let mut wire = wire.0.borrow_mut();
            wire.incoming.push_back(udp_frame(5000, b"in"));
            wire.wake();
        }
        let datagram = socket.receive().await.unwrap();
        assert_eq!(datagram.block.octets(), b"in");
        assert_eq!(datagram.remote, peer());
        socket.release(datagram.block).ok().unwrap();
    });
    returned(&mut ended, &mut socket);
}

#[test]
fn a_port_with_no_room_keeps_the_datagram_until_it_wakes_the_service() {
    brand!(Net);
    let (_, _, sent) = run(bench::<Net>(), async |socket, wire| {
        open_port(wire, true);
        let observer = send(socket, b"late").await;
        settle().await;
        assert!(wire.0.borrow().sent.is_empty());
        open_port(wire, false);
        result_of(observer).await
    });
    assert_eq!(sent, Sent::Ended(Outcome::Accepted));
}

#[test]
fn a_wait_for_a_result_may_be_cancelled_and_begun_again() {
    brand!(Net);
    let _ended = run(bench::<Net>(), async |socket, wire| {
        open_port(wire, true);
        let mut observer = send(socket, b"wait").await;
        {
            let mut waiting = pin!(observer.finished());
            let mut cx = Context::from_waker(Waker::noop());
            assert!(waiting.as_mut().poll(&mut cx).is_pending());
        }
        let observer = observer.take().err().unwrap();
        open_port(wire, false);
        assert_eq!(result_of(observer).await, Sent::Ended(Outcome::Accepted));
    });
}

#[test]
fn a_result_wakes_its_observer_and_needs_no_socket() {
    brand!(Net);
    let _ended = run(bench::<Net>(), async |socket, wire| {
        open_port(wire, true);
        let mut observer = send(socket, b"apart").await;
        let (count, waker) = counter();
        {
            let mut waiting = pin!(observer.finished());
            assert!(
                waiting
                    .as_mut()
                    .poll(&mut Context::from_waker(&waker))
                    .is_pending()
            );
        }
        open_port(wire, false);
        settle().await;
        assert_eq!(count.0.load(Ordering::SeqCst), 1);
        assert_eq!(observer.take().ok(), Some(Sent::Ended(Outcome::Accepted)));
    });
}

#[test]
fn two_commands_on_one_credit_keep_their_own_results() {
    brand!(Net);
    let _ended = run(bench::<Net>(), async |socket, wire| {
        for round in 0..2 * CREDITS {
            let data = [u8::try_from(round).unwrap()];
            let observer = send(socket, &data).await;
            assert_eq!(result_of(observer).await, Sent::Ended(Outcome::Accepted));
        }
        assert_eq!(datagrams(&wire.0.borrow()).len(), 2 * CREDITS);
    });
}

/// A credit's tag and its command's place in the core free at different
/// times, so they part: each end still reaches its own observer, once.
#[test]
fn an_end_reaches_its_own_observer_when_its_tag_is_not_its_place() {
    brand!(Net);
    let _ended = run(bench::<Net>(), async |socket, wire| {
        open_port(wire, true);
        // Tag 0 in place 0, tag 1 in place 1.
        let first = send(socket, b"first").await;
        let second = send(socket, b"second").await;
        settle().await;
        open_port(wire, false);
        // Both places free, the first first; only tag 1 frees, as its result
        // is read.
        assert_eq!(result_of(second).await, Sent::Ended(Outcome::Accepted));
        // Tag 1 runs in place 0.
        let third = send(socket, b"third").await;
        assert_eq!(result_of(third).await, Sent::Ended(Outcome::Accepted));
        assert_eq!(result_of(first).await, Sent::Ended(Outcome::Accepted));
        let sent = datagrams(&wire.0.borrow());
        assert_eq!(sent, [&b"first"[..], b"second", b"third"]);
    });
}

#[test]
fn a_dropped_observer_frees_its_credit_once_the_command_ended() {
    brand!(Net);
    let _ended = run(bench::<Net>(), async |socket, wire| {
        for round in 0..6_u8 {
            drop(send(socket, &[round]).await);
        }
        let sent = send_one(socket, b"after").await;
        assert_eq!(sent, Sent::Ended(Outcome::Accepted));
        assert_eq!(datagrams(&wire.0.borrow()).len(), 7);
    });
}

#[test]
fn a_dropped_observer_of_a_running_command_keeps_its_credit() {
    brand!(Net);
    let _ended = run(bench::<Net>(), async |socket, wire| {
        open_port(wire, true);
        for _ in 0..CREDITS {
            drop(send(socket, b"held").await);
        }
        let block = socket.reserve().await.unwrap();
        let refused = socket.send(peer(), block).err().unwrap();
        assert_eq!(refused.reason, Refusal::Busy);
        settle().await;
        let (count, waker) = counter();
        {
            let mut waiting = pin!(socket.credit());
            let mut cx = Context::from_waker(&waker);
            assert!(waiting.as_mut().poll(&mut cx).is_pending());
            open_port(wire, false);
            settle().await;
            assert!(count.0.load(Ordering::SeqCst) >= 1, "its end wakes credit");
            assert_eq!(waiting.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
        }
        drop(socket.send(peer(), refused.block).ok().unwrap());
    });
}

#[test]
fn dropping_an_observer_wakes_a_credit_already_waiting() {
    brand!(Net);
    let _ended = run(bench::<Net>(), async |socket, _| {
        let mut held = Vec::new();
        for _ in 0..CREDITS {
            let mut observer = send(socket, b"done").await;
            observer.finished().await;
            held.push(observer);
        }
        let (count, waker) = counter();
        let mut waiting = pin!(socket.credit());
        let mut cx = Context::from_waker(&waker);
        assert!(waiting.as_mut().poll(&mut cx).is_pending());
        assert_eq!(count.0.load(Ordering::SeqCst), 0);
        drop(held.pop());
        assert_eq!(count.0.load(Ordering::SeqCst), 1);
        assert_eq!(waiting.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    });
}

#[test]
fn reading_a_result_wakes_a_credit_already_waiting() {
    brand!(Net);
    let _ended = run(bench::<Net>(), async |socket, _| {
        let mut held = Vec::new();
        for _ in 0..CREDITS {
            let mut observer = send(socket, b"done").await;
            observer.finished().await;
            held.push(observer);
        }
        let (count, waker) = counter();
        let mut waiting = pin!(socket.credit());
        let mut cx = Context::from_waker(&waker);
        assert!(waiting.as_mut().poll(&mut cx).is_pending());
        let observer = held.pop().unwrap();
        assert_eq!(observer.take().ok(), Some(Sent::Ended(Outcome::Accepted)));
        assert_eq!(count.0.load(Ordering::SeqCst), 1);
        assert_eq!(waiting.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    });
}

#[test]
fn a_reservation_waits_for_a_block_to_return() {
    brand!(Net);
    let _ended = run(bench::<Net>(), async |socket, _| {
        let mut held = Vec::new();
        for _ in 0..BLOCKS {
            held.push(socket.reserve().await.unwrap());
        }
        {
            let mut waiting = pin!(socket.reserve());
            let mut cx = Context::from_waker(Waker::noop());
            for _ in 0..4 {
                assert!(waiting.as_mut().poll(&mut cx).is_pending());
                yield_now().await;
            }
        }
        socket.release(held.pop().unwrap()).ok().unwrap();
        held.push(socket.reserve().await.unwrap());
    });
}

#[test]
fn a_datagram_the_core_refuses_ends_with_that_reason() {
    brand!(Net);
    let (ended, _, sent) = run(bench::<Net>(), async |socket, _| {
        let block = socket.reserve().await.unwrap();
        let far = Remote {
            address: [198, 51, 100, 1],
            port: 7,
        };
        result_of(socket.send(far, block).ok().unwrap()).await
    });
    assert_eq!(sent, Sent::Refused(SendRefusal::NoRoute));
    assert_eq!(ended.retired.pool.outstanding(), 0);
}

#[test]
fn time_ends_a_neighbor_that_never_answers() {
    brand!(Net);
    let bench = bench::<Net>();
    bench.wire.0.borrow_mut().answers = false;
    let (_, _, sent) = run(bench, async |socket, _| send_one(socket, b"nobody").await);
    assert_eq!(sent, Sent::Ended(Outcome::Unreachable));
}

#[test]
fn a_close_ends_the_socket_and_later_commands_are_refused() {
    brand!(Net);
    let (mut ended, mut socket, ()) = run(bench::<Net>(), async |socket, _| {
        let block = socket.reserve().await.unwrap();
        socket.close().await;
        assert!(socket.closed());
        assert_eq!(socket.credit().await, Err(SocketClosed));
        let refused = socket.send(peer(), block).err().unwrap();
        assert_eq!(refused.reason, Refusal::Closed);
        assert_eq!(socket.receive().await.err(), Some(SocketClosed));
        socket.release(refused.block).ok().unwrap();
    });
    returned(&mut ended, &mut socket);
}

#[test]
fn a_stop_publishes_nothing_more_and_each_result_arrives_after_it() {
    brand!(Net);
    let (mut ended, mut socket, (known, running, held)) =
        run(bench::<Net>(), async |socket, wire| {
            let mut known = send(socket, b"known").await;
            known.finished().await;
            open_port(wire, true);
            let running = send(socket, b"unsent").await;
            let held = socket.reserve().await.unwrap();
            settle().await;
            (known, running, held)
        });
    open_port(&ended.retired.port.clone(), false);
    assert_eq!(
        datagrams(&ended.retired.port.0.borrow()),
        [b"known".to_vec()],
        "a port with room again gets nothing"
    );
    assert_eq!(known.take().ok(), Some(Sent::Ended(Outcome::Accepted)));
    assert_eq!(running.take().ok(), Some(Sent::Ended(Outcome::Retired)));
    assert_eq!(ended.cause, EndCause::Stopped);
    assert_eq!(
        ended.retired.pool.outstanding(),
        1,
        "the application's block"
    );
    ended.retired.pool.give(held);
    assert_eq!(ended.retired.pool.outstanding(), 0);
    assert!(socket.closed());
}

#[test]
fn a_stop_wakes_a_service_with_nothing_to_do() {
    brand!(Net);
    let _ended = run(bench::<Net>(), async |_, _| settle().await);
}

#[test]
fn a_stop_wakes_a_waiting_observer_and_a_waiting_credit() {
    brand!(Net);
    let (_, _, ((count, _), (credits, _))) = run(bench::<Net>(), async |socket, wire| {
        open_port(wire, true);
        let mut first = send(socket, b"one").await;
        let mut second = send(socket, b"two").await;
        settle().await;
        let observed = counter();
        let credited = counter();
        assert!(
            pin!(first.finished())
                .poll(&mut Context::from_waker(&observed.1))
                .is_pending()
        );
        assert!(
            pin!(socket.credit())
                .poll(&mut Context::from_waker(&credited.1))
                .is_pending()
        );
        let _ = pin!(second.finished()).poll(&mut Context::from_waker(Waker::noop()));
        core::mem::forget((first, second));
        (observed, credited)
    });
    assert_eq!(count.0.load(Ordering::SeqCst), 1);
    assert_eq!(credits.0.load(Ordering::SeqCst), 1);
}

#[test]
fn a_datagram_written_as_the_service_stops_ends_retired_and_its_block_returns() {
    brand!(Net);
    let Bench {
        service,
        mut socket,
        wire,
        stop,
        clock,
    } = bench::<Net>();
    let mut events = ();
    let drive = async {
        let block = fill(&mut socket, b"late").await;
        // The stop comes first: the service takes it at its next look.
        stop.try_send(()).ok().unwrap();
        socket.send(peer(), block).ok().unwrap()
    };
    let (mut ended, observer) = run_tasks(clock, serve(service, clock, &mut events), drive);
    assert_eq!(observer.take().ok(), Some(Sent::Ended(Outcome::Retired)));
    assert_eq!(ended.retired.pool.outstanding(), 1);
    returned(&mut ended, &mut socket);
    assert!(socket.reclaim().is_none());
    assert!(datagrams(&wire.0.borrow()).is_empty());
}

#[test]
fn a_late_application_gets_its_blocks_back_after_a_stop() {
    brand!(Net);
    let (mut ended, mut socket, ()) = run(bench::<Net>(), async |socket, wire| {
        arrive(wire, 1);
        while socket.received() == 0 {
            yield_now().await;
        }
    });
    assert_eq!(ended.retired.pool.outstanding(), 1);
    returned(&mut ended, &mut socket);
}

#[test]
fn a_port_that_contradicts_itself_ends_the_service_with_that_cause() {
    brand!(Net);
    let (mut ended, mut socket, (first, second)) = run(bench::<Net>(), async |socket, wire| {
        assert_eq!(
            send_one(socket, b"resolved").await,
            Sent::Ended(Outcome::Accepted)
        );
        wire.0.borrow_mut().contradicts = true;
        let (one, two) = (fill(socket, b"one").await, fill(socket, b"two").await);
        let mut first = socket.send(peer(), one).ok().unwrap();
        let second = socket.send(peer(), two).ok().unwrap();
        first.finished().await;
        while !socket.closed() {
            yield_now().await;
        }
        let block = socket.reserve().await.err();
        assert_eq!(block, Some(SocketClosed));
        (first, second)
    });
    assert_eq!(ended.cause, EndCause::PortContract);
    assert_eq!(first.take().ok(), Some(Sent::Ended(Outcome::Unknown)));
    assert_eq!(second.take().ok(), Some(Sent::Ended(Outcome::Retired)));
    assert!(ended.retired.port.0.borrow().contradicts);
    returned(&mut ended, &mut socket);
}

#[test]
fn a_dropped_socket_ends_its_commands_and_returns_its_blocks() {
    brand!(Net);
    let (mut ended, (seen, unseen, held)) = run_owning(bench::<Net>(), async |mut socket, wire| {
        open_port(wire, true);
        let seen = send(&mut socket, b"seen").await;
        arrive(wire, 1);
        while socket.received() == 0 {
            yield_now().await;
        }
        let held = socket.reserve().await.unwrap();
        let block = fill(&mut socket, b"unseen").await;
        let unseen = socket.send(peer(), block).ok().unwrap();
        {
            let waiting = pin!(socket.reserve());
            let _ = waiting.poll(&mut Context::from_waker(Waker::noop()));
        }
        drop(socket);
        let (seen, unseen) = (result_of(seen).await, result_of(unseen).await);
        open_port(wire, false);
        settle().await;
        (seen, unseen, held)
    });
    assert_eq!(seen, Sent::Ended(Outcome::Closed));
    assert_eq!(unseen, Sent::Ended(Outcome::Closed));
    assert_eq!(
        ended.retired.pool.outstanding(),
        1,
        "the application's block"
    );
    ended.retired.pool.give(held);
    assert_eq!(ended.retired.pool.outstanding(), 0);
    assert!(datagrams(&ended.retired.port.0.borrow()).is_empty());
}

#[test]
fn a_dropped_idle_socket_wakes_the_service() {
    brand!(Net);
    let (_, sent) = run_owning(bench::<Net>(), async |mut socket, wire| {
        open_port(wire, true);
        let observer = send(&mut socket, b"held").await;
        settle().await;
        drop(socket);
        result_of(observer).await
    });
    assert_eq!(sent, Sent::Ended(Outcome::Closed));
}

#[test]
fn a_socket_dropped_as_the_service_stops_loses_no_block() {
    brand!(Net);
    let Bench {
        service,
        mut socket,
        wire,
        stop,
        clock,
    } = bench::<Net>();
    let mut events = ();
    let drive = async {
        arrive(&wire, 1);
        while socket.received() == 0 {
            yield_now().await;
        }
        let block = fill(&mut socket, b"late").await;
        let observer = socket.send(peer(), block).ok().unwrap();
        stop.try_send(()).ok().unwrap();
        drop(socket);
        observer
    };
    let (ended, observer) = run_tasks(clock, serve(service, clock, &mut events), drive);
    assert_eq!(observer.take().ok(), Some(Sent::Ended(Outcome::Retired)));
    assert_eq!(ended.retired.pool.outstanding(), 0);
}

#[test]
fn a_stop_alone_wakes_an_observer_of_a_command_it_never_saw() {
    brand!(Net);
    let Bench {
        service,
        mut socket,
        stop,
        clock,
        ..
    } = bench::<Net>();
    let mut events = ();
    let (count, waker) = counter();
    let drive = async {
        let block = fill(&mut socket, b"unseen").await;
        let mut observer = socket.send(peer(), block).ok().unwrap();
        assert!(
            pin!(observer.finished())
                .poll(&mut Context::from_waker(&waker))
                .is_pending()
        );
        stop.try_send(()).ok().unwrap();
        observer
    };
    let (_, observer) = run_tasks(clock, serve(service, clock, &mut events), drive);
    assert_eq!(count.0.load(Ordering::SeqCst), 1);
    assert_eq!(observer.take().ok(), Some(Sent::Ended(Outcome::Retired)));
}

#[test]
fn a_socket_dropped_after_a_stop_leaves_its_blocks_to_the_late_socket() {
    brand!(Net);
    let (mut ended, socket, ()) = run(bench::<Net>(), async |socket, wire| {
        arrive(wire, 1);
        while socket.received() == 0 {
            yield_now().await;
        }
    });
    let Ended {
        retired, sockets, ..
    } = &mut ended;
    let late = sockets[0].as_mut().unwrap();
    assert!(!late.collect(&mut retired.pool));
    assert_eq!(retired.pool.outstanding(), 1);
    drop(socket);
    assert!(late.collect(&mut retired.pool));
    assert_eq!(retired.pool.outstanding(), 0);
}

#[test]
fn a_full_datagram_lane_wakes_the_service_when_it_is_read() {
    brand!(Net);
    let _ended = run(bench::<Net>(), async |socket, wire| {
        arrive(wire, RECEIVED);
        while socket.received() < RECEIVED {
            yield_now().await;
        }
        // These wait in the core, as the lane is full.
        arrive(wire, RECEIVED);
        settle().await;
        // Nothing is released: only the room read here can wake the service.
        let mut held = Vec::new();
        for _ in 0..2 * RECEIVED {
            held.push(socket.receive().await.unwrap());
        }
    });
}

#[test]
fn a_refused_bind_changes_nothing_and_the_storage_serves_again() {
    brand!(Net);
    let memory = Box::leak(Box::new([[0; SIZE]; BLOCKS]));
    let (pool, brand) = claim::<Net, SIZE, BLOCKS>(memory).unwrap();
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let wire = WirePort::default();
    wire.0.borrow_mut().answers = true;
    let owner: NetworkOwner<'static, Net, SIZE, 2, CREDITS, RECEIVED, 2, BLOCKS, WirePort> =
        NetworkOwner::new(brand, interface, pool, wire.clone());
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut service = Service::new(owner, MemoryPolicy::KEEP, stopping);
    let dscp = Dscp::default();
    let mut first = service
        .bind(5000, dscp, Group::shared(), storage::<Net>())
        .ok()
        .unwrap();
    let refused = service
        .bind(5000, dscp, Group::shared(), storage::<Net>())
        .err()
        .unwrap();
    assert_eq!(refused.reason, OpenRefused::PortInUse);
    let mut second = service
        .bind(5001, dscp, Group::shared(), refused.storage)
        .ok()
        .unwrap();
    let full = service
        .bind(5002, dscp, Group::shared(), storage::<Net>())
        .err()
        .unwrap();
    assert_eq!(full.reason, OpenRefused::NoSocket);
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let drive = async {
        let mut one = first.reserve().await.unwrap();
        one.fill(b"one").unwrap();
        let mut two = second.reserve().await.unwrap();
        two.fill(b"two").unwrap();
        let (one, two) = (
            first.send(peer(), one).ok().unwrap(),
            second.send(peer(), two).ok().unwrap(),
        );
        let results = (result_of(one).await, result_of(two).await);
        stop.try_send(()).ok().unwrap();
        results
    };
    let (cause, results) = run_tasks(clock, service.run(clock, &mut events), drive);
    let accepted = Sent::Ended(Outcome::Accepted);
    assert_eq!(results, (accepted, accepted));
    assert_eq!(cause, EndCause::Stopped);
}

#[test]
fn a_service_dropped_unrun_ends_what_its_sockets_wait_for() {
    brand!(Net);
    let Bench {
        service,
        mut socket,
        ..
    } = bench::<Net>();
    let block = Waker::noop();
    let mut cx = Context::from_waker(block);
    let (count, waker) = counter();
    let waiting_cx = &mut Context::from_waker(&waker);
    {
        let mut reserve = pin!(socket.reserve());
        assert!(reserve.as_mut().poll(waiting_cx).is_pending());
        drop(service);
        assert!(
            count.0.load(Ordering::SeqCst) >= 1,
            "the end wakes a reserve"
        );
        assert!(matches!(
            reserve.as_mut().poll(&mut cx),
            Poll::Ready(Err(SocketClosed))
        ));
    }
    assert!(socket.closed());
    let mut receive = pin!(socket.receive());
    assert!(matches!(
        receive.as_mut().poll(&mut cx),
        Poll::Ready(Err(SocketClosed))
    ));
}

#[test]
fn a_waiting_receive_hears_its_service_end() {
    brand!(Net);
    let Bench {
        service,
        mut socket,
        ..
    } = bench::<Net>();
    let (count, waker) = counter();
    let cx = &mut Context::from_waker(&waker);
    let mut receive = pin!(socket.receive());
    assert!(receive.as_mut().poll(cx).is_pending());
    drop(service);
    assert!(count.0.load(Ordering::SeqCst) >= 1);
    assert!(matches!(
        receive.as_mut().poll(cx),
        Poll::Ready(Err(SocketClosed))
    ));
}

/// Take `count` blocks from a service driven by hand.
fn blocks_by_hand<B: 'static, R: Future>(
    socket: &mut TestSocket<B>,
    run: &mut core::pin::Pin<&mut R>,
    count: usize,
) -> Vec<Block<'static, B, SIZE>> {
    let cx = &mut Context::from_waker(Waker::noop());
    let mut blocks = Vec::new();
    for _ in 0..count {
        let mut reserve = pin!(socket.reserve());
        blocks.push(loop {
            if let Poll::Ready(block) = reserve.as_mut().poll(cx) {
                break block.unwrap();
            }
            assert!(run.as_mut().poll(cx).is_pending());
        });
    }
    blocks
}

#[test]
fn a_cancelled_run_leaves_every_owner_with_its_service() {
    brand!(Net);
    let Bench {
        mut service,
        mut socket,
        wire,
        clock,
        ..
    } = bench::<Net>();
    let mut events = ();
    let (count, waker) = counter();
    let cx = &mut Context::from_waker(Waker::noop());
    let (known, mut unseen) = {
        let mut run = pin!(service.run(clock, &mut events));
        let mut blocks = blocks_by_hand(&mut socket, &mut run, 2);
        let mut known = socket.send(peer(), blocks.pop().unwrap()).ok().unwrap();
        let mut finished = false;
        while !finished {
            assert!(run.as_mut().poll(cx).is_pending());
            finished = pin!(known.finished()).poll(cx).is_ready();
        }
        let unseen = socket.send(peer(), blocks.pop().unwrap()).ok().unwrap();
        (known, unseen)
    };
    assert!(
        pin!(unseen.finished())
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    assert_eq!(
        count.0.load(Ordering::SeqCst),
        0,
        "a cancelled run ends nothing"
    );
    assert!(!socket.closed());
    let mut ended = service.retire();
    assert_eq!(count.0.load(Ordering::SeqCst), 1, "its retirement does");
    assert_eq!(ended.cause, EndCause::Stopped);
    assert_eq!(known.take().ok(), Some(Sent::Ended(Outcome::Accepted)));
    assert_eq!(unseen.take().ok(), Some(Sent::Ended(Outcome::Retired)));
    returned(&mut ended, &mut socket);
    assert_eq!(datagrams(&wire.0.borrow()).len(), 1);
    let retired = ended.retired;
    let owner = NetworkOwner::new(
        retired.brand,
        Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap(),
        retired.pool,
        retired.port,
    );
    let (_, _, sent) = run(bench_on(owner, wire), async |socket, _| {
        send_one(socket, b"next").await
    });
    assert_eq!(sent, Sent::Ended(Outcome::Accepted));
}

#[test]
fn a_cancelled_run_may_start_again() {
    brand!(Net);
    let Bench {
        mut service,
        mut socket,
        stop,
        clock,
        ..
    } = bench::<Net>();
    let mut events = ();
    let observer = {
        let mut run = pin!(service.run(clock, &mut events));
        let mut blocks = blocks_by_hand(&mut socket, &mut run, 1);
        socket.send(peer(), blocks.pop().unwrap()).ok().unwrap()
    };
    let drive = async {
        let sent = result_of(observer).await;
        stop.try_send(()).ok().unwrap();
        sent
    };
    let (cause, sent) = run_tasks(clock, service.run(clock, &mut events), drive);
    assert_eq!(cause, EndCause::Stopped);
    assert_eq!(sent, Sent::Ended(Outcome::Accepted));
}

#[test]
fn a_run_that_ended_serves_no_more() {
    brand!(Net);
    let Bench {
        mut service,
        mut socket,
        wire,
        stop,
        clock,
    } = bench::<Net>();
    let mut events = ();
    let cx = &mut Context::from_waker(Waker::noop());
    let block = {
        let mut run = pin!(service.run(clock, &mut events));
        blocks_by_hand(&mut socket, &mut run, 1).pop().unwrap()
    };
    stop.try_send(()).ok().unwrap();
    let ended = pin!(service.run(clock, &mut events)).poll(cx);
    assert_eq!(ended, Poll::Ready(EndCause::Stopped));
    let observer = socket.send(peer(), block).ok().unwrap();
    let again = pin!(service.run(clock, &mut events)).poll(cx);
    assert_eq!(again, Poll::Ready(EndCause::Stopped));
    assert!(wire.0.borrow().sent.is_empty());
    let mut ended = service.retire();
    assert_eq!(observer.take().ok(), Some(Sent::Ended(Outcome::Retired)));
    returned(&mut ended, &mut socket);
}

fn a_service_end_ends_a_full_lane_wait<B: 'static>(end: impl FnOnce(TestService<B>)) {
    let Bench {
        mut service,
        mut socket,
        wire,
        clock,
        ..
    } = bench::<B>();
    let mut events = ();
    let mut blocks = {
        let mut run = pin!(service.run(clock, &mut events));
        blocks_by_hand(&mut socket, &mut run, BLOCKS)
    };
    wire.0.borrow_mut().full = true;
    let first = socket.send(peer(), blocks.pop().unwrap()).ok().unwrap();
    let second = socket.send(peer(), blocks.pop().unwrap()).ok().unwrap();
    socket.release(blocks.pop().unwrap()).ok().unwrap();
    socket.release(blocks.pop().unwrap()).ok().unwrap();
    let cx = &mut Context::from_waker(Waker::noop());
    assert!(pin!(socket.reserve()).poll(cx).is_pending());
    let (roomy, room_waker) = counter();
    let room_cx = &mut Context::from_waker(&room_waker);
    let mut room = pin!(socket.room());
    assert!(room.as_mut().poll(room_cx).is_pending());
    end(service);
    assert!(roomy.0.load(Ordering::SeqCst) >= 1, "the end wakes room");
    assert_eq!(room.as_mut().poll(room_cx), Poll::Ready(Err(SocketClosed)));
    assert_eq!(first.take().ok(), Some(Sent::Ended(Outcome::Retired)));
    assert_eq!(second.take().ok(), Some(Sent::Ended(Outcome::Retired)));
}

#[test]
fn a_retirement_ends_a_full_lane_wait() {
    brand!(Net);
    a_service_end_ends_a_full_lane_wait::<Net>(|service| drop(service.retire()));
}

#[test]
fn a_dropped_service_ends_a_full_lane_wait() {
    brand!(Net);
    a_service_end_ends_a_full_lane_wait::<Net>(drop);
}

#[test]
fn sockets_waiting_for_blocks_take_turns() {
    brand!(Net);
    let memory = Box::leak(Box::new([[0; SIZE]; 1]));
    let (pool, brand) = claim::<Net, SIZE, 1>(memory).unwrap();
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let owner: NetworkOwner<'static, Net, SIZE, 2, CREDITS, RECEIVED, 2, 1, WirePort> =
        NetworkOwner::new(brand, interface, pool, WirePort::default());
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut service = Service::new(owner, MemoryPolicy::KEEP, stopping);
    let dscp = Dscp::default();
    let mut first = service
        .bind(5000, dscp, Group::shared(), storage::<Net>())
        .ok()
        .unwrap();
    let mut second = service
        .bind(5001, dscp, Group::shared(), storage::<Net>())
        .ok()
        .unwrap();
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let cx = &mut Context::from_waker(Waker::noop());
    let drive = async {
        for _ in 0..4 {
            let block = first.reserve().await.unwrap();
            // The second asks while the first holds the one block.
            assert!(pin!(second.reserve()).poll(cx).is_pending());
            settle().await;
            first.release(block).ok().unwrap();
            // The first asks at once, and the second was first in line.
            assert!(pin!(first.reserve()).poll(cx).is_pending());
            let block = second.reserve().await.unwrap();
            settle().await;
            second.release(block).ok().unwrap();
        }
        stop.try_send(()).ok().unwrap();
    };
    let (cause, ()) = run_tasks(clock, service.run(clock, &mut events), drive);
    assert_eq!(cause, EndCause::Stopped);
}

#[test]
fn three_sockets_waiting_for_two_blocks_all_take_turns() {
    brand!(Net);
    let memory = Box::leak(Box::new([[0; SIZE]; 2]));
    let (pool, brand) = claim::<Net, SIZE, 2>(memory).unwrap();
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let owner: NetworkOwner<'static, Net, SIZE, 3, CREDITS, RECEIVED, 2, 2, WirePort> =
        NetworkOwner::new(brand, interface, pool, WirePort::default());
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut service = Service::new(owner, MemoryPolicy::KEEP, stopping);
    let dscp = Dscp::default();
    let mut first = service
        .bind(5000, dscp, Group::shared(), storage::<Net>())
        .ok()
        .unwrap();
    let mut second = service
        .bind(5001, dscp, Group::shared(), storage::<Net>())
        .ok()
        .unwrap();
    let mut third = service
        .bind(5002, dscp, Group::shared(), storage::<Net>())
        .ok()
        .unwrap();
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let cx = &mut Context::from_waker(Waker::noop());
    let drive = async {
        let mut got = [0_usize; 3];
        for _ in 0..99 {
            // Every request is in before a turn of the service.
            for socket in [&mut first, &mut second, &mut third] {
                assert!(pin!(socket.reserve()).poll(cx).is_pending());
            }
            settle().await;
            for (index, socket) in [&mut first, &mut second, &mut third]
                .into_iter()
                .enumerate()
            {
                let ready = pin!(socket.reserve()).poll(cx);
                if let Poll::Ready(Ok(block)) = ready {
                    got[index] += 1;
                    socket.release(block).ok().unwrap();
                }
            }
        }
        assert_eq!(got, [66, 66, 66]);
        stop.try_send(()).ok().unwrap();
    };
    let (cause, ()) = run_tasks(clock, service.run(clock, &mut events), drive);
    assert_eq!(cause, EndCause::Stopped);
}

/// A service of two sockets over `N` blocks, by hand.
type TestPair<B, const N: usize> =
    Service<'static, 'static, B, SIZE, 2, CREDITS, RECEIVED, 2, N, DEPTH, RECEIVED, WirePort>;

/// Two sockets, the second's group limited to `limit` and the pool to `BLOCKS`.
fn pair<B: 'static>(
    limit: usize,
    memory: MemoryPolicy,
) -> (
    TestPair<B, 2>,
    [TestSocket<B>; 2],
    WirePort,
    WakingSpscSender<'static, (), 1>,
) {
    let storage_of_blocks = Box::leak(Box::new([[0; SIZE]; 2]));
    let (pool, brand) = claim::<B, SIZE, 2>(storage_of_blocks).unwrap();
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let wire = WirePort::default();
    wire.0.borrow_mut().answers = true;
    let owner = NetworkOwner::new(brand, interface, pool, wire.clone());
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut service = Service::new(owner, memory, stopping);
    let dscp = Dscp::default();
    let first = service
        .bind(5000, dscp, Group::shared(), storage::<B>())
        .ok()
        .unwrap();
    let group = service.group(limit).unwrap();
    let second = service
        .bind(5001, dscp, group, storage::<B>())
        .ok()
        .unwrap();
    (service, [first, second], wire, stop)
}

#[test]
fn a_group_past_its_limit_waits_with_blocks_free() {
    brand!(Net);
    let (mut service, [_first, mut second], _, stop) = pair::<Net>(1, MemoryPolicy::KEEP);
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let cx = &mut Context::from_waker(Waker::noop());
    let drive = async {
        let held = second.reserve().await.unwrap();
        assert!(pin!(second.reserve()).poll(cx).is_pending());
        settle().await;
        assert!(
            pin!(second.reserve()).poll(cx).is_pending(),
            "one is free, and its limit is one"
        );
        second.release(held).ok().unwrap();
        let again = second.reserve().await.unwrap();
        stop.try_send(()).ok().unwrap();
        again
    };
    run_tasks(clock, service.run(clock, &mut events), drive);
}

#[test]
fn blocks_an_application_holds_are_never_evicted() {
    brand!(Net);
    let evict = MemoryPolicy {
        evict_after: Some(oer_portable_time::Duration::from_micros(1)),
    };
    let (mut service, [mut first, mut second], _, stop) = pair::<Net>(usize::MAX, evict);
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let cx = &mut Context::from_waker(Waker::noop());
    let drive = async {
        let held = [
            first.reserve().await.unwrap(),
            first.reserve().await.unwrap(),
        ];
        assert!(pin!(second.reserve()).poll(cx).is_pending());
        settle().await;
        assert!(
            clock
                .advance(oer_portable_time::Duration::from_secs(5))
                .is_ok()
        );
        settle().await;
        assert!(
            pin!(second.reserve()).poll(cx).is_pending(),
            "the block stays"
        );
        stop.try_send(()).ok().unwrap();
        held
    };
    let _ = run_tasks(clock, service.run(clock, &mut events), drive);
}

#[test]
fn a_received_datagram_counts_toward_its_group() {
    brand!(Net);
    let (mut service, [_first, mut second], wire, stop) = pair::<Net>(1, MemoryPolicy::KEEP);
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let cx = &mut Context::from_waker(Waker::noop());
    let drive = async {
        {
            let mut wire = wire.0.borrow_mut();
            wire.incoming.push_back(udp_frame(5001, b"held"));
            wire.wake();
        }
        let datagram = second.receive().await.unwrap();
        assert!(pin!(second.reserve()).poll(cx).is_pending());
        settle().await;
        assert!(
            pin!(second.reserve()).poll(cx).is_pending(),
            "the one it may hold is the one it received"
        );
        second.release(datagram.block).ok().unwrap();
        let block = second.reserve().await.unwrap();
        stop.try_send(()).ok().unwrap();
        block
    };
    run_tasks(clock, service.run(clock, &mut events), drive);
}

#[test]
fn pressure_takes_the_oldest_and_leaves_the_younger() {
    brand!(Net);
    let evict = MemoryPolicy {
        evict_after: Some(oer_portable_time::Duration::from_secs(1)),
    };
    let (mut service, [mut first, mut second], wire, stop) = pair::<Net>(usize::MAX, evict);
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let cx = &mut Context::from_waker(Waker::noop());
    let drive = async {
        wire.0.borrow_mut().full = true;
        let older = first.reserve().await.unwrap();
        let older = first.send(peer(), older).ok().unwrap();
        settle().await;
        clock
            .advance(oer_portable_time::Duration::from_millis(600))
            .unwrap();
        let younger = first.reserve().await.unwrap();
        let younger = first.send(peer(), younger).ok().unwrap();
        settle().await;
        assert!(pin!(second.reserve()).poll(cx).is_pending());
        settle().await;
        // The older is 0.7 s old, so the policy waits; time alone wakes it.
        let block = second.reserve().await.unwrap();
        assert_eq!(
            older.take().ok(),
            Some(Sent::Ended(Outcome::Evicted(Eviction::MemoryPressure)))
        );
        let Err(younger) = younger.take() else {
            panic!("the younger stays");
        };
        stop.try_send(()).ok().unwrap();
        (block, younger)
    };
    let _kept = run_tasks(clock, service.run(clock, &mut events), drive);
}

#[test]
fn a_group_evicts_nothing_of_its_own_for_itself() {
    brand!(Net);
    let evict = MemoryPolicy {
        evict_after: Some(oer_portable_time::Duration::from_micros(1)),
    };
    let (mut service, [mut first, _second], wire, stop) = pair::<Net>(usize::MAX, evict);
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let cx = &mut Context::from_waker(Waker::noop());
    let drive = async {
        wire.0.borrow_mut().full = true;
        let blocks = [
            first.reserve().await.unwrap(),
            first.reserve().await.unwrap(),
        ];
        let [one, two] = blocks;
        let one = first.send(peer(), one).ok().unwrap();
        let two = first.send(peer(), two).ok().unwrap();
        settle().await;
        assert!(pin!(first.reserve()).poll(cx).is_pending());
        clock
            .advance(oer_portable_time::Duration::from_secs(1))
            .unwrap();
        settle().await;
        let (one, two) = (one.take().err().unwrap(), two.take().err().unwrap());
        stop.try_send(()).ok().unwrap();
        (one, two)
    };
    let _kept = run_tasks(clock, service.run(clock, &mut events), drive);
}

#[test]
fn pressure_spares_a_group_that_holds_less_however_old_its_work() {
    brand!(Net);
    let memory = Box::leak(Box::new([[0; SIZE]; 3]));
    let (pool, brand) = claim::<Net, SIZE, 3>(memory).unwrap();
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let wire = WirePort::default();
    wire.0.borrow_mut().full = true;
    let owner: NetworkOwner<'static, Net, SIZE, 3, CREDITS, RECEIVED, 2, 3, WirePort> =
        NetworkOwner::new(brand, interface, pool, wire.clone());
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let evict = MemoryPolicy {
        evict_after: Some(oer_portable_time::Duration::from_millis(100)),
    };
    let mut service = Service::new(owner, evict, stopping);
    let dscp = Dscp::default();
    let groups = [
        service.group(usize::MAX).unwrap(),
        service.group(usize::MAX).unwrap(),
    ];
    let mut few = service
        .bind(5000, dscp, Group::shared(), storage::<Net>())
        .ok()
        .unwrap();
    let mut many = service
        .bind(5001, dscp, groups[0], storage::<Net>())
        .ok()
        .unwrap();
    let mut waits = service
        .bind(5002, dscp, groups[1], storage::<Net>())
        .ok()
        .unwrap();
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let cx = &mut Context::from_waker(Waker::noop());
    let drive = async {
        let oldest = few.reserve().await.unwrap();
        let oldest = few.send(peer(), oldest).ok().unwrap();
        settle().await;
        clock
            .advance(oer_portable_time::Duration::from_secs(1))
            .unwrap();
        let [one, two] = [many.reserve().await.unwrap(), many.reserve().await.unwrap()];
        let one = many.send(peer(), one).ok().unwrap();
        let two = many.send(peer(), two).ok().unwrap();
        settle().await;
        assert!(pin!(waits.reserve()).poll(cx).is_pending());
        let block = waits.reserve().await.unwrap();
        let Err(oldest) = oldest.take() else {
            panic!("its group holds less");
        };
        let evicted = Sent::Ended(Outcome::Evicted(Eviction::MemoryPressure));
        assert_eq!(
            one.take().ok(),
            Some(evicted),
            "the oldest of the group holding most"
        );
        let two = two.take().err().unwrap();
        stop.try_send(()).ok().unwrap();
        (block, oldest, two)
    };
    let _kept = run_tasks(clock, service.run(clock, &mut events), drive);
}

/// Runs `client` beside a pair's service until it stops the service.
fn on_a_pair<B: 'static, R>(
    limit: usize,
    client: impl AsyncFnOnce([TestSocket<B>; 2], WirePort, &mut Context<'_>) -> R,
) -> R {
    let (mut service, sockets, wire, stop) = pair::<B>(limit, MemoryPolicy::KEEP);
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let mut cx = Context::from_waker(Waker::noop());
    let drive = async {
        let result = client(sockets, wire, &mut cx).await;
        stop.try_send(()).ok().unwrap();
        result
    };
    run_tasks(clock, service.run(clock, &mut events), drive).1
}

#[test]
fn a_block_returned_through_another_socket_frees_its_group() {
    brand!(Net);
    on_a_pair::<Net, _>(1, async |[mut other, mut limited], _, cx| {
        let block = limited.reserve().await.unwrap();
        other.release(block).ok().unwrap();
        settle().await;
        assert!(pin!(limited.reserve()).poll(cx).is_pending());
        settle().await;
        assert!(pin!(limited.reserve()).poll(cx).is_ready(), "it is back");
    });
}

#[test]
fn a_block_sent_through_another_socket_frees_its_group_when_it_goes() {
    brand!(Net);
    on_a_pair::<Net, _>(1, async |[mut other, mut limited], _, cx| {
        let block = limited.reserve().await.unwrap();
        let observer = other.send(peer(), block).ok().unwrap();
        assert_eq!(result_of(observer).await, Sent::Ended(Outcome::Accepted));
        settle().await;
        assert!(pin!(limited.reserve()).poll(cx).is_pending());
        settle().await;
        assert!(pin!(limited.reserve()).poll(cx).is_ready(), "it went");
    });
}

#[test]
fn another_groups_block_returned_here_frees_nothing_of_this_group() {
    brand!(Net);
    on_a_pair::<Net, _>(1, async |[mut other, mut limited], _, cx| {
        let held = limited.reserve().await.unwrap();
        let foreign = other.reserve().await.unwrap();
        limited.release(foreign).ok().unwrap();
        settle().await;
        assert!(pin!(limited.reserve()).poll(cx).is_pending());
        settle().await;
        assert!(
            pin!(limited.reserve()).poll(cx).is_pending(),
            "the limit holds"
        );
        held
    });
}

#[test]
fn a_dropped_socket_leaves_its_blocks_charged_until_they_return() {
    brand!(Net);
    let memory = Box::leak(Box::new([[0; SIZE]; 2]));
    let (pool, brand) = claim::<Net, SIZE, 2>(memory).unwrap();
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let wire = WirePort::default();
    let owner: NetworkOwner<'static, Net, SIZE, 2, CREDITS, RECEIVED, 2, 2, WirePort> =
        NetworkOwner::new(brand, interface, pool, wire);
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut service = Service::new(owner, MemoryPolicy::KEEP, stopping);
    let dscp = Dscp::default();
    let group = service.group(1).unwrap();
    let mut first = service
        .bind(5000, dscp, group, storage::<Net>())
        .ok()
        .unwrap();
    let mut second = service
        .bind(5001, dscp, group, storage::<Net>())
        .ok()
        .unwrap();
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let cx = &mut Context::from_waker(Waker::noop());
    let drive = async {
        let held = first.reserve().await.unwrap();
        drop(first);
        settle().await;
        assert!(pin!(second.reserve()).poll(cx).is_pending());
        settle().await;
        assert!(pin!(second.reserve()).poll(cx).is_pending(), "one is held");
        second.release(held).ok().unwrap();
        settle().await;
        assert!(
            pin!(second.reserve()).poll(cx).is_ready(),
            "and now it is not"
        );
        stop.try_send(()).ok().unwrap();
    };
    run_tasks(clock, service.run(clock, &mut events), drive);
}

#[test]
fn received_data_stays_within_its_groups_limit() {
    brand!(Net);
    on_a_pair::<Net, _>(1, async |[_other, mut limited], wire, cx| {
        {
            let mut wire = wire.0.borrow_mut();
            wire.incoming
                .extend([udp_frame(5001, b"one"), udp_frame(5001, b"two")]);
            wire.wake();
        }
        let held = limited.receive().await.unwrap();
        settle().await;
        assert!(
            pin!(limited.receive()).poll(cx).is_pending(),
            "the second has no block within the limit"
        );
        held
    });
}

#[test]
fn a_group_has_one_limit_however_many_sockets_it_has() {
    brand!(Net);
    let memory = Box::leak(Box::new([[0; SIZE]; 2]));
    let (pool, brand) = claim::<Net, SIZE, 2>(memory).unwrap();
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let owner: NetworkOwner<'static, Net, SIZE, 2, CREDITS, RECEIVED, 2, 2, WirePort> =
        NetworkOwner::new(brand, interface, pool, WirePort::default());
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut service = Service::new(owner, MemoryPolicy::KEEP, stopping);
    let dscp = Dscp::default();
    let group = service.group(1).unwrap();
    let mut first = service
        .bind(5000, dscp, group, storage::<Net>())
        .ok()
        .unwrap();
    let mut second = service
        .bind(5001, dscp, group, storage::<Net>())
        .ok()
        .unwrap();
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let cx = &mut Context::from_waker(Waker::noop());
    let drive = async {
        let held = first.reserve().await.unwrap();
        assert!(pin!(second.reserve()).poll(cx).is_pending());
        settle().await;
        assert!(pin!(second.reserve()).poll(cx).is_pending());
        stop.try_send(()).ok().unwrap();
        held
    };
    run_tasks(clock, service.run(clock, &mut events), drive);
}

type BeforeTheService<B> = (
    TestPair<B, 3>,
    [TestSocket<B>; 2],
    Vec<Block<'static, B, SIZE>>,
    WirePort,
    WakingSpscSender<'static, (), 1>,
);

/// Three blocks of which `held` of `group` were taken before the service, with
/// one socket in the shared group and a waiting one in a group of its own.
fn before_the_service<B: 'static>(held: usize) -> BeforeTheService<B> {
    let memory = Box::leak(Box::new([[0; SIZE]; 3]));
    let (mut pool, brand) = claim::<B, SIZE, 3>(memory).unwrap();
    let absent = pool.group(usize::MAX).unwrap();
    let blocks = (0..held).map(|_| pool.take_as(absent).unwrap()).collect();
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let wire = WirePort::default();
    wire.0.borrow_mut().full = true;
    let owner = NetworkOwner::new(brand, interface, pool, wire.clone());
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let evict = MemoryPolicy {
        evict_after: Some(oer_portable_time::Duration::from_millis(100)),
    };
    let mut service = Service::new(owner, evict, stopping);
    let dscp = Dscp::default();
    let sender = service
        .bind(5000, dscp, Group::shared(), storage::<B>())
        .ok()
        .unwrap();
    let group = service.group(usize::MAX).unwrap();
    let waiter = service
        .bind(5001, dscp, group, storage::<B>())
        .ok()
        .unwrap();
    (service, [sender, waiter], blocks, wire, stop)
}

/// An eviction is a known end: the turn that evicts hands it to its observer
/// before its next await, so a cancelled `run` and a dropped service leave
/// it `Evicted`, never `Retired`.
#[test]
fn an_eviction_reaches_its_observer_in_the_turn_that_evicts() {
    brand!(Net);
    let (mut service, [mut sender, mut waiter], mut blocks, _wire, _stop) =
        before_the_service::<Net>(3);
    let clock: VirtualClock = VirtualClock::new();
    let mut events = ();
    let cx = &mut Context::from_waker(Waker::noop());
    let evicted = sender.send(peer(), blocks.pop().unwrap()).ok().unwrap();
    {
        let mut run = pin!(service.run(&clock, &mut events));
        for _ in 0..4 {
            assert!(run.as_mut().poll(cx).is_pending());
        }
        assert!(pin!(waiter.reserve()).poll(cx).is_pending());
        clock
            .advance(oer_portable_time::Duration::from_millis(100))
            .unwrap();
        // This turn takes the request and evicts at the pressure deadline.
        assert!(run.as_mut().poll(cx).is_pending());
    }
    drop(service);
    assert_eq!(
        evicted.take().ok(),
        Some(Sent::Ended(Outcome::Evicted(Eviction::MemoryPressure)))
    );
}

#[test]
fn pressure_reaches_a_group_with_no_socket_here() {
    brand!(Net);
    let (mut service, [mut sender, mut waiter], mut blocks, _, stop) = before_the_service::<Net>(3);
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let cx = &mut Context::from_waker(Waker::noop());
    let drive = async {
        let older = sender.send(peer(), blocks.pop().unwrap()).ok().unwrap();
        let younger = sender.send(peer(), blocks.pop().unwrap()).ok().unwrap();
        let held = blocks.pop().unwrap();
        settle().await;
        assert!(pin!(waiter.reserve()).poll(cx).is_pending());
        let block = waiter.reserve().await.unwrap();
        let evicted = Sent::Ended(Outcome::Evicted(Eviction::MemoryPressure));
        assert_eq!(older.take().ok(), Some(evicted));
        stop.try_send(()).ok().unwrap();
        (block, held, younger)
    };
    let _kept = run_tasks(clock, service.run(clock, &mut events), drive);
}

#[test]
fn pressure_leaves_a_lighter_group_alone_when_the_heaviest_is_in_hands() {
    brand!(Net);
    let (mut service, [mut sender, mut waiter], blocks, _, stop) = before_the_service::<Net>(2);
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let cx = &mut Context::from_waker(Waker::noop());
    let drive = async {
        let own = sender.reserve().await.unwrap();
        let queued = sender.send(peer(), own).ok().unwrap();
        settle().await;
        assert!(pin!(waiter.reserve()).poll(cx).is_pending());
        settle().await;
        clock
            .advance(oer_portable_time::Duration::from_secs(5))
            .unwrap();
        settle().await;
        let queued = queued.take().err().unwrap();
        assert!(
            pin!(waiter.reserve()).poll(cx).is_pending(),
            "the source holds them"
        );
        stop.try_send(()).ok().unwrap();
        (blocks, queued)
    };
    let _kept = run_tasks(clock, service.run(clock, &mut events), drive);
}

/// A pair's pieces with a return lane.
fn returning<B: 'static>(
    limit: usize,
) -> (
    TestPair<B, 2>,
    [TestSocket<B>; 2],
    WakingSpscSender<'static, (), 1>,
    Returner<'static, 'static, B, SIZE, 2>,
) {
    let (mut service, sockets, _, stop) = pair::<B>(limit, MemoryPolicy::KEEP);
    let (returner, end) = Box::leak(Box::new(ReturnLane::new())).split();
    service.attach_returns(end).ok().unwrap();
    (service, sockets, stop, returner)
}

#[test]
fn a_block_goes_home_with_no_socket_and_wakes_who_waits() {
    brand!(Net);
    let (mut service, [_other, mut limited], stop, returner) = returning::<Net>(1);
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let cx = &mut Context::from_waker(Waker::noop());
    let drive = async {
        let held = limited.reserve().await.unwrap();
        assert!(pin!(limited.reserve()).poll(cx).is_pending());
        settle().await;
        returner.give(held).ok().unwrap();
        let again = limited.reserve().await.unwrap();
        stop.try_send(()).ok().unwrap();
        again
    };
    let _kept = run_tasks(clock, service.run(clock, &mut events), drive);
}

#[test]
fn the_return_lane_holds_every_block() {
    brand!(Net);
    let (mut service, [mut other, _limited], stop, returner) = returning::<Net>(1);
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let drive = async {
        let blocks = [
            other.reserve().await.unwrap(),
            other.reserve().await.unwrap(),
        ];
        for block in blocks {
            returner.give(block).ok().unwrap();
        }
        stop.try_send(()).ok().unwrap();
    };
    let (cause, ()) = run_tasks(clock, service.run(clock, &mut events), drive);
    assert_eq!(cause, EndCause::Stopped);
    let mut ended = service.retire();
    ended.collect();
    assert_eq!(ended.retired.pool.outstanding(), 0);
}

#[test]
fn a_block_outlives_its_service_and_reaches_the_next() {
    brand!(Net);
    let (mut service, [_first, mut second], stop, returner) = returning::<Net>(1);
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let drive = async {
        let held = second.reserve().await.unwrap();
        stop.try_send(()).ok().unwrap();
        held
    };
    let (_, held) = run_tasks(clock, service.run(clock, &mut events), drive);
    let ended = service.retire();
    let retired = ended.retired;
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let owner = NetworkOwner::new(retired.brand, interface, retired.pool, retired.port);
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut next: TestPair<Net, 2> = Service::new(owner, MemoryPolicy::KEEP, stopping);
    next.attach_returns(ended.returns.unwrap()).ok().unwrap();
    let dscp = Dscp::default();
    let group = held.group();
    let mut socket = next.bind(5002, dscp, group, storage::<Net>()).ok().unwrap();
    let cx = &mut Context::from_waker(Waker::noop());
    let drive = async {
        assert!(pin!(socket.reserve()).poll(cx).is_pending());
        settle().await;
        assert!(
            pin!(socket.reserve()).poll(cx).is_pending(),
            "the limit holds"
        );
        returner.give(held).ok().unwrap();
        let block = socket.reserve().await.unwrap();
        stop.try_send(()).ok().unwrap();
        block
    };
    let _kept = run_tasks(clock, next.run(clock, &mut events), drive);
}

#[test]
fn an_adopted_socket_brings_its_blocks_to_the_running_service() {
    brand!(Net);
    let (mut service, [mut first, mut second], _, stop) =
        pair::<Net>(usize::MAX, MemoryPolicy::KEEP);
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let cx = &mut Context::from_waker(Waker::noop());
    let drive = async {
        // Each block stays unread in its socket's lanes.
        assert!(pin!(first.reserve()).poll(cx).is_pending());
        assert!(pin!(second.reserve()).poll(cx).is_pending());
        settle().await;
        stop.try_send(()).ok().unwrap();
    };
    run_tasks(clock, service.run(clock, &mut events), drive);
    let mut ended = service.retire();
    let late = ended.sockets.each_mut().map(|late| late.take().unwrap());
    assert_eq!(ended.retired.pool.outstanding(), 2);
    let retired = ended.retired;
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let owner = NetworkOwner::new(retired.brand, interface, retired.pool, retired.port);
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut next: TestPair<Net, 2> = Service::new(owner, MemoryPolicy::KEEP, stopping);
    let [one, two] = late;
    next.adopt(one).ok().unwrap();
    next.adopt(two).ok().unwrap();
    let dscp = Dscp::default();
    let mut socket = next
        .bind(5002, dscp, Group::shared(), storage::<Net>())
        .ok()
        .unwrap();
    let drive = async {
        assert!(pin!(socket.reserve()).poll(cx).is_pending());
        settle().await;
        assert!(pin!(socket.reserve()).poll(cx).is_pending(), "both are out");
        drop(first);
        // Only the dropped socket's message can wake the service now.
        let block = socket.reserve().await.unwrap();
        stop.try_send(()).ok().unwrap();
        block
    };
    let (_, block) = run_tasks(clock, next.run(clock, &mut events), drive);
    let mut ended = next.retire();
    drop(second);
    ended.collect();
    assert_eq!(ended.retired.pool.outstanding(), 1, "the block in hand");
    ended.retired.pool.give(block);
}

#[test]
fn a_retirement_brings_home_what_has_arrived() {
    brand!(Net);
    let (mut service, [mut other, _limited], stop, returner) = returning::<Net>(1);
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let drive = async {
        let block = other.reserve().await.unwrap();
        stop.try_send(()).ok().unwrap();
        block
    };
    let (_, block) = run_tasks(clock, service.run(clock, &mut events), drive);
    returner.give(block).ok().unwrap();
    let ended = service.retire();
    assert_eq!(ended.retired.pool.outstanding(), 0);
}

#[test]
fn adopted_sockets_not_yet_dropped_go_on_to_the_next_service() {
    brand!(Net);
    let (service, sockets, _, _stop) = pair::<Net>(usize::MAX, MemoryPolicy::KEEP);
    let mut ended = service.retire();
    let [one, two] = ended.sockets.each_mut().map(|late| late.take().unwrap());
    let retired = ended.retired;
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let owner = NetworkOwner::new(retired.brand, interface, retired.pool, retired.port);
    let (_, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut next: TestPair<Net, 2> = Service::new(owner, MemoryPolicy::KEEP, stopping);
    next.adopt(one).ok().unwrap();
    next.adopt(two).ok().unwrap();
    let (service, _late_sockets, _, _stop) = (next, sockets, (), ());
    let mut ended = service.retire();
    let adopted = ended.adopted.each_mut().map(Option::take);
    assert!(
        adopted.iter().all(Option::is_some),
        "they stay for the next"
    );
}

#[test]
fn a_second_return_lane_is_refused_and_the_first_stays() {
    brand!(Net);
    let (mut service, [mut other, _limited], stop, returner) = returning::<Net>(1);
    let (_, spare) = Box::leak(Box::new(ReturnLane::new())).split();
    assert!(service.attach_returns(spare).is_err());
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let drive = async {
        let block = other.reserve().await.unwrap();
        returner.give(block).ok().unwrap();
        settle().await;
        stop.try_send(()).ok().unwrap();
    };
    run_tasks(clock, service.run(clock, &mut events), drive);
    assert_eq!(service.retire().retired.pool.outstanding(), 0);
}

#[test]
fn a_block_arriving_after_the_retirement_is_collected() {
    brand!(Net);
    let (mut service, [mut other, _limited], stop, returner) = returning::<Net>(1);
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let drive = async {
        let block = other.reserve().await.unwrap();
        stop.try_send(()).ok().unwrap();
        block
    };
    let (_, block) = run_tasks(clock, service.run(clock, &mut events), drive);
    let mut ended = service.retire();
    assert_eq!(ended.retired.pool.outstanding(), 1);
    returner.give(block).ok().unwrap();
    ended.collect();
    assert_eq!(ended.retired.pool.outstanding(), 0);
}

#[test]
fn an_adopted_socket_goes_on_through_a_second_service() {
    brand!(Net);
    let (mut service, [mut first, _second], _, stop) = pair::<Net>(usize::MAX, MemoryPolicy::KEEP);
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let cx = &mut Context::from_waker(Waker::noop());
    let drive = async {
        assert!(pin!(first.reserve()).poll(cx).is_pending());
        settle().await;
        stop.try_send(()).ok().unwrap();
    };
    run_tasks(clock, service.run(clock, &mut events), drive);
    let mut ended = service.retire();
    let late = ended.sockets[0].take().unwrap();
    let retired = ended.retired;
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let owner = NetworkOwner::new(retired.brand, interface, retired.pool, retired.port);
    let (_, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut middle: TestPair<Net, 2> = Service::new(owner, MemoryPolicy::KEEP, stopping);
    middle.adopt(late).ok().unwrap();
    let mut ended = middle.retire();
    assert_eq!(ended.retired.pool.outstanding(), 1, "not dropped yet");
    let late = ended.adopted[0].take().unwrap();
    let retired = ended.retired;
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let owner = NetworkOwner::new(retired.brand, interface, retired.pool, retired.port);
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut last: TestPair<Net, 2> = Service::new(owner, MemoryPolicy::KEEP, stopping);
    last.adopt(late).ok().unwrap();
    let mut socket = last
        .bind(5002, Dscp::default(), Group::shared(), storage::<Net>())
        .ok()
        .unwrap();
    let drive = async {
        let held = socket.reserve().await.unwrap();
        assert!(pin!(socket.reserve()).poll(cx).is_pending());
        settle().await;
        drop(first);
        // Only the dropped socket can free the second block, and the service
        // is running.
        let returned = socket.reserve().await.unwrap();
        stop.try_send(()).ok().unwrap();
        (held, returned)
    };
    let (_, (one, two)) = run_tasks(clock, last.run(clock, &mut events), drive);
    let mut ended = last.retire();
    assert_eq!(ended.retired.pool.outstanding(), 2);
    ended.retired.pool.give(one);
    ended.retired.pool.give(two);
    assert_eq!(ended.retired.pool.outstanding(), 0);
}

#[test]
fn a_dropped_service_closes_the_lane_and_what_it_accepted_stays_within_reach() {
    brand!(Net);
    let (mut service, [mut other, _limited], stop, returner) = returning::<Net>(1);
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let drive = async {
        let blocks = [
            other.reserve().await.unwrap(),
            other.reserve().await.unwrap(),
        ];
        stop.try_send(()).ok().unwrap();
        blocks
    };
    let (_, [accepted, refused]) = run_tasks(clock, service.run(clock, &mut events), drive);
    assert!(returner.give(accepted).is_ok());
    drop(service);
    let not_home = returner.give(refused).err().unwrap();
    assert_eq!(not_home.reason, ReturnRefusal::Closed);
    assert!(
        returner.reclaim().is_some(),
        "what it accepted is reachable"
    );
}

#[test]
fn a_cancelled_run_closes_no_lane() {
    brand!(Net);
    let (mut service, [mut other, _limited], _, returner) = returning::<Net>(1);
    let clock: &VirtualClock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let block = {
        let mut run = pin!(service.run(clock, &mut events));
        let mut reserve = pin!(other.reserve());
        let cx = &mut Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(block) = reserve.as_mut().poll(cx) {
                break block.unwrap();
            }
            assert!(run.as_mut().poll(cx).is_pending());
        }
    };
    assert!(
        returner.give(block).is_ok(),
        "the run is gone, its service is not"
    );
    assert_eq!(service.retire().retired.pool.outstanding(), 0);
}

#[test]
fn a_close_at_the_end_of_all_keeps_what_was_accepted_and_refuses_the_rest() {
    brand!(Net);
    let (mut service, [mut other, _limited], stop, returner) = returning::<Net>(1);
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let drive = async {
        let blocks = [
            other.reserve().await.unwrap(),
            other.reserve().await.unwrap(),
        ];
        stop.try_send(()).ok().unwrap();
        blocks
    };
    let (_, [accepted, refused]) = run_tasks(clock, service.run(clock, &mut events), drive);
    let mut ended = service.retire();
    assert!(returner.give(accepted).is_ok());
    ended.close_returns();
    assert_eq!(ended.retired.pool.outstanding(), 1);
    let not_home = returner.give(refused).err().unwrap();
    assert_eq!(not_home.reason, ReturnRefusal::Closed);
    ended.retired.pool.give(not_home.block);
    assert_eq!(ended.retired.pool.outstanding(), 0);
}

#[test]
fn a_receive_loan_stays_owed_across_a_retired_service() {
    brand!(Net);
    let memory = Box::leak(Box::new([[0; SIZE]; BLOCKS]));
    let (mut pool, brand) = claim::<Net, SIZE, BLOCKS>(memory).unwrap();
    pool.loans().pool(2, 0).unwrap();
    pool.loans().grant(0, 1).unwrap();
    let held = pool.loans().lend(0).unwrap();
    let wire = WirePort::default();
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let owner = NetworkOwner::new(brand, interface, pool, wire.clone());
    let (mut ended, _, ()) = run(bench_on(owner, wire), async |_, _| {});
    let loans = ended.retired.pool.loans();
    assert_eq!(loans.outstanding(), 1);
    assert!(loans.grant(0, 2).is_err());
    loans.give_back(held);
    assert!(loans.released());
    loans.grant(0, 2).unwrap();
}

#[test]
fn a_frame_the_port_rejects_for_good_ends_and_the_service_goes_on() {
    brand!(Net);
    let (ended, _, sent) = run(bench::<Net>(), async |socket, wire| {
        send_one(socket, b"resolved").await;
        wire.0.borrow_mut().rejects = true;
        let rejected = send_one(socket, b"never").await;
        wire.0.borrow_mut().rejects = false;
        assert_eq!(
            send_one(socket, b"after").await,
            Sent::Ended(Outcome::Accepted)
        );
        rejected
    });
    assert_eq!(sent, Sent::Ended(Outcome::Rejected));
    assert_eq!(ended.retired.pool.outstanding(), 0);
}

#[test]
fn a_final_refusal_passes_the_turn_to_the_next_socket() {
    brand!(Net);
    let memory = Box::leak(Box::new([[0; SIZE]; 4]));
    let (pool, brand) = claim::<Net, SIZE, 4>(memory).unwrap();
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let wire = WirePort::default();
    wire.0.borrow_mut().answers = true;
    let owner = NetworkOwner::new(brand, interface, pool, wire.clone());
    let (_stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut service: TestPair<Net, 4> = Service::new(owner, MemoryPolicy::KEEP, stopping);
    let dscp = Dscp::default();
    let mut first = service
        .bind(5000, dscp, Group::shared(), storage::<Net>())
        .ok()
        .unwrap();
    let mut second = service
        .bind(5001, dscp, Group::shared(), storage::<Net>())
        .ok()
        .unwrap();
    let clock: &VirtualClock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let cx = &mut Context::from_waker(Waker::noop());
    let ends = {
        let mut run = pin!(service.run(clock, &mut events));
        // The second resolves the peer, so the first has the next turn.
        let mut block = blocks_by_hand(&mut second, &mut run, 1).pop().unwrap();
        block.fill(b"resolved").unwrap();
        let mut resolved = second.send(peer(), block).ok().unwrap();
        while pin!(resolved.finished()).poll(cx).is_pending() {
            assert!(run.as_mut().poll(cx).is_pending());
        }
        assert_eq!(resolved.take().ok(), Some(Sent::Ended(Outcome::Accepted)));
        let firsts = blocks_by_hand(&mut first, &mut run, 2);
        let seconds = blocks_by_hand(&mut second, &mut run, 1);
        let mut observers = Vec::new();
        for (block, data) in firsts.into_iter().zip([b"first 0", b"first 1"]) {
            let mut block = block;
            block.fill(data).unwrap();
            observers.push(first.send(peer(), block).ok().unwrap());
        }
        for mut block in seconds {
            block.fill(b"second0").unwrap();
            observers.push(second.send(peer(), block).ok().unwrap());
        }
        // All three are ready in one turn, and the first frame is refused
        // for good.
        wire.0.borrow_mut().rejections = 1;
        let mut ends = Vec::new();
        for mut observer in observers {
            while pin!(observer.finished()).poll(cx).is_pending() {
                assert!(run.as_mut().poll(cx).is_pending());
            }
            ends.push(observer.take().ok().unwrap());
        }
        ends
    };
    assert_eq!(
        ends,
        [
            Sent::Ended(Outcome::Rejected),
            Sent::Ended(Outcome::Accepted),
            Sent::Ended(Outcome::Accepted),
        ]
    );
    assert_eq!(
        datagrams(&wire.0.borrow()),
        [
            b"resolved".to_vec(),
            b"second0".to_vec(),
            b"first 1".to_vec()
        ],
        "after the first socket's final refusal, the second socket's turn"
    );
    let ended = service.retire();
    assert_eq!(ended.retired.pool.outstanding(), 0);
}

#[test]
fn a_block_asked_without_waiting_comes_on_a_later_call() {
    brand!(Net);
    let (mut ended, mut socket, ()) = run(bench::<Net>(), async |socket, _| {
        assert!(socket.try_reserve().unwrap().is_none(), "asked, not there");
        assert!(socket.try_reserve().unwrap().is_none(), "asked once");
        let block = loop {
            if let Some(block) = socket.try_reserve().unwrap() {
                break block;
            }
            yield_now().await;
        };
        socket.release(block).ok().unwrap();
    });
    returned(&mut ended, &mut socket);
}

#[test]
fn a_socket_asked_to_hold_blocks_ahead_has_them_without_a_turn() {
    brand!(Net);
    let (mut ended, mut socket, ()) = run(bench::<Net>(), async |socket, _| {
        socket.reserve_ahead(3);
        let first = socket.reserve().await.unwrap();
        settle().await;
        assert!(socket.try_reserve().unwrap().is_some_and(|block| {
            socket.release(block).ok().unwrap();
            true
        }));
        let second = socket.try_reserve().unwrap().unwrap();
        socket.release(first).ok().unwrap();
        socket.release(second).ok().unwrap();
        settle().await;
    });
    returned(&mut ended, &mut socket);
}

#[test]
fn blocks_held_ahead_come_home_when_the_service_stops() {
    brand!(Net);
    let (mut ended, mut socket, ()) = run(bench::<Net>(), async |socket, _| {
        socket.reserve_ahead(RESERVE_AHEAD);
        let block = socket.reserve().await.unwrap();
        settle().await;
        socket.release(block).ok().unwrap();
    });
    returned(&mut ended, &mut socket);
}

#[test]
fn a_dropped_socket_gives_back_the_blocks_it_held_ahead() {
    brand!(Net);
    let bench = bench::<Net>();
    let (mut ended, socket, ()) = run(bench, async |socket, _| {
        socket.reserve_ahead(4);
        let block = socket.reserve().await.unwrap();
        settle().await;
        socket.release(block).ok().unwrap();
    });
    drop(socket);
    let late = ended.sockets.first_mut().unwrap().as_mut().unwrap();
    assert!(late.collect(&mut ended.retired.pool));
    assert_eq!(ended.retired.pool.outstanding(), 0);
}

#[test]
fn a_socket_cannot_ask_ahead_past_the_lane_of_answers() {
    brand!(Net);
    let (mut ended, mut socket, ()) = run(bench::<Net>(), async |socket, _| {
        socket.reserve_ahead(usize::MAX);
        let block = socket.reserve().await.unwrap();
        settle().await;
        let mut held = 0;
        while let Some(block) = socket.try_reserve().unwrap() {
            socket.release(block).ok().unwrap();
            held += 1;
            settle().await;
            if held == RESERVE_AHEAD * 4 {
                break;
            }
        }
        socket.release(block).ok().unwrap();
        settle().await;
    });
    returned(&mut ended, &mut socket);
}

#[test]
fn a_reservation_asked_without_waiting_ends_with_its_service() {
    brand!(Net);
    let bench = bench::<Net>();
    let (ended, mut socket, ()) = run(bench, async |_, _| {});
    assert_eq!(
        socket.try_reserve().map(|block| block.is_some()),
        Err(SocketClosed)
    );
    assert!(socket.try_receive().is_none());
    assert_eq!(ended.retired.pool.outstanding(), 0);
}

#[test]
fn a_datagram_is_taken_without_waiting_once_it_is_here() {
    brand!(Net);
    let (mut ended, mut socket, octets) = run(bench::<Net>(), async |socket, wire| {
        assert!(socket.try_receive().is_none(), "nothing here yet");
        {
            let mut wire = wire.0.borrow_mut();
            wire.incoming.push_back(udp_frame(5000, b"here"));
            wire.wake();
        }
        let datagram = loop {
            if let Some(datagram) = socket.try_receive() {
                break datagram;
            }
            yield_now().await;
        };
        let octets = datagram.block.octets().to_vec();
        socket.release(datagram.block).ok().unwrap();
        octets
    });
    assert_eq!(octets, b"here");
    returned(&mut ended, &mut socket);
}

#[test]
fn a_neighbor_request_the_port_rejects_ends_the_datagram_at_once() {
    brand!(Net);
    let (ended, _, sent) = run(bench::<Net>(), async |socket, wire| {
        wire.0.borrow_mut().rejects = true;
        send_one(socket, b"never").await
    });
    assert_eq!(sent, Sent::Ended(Outcome::Rejected));
    assert_eq!(ended.retired.pool.outstanding(), 0);
}

#[test]
fn a_new_service_starts_on_what_a_stopped_one_handed_on() {
    brand!(Net);
    let (ended, _, _) = run(bench::<Net>(), async |socket, _| {
        send_one(socket, b"first").await
    });
    let retired = ended.retired;
    let wire = retired.port.clone();
    let owner = NetworkOwner::new(
        retired.brand,
        Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap(),
        retired.pool,
        retired.port,
    );
    let (_, _, sent) = run(bench_on(owner, wire.clone()), async |socket, _| {
        send_one(socket, b"second").await
    });
    assert_eq!(sent, Sent::Ended(Outcome::Accepted));
    assert_eq!(
        datagrams(&wire.0.borrow()),
        [b"first".to_vec(), b"second".to_vec()]
    );
}

/// A frame the port refuses for room stays ready, and goes once the port
/// has room: nothing is lost and nothing goes twice.
#[test]
fn a_refused_frame_stays_ready_and_goes_when_the_port_has_room() {
    brand!(Net);
    let (mut ended, mut socket, ()) = run(bench::<Net>(), async |socket, wire| {
        open_port(wire, true);
        let observer = send(socket, b"late").await;
        settle().await;
        assert!(wire.0.borrow().refusals >= 1);
        assert!(wire.0.borrow().sent.is_empty());
        open_port(wire, false);
        assert_eq!(result_of(observer).await, Sent::Ended(Outcome::Accepted));
        assert_eq!(datagrams(&wire.0.borrow()), [b"late".to_vec()]);
    });
    returned(&mut ended, &mut socket);
}

/// A notice the port has is taken in the next turn, with no work ready.
#[test]
fn a_notice_alone_is_taken() {
    brand!(Net);
    let _ended = run(bench::<Net>(), async |socket, wire| {
        send_one(socket, b"sent").await;
        settle().await;
        {
            let mut wire = wire.0.borrow_mut();
            wire.ended.push_back(1);
            wire.wake();
        }
        settle().await;
        assert!(wire.0.borrow().ended.is_empty(), "the notice was taken");
    });
}

#[test]
fn a_frame_of_unknown_end_is_not_a_refusal() {
    brand!(Net);
    let (ended, _, ()) = run(bench::<Net>(), async |socket, wire| {
        send_one(socket, b"resolved").await;
        wire.0.borrow_mut().contradicts = true;
        let observer = send(socket, b"unknown").await;
        assert_eq!(result_of(observer).await, Sent::Ended(Outcome::Unknown));
        settle().await;
    });
    assert_eq!(ended.cause, EndCause::PortContract);
}

/// What became of a datagram whose fill failed, as the port then answered.
struct FailedFill {
    sent: Sent,
    /// The next datagram's end, while the service goes on.
    next: Option<Sent>,
    cause: EndCause,
}

/// A datagram whose fill fails, as the port then answers `after`. A port
/// that keeps its contract is asked for one more datagram; after a fault,
/// the client waits for the service's end.
fn a_failed_fill<B: 'static>(after: AfterFailedFill) -> FailedFill {
    let (mut ended, mut socket, (sent, next)) = run(bench::<B>(), async |socket, wire| {
        send_one(socket, b"resolved").await;
        {
            let mut wire = wire.0.borrow_mut();
            wire.room = Some(10);
            wire.after_failed_fill = after;
        }
        let sent = send_one(socket, b"too long for its room").await;
        wire.0.borrow_mut().room = None;
        let next = match after {
            AfterFailedFill::Fill => Some(send_one(socket, b"after").await),
            AfterFailedFill::Refusal | AfterFailedFill::Yes => {
                while !socket.closed() {
                    yield_now().await;
                }
                None
            }
        };
        (sent, next)
    });
    returned(&mut ended, &mut socket);
    FailedFill {
        sent,
        next,
        cause: ended.cause,
    }
}

#[test]
fn a_failed_fill_ends_its_datagram_rejected_and_the_service_goes_on() {
    brand!(Net);
    let end = a_failed_fill::<Net>(AfterFailedFill::Fill);
    assert_eq!(end.sent, Sent::Ended(Outcome::Rejected));
    assert_eq!(end.next, Some(Sent::Ended(Outcome::Accepted)));
    assert_eq!(end.cause, EndCause::Stopped, "the port kept its contract");
}

#[test]
fn a_port_refusal_after_a_failed_fill_ends_the_service_with_the_port_s_fault() {
    brand!(Net);
    let end = a_failed_fill::<Net>(AfterFailedFill::Refusal);
    assert_eq!(end.sent, Sent::Ended(Outcome::Rejected), "the fill's cause");
    assert_eq!(end.next, None);
    assert_eq!(end.cause, EndCause::PortContract);
}

#[test]
fn a_yes_after_a_failed_fill_is_unknown_and_the_port_s_fault() {
    brand!(Net);
    let end = a_failed_fill::<Net>(AfterFailedFill::Yes);
    assert_eq!(end.sent, Sent::Ended(Outcome::Unknown));
    assert_eq!(end.next, None);
    assert_eq!(end.cause, EndCause::PortContract);
}

/// A port that loses the buffer a datagram was written in is at fault: the
/// datagram did not go and ends rejected, not tried again, and the service
/// ends with the port's fault, every block home.
#[test]
fn a_port_that_loses_a_written_buffer_ends_the_service_with_its_fault() {
    brand!(Net);
    let (mut ended, mut socket, (sent, sent_before)) = run(bench::<Net>(), async |socket, wire| {
        send_one(socket, b"resolved").await;
        let before = wire.0.borrow().sent.len();
        wire.0.borrow_mut().loses = true;
        let sent = send_one(socket, b"lost").await;
        while !socket.closed() {
            yield_now().await;
        }
        (sent, before)
    });
    assert_eq!(sent, Sent::Ended(Outcome::Rejected));
    assert_eq!(ended.cause, EndCause::PortFault);
    assert_eq!(
        ended.retired.port.0.borrow().sent.len(),
        sent_before,
        "nothing more went"
    );
    returned(&mut ended, &mut socket);
}

/// A port whose receive queue breaks ends the service with that fault,
/// named: no frame is read after it, and every block goes home.
#[test]
fn a_broken_receive_queue_ends_the_service_with_its_fault() {
    use oer_protocols_network_port::ReceiveFault;
    brand!(Net);
    let (mut ended, mut socket, ()) = run(bench::<Net>(), async |socket, wire| {
        send_one(socket, b"resolved").await;
        {
            let mut wire = wire.0.borrow_mut();
            wire.receive_fault = Some(ReceiveFault::Lost);
            // A frame waits, and its wake brings the turn whose intake
            // finds the break.
            wire.incoming.push_back(std::vec![0; 60]);
            wire.wake();
        }
        while !socket.closed() {
            yield_now().await;
        }
    });
    assert_eq!(ended.cause, EndCause::ReceiveFault(ReceiveFault::Lost));
    returned(&mut ended, &mut socket);
}

/// A run cancelled after its datagram went keeps what the port took: the
/// ARP request and the datagram, each once.
#[test]
fn a_cancelled_run_keeps_what_the_port_took() {
    brand!(Net);
    let Bench {
        mut service,
        mut socket,
        wire,
        clock,
        ..
    } = bench::<Net>();
    let mut events = ();
    let cx = &mut Context::from_waker(Waker::noop());
    {
        let mut run = pin!(service.run(clock, &mut events));
        let mut blocks = blocks_by_hand(&mut socket, &mut run, 1);
        let mut observer = socket.send(peer(), blocks.pop().unwrap()).ok().unwrap();
        while pin!(observer.finished()).poll(cx).is_pending() {
            assert!(run.as_mut().poll(cx).is_pending());
        }
        assert_eq!(observer.take().ok(), Some(Sent::Ended(Outcome::Accepted)));
    }
    let _ended = service.retire();
    assert_eq!(
        wire.0.borrow().sent.len(),
        2,
        "the ARP request and the datagram"
    );
}

mod arbiter;
mod lending;
mod local;
mod paced;
mod probe;
mod publish;
mod reserve;
mod rx_load;
mod turns;

#[test]
fn a_lease_ends_in_the_service_s_own_wait_and_the_watch_tells() {
    brand!(Net);
    let memory = Box::leak(Box::new([[0; SIZE]; BLOCKS]));
    let (pool, brand) = claim::<Net, SIZE, BLOCKS>(memory).unwrap();
    let wire = WirePort::default();
    wire.0.borrow_mut().answers = true;
    let interface = Interface::dhcp(LOCAL, 1500, 3).unwrap();
    let owner = NetworkOwner::new(brand, interface, pool, wire.clone());
    let mut bench = bench_on(owner, wire);
    let watch: &'static ConfigurationWatch = Box::leak(Box::new(ConfigurationWatch::new()));
    bench.service.watch(watch).unwrap();
    let clock = bench.clock;
    let (_, _, (obtained, ended_at)) = run(bench, async |socket, wire| {
        // The server: an OFFER to the DISCOVER, an ACK of ten seconds to the
        // REQUEST, and silence after.
        let answer = async |kind: MessageType, to: MessageType| {
            let xid = loop {
                let sent = dhcp_messages(&wire.0.borrow().sent);
                if let Some((_, xid)) = sent.iter().find(|(sent, _)| *sent == to) {
                    break *xid;
                }
                drop(sent);
                yield_now().await;
            };
            let mut line = wire.0.borrow_mut();
            line.incoming.push_back(dhcp_reply(kind, xid, 10));
            line.wake();
        };
        answer(MessageType::Offer, MessageType::Discover).await;
        answer(MessageType::Ack, MessageType::Request).await;
        let mut seen = watch.changes();
        let obtained = loop {
            seen = watch.changed(seen).await;
            if let Some(lease) = watch.read().unwrap().configuration.unwrap().lease {
                break lease.obtained;
            }
        };
        // A send now goes from the leased address.
        let sent = send_one(socket, b"leased").await;
        assert_eq!(sent, Sent::Ended(Outcome::Accepted));
        loop {
            seen = watch.changed(seen).await;
            if watch.read().unwrap().configuration.unwrap().ipv4.is_none() {
                break (obtained, oer_portable_time::Clock::now(clock));
            }
        }
    });
    // Renewals and rebinds went unanswered: the address went at its end, in
    // a wait the lease's own times woke.
    assert_eq!(
        ended_at,
        obtained.saturating_add(oer_portable_time::Duration::from_secs(10))
    );
}
