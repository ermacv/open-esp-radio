//! Mixed progress over the real service, device port, TX owner and slot
//! queues. The script names return/RX interleavings; it models no RF time.
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "host assertions on placed test owners"
)]

use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::task::{Context, Poll, Wake, Waker};

use oer_portable_events::EventSink;
use oer_portable_memory::{SlotReceiver, SlotSender, WakingSlotQueue, WakingSpscQueue};
use oer_portable_time::Instant;
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_ieee80211_runtime::station::{
    Control, LocalTx, LocalTxState, Outgoing, PoolRefused, SameHart, ServiceCommand, State,
    TxEnded, TxPool,
};
use oer_protocols_network_core::{
    Declined, Interface, NetEvent, NetworkOwner, Outcome, Payload, Remote, SendRefusal, claim,
};
use oer_protocols_network_port::{
    BatchEnd, FrameStorage, Incoming, LentFrame, LinkSnapshot, LinkState, NetDevice, ReceivedBatch,
    StorageAdvanceRefused, TxStorage,
};
use oer_protocols_network_runtime::{
    DevicePort, Interest, LendingLanes, LocalEnd, Next, Offer, Produce, Producer, PublishEnd,
    Service, ServiceEvent, StopLane, Work,
};
use oer_protocols_network_wire::ip::{DsField, Dscp, Ipv4Fields};
use oer_protocols_network_wire::transport::{PROTOCOL_UDP, Ports, UdpHeader};

const SIZE: usize = 128;
const SLOTS: usize = 8;
const DEPTH: usize = 16;
const LANE: usize = 32;
const CREDIT: usize = 32;
const RX_CREDIT: usize = 24;
const LOCAL: [u8; 6] = [2, 0, 0, 0, 0, 1];
const PEER: [u8; 6] = [2, 0, 0, 0, 0, 2];
const ADDRESS: [u8; 4] = [192, 0, 2, 1];
const PORT: u16 = 5001;
const UP: LinkSnapshot = LinkSnapshot {
    state: LinkState::Up,
    epoch: 1,
};

struct Buffer {
    place: u8,
    bytes: [u8; SIZE],
}
impl TxStorage for Buffer {
    fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }
}
struct Pool;
impl TxPool for Pool {
    type Slot = Buffer;
    type Memory = Buffer;
    fn place(slot: &Buffer) -> u8 {
        slot.place
    }
    fn publish(&mut self, slot: Buffer, length: usize) -> Result<Buffer, PoolRefused<Buffer>> {
        if length <= SIZE {
            Ok(slot)
        } else {
            Err(PoolRefused::Range(slot))
        }
    }
    fn slot(&mut self, memory: Buffer) -> Buffer {
        memory
    }
}

