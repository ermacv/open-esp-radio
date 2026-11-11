//! A local producer beside a channel socket: what the service asks of it, in
//! which turn, and what it hears back, through the public API only. The
//! expected calls and frames are written from the contract, not read from the
//! service.

extern crate std;

use std::boxed::Box;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::vec;
use std::vec::Vec;

use core::future::Future;
use core::pin::{Pin, pin};
use core::task::{Context, Poll, Waker};

use oer_portable_events::EventSink;
use oer_portable_memory::WakingSpscSender;
use oer_portable_time::{Clock, Duration, Instant};
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_network_core::{
    Block, Declined, Group, Interface, NetEvent, NetworkOwner, Outcome, Payload, Remote,
    SendRefusal, WorkId, claim,
};
use oer_protocols_network_port::{
    Egress, EthernetFrameView, FramePort, FrameSource, LinkSnapshot, LinkState, Unpublished,
};
use oer_protocols_network_wire::arp::Operation;
use oer_protocols_network_wire::checksum::{Checksum, ipv4_pseudo_header};
use oer_protocols_network_wire::ethernet::{EtherType, EthernetHeader};
use oer_protocols_network_wire::ip::{Dscp, Ipv4Header};
use oer_protocols_network_wire::transport::{PROTOCOL_UDP, UdpHeader};

use super::{
    BLOCKS, Count, DEPTH, LOCAL, OUR_IP, PEER, PEER_IP, Policy, RECEIVED, SIZE, counter,
    dhcp_messages, dhcp_reply, udp_frame,
};
use oer_protocols_network_wire::dhcp::MessageType;

use crate::{
    ConfigurationWatch, Direct, EndCause, Interest, LocalEnd, LocalRefused, MemoryPolicy, Next,
    Observer, Offer, Produce, Producer, Sent, Service, ServiceEvent, Socket, SocketLanes, StopLane,
    WatchRefused, Work,
};

/// The producer's port.
const LOCAL_PORT: u16 = 6000;
/// The channel socket's port.
const CHANNEL_PORT: u16 = 5000;

/// Names a producer's datagram: the source's generation and its place.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Tag {
    generation: u32,
    index: u32,
}

const fn tag(generation: u32, index: u32) -> Tag {
    Tag { generation, index }
}

/// What the service asked of the producer, in order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Call {
    Offered(Tag),
    Refused(Tag, SendRefusal),
    Filled(Tag),
    Ended(Tag, Outcome),
}

#[derive(Clone, Debug)]
struct Item {
    tag: Tag,
    remote: Remote,
    data: Vec<u8>,
}

/// The data of `tag`: its generation and index, then filler.
fn data_of(tag: Tag, length: usize) -> Vec<u8> {
    let mut data = vec![0x5a; length];
    if let Some(head) = data.get_mut(..2) {
        head.copy_from_slice(&[tag.generation as u8, tag.index as u8]);
    }
    data
}

/// The producer's source, which the test plays the application of.
#[derive(Default)]
struct Source {
    generation: u32,
    /// Not offered yet, first first.
    ready: VecDeque<Item>,
    /// Offered and not ended. Its store is found by index alone, as a source
    /// that reuses its entries: the fill's check of the entry's tag keeps a
    /// later generation's data out of an earlier work.
    held: Vec<Item>,
    /// Indices whose fill declines; every fill declines with `declines`.
    declining: Vec<u32>,
    declines: bool,
    /// An item that becomes ready at its time, which `offer` names.
    due: Option<(Instant, Item)>,
    calls: Vec<Call>,
    /// The works the producer saw, at fill and end.
    works: Vec<(WorkId, Tag)>,
    /// Whether the end flag was set at each `ended`.
    ended_after_end: Vec<bool>,
    end: Option<&'static LocalEnd>,
    waker: Option<Waker>,
    /// What the service registered for, at each wait.
    interests: Vec<Interest>,
}

#[derive(Clone, Default)]
struct Script(Rc<RefCell<Source>>);

impl Script {
    fn push(&self, tag: Tag, remote: Remote, length: usize) {
        self.0.borrow_mut().ready.push_back(Item {
            tag,
            remote,
            data: data_of(tag, length),
        });
    }

    /// New data came: the application wakes the producer's waiter.
    fn wake(&self) {
        if let Some(waker) = self.0.borrow_mut().waker.take() {
            waker.wake();
        }
    }

    fn calls(&self) -> Vec<Call> {
        self.0.borrow().calls.clone()
    }

    /// The calls since the first `skip`.
    fn calls_from(&self, skip: usize) -> Vec<Call> {
        self.calls().split_off(skip)
    }

    fn interest(&self) -> Option<Interest> {
        self.0.borrow().interests.last().copied()
    }

    fn filled(&self) -> usize {
        self.calls()
            .iter()
            .filter(|call| matches!(call, Call::Filled(_)))
            .count()
    }

    /// Drop `tag` from the source: it is not the producer's any more.
    fn forget(&self, tag: Tag) {
        let mut source = self.0.borrow_mut();
        source.ready.retain(|item| item.tag != tag);
        source.held.retain(|item| item.tag != tag);
    }
}

impl Produce for Script {
    type Tag = Tag;

    fn generation(&self) -> u32 {
        self.0.borrow().generation
    }

    fn current(&self, tag: Tag) -> bool {
        let source = self.0.borrow();
        source
            .ready
            .iter()
            .chain(source.held.iter())
            .any(|item| item.tag == tag)
    }

    fn offer(&mut self, now: Instant) -> Result<Offer<Tag>, Next> {
        let mut source = self.0.borrow_mut();
        if let Some((at, _)) = &source.due {
            if now < *at {
                return Err(Next::At(*at));
            }
            if let Some((_, item)) = source.due.take() {
                source.ready.push_back(item);
            }
        }
        let Some(item) = source.ready.pop_front() else {
            return Err(Next::Idle);
        };
        source.calls.push(Call::Offered(item.tag));
        let offer = Offer {
            remote: item.remote,
            length: item.data.len(),
            tag: item.tag,
        };
        source.held.push(item);
        Ok(offer)
    }

    fn fill(
        &mut self,
        _now: Instant,
        work: Work<Tag>,
        data: &mut Payload<'_>,
    ) -> Result<(), Declined> {
        let mut source = self.0.borrow_mut();
        source.calls.push(Call::Filled(work.tag));
        source.works.push((work.id, work.tag));
        if source.declines || source.declining.contains(&work.tag.index) {
            return Err(Declined::Stale);
        }
        let item = source
            .held
            .iter()
            .find(|item| item.tag.index == work.tag.index)
            // The entry at the index is reused: its tag says whether it is
            // still this work's data.
            .filter(|item| item.tag == work.tag)
            .ok_or(Declined::Stale)?;
        data.put(&item.data).map_err(|_| Declined::Unwritable)
    }

    fn refused(&mut self, tag: Tag, reason: SendRefusal) {
        let mut source = self.0.borrow_mut();
        source.calls.push(Call::Refused(tag, reason));
        let Some(at) = source.held.iter().position(|item| item.tag == tag) else {
            return;
        };
        let item = source.held.remove(at);
        // A temporary refusal keeps the data for the next ask; a final one
        // takes it out of the ready set.
        if matches!(
            reason,
            SendRefusal::NoPlace | SendRefusal::NeighborsFull | SendRefusal::NoAddress
        ) {
            source.ready.push_front(item);
        }
    }

    fn ended(&mut self, work: Work<Tag>, outcome: Outcome) {
        let mut source = self.0.borrow_mut();
        source.calls.push(Call::Ended(work.tag, outcome));
        source.works.push((work.id, work.tag));
        let ended = source.end.is_some_and(LocalEnd::is_ended);
        source.ended_after_end.push(ended);
        source.held.retain(|item| item.tag != work.tag);
    }

    fn register(&mut self, cx: &mut Context<'_>, interest: Interest) -> bool {
        let mut source = self.0.borrow_mut();
        source.waker = Some(cx.waker().clone());
        source.interests.push(interest);
        !source.ready.is_empty()
    }
}

/// A port's answer to one publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Reply {
    /// Fill, and take what was written; a failed fill's own error back.
    Take,
    /// No room.
    Full,
    /// `Ok`, whatever the fill did: the port's fault after a failed fill.
    Yes,
    /// `Ok` without calling the fill: the port's fault.
    Lazy,
    /// A refusal of its own after a failed fill: the port's fault.
    Misreport,
}

