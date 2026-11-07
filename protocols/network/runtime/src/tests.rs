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
    Block, FramePort, FrameSource, Interface, NetworkOwner, OpenRefused, Outcome, Remote,
    SendRefusal, SlotTooShort, claim,
};
use oer_protocols_network_wire::arp::{self, Arp, Operation};
use oer_protocols_network_wire::checksum::{Checksum, ipv4_pseudo_header, udp_field};
use oer_protocols_network_wire::ethernet::{EtherType, EthernetHeader};
use oer_protocols_network_wire::ip::{DsField, Dscp, Ipv4Fields, Ipv4Header};
use oer_protocols_network_wire::transport::{PROTOCOL_UDP, Ports, UdpHeader};

use crate::{
    EndCause, Ended, Observer, Refusal, Sent, Service, Socket, SocketClosed, SocketLanes, StopLane,
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
        if wire.contradicts {
            return Ok(());
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
    bench_on(
        NetworkOwner::new(brand, interface, pool, wire.clone()),
        wire,
    )
}

fn storage<B>() -> &'static mut SocketLanes<'static, 'static, B, SIZE, DEPTH, RECEIVED, CREDITS> {
    Box::leak(Box::new(SocketLanes::new()))
}

fn bench_on<B: 'static>(
    owner: NetworkOwner<'static, B, SIZE, 1, CREDITS, RECEIVED, 2, BLOCKS, WirePort>,
    wire: WirePort,
) -> Bench<B> {
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut service = Service::new(owner, stopping);
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
    let (ended, result) = run_tasks(clock, service.run(clock, &mut events), drive);
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
    run_tasks(clock, service.run(clock, &mut events), drive)
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
    let (mut ended, observer) = run_tasks(clock, service.run(clock, &mut events), drive);
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
    let (ended, observer) = run_tasks(clock, service.run(clock, &mut events), drive);
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
    let (_, observer) = run_tasks(clock, service.run(clock, &mut events), drive);
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
    let mut service = Service::new(owner, stopping);
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
    let (ended, results) = run_tasks(clock, service.run(clock, &mut events), drive);
    let accepted = Sent::Ended(Outcome::Accepted);
    assert_eq!(results, (accepted, accepted));
    assert_eq!(ended.cause, EndCause::Stopped);
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

#[test]
fn a_cancelled_service_task_ends_an_observer_and_a_full_lane_wait() {
    brand!(Net);
    let Bench {
        service,
        mut socket,
        wire,
        clock,
        ..
    } = bench::<Net>();
    let mut events = ();
    let mut run = Box::pin(service.run(clock, &mut events));
    let mut blocks = Vec::new();
    let cx_client = &mut Context::from_waker(Waker::noop());
    for _ in 0..BLOCKS {
        let mut reserve = pin!(socket.reserve());
        blocks.push(loop {
            if let Poll::Ready(block) = reserve.as_mut().poll(cx_client) {
                break block.unwrap();
            }
            assert!(run.as_mut().poll(cx_client).is_pending());
        });
    }
    wire.0.borrow_mut().full = true;
    let mut observer = socket.send(peer(), blocks.pop().unwrap()).ok().unwrap();
    let second = socket.send(peer(), blocks.pop().unwrap()).ok().unwrap();
    socket.release(blocks.pop().unwrap()).ok().unwrap();
    socket.release(blocks.pop().unwrap()).ok().unwrap();
    assert!(pin!(socket.reserve()).poll(cx_client).is_pending());
    let (observed, observer_waker) = counter();
    assert!(
        pin!(observer.finished())
            .poll(&mut Context::from_waker(&observer_waker))
            .is_pending()
    );
    let (roomy, room_waker) = counter();
    let room_cx = &mut Context::from_waker(&room_waker);
    {
        let mut room = pin!(socket.room());
        assert!(room.as_mut().poll(room_cx).is_pending());
        drop(run);
        assert!(roomy.0.load(Ordering::SeqCst) >= 1, "the end wakes room");
        assert_eq!(room.as_mut().poll(room_cx), Poll::Ready(Err(SocketClosed)));
    }
    assert_eq!(observed.0.load(Ordering::SeqCst), 1);
    assert_eq!(observer.take().ok(), Some(Sent::Ended(Outcome::Retired)));
    assert_eq!(second.take().ok(), Some(Sent::Ended(Outcome::Retired)));
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