struct Bytes {
    bytes: Vec<u8>,
    id: usize,
    returned: Arc<Mutex<Vec<usize>>>,
}
impl oer_portable_memory::BatchReturn for Bytes {
    type Batch = ();
}
impl Drop for Bytes {
    fn drop(&mut self) {
        self.returned.lock().unwrap().push(self.id);
    }
}
impl FrameStorage for Bytes {
    fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }
    fn advance(&mut self, by: usize) -> Result<(), StorageAdvanceRefused> {
        if by > self.bytes.len() {
            return Err(StorageAdvanceRefused {
                by,
                length: self.bytes.len(),
            });
        }
        self.bytes.drain(..by);
        Ok(())
    }
}
#[derive(Default)]
struct Rx {
    frames: VecDeque<LentFrame<Bytes>>,
    waker: Option<Waker>,
    next: usize,
    read: usize,
    lent: Vec<usize>,
    published: usize,
    held: bool,
    returned: Arc<Mutex<Vec<usize>>>,
}
#[derive(Clone, Default)]
struct Device(Rc<RefCell<Rx>>);
impl Device {
    fn receive_one(&self) {
        self.stage_one();
        self.flush();
    }
    fn stage_one(&self) {
        let mut bytes = Ipv4Fields {
            services: DsField::from_bytes([0]),
            total_length: 20 + 8 + 1,
            identification: 1,
            time_to_live: 64,
            protocol: PROTOCOL_UDP,
            source: [192, 0, 2, 2],
            destination: ADDRESS,
        }
        .to_bytes()
        .to_vec();
        bytes.extend_from_slice(
            &UdpHeader {
                ports: Ports {
                    source: 7,
                    destination: PORT,
                },
                length: 9,
                checksum: 0,
            }
            .to_bytes(),
        );
        let (id, returned) = {
            let mut rx = self.0.borrow_mut();
            let id = rx.next;
            rx.next += 1;
            (id, rx.returned.clone())
        };
        bytes.push(id as u8);
        let length = bytes.len();
        let frame = LentFrame::new(
            Bytes {
                bytes,
                id,
                returned,
            },
            LOCAL,
            PEER,
            0x0800,
            0..length,
        )
        .ok()
        .unwrap();
        let mut rx = self.0.borrow_mut();
        rx.frames.push_back(frame);
        rx.held = true;
    }
    fn expose(&self) {
        let mut rx = self.0.borrow_mut();
        rx.published = rx.frames.len();
    }
    fn flush(&self) {
        let mut rx = self.0.borrow_mut();
        let changed = rx.held;
        rx.held = false;
        rx.published = rx.frames.len();
        if changed && let Some(waker) = rx.waker.take() {
            waker.wake();
        }
    }
}
impl NetDevice for Device {
    type Storage = Bytes;
    fn link(&self) -> LinkSnapshot {
        UP
    }
    fn poll_link(&mut self, _: &mut Context<'_>) -> Poll<LinkSnapshot> {
        Poll::Pending
    }
    fn receive_queue_len(&self) -> usize {
        self.0.borrow().published
    }
    fn poll_receive_ready(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let mut rx = self.0.borrow_mut();
        rx.waker = Some(cx.waker().clone());
        if rx.published == 0 {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
    fn receive(
        &mut self,
        limit: usize,
        mut take: impl FnMut(Incoming<'_, LentFrame<Bytes>>),
    ) -> ReceivedBatch {
        let mut taken = 0;
        while taken < limit {
            if self.0.borrow().published == 0 {
                return ReceivedBatch {
                    taken,
                    end: BatchEnd::Drained,
                };
            }
            let Some(frame) = self.0.borrow_mut().frames.pop_front() else {
                return ReceivedBatch {
                    taken,
                    end: BatchEnd::Drained,
                };
            };
            let id = {
                let mut rx = self.0.borrow_mut();
                rx.published -= 1;
                let id = rx.read;
                rx.read += 1;
                id
            };
            let mut owner = Some(frame);
            take(Incoming::Lent(&mut owner));
            if owner.is_none() {
                self.0.borrow_mut().lent.push(id);
            }
            drop(owner);
            taken += 1;
        }
        ReceivedBatch {
            taken,
            end: BatchEnd::Limit,
        }
    }
}

/// Finite unpaced source, with more data than the native TX owner can hold.
struct Source {
    next: u64,
}
impl Produce for Source {
    type Tag = u64;
    fn generation(&self) -> u32 {
        0
    }
    fn current(&self, _: u64) -> bool {
        true
    }
    fn offer(&mut self, _: Instant) -> Result<Offer<u64>, Next> {
        if self.next == 128 {
            return Err(Next::Idle);
        }
        let tag = self.next;
        self.next += 1;
        Ok(Offer {
            remote: Remote {
                address: [192, 0, 2, 255],
                port: 7000,
            },
            length: 16,
            tag,
        })
    }
    fn fill(
        &mut self,
        _: Instant,
        work: Work<u64>,
        data: &mut Payload<'_>,
    ) -> Result<(), Declined> {
        data.repeat(work.tag as u8, 16)
            .map_err(|_| Declined::Unwritable)
    }
    fn refused(&mut self, _: u64, reason: SendRefusal) {
        panic!("unexpected declaration refusal: {reason:?}");
    }
    fn ended(&mut self, _: Work<u64>, outcome: Outcome) {
        assert!(matches!(outcome, Outcome::Accepted | Outcome::Retired));
    }
    fn register(&mut self, _: &mut Context<'_>, _: Interest) -> bool {
        self.next < 128
    }
}

type Tx = LocalTx<'static, Pool, SLOTS, DEPTH, SameHart>;
type Port = DevicePort<Device, Tx>;
type Net<B, D = ()> = Service<
    'static,
    'static,
    B,
    SIZE,
    2,
    CREDIT,
    8,
    2,
    4,
    DEPTH,
    LANE,
    Port,
    Producer<'static, B, Source, CREDIT>,
    D,
>;
type Socket<B> = oer_protocols_network_runtime::LendingSocket<
    'static,
    'static,
    B,
    SIZE,
    DEPTH,
    LANE,
    CREDIT,
    LentFrame<Bytes>,
>;
struct Bench<B: 'static, D = ()> {
    service: Net<B, D>,
    socket: Socket<B>,
    device: Device,
    outgoing: SlotReceiver<'static, Option<Outgoing<Buffer>>, DEPTH>,
    returned: SlotSender<'static, Option<TxEnded<Buffer>>, DEPTH>,
}
fn bench<B: 'static>() -> Bench<B> {
    bench_credited::<B, RX_CREDIT, _>(())
}
fn bench_credited<B: 'static, const RX: usize, D: oer_protocols_network_runtime::Diagnostics>(
    diagnostics: D,
) -> Bench<B, D> {
    let state = Box::leak(Box::new(State::new()));
    let commands = Box::leak(Box::new(WakingSpscQueue::<ServiceCommand<()>, 1>::new()));
    let (command, _requests) = commands.split();
    let (control, runner) = Control::new(command, state).unwrap();
    runner.publish_link_for_test(UP);
    let _lifecycle = Box::leak(Box::new((control, runner)));
    let tx = Box::leak(Box::new(WakingSlotQueue::new()));
    let back = Box::leak(Box::new(WakingSlotQueue::new()));
    let (tx, outgoing) = tx.split(|| None);
    let (returned, back) = back.split(|| None);
    let free = (0..SLOTS)
        .map(|place| Buffer {
            place: place as u8,
            bytes: [0; SIZE],
        })
        .collect();
    let transmit = LocalTx::new(
        tx,
        back,
        Pool,
        Box::leak(Box::new(LocalTxState::new())),
        free,
        state.network_link(),
    );
    let device = Device::default();
    let (mut pool, brand) = claim::<B, SIZE, 4>(Box::leak(Box::new([[0; SIZE]; 4]))).unwrap();
    pool.loans().pool(RX + 2, 2).unwrap();
    let owner = NetworkOwner::new(
        brand,
        Interface::new(LOCAL, ADDRESS, 24, None, 1500).unwrap(),
        pool,
        DevicePort::new(device.clone(), transmit),
    );
    let (_stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut service = Service::with_producer(
        owner,
        oer_protocols_network_runtime::MemoryPolicy::KEEP,
        stopping,
        Source { next: 0 },
        Box::leak(Box::new(LocalEnd::new())),
    )
    .with_diagnostics(diagnostics);
    service.bind_local(PORT + 1, Dscp::default()).unwrap();
    let group = service.group(RX).unwrap();
    let socket = service
        .bind_lending::<RX>(
            PORT,
            Dscp::default(),
            group,
            Box::leak(Box::new(LendingLanes::new())),
        )
        .ok()
        .unwrap();
    Bench {
        service,
        socket,
        device,
        outgoing,
        returned,
    }
}

type Hook = Rc<RefCell<Option<Box<dyn FnOnce()>>>>;

#[derive(Clone, Default)]
struct Tape(Rc<RefCell<Vec<ServiceEvent>>>, Hook);
impl EventSink<ServiceEvent> for Tape {
    fn event(&mut self, event: ServiceEvent) {
        self.0.borrow_mut().push(event);
        if event == (ServiceEvent::Waited { ready: true }) {
            let hook = self.1.borrow_mut().take();
            if let Some(hook) = hook {
                hook();
            }
        }
    }
}
impl EventSink<NetEvent> for Tape {
    fn event(&mut self, event: NetEvent) {
        panic!("unexpected RX event: {event:?}");
    }
}
struct Count(AtomicUsize);
impl Wake for Count {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
fn step(run: &mut Pin<Box<impl Future>>, count: &Count, waker: &Waker) {
    assert_ne!(
        count.0.swap(0, Ordering::SeqCst),
        0,
        "the executor polls only after a wake"
    );
    assert!(
        run.as_mut()
            .poll(&mut Context::from_waker(waker))
            .is_pending()
    );
}
fn settle(run: &mut Pin<Box<impl Future>>, count: &Count, waker: &Waker) {
    for _ in 0..32 {
        if count.0.load(Ordering::SeqCst) == 0 {
            return;
        }
        step(run, count, waker);
    }
    panic!("the service did not wait after the port filled");
}
fn return_one(
    sender: &mut SlotSender<'_, Option<TxEnded<Buffer>>, DEPTH>,
    flights: &mut Vec<Buffer>,
) {
    let mut memory = Some(TxEnded {
        memory: flights.pop().unwrap(),
    });
    sender
        .try_send_with(|slot| {
            *slot = memory.take();
            Ok::<_, ()>(())
        })
        .unwrap();
    assert!(memory.is_none());
}
/// Exactly the four events counted by the image's "empty" statistic,
/// with the producer and refusal work that statistic does not include.
#[derive(Debug, Default)]
struct Summary {
    waited: bool,
    yielded: bool,
    receive: bool,
    frames: usize,
    accepted: usize,
    notices: usize,
    commands: usize,
    offers: usize,
    published: Option<PublishEnd>,
}
impl Summary {
    fn uncounted(&self) -> bool {
        self.frames + self.accepted + self.notices + self.commands == 0
    }
}
fn summaries(events: &[ServiceEvent]) -> Vec<Summary> {
    let mut result = Vec::new();
    let mut turn = Summary::default();
    for event in events {
        match *event {
            ServiceEvent::TurnBegan => {}
            ServiceEvent::Waited { .. } => turn.waited = true,
            ServiceEvent::Yielded => turn.yielded = true,
            ServiceEvent::ReceiveTurn => turn.receive = true,
            ServiceEvent::FrameTaken => turn.frames += 1,
            ServiceEvent::Accepted => turn.accepted += 1,
            ServiceEvent::Noticed => turn.notices += 1,
            ServiceEvent::Took(_) => turn.commands += 1,
            ServiceEvent::Local(oer_protocols_network_runtime::LocalEvent::Offered) => {
                turn.offers += 1
            }
            ServiceEvent::Published(end) => turn.published = Some(end),
            ServiceEvent::TurnEnded(_) => result.push(std::mem::take(&mut turn)),
            _ => {}
        }
    }
    result
}

#[derive(Clone, Copy, Debug)]
enum Moment {
    WithRx,
    AfterReadiness,
    AfterRxTurn,
}

fn progress<B: 'static>(moment: Moment, received: usize) {
    let Bench {
        mut service,
        mut socket,
        device,
        mut outgoing,
        returned,
    } = bench::<B>();
    let clock: VirtualClock = VirtualClock::new();
    let tape = Tape::default();
    let mut sink = tape.clone();
    let mut run = Box::pin(service.run(&clock, &mut sink));
    let count = Arc::new(Count(AtomicUsize::new(1)));
    let waker = Waker::from(count.clone());
    settle(&mut run, &count, &waker);
    let mut flights = Vec::new();
    while let Ok(Some(frame)) = outgoing.try_receive_with(Option::take) {
        flights.push(frame.memory);
    }
    assert_eq!(flights.len(), SLOTS);
    assert_eq!(count.0.load(Ordering::SeqCst), 0);
    let returned = Rc::new(RefCell::new(returned));
    let mark = tape.0.borrow().len();
    if matches!(moment, Moment::AfterReadiness) {
        let returned = returned.clone();
        let mut memory = Some(TxEnded {
            memory: flights.pop().unwrap(),
        });
        *tape.1.borrow_mut() = Some(Box::new(move || {
            returned
                .borrow_mut()
                .try_send_with(|slot| {
                    *slot = memory.take();
                    Ok::<_, ()>(())
                })
                .unwrap();
            assert!(memory.is_none());
        }));
    }
    for _ in 0..received {
        device.receive_one();
    }
    if matches!(moment, Moment::WithRx) {
        return_one(&mut returned.borrow_mut(), &mut flights);
    }
    step(&mut run, &count, &waker);
    if matches!(moment, Moment::AfterRxTurn) {
        // Finish the consumer's scheduling opportunity, then return TX
        // memory after the network has actually registered its wait.
        settle(&mut run, &count, &waker);
        assert_eq!(
            count.0.load(Ordering::SeqCst),
            0,
            "the service is asleep before TX memory returns"
        );
        return_one(&mut returned.borrow_mut(), &mut flights);
    }
    settle(&mut run, &count, &waker);
    let after = tape.0.borrow()[mark..].to_vec();
    assert_eq!(
        after
            .iter()
            .filter(|e| **e == ServiceEvent::FrameTaken)
            .count(),
        received
    );
    assert_eq!(
        after
            .iter()
            .filter(|e| **e == ServiceEvent::Accepted)
            .count(),
        1,
        "one returned buffer publishes now, without a full batch: {after:?}"
    );
    let receive = after.contains(&ServiceEvent::ReceiveTurn);
    assert_eq!(
        receive,
        !matches!(moment, Moment::WithRx),
        "TX ready at registration forbids receive-only: {after:?}"
    );
    let turns = summaries(&after);
    let first_tx = turns.iter().position(|turn| turn.accepted != 0).unwrap();
    if matches!(moment, Moment::WithRx) {
        assert_eq!(first_tx, 0, "TX ready with RX is served immediately");
    } else {
        assert_eq!(first_tx, 1, "only one captured RX batch may precede TX");
        assert!(turns[0].receive);
        assert_eq!(turns[0].frames, received.min(8));
        assert!(!turns[first_tx].receive);
    }
    if received > 16 {
        assert!(
            turns[..=first_tx]
                .iter()
                .map(|turn| turn.frames)
                .sum::<usize>()
                < received,
            "TX is served before the RX queue drains: {turns:?}"
        );
    }
    for turn in turns.iter().filter(|turn| turn.uncounted()) {
        assert_eq!(turn.published, Some(PublishEnd::Refused), "{turn:?}");
        assert!(
            turn.yielded && !turn.waited,
            "the preceding acceptance lets the producer refill without a new wake: {turn:?}"
        );
        assert!(
            turn.offers > 0,
            "the no-room turn declared producer work: {turn:?}"
        );
    }
    println!("{moment:?}, RX={received}: {turns:?}");
    let before_returns = tape.0.borrow().len();
    for _ in 0..received {
        drop(socket.try_receive().unwrap());
    }
    settle(&mut run, &count, &waker);
    let returned_events = tape.0.borrow()[before_returns..].to_vec();
    assert!(returned_events.contains(&ServiceEvent::Returned { work: false }));
    assert!(
        !returned_events.contains(&ServiceEvent::TurnBegan),
        "RX credit returns alone need no turn"
    );
    drop(run);
    while let Ok(Some(frame)) = outgoing.try_receive_with(Option::take) {
        flights.push(frame.memory);
    }
    assert_eq!(flights.len(), SLOTS);
    while !flights.is_empty() {
        return_one(&mut returned.borrow_mut(), &mut flights);
    }
    let mut ended = service.retire();
    ended.retired.port.give_back().unwrap();
    ended.collect();
    assert!(ended.retired.pool.loans().released());
}
#[test]
fn tx_return_with_rx_is_served_in_the_same_regular_turn() {
    struct Brand;
    progress::<Brand>(Moment::WithRx, 1);
}
#[test]
fn tx_return_after_readiness_is_served_after_one_receive_turn() {
    struct Brand;
    progress::<Brand>(Moment::AfterReadiness, 1);
}
#[test]
fn tx_return_after_receive_turn_needs_no_new_rx_or_producer_wake() {
    struct Brand;
    progress::<Brand>(Moment::AfterRxTurn, 1);
}

#[test]
fn tx_return_is_served_before_the_receive_queue_drains() {
    struct Brand;
    progress::<Brand>(Moment::AfterReadiness, 17);
}

#[path = "mixed_service/credit.rs"]
mod credit;