#[derive(Default)]
struct Line {
    incoming: VecDeque<Vec<u8>>,
    notices: VecDeque<u32>,
    sent: Vec<Vec<u8>>,
    full: bool,
    /// The answers to the next publications; then `Take`, or `Full` while
    /// `full`.
    replies: VecDeque<Reply>,
    publishes: usize,
    tickets: u32,
    waker: Option<Waker>,
    /// The link is down.
    down: bool,
    /// The last frame was refused for room.
    refused: bool,
    /// Each publication moves the clock to this time, as a turn takes time.
    elapse: Option<(&'static VirtualClock, Instant)>,
}

#[derive(Clone, Default)]
struct LocalPort(Rc<RefCell<Line>>);

impl LocalPort {
    fn open(&self, full: bool) {
        let mut line = self.0.borrow_mut();
        line.full = full;
        if let Some(waker) = line.waker.take() {
            waker.wake();
        }
    }

    fn sent(&self) -> usize {
        self.0.borrow().sent.len()
    }

    fn publishes(&self) -> usize {
        self.0.borrow().publishes
    }

    /// The UDP payloads accepted, from the local port.
    fn produced(&self) -> Vec<Vec<u8>> {
        self.0
            .borrow()
            .sent
            .iter()
            .filter_map(|frame| {
                let (header, payload) = EthernetHeader::parse(frame).ok()?;
                (header.ether_type == EtherType::IPV4).then_some(())?;
                let packet = Ipv4Header::parse(payload).ok()?;
                let (udp, data) = UdpHeader::parse(packet.payload()).ok()?;
                (udp.ports.source == LOCAL_PORT).then(|| data.to_vec())
            })
            .collect()
    }

    /// The UDP payloads accepted from the channel socket.
    fn channel(&self) -> Vec<Vec<u8>> {
        self.0
            .borrow()
            .sent
            .iter()
            .filter_map(|frame| {
                let (header, payload) = EthernetHeader::parse(frame).ok()?;
                (header.ether_type == EtherType::IPV4).then_some(())?;
                let packet = Ipv4Header::parse(payload).ok()?;
                let (udp, data) = UdpHeader::parse(packet.payload()).ok()?;
                (udp.ports.source == CHANNEL_PORT).then(|| data.to_vec())
            })
            .collect()
    }
}

impl FramePort for LocalPort {
    type Refusal = ();
    type Ticket = u32;

    fn publish<E>(
        &mut self,
        meta: oer_protocols_network_port::FrameMeta,
        fill: impl FnOnce(&mut oer_protocols_network_port::Slot<'_>) -> Result<(), E>,
    ) -> Result<u32, Unpublished<(), E>> {
        let length = meta.length;
        let mut line = self.0.borrow_mut();
        line.publishes += 1;
        if let Some((clock, at)) = line.elapse
            && clock.now() < at
        {
            clock.advance_to(at);
        }
        let reply =
            line.replies
                .pop_front()
                .unwrap_or(if line.full { Reply::Full } else { Reply::Take });
        line.refused = reply == Reply::Full;
        let mut frame = vec![0; length];
        match reply {
            Reply::Full => Err(Unpublished::Port(())),
            Reply::Lazy => Ok(0),
            Reply::Yes => {
                let _either = fill(&mut oer_protocols_network_port::Slot::new(&mut frame, None));
                Ok(0)
            }
            Reply::Take | Reply::Misreport => {
                match fill(&mut oer_protocols_network_port::Slot::new(&mut frame, None)) {
                    Err(error) if reply == Reply::Take => Err(Unpublished::Fill(error)),
                    Err(_) => Err(Unpublished::Port(())),
                    Ok(()) => {
                        line.sent.push(frame);
                        line.tickets += 1;
                        Ok(line.tickets)
                    }
                }
            }
        }
    }
}

impl FrameSource for LocalPort {
    type Notice = u32;
    type Held = oer_protocols_network_port::NeverHeld;

    fn take_frame(
        &mut self,
        read: impl FnOnce(
            EthernetFrameView<'_>,
            oer_protocols_network_port::Holding,
        ) -> oer_protocols_network_port::Keep,
    ) -> oer_protocols_network_port::Taken<Self::Held> {
        use oer_protocols_network_port::{CopyReason, Holding, Taken};
        let frame = self.0.borrow_mut().incoming.pop_front();
        let Some(frame) = frame else {
            return Taken::Empty;
        };
        let (header, payload) = EthernetHeader::parse(&frame).unwrap();
        let _copied = read(
            EthernetFrameView {
                destination: header.destination,
                source: header.source,
                ether_type: header.ether_type.value(),
                payload,
            },
            Holding::Copied(CopyReason::Format),
        );
        Taken::Read
    }

    fn take_notice(&mut self) -> Option<u32> {
        self.0.borrow_mut().notices.pop_front()
    }

    fn link(&self) -> LinkSnapshot {
        if self.0.borrow().down {
            LinkSnapshot::DOWN
        } else {
            LinkSnapshot {
                state: LinkState::Up,
                epoch: 1,
            }
        }
    }

    fn register(&mut self, cx: &mut Context<'_>) -> bool {
        let mut line = self.0.borrow_mut();
        line.waker = Some(cx.waker().clone());
        !line.incoming.is_empty() || !line.notices.is_empty() || (line.refused && !line.full)
    }
}

type Channel<B, const C: usize> = Socket<'static, 'static, B, SIZE, DEPTH, RECEIVED, C>;
type Served<B, E, const C: usize> = Service<
    'static,
    'static,
    B,
    SIZE,
    2,
    C,
    RECEIVED,
    2,
    BLOCKS,
    DEPTH,
    RECEIVED,
    LocalPort,
    E,
    Producer<'static, B, Script, C>,
>;

/// A service with a producer on [`LOCAL_PORT`] and a channel socket on
/// [`CHANNEL_PORT`], and the test's handles.
struct Bench<B: 'static, E: Egress<u32, u32> = Direct, const C: usize = 2> {
    service: Served<B, E, C>,
    socket: Channel<B, C>,
    line: LocalPort,
    script: Script,
    end: &'static LocalEnd,
    stop: WakingSpscSender<'static, (), 1>,
    clock: &'static VirtualClock,
}

fn bench<B: 'static, E: Egress<u32, u32>, const C: usize>(egress: E) -> Bench<B, E, C> {
    bench_on(
        egress,
        Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap(),
    )
}

fn bench_on<B: 'static, E: Egress<u32, u32>, const C: usize>(
    egress: E,
    interface: Interface,
) -> Bench<B, E, C> {
    let memory = Box::leak(Box::new([[0; SIZE]; BLOCKS]));
    let (pool, brand) = claim::<B, SIZE, BLOCKS>(memory).unwrap();
    let line = LocalPort::default();
    let owner = NetworkOwner::new(brand, interface, pool, line.clone());
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let end: &'static LocalEnd = Box::leak(Box::new(LocalEnd::new()));
    let script = Script::default();
    script.0.borrow_mut().end = Some(end);
    let mut service = Service::with_producer(
        owner,
        egress,
        MemoryPolicy::KEEP,
        stopping,
        script.clone(),
        end,
    );
    service.bind_local(LOCAL_PORT, Dscp::default()).unwrap();
    let lanes: &'static mut SocketLanes<'static, 'static, B, SIZE, DEPTH, RECEIVED, C> =
        Box::leak(Box::new(SocketLanes::new()));
    let socket = service
        .bind(CHANNEL_PORT, Dscp::default(), Group::shared(), lanes)
        .ok()
        .unwrap();
    Bench {
        service,
        socket,
        line,
        script,
        end,
        stop,
        clock: Box::leak(Box::new(VirtualClock::new())),
    }
}

fn direct<B: 'static>() -> Bench<B> {
    bench(Direct)
}

fn broadcast() -> Remote {
    Remote {
        address: [192, 0, 2, 255],
        port: 7,
    }
}

fn host(last: u8) -> Remote {
    Remote {
        address: [192, 0, 2, last],
        port: 7,
    }
}

fn peer() -> Remote {
    Remote {
        address: PEER_IP,
        port: 7,
    }
}

/// The peer's ARP request for us, so the core knows it.
fn peer_asks() -> Vec<u8> {
    let mut frame = EthernetHeader {
        destination: [0xFF; 6],
        source: PEER,
        ether_type: EtherType::ARP,
    }
    .to_bytes()
    .to_vec();
    let mut packet = [0; oer_protocols_network_wire::arp::PACKET_LEN];
    oer_protocols_network_wire::arp::Arp {
        operation: Operation::Request,
        sender_hardware: PEER,
        sender_protocol: PEER_IP,
        target_hardware: [0; 6],
        target_protocol: OUR_IP,
    }
    .write(&mut packet)
    .unwrap();
    frame.extend_from_slice(&packet);
    frame
}

/// Poll `run` once: one turn, and the pass after it.
fn poll<F: Future>(run: &mut Pin<&mut F>, cx: &mut Context<'_>) -> Poll<F::Output> {
    run.as_mut().poll(cx)
}

