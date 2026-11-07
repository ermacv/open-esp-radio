extern crate std;

use std::boxed::Box;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Wake;
use std::vec::Vec;

use core::pin::pin;
use core::task::{Context, Poll, Waker};

use embassy_futures::yield_now;
use oer_portable_memory::WakingSpscSender;
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_network_core::{
    Block, FramePort, FrameSource, Interface, NetworkOwner, OpenRefused, Outcome, Remote,
    SendRefusal, SlotTooShort, claim,
};
use oer_protocols_network_wire::arp::{self, Arp, Operation};
use oer_protocols_network_wire::checksum::{Checksum, ipv4_pseudo_header, udp_field};
use oer_protocols_network_wire::ethernet::{EtherType, EthernetHeader};
use oer_protocols_network_wire::ip::{DsField, Dscp, Ipv4Fields, Ipv4Header};
use oer_protocols_network_wire::transport::{PROTOCOL_UDP, Ports, UdpHeader};

use crate::{
    ForeignObserver, Refusal, Sent, Service, Socket, SocketClosed, SocketLanes, StopLane, lanes,
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
    incoming: VecDeque<Vec<u8>>,
    sent: Vec<Vec<u8>>,
    full: bool,
    /// The peer answers ARP.
    answers: bool,
    waker: Option<Waker>,
}

impl Wire {
    fn wake(&mut self) {
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }
}

#[derive(Clone, Default)]
struct WirePort(Rc<RefCell<Wire>>);

impl FramePort for WirePort {
    type Refusal = ();

    fn publish(
        &mut self,
        length: usize,
        fill: impl FnOnce(&mut [u8]) -> Result<(), SlotTooShort>,
    ) -> Result<(), ()> {
        let mut wire = self.0.borrow_mut();
        if wire.full {
            return Err(());
        }
        let mut frame = std::vec![0; length];
        fill(&mut frame).map_err(|_| ())?;
        let reply = wire.answers.then(|| answer_of(&frame)).flatten();
        wire.sent.push(frame);
        wire.incoming.extend(reply);
        Ok(())
    }
}

impl FrameSource for WirePort {
    fn take_frame(&mut self, read: impl FnOnce(&[u8])) -> bool {
        let frame = self.0.borrow_mut().incoming.pop_front();
        match frame {
            Some(frame) => {
                read(&frame);
                true
            }
            None => false,
        }
    }

