extern crate std;

use std::boxed::Box;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::Wake;
use std::vec::Vec;

use core::pin::pin;
use core::task::{Context, Poll, Waker};

use embassy_futures::yield_now;
use oer_portable_memory::WakingSpscSender;
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_network_core::{
    Block, Interface, NetworkOwner, OpenRefused, Outcome, Remote, SendRefusal, claim,
};
use oer_protocols_network_port::{Candidate, Egress, FramePort, FrameSource, SlotTooShort};
use oer_protocols_network_wire::arp::{self, Arp, Operation};
use oer_protocols_network_wire::checksum::{Checksum, ipv4_pseudo_header, udp_field};
use oer_protocols_network_wire::ethernet::{EtherType, EthernetHeader};
use oer_protocols_network_wire::ip::{DsField, Dscp, Ipv4Fields, Ipv4Header};
use oer_protocols_network_wire::transport::{PROTOCOL_UDP, Ports, UdpHeader};

use crate::{
    Direct, EndCause, Ended, Observer, Refusal, Sent, Service, Socket, SocketClosed, SocketLanes,
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
    incoming: VecDeque<Vec<u8>>,
    sent: Vec<Vec<u8>>,
    full: bool,
    /// The peer answers ARP.
    answers: bool,
    /// The port says it took a frame it never filled.
    contradicts: bool,
    /// Tickets given.
    tickets: u32,
    /// Ended frames, by ticket.
    ended: VecDeque<u32>,
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
    type Ticket = u32;

    fn publish(
        &mut self,
        length: usize,
        fill: impl FnOnce(&mut [u8]) -> Result<(), SlotTooShort>,
    ) -> Result<u32, ()> {
        let mut wire = self.0.borrow_mut();
        if wire.full {
            return Err(());
        }
        if wire.contradicts {
            return Ok(0);
        }
        let mut frame = std::vec![0; length];
        fill(&mut frame).map_err(|_| ())?;
        let reply = wire.answers.then(|| answer_of(&frame)).flatten();
        wire.sent.push(frame);
        wire.incoming.extend(reply);
        wire.tickets += 1;
        Ok(wire.tickets)
    }
}

impl FrameSource for WirePort {
    type Completion = u32;

    fn take_completion(&mut self) -> Option<u32> {
        self.0.borrow_mut().ended.pop_front()
    }

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
type TestService<B, E = Direct> = Service<
    'static,
    'static,
    B,
    SIZE,
    1,
    CREDITS,
    RECEIVED,
    2,
    BLOCKS,
    DEPTH,
    RECEIVED,
    WirePort,
    E,
>;
type TestEnded<B, E = Direct> = Ended<
    'static,
    'static,
    B,
    SIZE,
    1,
    CREDITS,
    BLOCKS,
    DEPTH,
    RECEIVED,
    WirePort,
    E,
    <E as Egress<u32, u32>>::Fault,
>;

/// A stack of brand `B`, its one socket, and the test's handles.
struct Bench<B: 'static, E: Egress<u32, u32> = Direct> {
    service: TestService<B, E>,
    socket: TestSocket<B>,
    wire: WirePort,
    stop: WakingSpscSender<'static, (), 1>,
    clock: &'static VirtualClock,
}

fn bench<B: 'static>() -> Bench<B> {
    bench_with(Direct)
}

fn bench_with<B: 'static, E: Egress<u32, u32>>(egress: E) -> Bench<B, E> {
    let memory = Box::leak(Box::new([[0; SIZE]; BLOCKS]));
    let (pool, brand) = claim::<B, SIZE, BLOCKS>(memory).unwrap();
    let wire = WirePort::default();
    wire.0.borrow_mut().answers = true;
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let owner = NetworkOwner::new(brand, interface, pool, wire.clone());
    bench_on(owner, wire, egress)
}

fn storage<B>() -> &'static mut SocketLanes<'static, 'static, B, SIZE, DEPTH, RECEIVED, CREDITS> {
    Box::leak(Box::new(SocketLanes::new()))
}