/// Poll `run` until a poll leaves it waiting with no wake of its own, which
/// it must reach within a few polls: no turn spins.
fn settle<F: Future>(run: &mut Pin<&mut F>, count: &Count, waker: &Waker) {
    let cx = &mut Context::from_waker(waker);
    for _ in 0..16 {
        let before = count.0.load(core::sync::atomic::Ordering::SeqCst);
        assert!(poll(run, cx).is_pending(), "the service ended");
        if count.0.load(core::sync::atomic::Ordering::SeqCst) == before {
            return;
        }
    }
    panic!("the service never waited: it spins");
}

/// Blocks for the channel socket, `run` turning for them.
fn blocks<B: 'static, F: Future, const C: usize>(
    socket: &mut Channel<B, C>,
    run: &mut Pin<&mut F>,
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

/// A channel datagram to `remote`, sent by hand.
fn channel_send<B: 'static, F: Future, const C: usize>(
    socket: &mut Channel<B, C>,
    run: &mut Pin<&mut F>,
    remote: Remote,
    data: &[u8],
) -> Observer<'static> {
    let mut block = blocks(socket, run, 1).pop().unwrap();
    block.fill(data).unwrap();
    socket.send(remote, block).ok().unwrap()
}

#[test]
fn a_refused_offer_does_not_hold_back_the_next_in_the_first_poll() {
    brand!(Net);
    let Bench {
        mut service,
        line,
        script,
        clock,
        ..
    } = direct::<Net>();
    script.push(tag(0, 0), broadcast(), 2000);
    script.push(tag(0, 1), broadcast(), 40);
    let mut events = ();
    let mut run = pin!(service.run(clock, &mut events));
    // A waker nothing registers before this poll, and the producer never
    // wakes: the second offer must come in this very turn.
    let cx = &mut Context::from_waker(Waker::noop());
    assert!(poll(&mut run, cx).is_pending());
    assert_eq!(
        script.calls(),
        [
            Call::Offered(tag(0, 0)),
            Call::Refused(tag(0, 0), SendRefusal::TooLong),
            Call::Offered(tag(0, 1)),
            Call::Filled(tag(0, 1)),
            Call::Ended(tag(0, 1), Outcome::Accepted),
        ]
    );
    let frame = line.0.borrow().sent[0].clone();
    let (_, packet) = EthernetHeader::parse(&frame).unwrap();
    let ip = Ipv4Header::parse(packet).unwrap();
    let (udp, data) = UdpHeader::parse(ip.payload()).unwrap();
    assert_eq!(data, data_of(tag(0, 1), 40));
    assert!(
        Checksum::new()
            .with(&ipv4_pseudo_header(
                OUR_IP,
                [192, 0, 2, 255],
                PROTOCOL_UDP,
                udp.length
            ))
            .with(ip.payload())
            .verifies()
    );
    for _ in 0..4 {
        let _pending = poll(&mut run, cx);
    }
    assert_eq!(script.calls().len(), 5, "the refused offer never came back");
}

#[test]
fn eight_final_refusals_spend_the_turn_and_the_next_turn_goes_on() {
    brand!(Net);
    let Bench {
        mut service,
        script,
        clock,
        ..
    } = direct::<Net>();
    for index in 0..8 {
        script.push(tag(0, index), broadcast(), 2000);
    }
    script.push(tag(0, 8), broadcast(), 40);
    let mut events = ();
    let mut run = pin!(service.run(clock, &mut events));
    let (count, waker) = counter();
    let cx = &mut Context::from_waker(&waker);
    assert!(poll(&mut run, cx).is_pending());
    let first: Vec<Call> = (0..8)
        .flat_map(|index| {
            [
                Call::Offered(tag(0, index)),
                Call::Refused(tag(0, index), SendRefusal::TooLong),
            ]
        })
        .collect();
    assert_eq!(script.calls(), first, "eight offers, the turn's budget");
    assert!(
        count.0.load(core::sync::atomic::Ordering::SeqCst) > 0,
        "the spent budget is progress: the service yields"
    );
    assert!(poll(&mut run, cx).is_pending());
    assert_eq!(
        script.calls_from(16),
        [
            Call::Offered(tag(0, 8)),
            Call::Filled(tag(0, 8)),
            Call::Ended(tag(0, 8), Outcome::Accepted),
        ]
    );
}

/// `NoPlace` holds the producer's offers until one of its places frees, and
/// the offer comes in the turn after it freed. This is the success case: the
/// held offer is accepted.
#[test]
fn no_place_asks_nothing_until_a_place_frees() {
    brand!(Net);
    let Bench {
        mut service,
        line,
        script,
        clock,
        ..
    } = direct::<Net>();
    line.open(true);
    for index in 0..3 {
        script.push(tag(0, index), broadcast(), 16);
    }
    let mut events = ();
    let mut run = pin!(service.run(clock, &mut events));
    let (count, waker) = counter();
    settle(&mut run, &count, &waker);
    assert_eq!(
        script.calls(),
        [
            Call::Offered(tag(0, 0)),
            Call::Offered(tag(0, 1)),
            Call::Offered(tag(0, 2)),
            Call::Refused(tag(0, 2), SendRefusal::NoPlace),
        ]
    );
    assert_eq!(
        script.interest(),
        Some(Interest::Changes),
        "a held producer waits for changes only"
    );
    // A wake with nothing changed asks nothing.
    script.wake();
    settle(&mut run, &count, &waker);
    assert_eq!(script.calls().len(), 4, "no ask before a place frees");
    line.open(false);
    let cx = &mut Context::from_waker(&waker);
    assert!(poll(&mut run, cx).is_pending());
    assert_eq!(
        script.calls_from(4),
        [
            Call::Filled(tag(0, 0)),
            Call::Ended(tag(0, 0), Outcome::Accepted),
            Call::Filled(tag(0, 1)),
            Call::Ended(tag(0, 1), Outcome::Accepted),
        ],
        "each place frees as its port takes its frame"
    );
    assert!(poll(&mut run, cx).is_pending());
    assert_eq!(
        script.calls_from(8),
        [
            Call::Offered(tag(0, 2)),
            Call::Filled(tag(0, 2)),
            Call::Ended(tag(0, 2), Outcome::Accepted),
        ],
        "the held offer comes in the next turn"
    );
}

/// How a test makes the held offer stale.
#[derive(Clone, Copy)]
enum Stale {
    /// The source starts a new generation, without the held offer.
    Generation,
    /// The held offer is no longer the producer's, in the same generation.
    NotCurrent,
}

/// Make `held` stale as `how` says, and give the producer `next`.
fn make_stale(script: &Script, how: Stale, held: Tag, next: Tag, remote: Remote) {
    script.forget(held);
    if let Stale::Generation = how {
        script.0.borrow_mut().generation = next.generation;
    }
    script.push(next, remote, 16);
    script.wake();
}

/// A `NeighborsFull` hold with no accepted work of the producer: generation
/// A's first offer goes to an unknown address while the channel socket's
/// datagrams hold every neighbor entry. The application then offers B to
/// broadcast, the neighbor table unchanged: the next turn drops the stale
/// hold and declares B, and A hears no `ended`.
fn a_stale_neighbors_full_hold_gives_way<Net: 'static>(how: Stale) {
    let Bench {
        mut service,
        mut socket,
        script,
        clock,
        ..
    } = direct::<Net>();
    let mut events = ();
    let mut run = pin!(service.run(clock, &mut events));
    let (count, waker) = counter();
    let _seven = channel_send(&mut socket, &mut run, host(7), b"seven");
    let _eight = channel_send(&mut socket, &mut run, host(8), b"eight");
    settle(&mut run, &count, &waker);
    let a = tag(0, 0);
    script.push(a, host(9), 16);
    script.wake();
    settle(&mut run, &count, &waker);
    assert_eq!(
        script.calls(),
        [
            Call::Offered(a),
            Call::Refused(a, SendRefusal::NeighborsFull)
        ]
    );
    let b = match how {
        Stale::Generation => tag(1, 0),
        Stale::NotCurrent => tag(0, 1),
    };
    make_stale(&script, how, a, b, broadcast());
    settle(&mut run, &count, &waker);
    assert_eq!(
        script.calls_from(2),
        [
            Call::Offered(b),
            Call::Filled(b),
            Call::Ended(b, Outcome::Accepted)
        ],
        "B goes, and A heard no end"
    );
}

#[test]
fn a_new_generation_drops_a_stale_neighbors_full_hold() {
    brand!(Net);
    a_stale_neighbors_full_hold_gives_way::<Net>(Stale::Generation);
}

#[test]
fn an_offer_no_longer_the_producer_s_drops_its_neighbors_full_hold() {
    brand!(Net);
    a_stale_neighbors_full_hold_gives_way::<Net>(Stale::NotCurrent);
}