    fn register(&mut self, cx: &mut Context<'_>) {
        let mut wire = self.0.borrow_mut();
        wire.waker = Some(cx.waker().clone());
        if !wire.incoming.is_empty() {
            cx.waker().wake_by_ref();
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

type TestSocket<B> = Socket<'static, 'static, B, SIZE, DEPTH, RECEIVED, CREDITS>;
type TestService<B> =
    Service<'static, 'static, B, SIZE, 1, CREDITS, RECEIVED, 2, BLOCKS, DEPTH, RECEIVED, WirePort>;
type TestRetired<B> =
    oer_protocols_network_core::Retired<'static, B, SIZE, 1, CREDITS, BLOCKS, WirePort>;

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
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let wire = WirePort::default();
    wire.0.borrow_mut().answers = true;
    let owner = NetworkOwner::new(brand, interface, pool, wire.clone());
    let storage = Box::leak(Box::new(SocketLanes::new()));
    let (socket, end) = lanes::<_, SIZE, DEPTH, RECEIVED, CREDITS>(storage);
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let service = Service::new(owner, [(5000, Dscp::default(), end)], stopping)
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

/// Run two tasks as an executor would: a task is polled only once woken, so
/// a lost wake stops the test. With nothing woken, time moves on.
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

/// Run `client` beside the service until it ends, then stop the service.
fn run<B: 'static, R>(
    bench: Bench<B>,
    client: impl AsyncFnOnce(&mut TestSocket<B>, &WirePort) -> R,
) -> (TestRetired<B>, TestSocket<B>, R) {
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
    let (retired, result) = run_tasks(clock, service.run(clock, &mut events), drive);
    (retired, socket, result)
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

async fn send_one<B>(socket: &mut TestSocket<B>, data: &[u8]) -> Sent {
    let block = fill(socket, data).await;
    socket.credit().await.unwrap();
    let mut observer = socket.send(peer(), block).ok().unwrap();
    socket.result(&mut observer).await.unwrap()
}

/// Every block the service did not see again comes back from the socket.
fn returned<B>(retired: &mut TestRetired<B>, socket: &mut TestSocket<B>) {
    while let Some(block) = socket.reclaim() {
        retired.pool.give(block);
    }
    assert_eq!(retired.pool.outstanding(), 0);
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
    let (mut retired, mut socket, ()) = run(bench::<Net>(), async |socket, wire| {
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
    returned(&mut retired, &mut socket);
}

#[test]
fn a_port_with_no_room_keeps_the_datagram_until_it_wakes_the_service() {
    brand!(Net);
    let (_, _, sent) = run(bench::<Net>(), async |socket, wire| {
        open_port(wire, true);
        let block = fill(socket, b"late").await;
        socket.credit().await.unwrap();
        let mut observer = socket.send(peer(), block).ok().unwrap();
        settle().await;
        assert!(wire.0.borrow().sent.is_empty());
        open_port(wire, false);
        socket.result(&mut observer).await.unwrap()
    });
    assert_eq!(sent, Sent::Ended(Outcome::Accepted));
}

#[test]
fn a_wait_for_a_result_may_be_cancelled_and_begun_again() {
    brand!(Net);
    run(bench::<Net>(), async |socket, wire| {
        open_port(wire, true);
        let block = socket.reserve().await.unwrap();
        socket.credit().await.unwrap();
        let mut observer = socket.send(peer(), block).ok().unwrap();
        {
            let mut waiting = pin!(socket.result(&mut observer));
            let mut cx = Context::from_waker(Waker::noop());
            assert!(waiting.as_mut().poll(&mut cx).is_pending());
        }
        open_port(wire, false);
        assert_eq!(
            socket.result(&mut observer).await,
            Ok(Sent::Ended(Outcome::Accepted))
        );
    });
}

#[test]
fn a_dropped_observer_frees_its_credit_once_the_command_ended() {
    brand!(Net);
    run(bench::<Net>(), async |socket, wire| {
        for round in 0..6_u8 {
            let block = fill(socket, &[round]).await;
            socket.credit().await.unwrap();
            drop(socket.send(peer(), block).ok().unwrap());
        }
        let sent = send_one(socket, b"after").await;
        assert_eq!(sent, Sent::Ended(Outcome::Accepted));
        assert_eq!(datagrams(&wire.0.borrow()).len(), 7);
    });
}

#[test]
fn a_dropped_observer_of_a_running_command_keeps_its_credit() {
    brand!(Net);
    run(bench::<Net>(), async |socket, wire| {
        open_port(wire, true);
        for _ in 0..CREDITS {
            let block = socket.reserve().await.unwrap();
            drop(socket.send(peer(), block).ok().unwrap());
        }
        let block = socket.reserve().await.unwrap();
        let refused = socket.send(peer(), block).err().unwrap();
        assert_eq!(refused.reason, Refusal::Busy);
        open_port(wire, false);
        socket.credit().await.unwrap();
        drop(socket.send(peer(), refused.block).ok().unwrap());
    });
}

#[test]
fn a_reservation_waits_for_a_block_to_return() {
    brand!(Net);
    run(bench::<Net>(), async |socket, _| {
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
    let (retired, _, sent) = run(bench::<Net>(), async |socket, _| {
        let block = socket.reserve().await.unwrap();
        let far = Remote {
            address: [198, 51, 100, 1],
            port: 7,
        };
        let mut observer = socket.send(far, block).ok().unwrap();
        socket.result(&mut observer).await.unwrap()
    });
    assert_eq!(sent, Sent::Refused(SendRefusal::NoRoute));
    assert_eq!(retired.pool.outstanding(), 0);
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
fn an_observer_of_another_socket_reads_nothing() {
    brand!(Net);
    brand!(Other);
    let mut other = bench::<Other>();
    run(bench::<Net>(), async |socket, _| {
        let block = socket.reserve().await.unwrap();
        let mut observer = socket.send(peer(), block).ok().unwrap();
        let foreign = other.socket.result(&mut observer).await;
        assert_eq!(foreign, Err(ForeignObserver));
        assert_eq!(
            socket.result(&mut observer).await,
            Ok(Sent::Ended(Outcome::Accepted))
        );
    });
}

#[test]
fn a_close_ends_the_socket_and_later_commands_are_refused() {
    brand!(Net);
    let (mut retired, mut socket, ()) = run(bench::<Net>(), async |socket, _| {
        let block = socket.reserve().await.unwrap();
        socket.close().await;
        assert!(socket.closed());
        assert_eq!(socket.credit().await, Err(SocketClosed));
        let refused = socket.send(peer(), block).err().unwrap();
        assert_eq!(refused.reason, Refusal::Closed);
        assert_eq!(socket.receive().await.err(), Some(SocketClosed));
        socket.release(refused.block).ok().unwrap();
    });
    returned(&mut retired, &mut socket);
}

#[test]
fn a_stop_publishes_nothing_more_and_each_result_arrives_after_it() {
    brand!(Net);
    let (mut retired, mut socket, (mut known, mut running, held)) =
        run(bench::<Net>(), async |socket, wire| {
            let block = fill(socket, b"known").await;
            let known = socket.send(peer(), block).ok().unwrap();
            while datagrams(&wire.0.borrow()).is_empty() {
                yield_now().await;
            }
            open_port(wire, true);
            let block = fill(socket, b"unsent").await;
            socket.credit().await.unwrap();
            let running = socket.send(peer(), block).ok().unwrap();
            let held = socket.reserve().await.unwrap();
            while socket.ended_count() == 0 {
                yield_now().await;
            }
            assert_eq!(wire.0.borrow().sent.len(), 2, "ARP and the first");
            (known, running, held)
        });
    open_port(&retired.port.clone(), false);
    assert_eq!(
        datagrams(&retired.port.0.borrow()),
        [b"known".to_vec()],
        "a port with room again gets nothing"
    );
    let (known, running) = embassy_futures::block_on(async {
        (
            socket.result(&mut known).await,
            socket.result(&mut running).await,
        )
    });
    assert_eq!(known, Ok(Sent::Ended(Outcome::Accepted)));
    assert_eq!(running, Ok(Sent::Ended(Outcome::Retired)));
    assert_eq!(retired.pool.outstanding(), 1, "the application's block");
    retired.pool.give(held);
    assert_eq!(retired.pool.outstanding(), 0);
    assert!(socket.closed());
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
    let (mut retired, mut observer) = run_tasks(clock, service.run(clock, &mut events), drive);
    assert_eq!(
        embassy_futures::block_on(socket.result(&mut observer)),
        Ok(Sent::Ended(Outcome::Retired))
    );
    assert_eq!(retired.pool.outstanding(), 1);
    returned(&mut retired, &mut socket);
    assert!(datagrams(&wire.0.borrow()).is_empty());
}

#[test]
fn a_late_application_gets_its_blocks_back_after_a_stop() {
    brand!(Net);
    let (mut retired, mut socket, ()) = run(bench::<Net>(), async |socket, wire| {
        {
            let mut wire = wire.0.borrow_mut();
            wire.incoming.push_back(udp_frame(5000, b"unread"));
            wire.wake();
        }
        while socket.received() == 0 {
            yield_now().await;
        }
    });
    assert_eq!(retired.pool.outstanding(), 1);
    returned(&mut retired, &mut socket);
}

#[test]
fn a_full_datagram_lane_wakes_the_service_when_it_is_read() {
    brand!(Net);
    let arrive = |wire: &WirePort| {
        let mut wire = wire.0.borrow_mut();
        wire.incoming
            .extend((0..RECEIVED).map(|_| udp_frame(5000, b"many")));
        wire.wake();
    };
    run(bench::<Net>(), async |socket, wire| {
        arrive(wire);
        while socket.received() < RECEIVED {
            yield_now().await;
        }
        // These wait in the core, as the lane is full.
        arrive(wire);
        settle().await;
        // Nothing is released: only the room read here can wake the service.
        let mut held = Vec::new();
        for _ in 0..2 * RECEIVED {
            held.push(socket.receive().await.unwrap());
        }
    });
}

#[test]
fn a_stop_wakes_a_service_with_nothing_to_do() {
    brand!(Net);
    run(bench::<Net>(), async |_, _| settle().await);
}

#[test]
fn an_observer_dropped_after_its_result_came_frees_its_credit() {
    brand!(Net);
    run(bench::<Net>(), async |socket, _| {
        for round in 0..2 * CREDITS {
            let block = fill(socket, &[0]).await;
            socket.credit().await.unwrap();
            let observer = socket.send(peer(), block).ok().unwrap();
            while socket.ended_count() == 0 {
                yield_now().await;
            }
            drop(observer);
            assert_eq!(socket.credit().await, Ok(()), "round {round}");
            assert_eq!(socket.ended_count(), 0);
        }
    });
}

#[test]
fn a_service_refused_a_socket_hands_its_owner_back() {
    brand!(Net);
    let memory = Box::leak(Box::new([[0; SIZE]; BLOCKS]));
    let (pool, brand) = claim::<Net, SIZE, BLOCKS>(memory).unwrap();
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let owner: NetworkOwner<'static, Net, SIZE, 2, CREDITS, RECEIVED, 2, BLOCKS, WirePort> =
        NetworkOwner::new(brand, interface, pool, WirePort::default());
    let (_first, first) =
        lanes::<_, SIZE, DEPTH, RECEIVED, CREDITS>(Box::leak(Box::new(SocketLanes::new())));
    let (_second, second) =
        lanes::<_, SIZE, DEPTH, RECEIVED, CREDITS>(Box::leak(Box::new(SocketLanes::new())));
    let (_, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let ends = [
        (5000, Dscp::default(), first),
        (5000, Dscp::default(), second),
    ];
    let refused = Service::new(owner, ends, stopping).err().unwrap();
    assert_eq!(refused.reason, OpenRefused::PortInUse);
    assert_eq!(refused.owner.retire().pool.free(), BLOCKS);
}

#[test]
fn a_new_service_starts_on_what_a_stopped_one_handed_on() {
    brand!(Net);
    let (retired, _, _) = run(bench::<Net>(), async |socket, _| {
        send_one(socket, b"first").await
    });
    let storage = Box::leak(Box::new(SocketLanes::new()));
    let (socket, end) = lanes::<_, SIZE, DEPTH, RECEIVED, CREDITS>(storage);
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let wire = retired.port.clone();
    let owner = NetworkOwner::new(
        retired.brand,
        Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap(),
        retired.pool,
        retired.port,
    );
    let service = Service::new(owner, [(5000, Dscp::default(), end)], stopping)
        .ok()
        .unwrap();
    let again = Bench {
        service,
        socket,
        wire: wire.clone(),
        stop,
        clock: Box::leak(Box::new(VirtualClock::new())),
    };
    let (_, _, sent) = run(again, async |socket, _| send_one(socket, b"second").await);
    assert_eq!(sent, Sent::Ended(Outcome::Accepted));
    assert_eq!(
        datagrams(&wire.0.borrow()),
        [b"first".to_vec(), b"second".to_vec()]
    );
}