fn bench_on<B: 'static, E: Egress<u32, u32>>(
    owner: NetworkOwner<'static, B, SIZE, 1, CREDITS, RECEIVED, 2, BLOCKS, WirePort>,
    wire: WirePort,
    egress: E,
) -> Bench<B, E> {
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut service = Service::new(owner, egress, stopping);
    let socket = service
        .bind(5000, Dscp::default(), storage::<B>())
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
async fn serve<B, E: Egress<u32, u32>>(
    mut service: TestService<B, E>,
    clock: &VirtualClock,
    events: &mut (),
) -> TestEnded<B, E> {
    let _cause = service.run(clock, events).await;
    service.retire()
}

/// Run `client` beside the service until it ends, then stop the service.
fn run<B: 'static, E: Egress<u32, u32>, R>(
    bench: Bench<B, E>,
    client: impl AsyncFnOnce(&mut TestSocket<B>, &WirePort) -> R,
) -> (TestEnded<B, E>, TestSocket<B>, R) {
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
fn returned<B, E: Egress<u32, u32>>(ended: &mut TestEnded<B, E>, socket: &mut TestSocket<B>) {
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
    run(bench::<Net>(), async |socket, wire| {
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
    run(bench::<Net>(), async |socket, wire| {
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
    run(bench::<Net>(), async |socket, wire| {
        for round in 0..2 * CREDITS {
            let data = [u8::try_from(round).unwrap()];
            let observer = send(socket, &data).await;
            assert_eq!(result_of(observer).await, Sent::Ended(Outcome::Accepted));
        }
        assert_eq!(datagrams(&wire.0.borrow()).len(), 2 * CREDITS);
    });
}

#[test]
fn a_dropped_observer_frees_its_credit_once_the_command_ended() {
    brand!(Net);
    run(bench::<Net>(), async |socket, wire| {
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
    run(bench::<Net>(), async |socket, wire| {
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
    run(bench::<Net>(), async |socket, _| {
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
    run(bench::<Net>(), async |socket, _| {
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
    run(bench::<Net>(), async |_, _| settle().await);
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
    run(bench::<Net>(), async |socket, wire| {
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
    let mut service = Service::new(owner, Direct, stopping);
    let dscp = Dscp::default();
    let mut first = service.bind(5000, dscp, storage::<Net>()).ok().unwrap();
    let refused = service.bind(5000, dscp, storage::<Net>()).err().unwrap();
    assert_eq!(refused.reason, OpenRefused::PortInUse);
    let mut second = service.bind(5001, dscp, refused.storage).ok().unwrap();
    let full = service.bind(5002, dscp, storage::<Net>()).err().unwrap();
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
    let (_, _, sent) = run(bench_on(owner, wire, Direct), async |socket, _| {
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
    let (_, _, sent) = run(bench_on(owner, wire.clone(), Direct), async |socket, _| {
        send_one(socket, b"second").await
    });
    assert_eq!(sent, Sent::Ended(Outcome::Accepted));
    assert_eq!(
        datagrams(&wire.0.borrow()),
        [b"first".to_vec(), b"second".to_vec()]
    );
}

/// What a policy under test saw and did.
#[derive(Debug, Default)]
struct Account {
    /// Nothing may go.
    shut: bool,
    /// Choose the last ready work, not the first.
    last: bool,
    /// `choose` cannot go on.
    fails: bool,
    /// `accepted` cannot go on.
    fails_late: bool,
    /// Reservations not yet answered.
    open: usize,
    accepted: Vec<u32>,
    refused: usize,
    unknown: usize,
    completed: Vec<u32>,
}

#[derive(Clone, Debug, Default)]
struct Policy(Rc<RefCell<Account>>);

impl Egress<u32, u32> for Policy {
    type Reservation = ();
    type Fault = &'static str;

    fn choose<C: Candidate>(
        &mut self,
        mut ready: impl Iterator<Item = C> + Clone,
    ) -> Result<Option<(C, ())>, &'static str> {
        let mut account = self.0.borrow_mut();
        if account.fails && ready.clone().next().is_some() {
            return Err("choose");
        }
        if account.shut {
            return Ok(None);
        }
        let work = if account.last {
            ready.last()
        } else {
            ready.next()
        };
        account.open += usize::from(work.is_some());
        Ok(work.map(|work| (work, ())))
    }

    fn accepted(&mut self, (): (), ticket: u32) -> Result<(), &'static str> {
        let mut account = self.0.borrow_mut();
        account.open -= 1;
        account.accepted.push(ticket);
        if account.fails_late {
            Err("accepted")
        } else {
            Ok(())
        }
    }

    fn refused(&mut self, (): ()) -> Result<(), &'static str> {
        let mut account = self.0.borrow_mut();
        account.open -= 1;
        account.refused += 1;
        Ok(())
    }

    fn unknown(&mut self, (): ()) -> Result<(), &'static str> {
        let mut account = self.0.borrow_mut();
        account.open -= 1;
        account.unknown += 1;
        Ok(())
    }

    fn completed(&mut self, ticket: u32) -> Result<(), &'static str> {
        self.0.borrow_mut().completed.push(ticket);
        Ok(())
    }
}

#[test]
fn a_policy_chooses_among_all_ready_work() {
    brand!(Net);
    let policy = Policy::default();
    let bench = bench_with::<Net, _>(policy.clone());
    run(bench, async |socket, wire| {
        assert_eq!(
            send_one(socket, b"resolved").await,
            Sent::Ended(Outcome::Accepted)
        );
        {
            let mut account = policy.0.borrow_mut();
            account.shut = true;
            account.last = true;
        }
        let first = send(socket, b"first").await;
        let second = send(socket, b"second").await;
        settle().await;
        policy.0.borrow_mut().shut = false;
        wire.0.borrow_mut().wake();
        result_of(first).await;
        result_of(second).await;
        assert_eq!(
            datagrams(&wire.0.borrow())[1..],
            [b"second".to_vec(), b"first".to_vec()]
        );
    });
}

#[test]
fn a_refused_frame_answers_its_reservation_and_stays_ready() {
    brand!(Net);
    let policy = Policy::default();
    let bench = bench_with::<Net, _>(policy.clone());
    run(bench, async |socket, wire| {
        open_port(wire, true);
        let observer = send(socket, b"late").await;
        settle().await;
        assert!(policy.0.borrow().refused >= 1);
        assert!(policy.0.borrow().accepted.is_empty());
        open_port(wire, false);
        assert_eq!(result_of(observer).await, Sent::Ended(Outcome::Accepted));
    });
    assert_eq!(policy.0.borrow().open, 0);
}

#[test]
fn a_completion_alone_reaches_the_policy() {
    brand!(Net);
    let policy = Policy::default();
    let bench = bench_with::<Net, _>(policy.clone());
    run(bench, async |socket, wire| {
        send_one(socket, b"sent").await;
        settle().await;
        let ticket = *policy.0.borrow().accepted.last().unwrap();
        assert!(policy.0.borrow().completed.is_empty());
        {
            let mut wire = wire.0.borrow_mut();
            wire.ended.push_back(ticket);
            wire.wake();
        }
        settle().await;
        assert_eq!(policy.0.borrow().completed, [ticket]);
    });
}

#[test]
fn a_frame_of_unknown_end_is_not_a_refusal() {
    brand!(Net);
    let policy = Policy::default();
    let bench = bench_with::<Net, _>(policy.clone());
    let (ended, _, ()) = run(bench, async |socket, wire| {
        send_one(socket, b"resolved").await;
        wire.0.borrow_mut().contradicts = true;
        let observer = send(socket, b"unknown").await;
        assert_eq!(result_of(observer).await, Sent::Ended(Outcome::Unknown));
        settle().await;
    });
    assert_eq!(ended.cause, EndCause::PortContract);
    let account = policy.0.borrow();
    assert_eq!((account.unknown, account.refused, account.open), (1, 0, 0));
}

#[test]
fn a_policy_that_cannot_choose_ends_the_service_with_its_fault() {
    brand!(Net);
    let policy = Policy::default();
    let bench = bench_with::<Net, _>(policy.clone());
    let (mut ended, mut socket, observer) = run(bench, async |socket, _| {
        policy.0.borrow_mut().fails = true;
        let observer = send(socket, b"never").await;
        while !socket.closed() {
            yield_now().await;
        }
        observer
    });
    assert_eq!(ended.cause, EndCause::Egress);
    assert_eq!(ended.fault, Some("choose"));
    assert_eq!(observer.take().ok(), Some(Sent::Ended(Outcome::Retired)));
    assert!(ended.egress.0.borrow().accepted.is_empty());
    returned(&mut ended, &mut socket);
}

#[test]
fn a_fault_in_an_answer_ends_the_service_with_its_result_kept() {
    brand!(Net);
    let policy = Policy::default();
    let bench = bench_with::<Net, _>(policy.clone());
    let (ended, _, sent) = run(bench, async |socket, _| {
        policy.0.borrow_mut().fails_late = true;
        let observer = send(socket, b"went").await;
        while !socket.closed() {
            yield_now().await;
        }
        result_of(observer).await
    });
    assert_eq!(ended.cause, EndCause::Egress);
    assert_eq!(ended.fault, Some("accepted"));
    // The ARP request was what the port took: the datagram never went.
    assert_eq!(sent, Sent::Ended(Outcome::Retired));
}

#[test]
fn a_cancelled_run_keeps_the_policy_with_its_account() {
    brand!(Net);
    let policy = Policy::default();
    let Bench {
        mut service,
        mut socket,
        clock,
        ..
    } = bench_with::<Net, _>(policy.clone());
    let mut events = ();
    let cx = &mut Context::from_waker(Waker::noop());
    {
        let mut run = pin!(service.run(clock, &mut events));
        let mut blocks = blocks_by_hand(&mut socket, &mut run, 1);
        let mut observer = socket.send(peer(), blocks.pop().unwrap()).ok().unwrap();
        while pin!(observer.finished()).poll(cx).is_pending() {
            assert!(run.as_mut().poll(cx).is_pending());
        }
    }
    let ended = service.retire();
    let account = ended.egress.0.borrow();
    assert_eq!(account.open, 0);
    assert_eq!(
        account.accepted.len(),
        2,
        "the ARP request and the datagram"
    );
}