/// A `NoPlace` hold made stale is cancelled and a new offer is asked; the
/// places are still taken, so that offer gets `NoPlace` again. The old works
/// stay current and keep their places: this is the cancellation alone, apart
/// from an acceptance.
fn a_stale_no_place_hold_is_cancelled<Net: 'static>(how: Stale) {
    let Bench {
        mut service,
        line,
        script,
        clock,
        ..
    } = direct::<Net>();
    line.open(true);
    for index in 0..3 {
        script.push(tag(0, index), broadcast(), 16);
    }
    let mut events = ();
    let mut run = pin!(service.run(clock, &mut events));
    let (count, waker) = counter();
    settle(&mut run, &count, &waker);
    let held = tag(0, 2);
    assert_eq!(
        script.calls().last(),
        Some(&Call::Refused(held, SendRefusal::NoPlace))
    );
    let next = match how {
        Stale::Generation => tag(1, 3),
        Stale::NotCurrent => tag(0, 3),
    };
    make_stale(&script, how, held, next, broadcast());
    settle(&mut run, &count, &waker);
    assert_eq!(
        script.calls_from(4),
        [
            Call::Offered(next),
            Call::Refused(next, SendRefusal::NoPlace)
        ],
        "a new attempt, refused again: the places are taken"
    );
    script.wake();
    settle(&mut run, &count, &waker);
    assert_eq!(script.calls().len(), 6, "held again until a place frees");
    assert_eq!(line.sent(), 0, "nothing was published");
}

#[test]
fn a_new_generation_cancels_a_no_place_hold() {
    brand!(Net);
    a_stale_no_place_hold_is_cancelled::<Net>(Stale::Generation);
}

#[test]
fn an_offer_no_longer_the_producer_s_cancels_its_no_place_hold() {
    brand!(Net);
    a_stale_no_place_hold_is_cancelled::<Net>(Stale::NotCurrent);
}

/// `NeighborsFull` asks nothing until the neighbor change counter moves,
/// and a release of protection moves it with no ARP entry changing: a
/// channel datagram to a known neighbor goes out, and the held offer comes
/// next turn. A wake with nothing changed is the control: it asks nothing.
#[test]
fn a_neighbors_full_hold_waits_for_a_release_without_any_arp_entry() {
    brand!(Net);
    let Bench {
        mut service,
        mut socket,
        line,
        script,
        clock,
        ..
    } = direct::<Net>();
    line.0.borrow_mut().incoming.push_back(peer_asks());
    line.open(true);
    let mut events = ();
    let mut run = pin!(service.run(clock, &mut events));
    let (count, waker) = counter();
    let known = channel_send(&mut socket, &mut run, peer(), b"known");
    let _asking = channel_send(&mut socket, &mut run, host(7), b"asking");
    settle(&mut run, &count, &waker);
    let a = tag(0, 0);
    script.push(a, host(9), 16);
    script.wake();
    settle(&mut run, &count, &waker);
    assert_eq!(
        script.calls(),
        [
            Call::Offered(a),
            Call::Refused(a, SendRefusal::NeighborsFull)
        ]
    );
    // The control: the same offer, nothing released.
    script.wake();
    settle(&mut run, &count, &waker);
    assert_eq!(script.calls().len(), 2, "the hold stays");
    line.open(false);
    settle(&mut run, &count, &waker);
    assert_eq!(
        result_now(known),
        Some(Sent::Ended(Outcome::Accepted)),
        "its neighbor's protection was released"
    );
    assert!(line.0.borrow().incoming.is_empty(), "no ARP answer came");
    assert_eq!(script.calls_from(2), [Call::Offered(a)], "asked again");
}

/// What an observer holds now, without waiting.
fn result_now(observer: Observer<'_>) -> Option<Sent> {
    observer.take().ok()
}

/// Eight declared works that always decline, beside a channel datagram: the
/// datagram goes in the first turn, and each turn has at most one declining
/// fill.
#[test]
fn one_declining_fill_a_turn_and_the_channel_still_goes() {
    brand!(Net);
    let Bench {
        mut service,
        mut socket,
        line,
        script,
        clock,
        ..
    } = bench::<Net, Direct, 8>(Direct);
    let mut events = ();
    let mut run = pin!(service.run(clock, &mut events));
    let cx = &mut Context::from_waker(Waker::noop());
    let first = blocks(&mut socket, &mut run, 2);
    let mut first = first.into_iter();
    let mut block = first.next().unwrap();
    block.fill(b"first").unwrap();
    let mut second = first.next().unwrap();
    second.fill(b"second").unwrap();
    script.0.borrow_mut().declines = true;
    for index in 0..8 {
        script.push(tag(0, index), broadcast(), 16);
    }
    drop(socket.send(broadcast(), block).ok().unwrap());
    assert!(poll(&mut run, cx).is_pending());
    assert_eq!(line.channel(), [b"first".to_vec()], "the datagram went");
    assert_eq!(script.filled(), 1, "one declining fill in the turn");
    drop(socket.send(broadcast(), second).ok().unwrap());
    assert!(poll(&mut run, cx).is_pending());
    assert_eq!(line.channel().len(), 2);
    assert_eq!(script.filled(), 2, "at most one more in the next turn");
    let declined = script
        .calls()
        .iter()
        .filter(|call| matches!(call, Call::Ended(_, Outcome::Declined)))
        .count();
    assert_eq!(declined, 2, "each decline ends its work Declined");
    assert!(
        !script
            .calls()
            .iter()
            .any(|call| matches!(call, Call::Ended(_, Outcome::Rejected))),
        "never Rejected"
    );
}

/// A decline ends at publication and moves the turn past its socket: once
/// the port has room again, the channel's datagram goes before the
/// producer's next work.
#[test]
fn after_a_declining_fill_the_next_socket_goes_first() {
    brand!(Net);
    let Bench {
        mut service,
        mut socket,
        line,
        script,
        clock,
        ..
    } = direct::<Net>();
    let mut events = ();
    let mut run = pin!(service.run(clock, &mut events));
    let cx = &mut Context::from_waker(Waker::noop());
    let mut block = blocks(&mut socket, &mut run, 1).pop().unwrap();
    block.fill(b"channel").unwrap();
    script.0.borrow_mut().declining.push(0);
    script.push(tag(0, 0), broadcast(), 16);
    script.push(tag(0, 1), broadcast(), 16);
    line.0
        .borrow_mut()
        .replies
        .extend([Reply::Take, Reply::Full]);
    drop(socket.send(broadcast(), block).ok().unwrap());
    assert!(poll(&mut run, cx).is_pending());
    assert_eq!(line.sent(), 0, "the decline, then no room");
    line.open(false);
    for _ in 0..4 {
        let _pending = poll(&mut run, cx);
    }
    let sent = line.0.borrow().sent.clone();
    let source = |frame: &Vec<u8>| {
        let (_, packet) = EthernetHeader::parse(frame).unwrap();
        let ip = Ipv4Header::parse(packet).unwrap();
        UdpHeader::parse(ip.payload()).unwrap().0.ports.source
    };
    assert_eq!(
        sent.iter().map(source).collect::<Vec<_>>(),
        [CHANNEL_PORT, LOCAL_PORT],
        "the channel's datagram first"
    );
}

/// A new generation ends every old work in one turn, with no fill and no
/// publication, and the new generation's offer is declared in that same
/// turn. The source reuses its entry for the new data, so no frame carries
/// generation B's data under A's work.
#[test]
fn a_new_generation_ends_old_works_and_its_offers_go_in_the_same_turn() {
    brand!(Net);
    let Bench {
        mut service,
        line,
        script,
        clock,
        ..
    } = direct::<Net>();
    line.open(true);
    script.push(tag(0, 0), broadcast(), 16);
    script.push(tag(0, 1), broadcast(), 16);
    let mut events = ();
    let mut run = pin!(service.run(clock, &mut events));
    let (count, waker) = counter();
    settle(&mut run, &count, &waker);
    let publishes = line.publishes();
    {
        // Run B overwrites entry 0 with its own data.
        let mut source = script.0.borrow_mut();
        source.generation = 1;
        source.held.clear();
    }
    script.push(tag(1, 0), broadcast(), 16);
    line.open(false);
    script.wake();
    let cx = &mut Context::from_waker(&waker);
    assert!(poll(&mut run, cx).is_pending());
    assert_eq!(
        script.calls_from(2),
        [
            Call::Ended(tag(0, 0), Outcome::Declined),
            Call::Ended(tag(0, 1), Outcome::Declined),
            Call::Offered(tag(1, 0)),
            Call::Filled(tag(1, 0)),
            Call::Ended(tag(1, 0), Outcome::Accepted),
        ]
    );
    assert_eq!(line.publishes(), publishes + 1, "withdrawal calls no port");
    assert_eq!(line.produced(), [data_of(tag(1, 0), 16)]);
    let works = script.0.borrow().works.clone();
    for (at, (id, tag)) in works.iter().enumerate() {
        for (other, other_tag) in &works[at + 1..] {
            assert_eq!(id == other, tag == other_tag, "one identity per work");
        }
    }
}

/// An accepted work's one outcome is given as its port takes its frame: a
/// new generation after it withdraws only the work still declared, and the
/// accepted ones are never told again.
#[test]
fn an_accepted_work_ends_once_and_a_new_generation_withdraws_only_open_work() {
    brand!(Net);
    let Bench {
        mut service,
        line,
        script,
        clock,
        ..
    } = direct::<Net>();
    script.push(tag(0, 0), broadcast(), 16);
    script.push(tag(0, 1), broadcast(), 16);
    let mut events = ();
    let mut run = pin!(service.run(clock, &mut events));
    let (count, waker) = counter();
    settle(&mut run, &count, &waker);
    assert_eq!(
        script.calls(),
        [
            Call::Offered(tag(0, 0)),
            Call::Offered(tag(0, 1)),
            Call::Filled(tag(0, 0)),
            Call::Ended(tag(0, 0), Outcome::Accepted),
            Call::Filled(tag(0, 1)),
            Call::Ended(tag(0, 1), Outcome::Accepted),
        ]
    );
    line.open(true);
    script.push(tag(0, 2), broadcast(), 16);
    script.wake();
    settle(&mut run, &count, &waker);
    assert_eq!(script.calls_from(6), [Call::Offered(tag(0, 2))]);
    {
        // A new generation takes the source's data, the open work's too.
        let mut source = script.0.borrow_mut();
        source.generation = 1;
        source.held.clear();
    }
    script.wake();
    let cx = &mut Context::from_waker(&waker);
    assert!(poll(&mut run, cx).is_pending());
    assert_eq!(
        script.calls_from(7),
        [Call::Ended(tag(0, 2), Outcome::Declined)],
        "only the work still declared"
    );
}

/// A work no longer current when its slot comes is declined by its
/// producer's fill, which finds the data gone and writes nothing, though the
/// generation did not change: no frame goes.
#[test]
fn a_work_not_current_at_its_fill_ends_declined_unfilled() {
    brand!(Net);
    let Bench {
        mut service,
        line,
        script,
        clock,
        ..
    } = direct::<Net>();
    line.open(true);
    script.push(tag(0, 0), broadcast(), 16);
    let mut events = ();
    let mut run = pin!(service.run(clock, &mut events));
    let (count, waker) = counter();
    settle(&mut run, &count, &waker);
    // The entry is reused for other data, the generation unchanged.
    script.0.borrow_mut().held[0].tag = tag(0, 7);
    line.open(false);
    settle(&mut run, &count, &waker);
    assert_eq!(
        script.calls(),
        [
            Call::Offered(tag(0, 0)),
            Call::Filled(tag(0, 0)),
            Call::Ended(tag(0, 0), Outcome::Declined),
        ]
    );
    assert_eq!(line.sent(), 0);
}

/// An `Egress::notice` fault in the turn where a tick ended the producer's
/// work `Unreachable` and a close ended a channel datagram `Closed`: both
/// results reach their clients before `run` returns, and nothing is
/// published after the fault.
#[test]
fn a_notice_fault_still_delivers_what_the_turn_ended() {
    brand!(Net);
    let policy = Policy::default();
    let Bench {
        mut service,
        mut socket,
        line,
        script,
        clock,
        ..
    } = bench::<Net, Policy, 2>(policy.clone());
    let mut events = ();
    let mut run = pin!(service.run(clock, &mut events));
    let (count, waker) = counter();
    let cx = &mut Context::from_waker(&waker);
    // The producer's work asks for its neighbor from the start, the
    // channel's a second later: only the first is given up at three.
    script.push(tag(0, 0), host(9), 16);
    script.wake();
    settle(&mut run, &count, &waker);
    let start = clock.now();
    clock.advance_to(start.saturating_add(Duration::from_secs(1)));
    let closed = channel_send(&mut socket, &mut run, host(7), b"closed");
    settle(&mut run, &count, &waker);
    clock.advance_to(start.saturating_add(Duration::from_secs(2)));
    settle(&mut run, &count, &waker);
    let sent = line.sent();
    clock.advance_to(start.saturating_add(Duration::from_secs(3)));
    {
        let mut close = pin!(socket.close());
        assert!(close.as_mut().poll(cx).is_pending(), "the close is asked");
    }
    {
        let mut line = line.0.borrow_mut();
        line.notices.push_back(1);
    }
    policy.0.borrow_mut().fails_completing = true;
    // Ready work the faulted turn must neither declare nor publish.
    script.push(tag(0, 1), broadcast(), 16);
    let cause = poll(&mut run, cx);
    assert_eq!(cause, Poll::Ready(EndCause::Egress));
    assert!(
        script
            .calls()
            .contains(&Call::Ended(tag(0, 0), Outcome::Unreachable)),
        "{:?}",
        script.calls()
    );
    assert_eq!(
        result_now(closed),
        Some(Sent::Ended(Outcome::Closed)),
        "before retirement"
    );
    assert_eq!(line.sent(), sent, "nothing published after the fault");
    assert!(
        !script.calls().contains(&Call::Offered(tag(0, 1))),
        "nothing offered after the fault"
    );
}

/// A publish fault after an `Accepted`: the producer hears it before `run`
/// returns, and nothing more is published.
#[test]
fn a_publish_fault_still_delivers_the_accepted_work() {
    brand!(Net);
    let policy = Policy::default();
    let Bench {
        mut service,
        script,
        line,
        clock,
        ..
    } = bench::<Net, Policy, 2>(policy.clone());
    policy.0.borrow_mut().fails_late = true;
    script.push(tag(0, 0), broadcast(), 16);
    script.push(tag(0, 1), broadcast(), 16);
    let mut events = ();
    let mut run = pin!(service.run(clock, &mut events));
    let cx = &mut Context::from_waker(Waker::noop());
    assert_eq!(poll(&mut run, cx), Poll::Ready(EndCause::Egress));
    assert_eq!(line.sent(), 1, "nothing after the fault");
    assert_eq!(
        script.calls(),
        [
            Call::Offered(tag(0, 0)),
            Call::Offered(tag(0, 1)),
            Call::Filled(tag(0, 0)),
            Call::Ended(tag(0, 0), Outcome::Accepted),
        ]
    );
}

/// The port says `Ok` after a declining fill: `Unknown` and the port's
/// fault, which wins over the decline.
#[test]
fn a_yes_after_a_declining_fill_is_unknown_and_ends_the_service() {
    brand!(Net);
    let Bench {
        mut service,
        line,
        script,
        clock,
        ..
    } = direct::<Net>();
    script.0.borrow_mut().declines = true;
    line.0.borrow_mut().replies.push_back(Reply::Yes);
    script.push(tag(0, 0), broadcast(), 16);
    let mut events = ();
    let mut run = pin!(service.run(clock, &mut events));
    let cx = &mut Context::from_waker(Waker::noop());
    assert_eq!(poll(&mut run, cx), Poll::Ready(EndCause::PortContract));
    assert_eq!(
        script.calls_from(2),
        [Call::Ended(tag(0, 0), Outcome::Unknown)]
    );
}

/// The port refuses on its own after a declining fill: the port's fault,
/// and the work ends by the fill's recorded cause.
#[test]
fn a_refusal_after_a_declining_fill_ends_the_work_declined_and_the_service() {
    brand!(Net);
    let Bench {
        mut service,
        line,
        script,
        clock,
        ..
    } = direct::<Net>();
    script.0.borrow_mut().declines = true;
    line.0.borrow_mut().replies.push_back(Reply::Misreport);
    script.push(tag(0, 0), broadcast(), 16);
    let mut events = ();
    let mut run = pin!(service.run(clock, &mut events));
    let cx = &mut Context::from_waker(Waker::noop());
    assert_eq!(poll(&mut run, cx), Poll::Ready(EndCause::PortContract));
    assert_eq!(
        script.calls_from(2),
        [Call::Ended(tag(0, 0), Outcome::Declined)]
    );
}

/// `Accepted` and `Unknown` in one turn, then the service is dropped: the
/// producer heard both before the end flag was set, and the work still open
/// never reached the port.
#[test]
fn a_dropped_service_has_delivered_every_known_outcome() {
    brand!(Net);
    let Bench {
        mut service,
        line,
        script,
        end,
        clock,
        ..
    } = bench::<Net, Direct, 8>(Direct);
    line.0
        .borrow_mut()
        .replies
        .extend([Reply::Take, Reply::Lazy]);
    for index in 0..3 {
        script.push(tag(0, index), broadcast(), 16);
    }
    {
        let mut events = ();
        let mut run = pin!(service.run(clock, &mut events));
        let cx = &mut Context::from_waker(Waker::noop());
        assert_eq!(poll(&mut run, cx), Poll::Ready(EndCause::PortContract));
    }
    assert!(!end.is_ended());
    let calls = script.calls();
    drop(service);
    assert!(end.is_ended(), "the guard set the flag");
    assert_eq!(script.calls(), calls, "the drop called nothing");
    assert!(calls.contains(&Call::Ended(tag(0, 0), Outcome::Accepted)));
    assert!(calls.contains(&Call::Ended(tag(0, 1), Outcome::Unknown)));
    assert!(!calls.contains(&Call::Filled(tag(0, 2))));
    assert_eq!(line.publishes(), 2, "the open work never reached the port");
    assert_eq!(
        script.0.borrow().ended_after_end,
        [false, false],
        "both before the flag"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| matches!(call, Call::Offered(tag) if tag.index == 1))
            .count(),
        1,
        "no second declaration after Unknown"
    );
}

#[test]
fn a_cancelled_run_keeps_declared_work_and_a_new_run_fills_each_once() {
    brand!(Net);
    let Bench {
        mut service,
        line,
        script,
        clock,
        ..
    } = direct::<Net>();
    line.open(true);
    script.push(tag(0, 0), broadcast(), 16);
    script.push(tag(0, 1), broadcast(), 16);
    let mut events = ();
    let (count, waker) = counter();
    {
        let mut run = pin!(service.run(clock, &mut events));
        settle(&mut run, &count, &waker);
    }
    assert_eq!(script.calls().len(), 2, "no end from the cancel");
    line.open(false);
    let mut run = pin!(service.run(clock, &mut events));
    settle(&mut run, &count, &waker);
    assert_eq!(
        script.calls_from(2),
        [
            Call::Filled(tag(0, 0)),
            Call::Ended(tag(0, 0), Outcome::Accepted),
            Call::Filled(tag(0, 1)),
            Call::Ended(tag(0, 1), Outcome::Accepted),
        ]
    );
}

#[test]
fn a_retirement_ends_each_open_work_retired_and_returns_the_producer() {
    brand!(Net);
    let Bench {
        mut service,
        line,
        script,
        end,
        stop,
        clock,
        ..
    } = direct::<Net>();
    line.open(true);
    script.push(tag(0, 0), broadcast(), 16);
    script.push(tag(0, 1), broadcast(), 16);
    let mut events = ();
    let (count, waker) = counter();
    {
        let mut run = pin!(service.run(clock, &mut events));
        settle(&mut run, &count, &waker);
        stop.try_send(()).ok().unwrap();
        let cx = &mut Context::from_waker(&waker);
        assert_eq!(poll(&mut run, cx), Poll::Ready(EndCause::Stopped));
    }
    assert_eq!(script.calls().len(), 2, "nothing yet");
    let ended = service.retire();
    assert!(end.is_ended());
    assert_eq!(
        script.calls_from(2),
        [
            Call::Ended(tag(0, 0), Outcome::Retired),
            Call::Ended(tag(0, 1), Outcome::Retired),
        ]
    );
    assert_eq!(script.0.borrow().ended_after_end, [false, false]);
    assert!(Rc::ptr_eq(&ended.producers.0, &script.0), "returned");
    assert_eq!(line.sent(), 0, "nothing published");
}

#[test]
fn a_service_dropped_unrun_sets_the_end_flag_and_calls_nothing() {
    brand!(Net);
    let Bench {
        service,
        script,
        end,
        ..
    } = direct::<Net>();
    script.push(tag(0, 0), broadcast(), 16);
    drop(service);
    assert!(end.is_ended());
    assert!(script.calls().is_empty());
}

/// The time alone wakes the service: no other task looks.
#[test]
fn a_producer_s_next_time_alone_wakes_the_service() {
    brand!(Net);
    let Bench {
        mut service,
        line,
        script,
        clock,
        ..
    } = direct::<Net>();
    let due = clock.now().saturating_add(Duration::from_millis(5));
    script.0.borrow_mut().due = Some((
        due,
        Item {
            tag: tag(0, 0),
            remote: broadcast(),
            data: data_of(tag(0, 0), 16),
        },
    ));
    let mut events = ();
    let mut run = pin!(service.run(clock, &mut events));
    let (count, waker) = counter();
    settle(&mut run, &count, &waker);
    assert_eq!(clock.next_deadline(), Some(due), "the wait names the time");
    clock.advance_to(due);
    settle(&mut run, &count, &waker);
    assert_eq!(line.produced(), [data_of(tag(0, 0), 16)]);
}

#[derive(Default)]
struct Drops(Vec<NetEvent>);

impl EventSink<NetEvent> for Drops {
    fn event(&mut self, event: NetEvent) {
        self.0.push(event);
    }
}

impl EventSink<ServiceEvent> for Drops {
    const ENABLED: bool = false;
    fn event(&mut self, _: ServiceEvent) {}
}

/// Input for the producer's port is dropped before a block is taken.
#[test]
fn a_producer_s_port_takes_no_block_on_input() {
    brand!(Net);
    let Bench {
        mut service,
        line,
        clock,
        ..
    } = direct::<Net>();
    line.0
        .borrow_mut()
        .incoming
        .push_back(udp_frame(LOCAL_PORT, b"no"));
    let mut events = Drops::default();
    {
        let mut run = pin!(service.run(clock, &mut events));
        let cx = &mut Context::from_waker(Waker::noop());
        for _ in 0..4 {
            assert!(poll(&mut run, cx).is_pending());
        }
    }
    assert_eq!(events.0, [NetEvent::SendOnly]);
    let ended = service.retire();
    assert_eq!(ended.retired.pool.free(), BLOCKS);
}

#[test]
fn a_producer_binds_once_and_a_refusal_changes_nothing() {
    brand!(Net);
    let Bench { mut service, .. } = direct::<Net>();
    assert_eq!(
        service.bind_local(LOCAL_PORT + 1, Dscp::default()),
        Err(LocalRefused::Bound)
    );
}

/// A producer whose next offer cannot be declared registers for changes
/// only: with its open works at the turn's budget it is asked nothing, and
/// new data alone is no reason to wake. With room again it registers for
/// offers.
#[test]
fn a_producer_at_its_open_budget_waits_for_changes_only() {
    brand!(Net);
    let Bench {
        mut service,
        line,
        script,
        clock,
        ..
    } = bench::<Net, Direct, 16>(Direct);
    line.open(true);
    let mut events = ();
    let mut run = pin!(service.run(clock, &mut events));
    let (count, waker) = counter();
    settle(&mut run, &count, &waker);
    assert_eq!(
        script.interest(),
        Some(Interest::Offers),
        "nothing to offer yet"
    );
    for index in 0..9 {
        script.push(tag(0, index), broadcast(), 16);
    }
    script.wake();
    settle(&mut run, &count, &waker);
    let offered = |script: &Script| {
        script
            .calls()
            .iter()
            .filter(|call| matches!(call, Call::Offered(_)))
            .count()
    };
    assert_eq!(offered(&script), 8, "eight open works at most");
    assert_eq!(script.interest(), Some(Interest::Changes));
    line.open(false);
    settle(&mut run, &count, &waker);
    assert_eq!(offered(&script), 9, "the ninth once places came back");
    assert_eq!(line.produced().len(), 9);
    assert_eq!(script.interest(), Some(Interest::Offers));
}

/// What the service told a sink of its producer.
#[derive(Default)]
struct Steps(Vec<crate::LocalEvent>);

impl EventSink<ServiceEvent> for Steps {
    fn event(&mut self, event: ServiceEvent) {
        if let ServiceEvent::Local(local) = event {
            self.0.push(local);
        }
    }
}

impl EventSink<NetEvent> for Steps {
    const ENABLED: bool = false;
    fn event(&mut self, _: NetEvent) {}
}

/// A sink that counts hears each of the producer's steps as it happens: an
/// offer, a final refusal, a hold, a withdrawal, a declining fill, and what
/// each wait registered the producer for.
#[test]
fn a_counting_sink_hears_each_of_the_producer_s_steps() {
    use crate::LocalEvent::{Declined, Held, Offered, Refused, Waits, Withdrawn};
    brand!(Net);
    let Bench {
        mut service,
        line,
        script,
        clock,
        ..
    } = direct::<Net>();
    line.open(true);
    script.push(tag(0, 9), broadcast(), 2000);
    for index in 0..3 {
        script.push(tag(0, index), broadcast(), 16);
    }
    let mut steps = Steps::default();
    let (count, waker) = counter();
    {
        let mut run = pin!(service.run(clock, &mut steps));
        settle(&mut run, &count, &waker);
    }
    assert_eq!(
        core::mem::take(&mut steps.0),
        [
            Offered,
            Refused,
            Offered,
            Offered,
            Offered,
            Held,
            Waits(Interest::Changes)
        ]
    );
    {
        let mut source = script.0.borrow_mut();
        source.generation = 1;
        source.held.clear();
        source.ready.clear();
    }
    script.push(tag(1, 0), broadcast(), 16);
    {
        let mut run = pin!(service.run(clock, &mut steps));
        settle(&mut run, &count, &waker);
    }
    assert_eq!(
        core::mem::take(&mut steps.0),
        [Withdrawn, Withdrawn, Offered, Waits(Interest::Offers)]
    );
    script.0.borrow_mut().declines = true;
    line.open(false);
    {
        let mut run = pin!(service.run(clock, &mut steps));
        settle(&mut run, &count, &waker);
    }
    assert_eq!(steps.0.first(), Some(&Declined));
}

#[test]
fn a_producer_held_without_an_address_offers_once_the_lease_comes_and_the_watch_tells() {
    brand!(Net);
    let Bench {
        mut service,
        line,
        script,
        clock,
        ..
    } = bench_on::<Net, _, 2>(Direct, Interface::dhcp(LOCAL, 1500, 7).unwrap());
    let watch: &'static ConfigurationWatch = Box::leak(Box::new(ConfigurationWatch::new()));
    service.watch(watch).unwrap();
    assert_eq!(service.watch(watch), Err(WatchRefused::Taken));
    let first = watch.changes();
    assert_eq!(watch.read().unwrap().configuration.unwrap().ipv4, None);
    let mut events = ();
    {
        let mut run = pin!(service.run(clock, &mut events));
        let (count, waker) = counter();
        settle(&mut run, &count, &waker);
        let sent = dhcp_messages(&line.0.borrow().sent);
        assert_eq!(sent.len(), 1);
        let (kind, xid) = sent[0];
        assert_eq!(kind, MessageType::Discover);
        // The producer's offer meets no address and holds.
        let a = tag(0, 0);
        script.push(a, peer(), 16);
        script.wake();
        settle(&mut run, &count, &waker);
        assert_eq!(
            script.calls(),
            [Call::Offered(a), Call::Refused(a, SendRefusal::NoAddress)]
        );
        script.wake();
        settle(&mut run, &count, &waker);
        assert_eq!(script.calls().len(), 2, "the hold stays without an address");
        // OFFER, REQUEST, ACK.
        line.0
            .borrow_mut()
            .incoming
            .push_back(dhcp_reply(MessageType::Offer, xid, 600));
        line.open(false);
        settle(&mut run, &count, &waker);
        assert_eq!(
            dhcp_messages(&line.0.borrow().sent).last(),
            Some(&(MessageType::Request, xid))
        );
        line.0
            .borrow_mut()
            .incoming
            .push_back(dhcp_reply(MessageType::Ack, xid, 600));
        line.0.borrow_mut().incoming.push_back(peer_asks());
        line.open(false);
        settle(&mut run, &count, &waker);
        // The watch has the address, published at the turn's end.
        assert_ne!(watch.changes(), first);
        let configuration = watch.read().unwrap().configuration.unwrap();
        assert_eq!(configuration.ipv4.unwrap().address(), OUR_IP);
        assert_eq!(configuration.lease.unwrap().server, PEER_IP);
        // The address version released the hold: the offer went.
        script.wake();
        settle(&mut run, &count, &waker);
        assert_eq!(
            script.calls_from(2),
            [
                Call::Offered(a),
                Call::Filled(a),
                Call::Ended(a, Outcome::Accepted)
            ]
        );
        // T1 is half of 600 s from the REQUEST.
        let lease = watch.read().unwrap().configuration.unwrap().lease.unwrap();
        assert_eq!(lease.obtained, clock.now());
        assert_eq!(
            lease.renew,
            clock.now().saturating_add(Duration::from_secs(300))
        );
    }
    let ended = service.retire();
    assert_eq!(
        watch.read().unwrap().configuration,
        None,
        "the lease left with the core"
    );
    drop(ended);
}

/// The peer's echo request to us.
fn peer_pings() -> Vec<u8> {
    use oer_protocols_network_wire::icmp::{Echo, MessageType as Icmp, PROTOCOL_ICMP};
    use oer_protocols_network_wire::ip::{DsField, Ipv4Fields};
    let request = Echo {
        kind: Icmp::Echo,
        code: 0,
        checksum: 0,
        identifier: 1,
        sequence: 1,
        data: b"ping",
    };
    let mut message = request.header(Icmp::Echo).to_vec();
    message.extend_from_slice(b"ping");
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
            total_length: u16::try_from(20 + message.len()).unwrap(),
            identification: 1,
            time_to_live: 64,
            protocol: PROTOCOL_ICMP,
            source: PEER_IP,
            destination: OUR_IP,
        }
        .to_bytes(),
    );
    frame.extend_from_slice(&message);
    frame
}

#[test]
fn an_echo_reply_s_end_releases_a_neighbors_full_hold() {
    brand!(Net);
    let Bench {
        mut service,
        mut socket,
        line,
        script,
        clock,
        ..
    } = direct::<Net>();
    // The peer is known, and its echo reply waits for a full port: the
    // reply holds its entry, a channel datagram holds the other.
    line.0.borrow_mut().incoming.push_back(peer_asks());
    line.0.borrow_mut().incoming.push_back(peer_pings());
    line.open(true);
    let mut events = ();
    let mut run = pin!(service.run(clock, &mut events));
    let (count, waker) = counter();
    let _asking = channel_send(&mut socket, &mut run, host(7), b"asking");
    settle(&mut run, &count, &waker);
    let a = tag(0, 0);
    script.push(a, host(9), 16);
    script.wake();
    settle(&mut run, &count, &waker);
    assert_eq!(
        script.calls(),
        [
            Call::Offered(a),
            Call::Refused(a, SendRefusal::NeighborsFull)
        ]
    );
    // The reply goes: its entry is free, and the hold gives way.
    line.open(false);
    settle(&mut run, &count, &waker);
    let replies = line
        .0
        .borrow()
        .sent
        .iter()
        .filter(|frame| frame.get(23) == Some(&1))
        .count();
    assert_eq!(replies, 1, "the echo reply went");
    assert_eq!(script.calls_from(2), [Call::Offered(a)], "asked again");
}

#[test]
fn dropping_the_service_ends_the_watch_and_wakes_its_reader() {
    brand!(Net);
    let Bench {
        mut service, clock, ..
    } = direct::<Net>();
    let watch: &'static ConfigurationWatch = Box::leak(Box::new(ConfigurationWatch::new()));
    service.watch(watch).unwrap();
    assert!(watch.read().unwrap().configuration.unwrap().ipv4.is_some());
    let seen = watch.changes();
    let (count, waker) = counter();
    let cx = &mut Context::from_waker(&waker);
    let mut changed = pin!(watch.changed(seen));
    assert!(changed.as_mut().poll(cx).is_pending());
    // A cancelled run keeps the service, and the watch with it.
    {
        let mut events = ();
        let mut run = pin!(service.run(clock, &mut events));
        assert!(run.as_mut().poll(cx).is_pending());
    }
    assert!(watch.read().unwrap().configuration.is_some());
    // The run's first turn published the link it saw: that wait is over, and
    // the reader waits again.
    assert!(changed.as_mut().poll(cx).is_ready());
    let seen = watch.changes();
    let mut changed = pin!(watch.changed(seen));
    assert!(changed.as_mut().poll(cx).is_pending());
    let woken = count.0.load(core::sync::atomic::Ordering::SeqCst);
    drop(service);
    assert_eq!(watch.read().unwrap().configuration, None);
    assert!(
        count.0.load(core::sync::atomic::Ordering::SeqCst) > woken,
        "the reader was woken"
    );
    assert!(changed.as_mut().poll(cx).is_ready());
}

/// A declining fill keeps the producer's flow out of the rest of its turn
/// only: its next work goes in the next turn, which nothing but the service
/// itself starts.
#[test]
fn after_a_declining_fill_the_producer_s_next_work_goes_unwoken() {
    brand!(Net);
    let Bench {
        mut service,
        line,
        script,
        clock,
        ..
    } = direct::<Net>();
    let mut events = ();
    let mut run = pin!(service.run(clock, &mut events));
    let (count, waker) = counter();
    script.0.borrow_mut().declining.push(0);
    script.push(tag(0, 0), broadcast(), 16);
    script.push(tag(0, 1), broadcast(), 16);
    settle(&mut run, &count, &waker);
    assert_eq!(
        line.produced(),
        [data_of(tag(0, 1), 16)],
        "the next work went"
    );
}

/// The owner's case: the producer is held on `NeighborsFull`, then starts a
/// new generation with a broadcast ready, and a block comes home by the
/// return lane at the same moment. The pass that takes the return also sees
/// the producer's change, by its own reading: the broadcast goes without
/// any other wake.
#[test]
fn a_producer_s_change_that_comes_with_a_return_is_not_lost_to_it() {
    brand!(Net);
    let Bench {
        mut service,
        mut socket,
        script,
        line,
        clock,
        ..
    } = direct::<Net>();
    let lane = Box::leak(Box::new(crate::ReturnLane::new()));
    let (returner, end) = lane.split();
    assert!(service.attach_returns(end).is_ok());
    let mut events = ();
    let mut run = pin!(service.run(clock, &mut events));
    let (count, waker) = counter();
    let _seven = channel_send(&mut socket, &mut run, host(7), b"seven");
    let _eight = channel_send(&mut socket, &mut run, host(8), b"eight");
    let spare = blocks(&mut socket, &mut run, 1).pop().unwrap();
    settle(&mut run, &count, &waker);
    let held = tag(0, 0);
    script.push(held, host(9), 16);
    script.wake();
    settle(&mut run, &count, &waker);
    assert_eq!(
        script.calls(),
        [
            Call::Offered(held),
            Call::Refused(held, SendRefusal::NeighborsFull)
        ]
    );
    let next = tag(1, 0);
    script.forget(held);
    script.0.borrow_mut().generation = 1;
    script.push(next, broadcast(), 16);
    script.wake();
    returner.give(spare).ok().unwrap();
    settle(&mut run, &count, &waker);
    assert_eq!(line.produced(), [data_of(next, 16)], "the broadcast went");
}

/// A time already due when the wait begins ends it at its first poll as a
/// ready source would: the useful turn before it is not followed by another
/// in the same poll.
#[test]
fn a_time_due_at_the_wait_still_leaves_one_useful_turn_a_poll() {
    brand!(Net);
    let Bench {
        mut service,
        mut socket,
        line,
        script,
        clock,
        ..
    } = direct::<Net>();
    let mut events = ();
    let mut run = pin!(service.run(clock, &mut events));
    let cx = &mut Context::from_waker(Waker::noop());
    let mut block = blocks(&mut socket, &mut run, 1).pop().unwrap();
    block.fill(b"first").unwrap();
    let due = clock.now().saturating_add(Duration::from_millis(5));
    script.0.borrow_mut().due = Some((
        due,
        Item {
            tag: tag(0, 0),
            remote: broadcast(),
            data: data_of(tag(0, 0), 16),
        },
    ));
    // Publishing the channel's datagram takes the clock to the producer's
    // time: the wait after that turn finds it already due.
    line.0.borrow_mut().elapse = Some((clock, due));
    drop(socket.send(broadcast(), block).ok().unwrap());
    assert!(poll(&mut run, cx).is_pending());
    assert_eq!(line.channel(), [b"first".to_vec()]);
    assert!(line.produced().is_empty(), "one useful turn in this poll");
    assert!(poll(&mut run, cx).is_pending());
    assert_eq!(line.produced(), [data_of(tag(0, 0), 16)], "the next poll's");
}

/// With no data the producer offers nothing and the turns collect nothing;
/// its first offer after them is collected and goes.
#[test]
fn a_producer_s_offer_after_turns_with_nothing_ready_is_collected_and_goes() {
    brand!(Net);
    let policy = Policy::default();
    let Bench {
        mut service,
        line,
        script,
        clock,
        ..
    } = bench::<Net, Policy, 2>(policy.clone());
    let mut events = ();
    let mut run = pin!(service.run(clock, &mut events));
    let (count, waker) = counter();
    settle(&mut run, &count, &waker);
    {
        let account = policy.0.borrow();
        assert_eq!(account.plans, 0, "nothing was collected");
        assert!(
            account.nothing_ready > 0,
            "the policy heard each empty turn"
        );
    }
    script.push(tag(0, 0), broadcast(), 16);
    script.wake();
    settle(&mut run, &count, &waker);
    assert_eq!(line.produced(), [data_of(tag(0, 0), 16)]);
    assert!(policy.0.borrow().plans > 0, "the offer was collected");
}

/// Every service event, in order, where the test reads it while the service
/// runs.
#[derive(Clone, Default)]
struct Tape(Rc<RefCell<Vec<ServiceEvent>>>);

impl EventSink<ServiceEvent> for Tape {
    fn event(&mut self, event: ServiceEvent) {
        self.0.borrow_mut().push(event);
    }
}

impl EventSink<NetEvent> for Tape {
    fn event(&mut self, _: NetEvent) {}
}

/// Each turn `events` tell: whether it took only the frames, and the
/// requests it took.
fn turn_kinds(events: &[ServiceEvent]) -> Vec<(bool, usize)> {
    let mut turns = Vec::new();
    for event in events {
        match event {
            ServiceEvent::TurnBegan => turns.push((false, 0)),
            ServiceEvent::PacketTurn => {
                if let Some(turn) = turns.last_mut() {
                    turn.0 = true;
                }
            }
            ServiceEvent::Took(_) => {
                if let Some(turn) = turns.last_mut() {
                    turn.1 += 1;
                }
            }
            _ => {}
        }
    }
    turns
}

/// Frames that are always ready, with no control work pending, move in
/// packet turns only: after the first turn, which found the frames, no full
/// turn comes until control work does.
#[test]
fn frames_always_ready_take_packet_turns_until_control_work_comes() {
    brand!(Net);
    let Bench {
        mut service,
        script,
        clock,
        line,
        ..
    } = direct::<Net>();
    for index in 0..200 {
        script.push(tag(0, index), broadcast(), 40);
    }
    let tape = Tape::default();
    let mut events = tape.clone();
    {
        let mut run = pin!(service.run(clock, &mut events));
        let cx = &mut Context::from_waker(Waker::noop());
        while line.sent() < 200 {
            assert!(poll(&mut run, cx).is_pending());
        }
    }
    let turns = turn_kinds(&tape.0.borrow());
    let (first, rest) = turns.split_first().unwrap();
    assert!(!first.0, "the first turn is whole");
    let packets = rest.iter().take_while(|turn| turn.0).count();
    assert!(packets > 1, "frames always ready take packet turns");
    let last = rest.iter().rposition(|turn| turn.0).unwrap();
    assert!(
        rest[..last].iter().all(|turn| turn.0),
        "no full turn without control work while frames are ready"
    );
    assert_eq!(script.filled(), 200, "every frame went, each once");
}

/// A socket's command while frames are always ready takes the very next
/// turn whole: control work pending ends the packet turns at once.
#[test]
fn a_command_while_frames_are_always_ready_takes_the_next_turn_whole() {
    brand!(Net);
    let Bench {
        mut service,
        mut socket,
        script,
        clock,
        ..
    } = direct::<Net>();
    for index in 0..200 {
        script.push(tag(0, index), broadcast(), 40);
    }
    let tape = Tape::default();
    let mut events = tape.clone();
    let cx = &mut Context::from_waker(Waker::noop());
    let mut run = pin!(service.run(clock, &mut events));
    // Into the packet turns.
    let mut polls = 0;
    while !tape.0.borrow().contains(&ServiceEvent::PacketTurn) {
        assert!(poll(&mut run, cx).is_pending());
        polls += 1;
        assert!(polls < 64, "frames always ready reach a packet turn");
    }
    let mut reserve = pin!(socket.reserve());
    assert!(
        reserve.as_mut().poll(cx).is_pending(),
        "the request waits for a turn"
    );
    let before = turn_kinds(&tape.0.borrow()).len();
    assert!(poll(&mut run, cx).is_pending());
    let turns = turn_kinds(&tape.0.borrow());
    let next = turns.get(before).copied();
    assert_eq!(
        next,
        Some((false, 1)),
        "the next turn is whole and takes the request"
    );
}

/// A socket owed a block is control work: while it waits, every turn is
/// whole, so the grant of a block freed meanwhile, or with none free the
/// eviction, comes at the next turn though frames are always ready.
#[test]
fn a_socket_owed_a_block_keeps_every_turn_whole() {
    brand!(Net);
    let Bench {
        mut service,
        mut socket,
        script,
        clock,
        line,
        ..
    } = direct::<Net>();
    let tape = Tape::default();
    let mut events = tape.clone();
    let cx = &mut Context::from_waker(Waker::noop());
    let mut run = pin!(service.run(clock, &mut events));
    // Every block the socket's: the next reserve waits.
    let held = blocks(&mut socket, &mut run, BLOCKS);
    let mut reserve = pin!(socket.reserve());
    assert!(reserve.as_mut().poll(cx).is_pending(), "no block is free");
    for index in 0..64 {
        script.push(tag(0, index), broadcast(), 40);
    }
    let before = turn_kinds(&tape.0.borrow()).len();
    while line.sent() < 64 {
        assert!(poll(&mut run, cx).is_pending());
    }
    let turns = turn_kinds(&tape.0.borrow());
    assert!(
        turns[before..].iter().all(|turn| !turn.0),
        "no packet turn while the socket is owed a block"
    );
    drop(held);
}
