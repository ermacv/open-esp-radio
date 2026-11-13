//! One joined host path: a socket, the network core, the station's network
//! ends, the datapath actor, the real S31 port, device, receive session and
//! DMA owners, over the backend's model of the hardware atoms, for one open
//! connection on virtual time.
//!
//! The test plays the lifecycle (as the actor's bench does), the application,
//! the air and the peer. A task is polled only once woken, and with nothing
//! woken time moves to the next deadline: a lost wake ends a case. A hardware
//! event runs the interrupt handler at once, between two polls of a task or,
//! where a case asks, as the actor's turn ends ([`TURN_ENDS`]). The handler's
//! signals pend their executor, as the chip's do: before each round of polls
//! the bench services them, as the executor's handler does, so a fact a task
//! took before that round wakes no one.
//!
//! What stands in for the product here: the lifecycle (no scan, join or
//! keys: an open link handed over). The network's transmit end is the
//! application's own: the station's [`crate::supply::LocalEnd`].
//!
//! The numbers a case reports are observations, not limits.
#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    reason = "a test's own checks"
)]

extern crate std;

use core::{
    cell::{Cell, RefCell},
    future::Future,
    marker::PhantomData,
    mem::MaybeUninit,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};
use std::{boxed::Box, println, rc::Rc, sync::Arc, task::Wake, vec::Vec};

use embassy_futures::select::{Either, select};
use oer_chips_esp32s31_hal::radio::ieee80211::dma::{
    rx_dma::model::{RxWalkerLanding, RxWalkerModel},
    rx_storage::RxDmaStorage,
    space::HostDma,
};
use oer_chips_esp32s31_hal::system::memory::dma_static::Placed;
use oer_chips_esp32s31_radio_ieee80211_backend::{
    hardware_model::{
        AirFormat, AirFrame, AtomCounts, Atoms, ModelDoorbell, ModelFrontier, ModelLine,
        ModelPower, ModelRegisters, ModelRx, RxPrefix, leak,
    },
    irq::{HostActivation, WifiIrqLine},
    lower_mac::{ConnectionFramePriorities, LowerMacConfig, RadioCoexPriorities},
    mac::rx::pool::RxStagePool,
    mac::tx::{
        TxSlot as DescriptorSlot,
        ampdu::{HtAmpduTxStorage, RetainedAmpduDmaStorage, RetainedDmaAmpduTx},
        runtime::WifiTxRuntimePolicy,
        test_support::model_pool,
    },
    ordinary_tx::OrdinaryTxOwner,
    port::{
        self, Esp32s31Caps, Esp32s31Port, LentReceive, PUBLICATION_BOUND, PortDriver, PortParts,
        PortSharedStorage, TxDescriptors, TxPool,
        device::{Composition, RadioDevice, RadioParts, StagedReceive},
    },
    rx::arena::{WifiIrqSignals, WifiRxArena, WifiRxStatic},
    tx::{CappedTxPowerProfile, WifiTxResources},
};
use oer_chips_family_platform_interrupt_table::signal::host::{Deferred, deferred_signal};
use oer_chips_family_radio_ieee80211_policy::rate_selection::EspressifRateSelection;
use oer_portable_events::EventSink;
use oer_portable_memory::{
    ClosingSpscQueue, ClosingSpscReceiver, ClosingSpscSender, HandlerCell, WakingSlotQueue,
    WakingSpscQueue, WakingSpscSender,
};
use oer_portable_sync::PriorityWaker;
use oer_portable_time::{Clock, Duration, Instant, Timer};
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_ieee80211_contracts_port::{
    Channel, ChannelWidth, ConnectionFrame, LowerMacTerminal, PortCommand, ReceiveFilter,
    RegulatedChannel, SnrSample, StationReceive, SupportedChannel, SupportedFilter,
};
use oer_protocols_ieee80211_core_roles_sta::{
    data::plane::DataLink,
    events::StaEvent,
    link::{LinkHandover, LinkReturn},
};
use oer_protocols_ieee80211_core_upper_mac::{block_ack::RxBlockAckEvent, qos::AcAdmission};
use oer_protocols_ieee80211_runtime::{
    datapath::{
        Actor, ActorEnd, ActorEpochPath, ActorEvent, ActorLink, ActorSpan, DUE_REPOSTS,
        DatapathPort, FromActor, GroupPending, LIFECYCLE_FRAMES, LifecycleEnds, NetworkEnd,
        NetworkEnds, PARCELS, Parcel, Returned, StopOutcome, ToActor, from_actor_depth,
        returns_depth,
    },
    station::{self as runtime, RxBlockAckStorage, ServiceEvent},
};
use oer_protocols_ieee80211_wire::{
    address::{Bssid, IndividualAddress, OwnAddress},
    channel::Ghz2_4Channel,
    security::LinkProtection,
    station::StaDataHeaderTemplate,
};
use oer_protocols_network_core::{Datagram, Group, Interface, NetworkOwner, Outcome, Remote};
use oer_protocols_network_port::{
    EthernetFrameView, FramePort, FrameSource, Holding, Keep, LinkSnapshot, LinkState,
    OwnedTransmit, PortRefusal, Taken, Unpublished,
};
use oer_protocols_network_runtime::{
    DevicePort, EndCause, Ended, MemoryPolicy, Observer, Sent, Service, Socket, SocketLanes,
    StopLane,
};
use oer_protocols_network_wire::{
    arp::{self, Arp, Operation},
    checksum::{Checksum, ipv4_pseudo_header, udp_field},
    ethernet::EtherType,
    ip::{DsField, Dscp, Ipv4Fields, Ipv4Header},
    transport::{PROTOCOL_UDP, Ports, UdpHeader},
};
use oer_protocols_radio_port::{Installations, LocalId, Minted, SlotIdentity, with_local_slot};
use oer_protocols_radio_regulatory::{Country, PowerLimits, RegulatoryDomain};

use crate::{
    ActorConnection, Channels, Connection, FRAME_CAPACITY, Network, NetworkChannels,
    NetworkStopState, NetworkStopper, Notifies, RestartState, ServiceConnection, StartState,
    StopState, TX_DEPTH,
    supply::{NETWORK_BUFFERS, NetworkTx, PORT_BUFFERS, TxMemory},
};

mod access_point;

/// The product's ring and frame geometry (`chip.rs`).
const N: usize = 96;
const B: usize = FRAME_CAPACITY;
const S: usize = B;
const Q: usize = 32;
const TX: usize = FRAME_CAPACITY;
/// The product's network geometry (`app/src/wifi/network.rs`), but its blocks.
const SOCKETS: usize = 1;
const BLOCK: usize = 1472;
const MTU: u16 = 1500;
const BLOCKS: usize = 16;
const CREDITS: usize = 8;
const RECEIVED: usize = 8;
const LANE: usize = 8;
const DEPTH: usize = 8;
const NEIGHBORS: usize = 4;

const LIMITS: PowerLimits = PowerLimits {
    antenna_gain_mbm: 0,
    chip_maximum_mbm: 2000,
    owner_cap_mbm: None,
};
const STATION: [u8; 6] = [0x02, 0, 0, 0x12, 0x34, 0x56];
const ACCESS_POINT: [u8; 6] = [0x02, 0, 0, 0x12, 0x34, 0x57];
const BSSID: [u8; 6] = [0x30, 0x05, 0x5c, 0x11, 0x22, 0x33];
const PEER: [u8; 6] = [0x02, 0, 0, 0, 0, 0x09];
const STATION_IP: [u8; 4] = [192, 0, 2, 1];
const PEER_IP: [u8; 4] = [192, 0, 2, 2];
const STATION_PORT: u16 = 5000;
const PEER_PORT: u16 = 7;

/// Passes of a case before it counts as spinning.
const PASSES: usize = 100_000;
/// What a poll of a task costs, in microseconds of the model's time, where a
/// case asks for a cost: an input, and no case's progress may rest on it. By
/// default a poll takes no time, so only armed timers move the clock.
const POLL_MICROS: u64 = 2;
/// Units lent to a network that has not run, and more than its turn takes.
const LENT_UNREAD: usize = 1;
const LENT_PAST_A_TURN: usize = 10;
/// A stop's bound, past what a publication and its abort may take.
const STOP_BOUND: Duration = Duration::from_secs(10);

/// The hart notifies of a station whose receive buffers are the real ones.
struct Joined;

impl Notifies for Joined {
    type Channels = runtime::SameHart;
    type LentRx = Lent;
}

type Lent = LentReceive<StagedReceive<Q, B>, port::SameHart>;

struct JoinedComposition;

impl Composition for JoinedComposition {
    type Coex = Coex;
    type Entropy = fn() -> u32;
    type Body = core::convert::Infallible;
    type OwnedContext<Id: SlotIdentity> = ();
    type Context<'p, Id: SlotIdentity + 'p> = ();
    type OwnedConfigureContext<Id: SlotIdentity> = ();
    type ConfigureContext<'p, Id: SlotIdentity + 'p> = ();
}

struct Coex;

impl RadioCoexPriorities for Coex {
    fn beacon_window_pti(&self) -> oer_chips_esp32s31_hal::types::MacPti {
        oer_chips_esp32s31_hal::types::MacPti::new(9).unwrap()
    }
    fn connection_frame_priorities(&self, _: ConnectionFrame) -> Option<ConnectionFramePriorities> {
        None
    }
}

/// The clock the joined tasks read, counting its reads. A wait wakes its task
/// at its deadline and no other's, as the executors' timers do.
#[derive(Clone)]
struct Clocked {
    clock: Rc<VirtualClock>,
    reads: Rc<Cell<u32>>,
    sleepers: Rc<RefCell<Vec<(Instant, Waker)>>>,
}

impl Clocked {
    fn new() -> Self {
        Self {
            clock: Rc::new(VirtualClock::new()),
            reads: Rc::new(Cell::new(0)),
            sleepers: Rc::default(),
        }
    }

    /// Move time on by `micros`, waking the waits that came due.
    fn pass(&self, micros: u64) {
        self.clock
            .advance(Duration::from_micros(micros))
            .expect("the model's time fits");
        self.wake_due();
    }

    /// Move time to the next deadline: whether there is one.
    fn pass_to_next(&self) -> bool {
        let next = self.sleepers.borrow().iter().map(|(at, _)| *at).min();
        let Some(next) = next else {
            return false;
        };
        self.clock.advance_to(next);
        self.wake_due();
        true
    }

    fn wake_due(&self) {
        let now = self.clock.now();
        let due: Vec<_> = {
            let mut sleepers = self.sleepers.borrow_mut();
            let (due, later) = sleepers.drain(..).partition(|(at, _)| *at <= now);
            *sleepers = later;
            due
        };
        for (_, waker) in due {
            waker.wake();
        }
    }
}

impl Clock for Clocked {
    fn now(&self) -> Instant {
        self.reads.set(self.reads.get() + 1);
        self.clock.now()
    }
}

impl Timer for Clocked {
    fn wait_until(&self, deadline: Instant) -> impl Future<Output = ()> {
        let clocked = self.clone();
        core::future::poll_fn(move |cx| {
            if clocked.clock.now() >= deadline {
                return Poll::Ready(());
            }
            let mut sleepers = clocked.sleepers.borrow_mut();
            let known = sleepers
                .iter()
                .any(|(at, waker)| *at == deadline && waker.will_wake(cx.waker()));
            if !known {
                sleepers.push((deadline, cx.waker().clone()));
            }
            Poll::Pending
        })
    }
}

type Device = RadioDevice<
    JoinedComposition,
    Clocked,
    ModelFrontier,
    HostActivation,
    ModelDoorbell<Deferred>,
    N,
    B,
    S,
    Q,
    Deferred,
>;
type Types<'s> = port::Types<LocalId<'s>, Device, TX, ()>;
type Caps = Esp32s31Caps<TX>;

/// The real port as the actor's.
struct Datapath<'s>(PhantomData<fn(&'s ()) -> &'s ()>);

impl<'s> DatapathPort<'s> for Datapath<'s> {
    type Port = Esp32s31Port<'s, LocalId<'s>, Device, TX, ()>;
    type Caps = Caps;
    type Types = Types<'s>;
    type Small = port::BufferClass<TX>;
    type TxMemory = TxMemory;
    type LentRx = Lent;
    /// The station's share (`radio.rs`), less the lifecycle's frames.
    const RX_HELD: u8 = (port::RX_HELD - LIFECYCLE_FRAMES) as u8;

    fn arena_ack(response: port::BlockAckAnswer) -> Option<port::Ack> {
        response.snr_db.map(|snr_db| port::Ack {
            snr: SnrSample::Measured(snr_db),
        })
    }

    fn span(span: ActorSpan, end: bool) {
        if span == ActorSpan::Turn && end {
            let edge = TURN_ENDS.with_borrow_mut(std::collections::VecDeque::pop_front);
            if let Some(edge) = edge {
                edge();
            }
        }
    }
}

std::thread_local! {
    /// What the air does as each of the actor's next turns ends, after the
    /// turn's last pull and before the facts the actor registers on for its
    /// next one: the edge preempts the task there, as the radio handler
    /// preempts it on the chip. One per turn, in order.
    static TURN_ENDS: RefCell<std::collections::VecDeque<Box<dyn FnOnce()>>> =
        const { RefCell::new(std::collections::VecDeque::new()) };
}

type P<'s> = Datapath<'s>;

/// The actor's events go unobserved.
struct Quiet;

impl EventSink<ActorEvent> for Quiet {
    const ENABLED: bool = false;
    fn event(&mut self, _: ActorEvent) {}
}
impl EventSink<ServiceEvent> for Quiet {
    const ENABLED: bool = false;
    fn event(&mut self, _: ServiceEvent) {}
}
impl EventSink<StaEvent> for Quiet {
    const ENABLED: bool = false;
    fn event(&mut self, _: StaEvent) {}
}
impl EventSink<RxBlockAckEvent> for Quiet {
    const ENABLED: bool = false;
    fn event(&mut self, _: RxBlockAckEvent) {}
}

const TO: usize = 24;
const RETURNS: usize = returns_depth(port::TERMINAL_CAPACITY);
const FROM: usize = from_actor_depth(RETURNS);

/// The network's transmit end, held beside the port so that a test can
/// still take what the station gives back after the network ended.
#[derive(Clone)]
struct Transmit(Rc<RefCell<crate::supply::LocalTransmit<'static, Joined>>>);

impl OwnedTransmit for Transmit {
    type Ticket = u8;
    type Notice = core::convert::Infallible;
    fn publish<E>(
        &mut self,
        meta: oer_protocols_network_port::FrameMeta,
        fill: impl FnOnce(&mut oer_protocols_network_port::Slot<'_>) -> Result<(), E>,
    ) -> Result<u8, Unpublished<PortRefusal, E>> {
        self.0.borrow_mut().publish(meta, fill)
    }
    fn take_notice(&mut self) -> Option<Self::Notice> {
        self.0.borrow_mut().take_notice()
    }
    fn notice_waiting(&self) -> bool {
        self.0.borrow().notice_waiting()
    }
    fn poll_room(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.0.borrow_mut().poll_room(cx)
    }
    fn flush(&mut self) {
        self.0.borrow_mut().flush();
    }
    fn reclaim(&mut self) {
        self.0.borrow_mut().reclaim();
    }
}

type Inner = DevicePort<Network<'static, Joined>, Transmit>;

/// The bytes the network moved at its port: a test's count, not the product's.
#[derive(Clone, Copy, Debug, Default)]
struct Moved {
    tx_frames: u32,
    tx_bytes: u32,
    rx_frames: u32,
    rx_bytes: u32,
}

/// The service's port, counting what crosses it.
struct Counted {
    /// The device and its transmit half, until the network's end.
    inner: Option<Inner>,
    moved: Rc<Cell<Moved>>,
}

impl FramePort for Counted {
    type Refusal = PortRefusal;
    type Ticket = ();
    fn publish<E>(
        &mut self,
        meta: oer_protocols_network_port::FrameMeta,
        fill: impl FnOnce(&mut oer_protocols_network_port::Slot<'_>) -> Result<(), E>,
    ) -> Result<(), Unpublished<PortRefusal, E>> {
        let length = meta.length;
        self.inner
            .as_mut()
            .ok_or(Unpublished::Port(PortRefusal::LinkDown))?
            .publish(meta, fill)?;
        let mut moved = self.moved.get();
        moved.tx_frames += 1;
        moved.tx_bytes += u32::try_from(length).unwrap();
        self.moved.set(moved);
        Ok(())
    }
    fn is_final(refusal: &PortRefusal) -> bool {
        Inner::is_final(refusal)
    }
    fn flush(&mut self) {
        if let Some(inner) = &mut self.inner {
            FramePort::flush(inner);
        }
    }
}

impl FrameSource for Counted {
    type Notice = ();
    type Held = <Inner as FrameSource>::Held;
    fn take_frame(
        &mut self,
        read: impl FnOnce(EthernetFrameView<'_>, Holding) -> Keep,
    ) -> Taken<Self::Held> {
        let moved = &self.moved;
        self.inner.as_mut().map_or(Taken::Empty, |inner| {
            inner.take_frame(|view, holding| {
                let mut count = moved.get();
                count.rx_frames += 1;
                count.rx_bytes += u32::try_from(view.payload.len()).unwrap();
                moved.set(count);
                read(view, holding)
            })
        })
    }
    fn take_notice(&mut self) -> Option<()> {
        // The direct egress settles nothing: an end only frees its buffer.
        self.inner
            .as_mut()
            .and_then(FrameSource::take_notice)
            .map(|_| ())
    }
    fn link(&self) -> LinkSnapshot {
        self.inner
            .as_ref()
            .map_or(LinkSnapshot::DOWN, FrameSource::link)
    }
    fn register(&mut self, cx: &mut Context<'_>) -> bool {
        self.inner.as_mut().is_some_and(|inner| inner.register(cx))
    }
}

type NetService<T> = Service<
    'static,
    'static,
    T,
    BLOCK,
    SOCKETS,
    CREDITS,
    RECEIVED,
    NEIGHBORS,
    BLOCKS,
    DEPTH,
    LANE,
    Counted,
>;
type NetSocket<T> = Socket<'static, 'static, T, BLOCK, DEPTH, LANE, CREDITS>;
type NetDatagram<T> = Datagram<'static, T, BLOCK>;
type NetEnded<T> =
    Ended<'static, 'static, T, BLOCK, SOCKETS, CREDITS, BLOCKS, DEPTH, LANE, Counted>;

/// A task's "poll me", counting its wakes.
#[derive(Default)]
struct Woken {
    woken: std::sync::atomic::AtomicBool,
    wakes: std::sync::atomic::AtomicU32,
}

impl Wake for Woken {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        use std::sync::atomic::Ordering;
        self.woken.store(true, Ordering::SeqCst);
        self.wakes.fetch_add(1, Ordering::SeqCst);
    }
}

/// One task of the virtual executor.
struct Task<'t> {
    future: Pin<&'t mut dyn Future<Output = ()>>,
    woken: Arc<Woken>,
    waker: Waker,
    polls: u32,
    done: bool,
}

impl<'t> Task<'t> {
    fn new(future: Pin<&'t mut dyn Future<Output = ()>>) -> Self {
        let woken = Arc::new(Woken::default());
        woken.woken.store(true, std::sync::atomic::Ordering::SeqCst);
        let waker = Waker::from(woken.clone());
        Self {
            future,
            woken,
            waker,
            polls: 0,
            done: false,
        }
    }

    /// Poll once if woken: whether it ran.
    fn run(&mut self) -> bool {
        use std::sync::atomic::Ordering;
        if self.done || !self.woken.woken.swap(false, Ordering::SeqCst) {
            return false;
        }
        self.polls += 1;
        let mut cx = Context::from_waker(&self.waker);
        if self.future.as_mut().poll(&mut cx).is_ready() {
            self.done = true;
        }
        true
    }

    fn wakes(&self) -> u32 {
        self.woken.wakes.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// What the network's task and the actor's left behind.
struct Ends<T: 'static> {
    service: Option<NetEnded<T>>,
    /// The cause the network's task saw.
    cause: Option<EndCause>,
    actor: Option<ActorEndSeen>,
    /// The device came back from the retired port, and its frontier.
    released: Option<bool>,
    frontier: Option<ModelFrontier>,
}

impl<T: 'static> Default for Ends<T> {
    fn default() -> Self {
        Self {
            service: None,
            cause: None,
            actor: None,
            released: None,
            frontier: None,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum ActorEndSeen {
    Retired,
    Reinstall,
    Faulted,
}

/// How the air treats a started PPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Air {
    /// It ends delivered at once.
    Delivers,
    /// It stays on the air until a test ends it.
    Holds,
}

/// One joined stack, the test playing everything around it.
struct Bench<'b, 's, T: 'static> {
    clock: Clocked,
    atoms: Atoms,
    line: Rc<ModelLine<Deferred>>,
    rx: ModelRx<N, B, S>,
    walker: &'static RefCell<RxWalkerModel>,
    storage: &'static RxDmaStorage<N, B, S, HostDma>,
    stage: &'static RxStagePool<Q, B>,
    actor: Task<'b>,
    service: Task<'b>,
    to_actor: ClosingSpscSender<'b, ToActor<'s, P<'s>>, TO>,
    from_actor: ClosingSpscReceiver<'b, FromActor<'s, P<'s>>, FROM>,
    returns: ClosingSpscReceiver<'b, Returned<'s, P<'s>>, RETURNS>,
    link: oer_portable_memory::ClosingSpscSender<'b, ActorLink<'s, P<'s>>, 1>,
    socket: Option<NetSocket<T>>,
    runner: runtime::RunnerControl<'static, runtime::SameHart>,
    transmit: Transmit,
    moved: Rc<Cell<Moved>>,
    ends: Rc<RefCell<Ends<T>>>,
    air: Air,
    /// The PPDUs the air saw, in order.
    seen: Vec<AirFrame>,
    /// The Sequence Number of the access point's next frame.
    sequence: u16,
    /// The network's task gets no poll: what the station hands it waits.
    network_paused: bool,
    /// The radio's start answered.
    started: bool,
    /// How the stop ended, once it answered.
    stopped: Option<StopOutcome<oer_chips_esp32s31_radio_ieee80211_backend::port::Fault>>,
    /// The actor ended on a broken contract.
    faulted: bool,
    /// Terminals of the lifecycle's commands.
    configured: u32,
    /// The lifecycle reads no return: it counts their announcements.
    hold_returns: bool,
    /// Returns announced and not read.
    unread_returns: usize,
    /// Commands the actor returned unrun.
    commands_back: usize,
    /// What a poll costs of the model's time.
    poll_micros: u64,
    /// From asking the radio to start until it had.
    start_took: Duration,
}

fn channel() -> Ghz2_4Channel {
    Ghz2_4Channel::new(6, ChannelWidth::Mhz20).unwrap()
}

fn domain() -> RegulatoryDomain {
    RegulatoryDomain::of(Country::new(*b"DE").unwrap()).unwrap()
}

fn supported() -> SupportedChannel<Caps> {
    SupportedChannel::new(
        domain()
            .permit(RegulatedChannel::from(
                Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
            ))
            .unwrap(),
    )
    .unwrap()
}

fn entropy() -> u32 {
    0x1234_5678
}

fn descriptors() -> TxDescriptors<TX> {
    let descriptor = || {
        Pin::static_mut(leak(
            DescriptorSlot::new_model_descriptor().expect("model descriptor"),
        ))
    };
    let chain = || {
        RetainedDmaAmpduTx::new_model(
            Pin::static_mut(leak(HtAmpduTxStorage::new())),
            leak(RetainedAmpduDmaStorage::new()),
        )
        .ok()
        .expect("model chain")
    };
    TxDescriptors {
        single: core::array::from_fn(|_| descriptor()),
        chains: core::array::from_fn(|_| chain()),
    }
}

/// The network's task, as the application's: the service until a stop, then
/// its port's end, which the station's stop hears of last.
async fn serve_network<T: 'static>(
    mut service: NetService<T>,
    stop: WakingSpscSender<'static, (), 1>,
    stopper: NetworkStopper<'static, Joined>,
    clock: Clocked,
    ends: Rc<RefCell<Ends<T>>>,
) {
    let mut events = ();
    let cause = {
        let mut run = pin!(service.run(&clock, &mut events));
        match select(run.as_mut(), stopper.requested()).await {
            Either::First(cause) => cause,
            Either::Second(()) => {
                let _sent = stop.try_send(());
                run.await
            }
        }
    };
    let mut ended = service.retire();
    // The queue outlives the port, so its frames are given back first, as the
    // application's stop does.
    if let Some(port) = ended.retired.port.inner.as_mut() {
        port.give_back();
    }
    ended.retired.port.inner = None;
    {
        let mut ends = ends.borrow_mut();
        ends.cause = Some(cause);
        ends.service = Some(ended);
    }
    stopper.stopped();
}

/// Run `test` on a joined stack with its link up. `T` names its network: a
/// type claims one.
fn joined<T: 'static>(test: impl for<'b, 's> FnOnce(&mut Bench<'b, 's, T>)) {
    joined_at::<T>(0, test);
}

/// [`joined`], each poll costing `poll_micros` of the model's time.
fn joined_at<T: 'static>(poll_micros: u64, test: impl for<'b, 's> FnOnce(&mut Bench<'b, 's, T>)) {
    joined_with::<T>(poll_micros, Setup::Connected, test);
}

/// How far the bench sets the station up before the test.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Setup {
    /// Started, configured and connected.
    Connected,
    /// The lifecycle reads none of its returns: before the start, commands
    /// the stopped port refuses fill the return queue; then the radio
    /// starts and the station's configuration is asked, whose terminal the
    /// full queue refuses and the port keeps. Not connected.
    ReturnsFull,
}

fn joined_with<T: 'static>(
    poll_micros: u64,
    setup: Setup,
    test: impl for<'b, 's> FnOnce(&mut Bench<'b, 's, T>),
) {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let clock = Clocked::new();
        let reader = clock.clock.clone();
        let atoms = Atoms::new(Rc::new(move || reader.now().as_micros()));

        // The receive arena, as the facade's `install` binds it.
        let arena: &'static mut WifiRxStatic<N, B, S, Q, Deferred, HostDma> =
            leak(bytemuck::Zeroable::zeroed());
        let (rx, _stop, _frames) = WifiRxArena::bind(
            Placed::host(arena),
            WifiIrqSignals {
                mac: deferred_signal().0,
                receive: deferred_signal().0,
                power: deferred_signal().0,
            },
        )
        .ok()
        .expect("a zeroed static binds");
        let storage = rx.rx();
        let requests = rx.requests();
        let stage = rx.stage();
        let walker: &'static RefCell<RxWalkerModel> = leak(RefCell::new(RxWalkerModel::new()));
        let cell: &'static HandlerCell<WifiIrqLine<Deferred>> = leak(HandlerCell::new());
        let line = Rc::new(ModelLine::new(cell, requests, atoms.clone()));

        let device: &'static mut Device = leak(RadioDevice::new(RadioParts {
            wifi: ModelFrontier::new(ModelRegisters::new(atoms.clone(), walker), channel()),
            rx,
            interrupt_cell: cell,
            bound: HostActivation,
            actor: (),
            timer: clock.clone(),
            doorbell: ModelDoorbell(line.clone()),
            domain: domain(),
            power_limits: LIMITS,
            #[cfg(oer_trace)]
            stall: leak(
                oer_chips_esp32s31_radio_ieee80211_backend::rx::counters::stall::RxStall::new(),
            ),
        }));

        // The port, as the facade's `install` builds its parts.
        let pool: &'static TxPool<TX> = model_pool().expect("model pool");
        let ceiling = domain()
            .permit(RegulatedChannel::from(
                Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
            ))
            .unwrap()
            .tx_ceiling(LIMITS);
        let transmitter = OrdinaryTxOwner::new(WifiTxResources {
            slot: Pin::static_mut(leak(
                DescriptorSlot::new_model_descriptor().expect("model descriptor"),
            )),
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: CappedTxPowerProfile::new(ModelPower, ceiling),
            entropy: entropy as fn() -> u32,
            timer: clock.clone(),
        })
        .into_idle()
        .ok()
        .expect("idle transmitter");
        let parts = PortParts {
            device,
            shared: leak(PortSharedStorage::new()),
            transmitter,
            pool,
            bank: PORT_BUFFERS,
            descriptors: descriptors(),
            config: LowerMacConfig {
                station_address: STATION,
                channel: channel(),
                publication_timeout: PUBLICATION_BOUND,
                tsf_epoch: 1,
            },
            access_point_address: IndividualAddress::new(ACCESS_POINT).unwrap(),
            coex: leak(Coex),
            observations: (),
        };
        let mut installations = Installations::<_, port::InstallationPools>::claim(claim);
        let minted: Minted<_, port::InstallationPools> = installations.first().unwrap();
        let driver = Box::leak(Box::new(PortDriver::vacant()));
        let installation = Esp32s31Port::install(parts, minted, driver)
            .ok()
            .expect("install");
        let port::PortInstallation {
            port: data_port,
            commands: _commands,
            resources,
        } = installation;
        let (block_acks, tokens) = resources.into_station();
        // This lifecycle sends no frame of its own: its buffers go home.
        let port::StationTokens {
            tickets,
            buffers,
            station_commands,
            ..
        } = tokens;
        drop(buffers);

        // The station's channels, as the facade's `station` splits them.
        let network_tx = NetworkTx::claim(pool, crate::supply::PORT_BUFFERS)
            .ok()
            .expect("network buffers");
        let channels: &'static mut Channels<Joined, TxMemory> = leak(Channels {
            start: StartState::new(),
            restarts: RestartState::new(),
            stop: StopState::new(),
            commands: WakingSpscQueue::new(),
            network: NetworkChannels {
                network_stop: NetworkStopState::with(PriorityWaker::new(), PriorityWaker::new()),
                tx: WakingSlotQueue::new(),
                returned: WakingSlotQueue::new(),
                rx: WakingSpscQueue::new(),
                rx_copied: WakingSpscQueue::new(),
                state: runtime::State::new(),
            },
        });
        let (control, network, transmit, stopper, connection) = channels
            .split(|queues| {
                <crate::supply::LocalEnd as crate::supply::NetworkTxEnd<Joined>>::end(
                    queues,
                    network_tx,
                    leak(crate::supply::LocalTxState::new()),
                )
            })
            .ok()
            .expect("the channels split");
        let Connection {
            service:
                ServiceConnection {
                    runner,
                    start: _start,
                    restarts: _restarts,
                    commands: _service_commands,
                },
            actor:
                ActorConnection {
                    network_stop,
                    transmit: transmit_end,
                    returned,
                    receive,
                    receive_copied,
                },
            stop: _stop_guard,
        } = connection;

        // The actor and its lifecycle's queues.
        let block_ack_storage = Box::leak(Box::new(RxBlockAckStorage::<
            Types<'_>,
            8,
            { port::RX_HELD },
        >::new(
            block_acks, Duration::from_millis(100)
        )));
        let mut actor = Actor::<P<'_>, _, _>::init_in_place(
            Box::leak(Box::new(MaybeUninit::uninit())),
            data_port,
            EspressifRateSelection::<Caps>::new(),
            block_ack_storage.view(),
            Quiet,
        );
        let mut to_queue = ClosingSpscQueue::<ToActor<'_, P<'_>>, TO>::new();
        let mut parcel_queue = ClosingSpscQueue::<Parcel<'_, P<'_>>, PARCELS>::new();
        let mut from_queue = ClosingSpscQueue::<FromActor<'_, P<'_>>, FROM>::new();
        let mut return_queue = ClosingSpscQueue::<Returned<'_, P<'_>>, RETURNS>::new();
        let mut link_queue = ClosingSpscQueue::<ActorLink<'_, P<'_>>, 1>::new();
        let mut group_queue =
            ClosingSpscQueue::<GroupPending<'_, ActorEpochPath<'_, P<'_>>>, 1>::new();
        let mut link_return_queue = ClosingSpscQueue::<LinkReturn, 1>::new();
        let (to_actor, to_actor_end) = to_queue.split();
        let (_parcels, parcels_end) = parcel_queue.split();
        let (from_actor_end, from_actor) = from_queue.split();
        let (returns_end, returns) = return_queue.split();
        let (link, link_end) = link_queue.split();
        let (groups_end, _groups) = group_queue.split();
        let (link_return_end, _link_return) = link_return_queue.split();
        let mut lifecycle = LifecycleEnds {
            to_actor: to_actor_end,
            parcels: parcels_end,
            from_actor: from_actor_end,
            returns: returns_end,
            groups: groups_end,
            link: link_end,
            link_return: link_return_end,
        };
        let mut network_ends =
            NetworkEnds::<_, _, _, TX_DEPTH, { crate::RX_DEPTH }, runtime::SameHart> {
                transmit: transmit_end,
                returned,
                receive,
                receive_copied,
                network: network_stop,
            };

        // The network, as the application's `configure` builds it.
        let memory = leak([[0_u8; BLOCK]; BLOCKS]);
        let (blocks, brand) = oer_protocols_network_core::claim::<T, BLOCK, BLOCKS>(memory)
            .ok()
            .expect("the pool's memory");
        let interface = Interface::new(STATION, STATION_IP, 24, None, MTU).unwrap();
        let moved = Rc::new(Cell::new(Moved::default()));
        let transmit = Transmit(Rc::new(RefCell::new(transmit)));
        assert_eq!(transmit.0.borrow().buffers(), NETWORK_BUFFERS);
        let port_end = Counted {
            inner: Some(DevicePort::new(network, transmit.clone())),
            moved: moved.clone(),
        };
        let owner = NetworkOwner::new(brand, interface, blocks, port_end);
        let (stop, stopping) = leak(StopLane::new()).split();
        let mut service = Service::new(owner, MemoryPolicy::KEEP, stopping);
        let socket = service
            .bind(
                STATION_PORT,
                Dscp::default(),
                Group::shared(),
                leak(SocketLanes::new()),
            )
            .ok()
            .expect("the socket opens");

        // The two tasks.
        let ends = Rc::new(RefCell::new(Ends::default()));
        let network_task = serve_network(service, stop, stopper, clock.clone(), ends.clone());
        let mut network_task = pin!(network_task);
        let actor_clock = clock.clone();
        let actor_ends = ends.clone();
        let actor_task = async {
            let end = actor
                .run(&mut lifecycle, &mut network_ends, &actor_clock)
                .await;
            let mut ends = actor_ends.borrow_mut();
            ends.actor = Some(match end {
                ActorEnd::Retired => ActorEndSeen::Retired,
                ActorEnd::Reinstall => ActorEndSeen::Reinstall,
                ActorEnd::Faulted(_) => ActorEndSeen::Faulted,
            });
            // The facade's `release`: the stopped port's cleanup, then the
            // port's release.
            // Its receipt, if one, stays here past the release.
            let cleaned = crate::release::clean(actor.port_mut());
            match cleaned
                .result
                .ok()
                .and_then(|()| actor.port_mut().release())
            {
                Some(device) => {
                    ends.released = Some(true);
                    ends.frontier = device.take_stopped().map(|(frontier, ())| frontier);
                }
                None => ends.released = Some(false),
            }
            drop(cleaned.removed);
        };
        let mut actor_task = pin!(actor_task);

        let mut bench = Bench {
            clock: clock.clone(),
            atoms: atoms.clone(),
            line: line.clone(),
            rx: ModelRx::new(storage, walker, atoms),
            walker,
            storage,
            stage,
            actor: Task::new(actor_task.as_mut()),
            service: Task::new(network_task.as_mut()),
            to_actor,
            from_actor,
            returns,
            link,
            socket: Some(socket),
            runner,
            transmit,
            moved,
            ends,
            air: Air::Delivers,
            seen: Vec::new(),
            sequence: 1,
            network_paused: false,
            started: false,
            stopped: None,
            faulted: false,
            configured: 0,
            hold_returns: setup == Setup::ReturnsFull,
            unread_returns: 0,
            commands_back: 0,
            poll_micros,
            start_took: Duration::from_micros(0),
        };
        if setup == Setup::ReturnsFull {
            let color = oer_protocols_ieee80211_contracts_port::BssColor::new(1).unwrap();
            for _ in 0..RETURNS {
                bench.send(ToActor::Port(PortCommand::SetBssColor(color)));
                bench.settle();
            }
            assert_eq!(bench.unread_returns, RETURNS, "the return queue is full");
        }
        let asked = bench.clock.now();
        bench.send(ToActor::Start(supported()));
        bench.until("the radio starts", |bench| bench.started);
        bench.start_took = bench.clock.now().saturating_duration_since(asked);
        bench.send(ToActor::Port(PortCommand::Configure {
            permission: station_commands.configure.with_context(()),
            receive: StationReceive::Bss {
                bssid: IndividualAddress::new(BSSID).unwrap(),
                filter: SupportedFilter::new(ReceiveFilter::BSS_MEMBER).unwrap(),
            },
        }));
        if setup == Setup::Connected {
            bench.until("the station is configured", |bench| bench.configured == 1);
            bench.connect();
        } else {
            bench.settle();
        }
        test(&mut bench);
        let _held = (control, tickets);
    }));
}

impl<'s, T: 'static> Bench<'_, 's, T> {
    /// Poll what is woken, serve the air, until nothing is left to do now.
    fn settle(&mut self) {
        for _ in 0..PASSES {
            oer_chips_family_platform_interrupt_table::signal::service::<Deferred>();
            let actor = self.actor.run();
            let service = !self.network_paused && self.service.run();
            if actor || service {
                self.clock.pass(self.poll_micros);
            }
            self.lifecycle();
            let air = self.serve_air();
            if !actor && !service && !air {
                return;
            }
        }
        panic!(
            "{PASSES} passes without an end: a task spins: actor {} polls, network {} polls, \
             stopped {:?}, cause {:?}, buffers {}",
            self.actor.polls,
            self.service.polls,
            self.stopped.is_some(),
            self.ends.borrow().cause,
            self.transmit.0.borrow().buffers(),
        );
    }

    /// Settle, moving time on, until `done`.
    fn until(&mut self, what: &str, mut done: impl FnMut(&mut Self) -> bool) {
        for _ in 0..PASSES {
            self.settle();
            if done(self) {
                return;
            }
            if !self.clock.pass_to_next() {
                panic!(
                    "{what}: nothing is woken and no time is due: a wake was lost, breaks {:?}, \
                     actor {} polls {} wakes, done {}, running {}, requests {}",
                    self.atoms.rule_breaks(),
                    self.actor.polls,
                    self.actor.wakes(),
                    self.actor.done,
                    self.atoms.running(),
                    self.line.masked(),
                );
            }
        }
        panic!("{what}: {PASSES} passes without an end");
    }

    /// Take the actor's messages, as the lifecycle does.
    fn lifecycle(&mut self) {
        while let Some(message) = self.from_actor.try_receive() {
            match message {
                FromActor::Started(result) => {
                    assert!(result.is_ok(), "the radio starts");
                    self.started = true;
                }
                FromActor::Stopped(outcome) => self.stopped = Some(outcome),
                FromActor::Returned if self.hold_returns => self.unread_returns += 1,
                FromActor::Returned => match self.returns.try_receive() {
                    Some(Returned::Terminal { terminal, .. }) => {
                        if let LowerMacTerminal::Configured(_) = terminal {
                            self.configured += 1;
                        }
                    }
                    Some(Returned::Command { .. }) if self.unread_returns > 0 => {
                        // One the held queue announced before.
                        self.commands_back += 1;
                    }
                    Some(_) => panic!("the lifecycle's command came back"),
                    None => panic!("a return without its owner"),
                },
                FromActor::TxTraffic { .. } | FromActor::Tbtt(_) => {}
                FromActor::Received { frame, .. } => {
                    // Not data of the link.
                    let sent = self.to_actor.try_send(ToActor::Recycle(frame));
                    assert!(sent.is_ok(), "the queue has room");
                }
                FromActor::Faulted(_) => self.faulted = true,
                _ => {}
            }
        }
    }

    /// Hand the PPDUs to the air: whether any started.
    fn serve_air(&mut self) -> bool {
        let frames = self.atoms.take_air();
        let any = !frames.is_empty();
        for frame in frames {
            if self.air == Air::Delivers {
                self.line.latch(self.atoms.end_ppdu(frame.queue, 0, 0));
            }
            for mpdu in &frame.mpdus {
                if let Some(reply) = arp_reply(mpdu) {
                    self.land_from_peer(&reply);
                }
            }
            self.seen.push(frame);
        }
        any
    }

    /// The access point relays `msdu` (an EtherType, its payload) to the
    /// station: it lands, and its edge runs the handler.
    fn land_from_peer(&mut self, msdu: &[u8]) {
        let (landing, event) = self
            .rx
            .land(&RxPrefix::new(), &data_from_ap(self.sequence, msdu))
            .expect("the unit fits its buffer");
        assert!(
            matches!(landing, RxWalkerLanding::Landed(_)),
            "the walker takes the unit: {landing:?}"
        );
        self.sequence += 1;
        if let Some(event) = event {
            self.line.latch(event);
        }
    }

    /// Tell the actor `message`.
    fn send(&mut self, message: ToActor<'s, P<'s>>) {
        assert!(
            self.to_actor.try_send(message).is_ok(),
            "the queue has room"
        );
    }

    /// Hand over the open link and bring it up.
    fn connect(&mut self) {
        let address = OwnAddress::new(IndividualAddress::new(STATION).unwrap());
        let bssid = Bssid::new(IndividualAddress::new(BSSID).unwrap()).unwrap();
        let data = DataLink {
            header: StaDataHeaderTemplate::new(address, bssid, false, LinkProtection::Open)
                .unwrap(),
            admission: AcAdmission::OPEN,
            retry: None,
            ht: None,
            windows: [None; 8],
            protection: Default::default(),
        };
        let link: ActorLink<'s, P<'s>> =
            LinkHandover::for_test(1, address, bssid, false, false, None, Some(data));
        assert!(self.link.try_send(link).is_ok(), "the mailbox is empty");
        self.send(ToActor::Connect);
        self.send(ToActor::Release);
        self.runner.publish_link_for_test(LinkSnapshot {
            state: LinkState::Up,
            epoch: 1,
        });
        self.settle();
    }

    /// Run the application's `future` beside the tasks until it ends.
    fn app<F: Future>(&mut self, what: &str, future: F) -> F::Output {
        let mut future = pin!(future);
        let woken = Arc::new(Woken::default());
        let waker = Waker::from(woken.clone());
        for _ in 0..PASSES {
            let mut cx = Context::from_waker(&waker);
            if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
                return output;
            }
            self.settle();
            if !woken.woken.swap(false, std::sync::atomic::Ordering::SeqCst)
                && !self.clock.pass_to_next()
            {
                panic!("{what}: nothing is woken and no time is due: a wake was lost");
            }
        }
        panic!("{what}: {PASSES} passes without an end");
    }

    /// The application sends `data` to the peer; its result's observer.
    fn send_datagram(&mut self, data: &[u8]) -> Observer<'static> {
        let mut socket = self.socket.take().expect("the socket");
        let mut block = self.app("a block", socket.reserve()).expect("a block");
        block.fill(data).expect("the block holds it");
        self.app("a credit", socket.credit()).expect("a credit");
        let observer = socket
            .send(
                Remote {
                    address: PEER_IP,
                    port: PEER_PORT,
                },
                block,
            )
            .ok()
            .expect("the command is accepted");
        self.socket = Some(socket);
        observer
    }

    /// The application waits for the next datagram.
    fn receive_datagram(&mut self) -> NetDatagram<T> {
        let mut socket = self.socket.take().expect("the socket");
        let datagram = self
            .app("a datagram", socket.receive())
            .expect("a datagram");
        self.socket = Some(socket);
        datagram
    }

    /// The result of `observer`'s command.
    fn result(&mut self, mut observer: Observer<'static>) -> Sent {
        self.app("the result", observer.finished());
        observer
            .take()
            .ok()
            .expect("a finished command has a result")
    }

    /// The buffers home: what the station gave back and the network took.
    fn buffers_home(&mut self) -> usize {
        let mut transmit = self.transmit.0.borrow_mut();
        transmit.reclaim();
        transmit.buffers()
    }

    /// The work so far.
    fn work(&self) -> Work {
        Work {
            actor_polls: self.actor.polls,
            actor_wakes: self.actor.wakes(),
            service_polls: self.service.polls,
            service_wakes: self.service.wakes(),
            clock_reads: self.clock.reads.get(),
            moved: self.moved.get(),
            atoms: self.atoms.counts(),
        }
    }

    /// The MPDUs the air saw whose MSDU has `kind`.
    fn mpdus(&self, kind: u16) -> Vec<Vec<u8>> {
        self.seen
            .iter()
            .flat_map(|frame| frame.mpdus.iter())
            .filter(|mpdu| msdu_of(mpdu).is_some_and(|(seen, _)| seen == kind))
            .cloned()
            .collect()
    }
}

/// The work of a stretch of a case: observed numbers.
#[derive(Clone, Copy, Debug)]
struct Work {
    actor_polls: u32,
    actor_wakes: u32,
    service_polls: u32,
    service_wakes: u32,
    clock_reads: u32,
    moved: Moved,
    atoms: AtomCounts,
}

impl Work {
    /// Report what happened since `earlier`.
    fn report(&self, case: &str, earlier: &Work) {
        println!(
            "joined {case}: actor polls {} wakes {}, service polls {} wakes {}, clock reads {}, \
             network frames tx {} rx {}, network bytes tx {} rx {}, dma bytes tx {} rx {}, \
             ppdus {}, units {}, interrupt entries {}, doorbell rings {}, register calls {}",
            self.actor_polls - earlier.actor_polls,
            self.actor_wakes - earlier.actor_wakes,
            self.service_polls - earlier.service_polls,
            self.service_wakes - earlier.service_wakes,
            self.clock_reads - earlier.clock_reads,
            self.moved.tx_frames - earlier.moved.tx_frames,
            self.moved.rx_frames - earlier.moved.rx_frames,
            self.moved.tx_bytes - earlier.moved.tx_bytes,
            self.moved.rx_bytes - earlier.moved.rx_bytes,
            self.atoms.tx_dma_bytes - earlier.atoms.tx_dma_bytes,
            self.atoms.rx_dma_bytes - earlier.atoms.rx_dma_bytes,
            self.atoms.ppdus - earlier.atoms.ppdus,
            self.atoms.units_landed - earlier.atoms.units_landed,
            self.atoms.interrupt_entries - earlier.atoms.interrupt_entries,
            self.atoms.doorbell_rings - earlier.atoms.doorbell_rings,
            self.atoms.register_calls - earlier.atoms.register_calls,
        );
    }
}

/// A data frame from the access point: `msdu` after its LLC header's OUI.
fn data_from_ap(sequence: u16, msdu: &[u8]) -> Vec<u8> {
    let mut frame = std::vec![0_u8; 24];
    frame[0..2].copy_from_slice(&0x0208_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&STATION);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&PEER);
    frame[22..24].copy_from_slice(&(sequence << 4).to_le_bytes());
    frame.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0]);
    frame.extend_from_slice(msdu);
    frame
}

/// The LLC payload of a station's data MPDU: its EtherType and packet.
fn msdu_of(mpdu: &[u8]) -> Option<(u16, &[u8])> {
    let llc = mpdu.get(24..)?;
    (llc.get(..6)? == [0xaa, 0xaa, 0x03, 0, 0, 0]).then_some(())?;
    let kind = u16::from_be_bytes([*llc.get(6)?, *llc.get(7)?]);
    Some((kind, llc.get(8..)?))
}

/// What the peer says to a station's MPDU: its ARP reply, EtherType first.
fn arp_reply(mpdu: &[u8]) -> Option<Vec<u8>> {
    let (kind, packet) = msdu_of(mpdu)?;
    if kind != EtherType::ARP.value() {
        return None;
    }
    let request = Arp::parse(packet).ok()??;
    (request.operation == Operation::Request).then(|| {
        let mut reply = EtherType::ARP.value().to_be_bytes().to_vec();
        let mut packet = [0; arp::PACKET_LEN];
        Arp {
            operation: Operation::Reply,
            sender_hardware: PEER,
            sender_protocol: PEER_IP,
            target_hardware: STATION,
            target_protocol: STATION_IP,
        }
        .write(&mut packet)
        .unwrap();
        reply.extend_from_slice(&packet);
        reply
    })
}

/// An IPv4 UDP packet from the peer to the station's socket, EtherType first.
fn udp_from_peer(data: &[u8]) -> Vec<u8> {
    let length = u16::try_from(8 + data.len()).unwrap();
    let unfilled = UdpHeader {
        ports: Ports {
            source: PEER_PORT,
            destination: STATION_PORT,
        },
        length,
        checksum: 0,
    };
    let pseudo = ipv4_pseudo_header(PEER_IP, STATION_IP, PROTOCOL_UDP, length);
    let checksum = udp_field(
        Checksum::new()
            .with(&pseudo)
            .with(&unfilled.to_bytes())
            .with(data)
            .finish(),
    );
    let mut msdu = EtherType::IPV4.value().to_be_bytes().to_vec();
    msdu.extend_from_slice(
        &Ipv4Fields {
            services: DsField::from_bytes([0]),
            total_length: length + 20,
            identification: 1,
            time_to_live: 64,
            protocol: PROTOCOL_UDP,
            source: PEER_IP,
            destination: STATION_IP,
        }
        .to_bytes(),
    );
    msdu.extend_from_slice(
        &UdpHeader {
            checksum,
            ..unfilled
        }
        .to_bytes(),
    );
    msdu.extend_from_slice(data);
    msdu
}

/// The payload of a station's UDP MPDU, its addresses checked.
fn datagram_of(mpdu: &[u8]) -> Vec<u8> {
    // To the DS: receiver the BSSID, transmitter the station, destination
    // the peer.
    assert_eq!(mpdu[0], 0x08, "a data frame");
    assert_eq!(mpdu[1] & 0x03, 0x01, "to the DS");
    assert_eq!(mpdu[4..10], BSSID);
    assert_eq!(mpdu[10..16], STATION);
    assert_eq!(mpdu[16..22], PEER);
    let (kind, packet) = msdu_of(mpdu).expect("an LLC header");
    assert_eq!(kind, EtherType::IPV4.value());
    let packet = Ipv4Header::parse(packet).unwrap();
    assert_eq!(packet.addresses()[..4], STATION_IP);
    assert_eq!(packet.addresses()[4..], PEER_IP);
    let (header, payload) = UdpHeader::parse(packet.payload()).unwrap();
    assert_eq!(header.ports.source, STATION_PORT);
    assert_eq!(header.ports.destination, PEER_PORT);
    payload.to_vec()
}

fn one_datagram_out<T: 'static>(bench: &mut Bench<'_, '_, T>) {
    let start = bench.work();
    let home = bench.buffers_home();
    assert_eq!(home, NETWORK_BUFFERS);

    let observer = bench.send_datagram(b"out through the ring");
    let sent = bench.result(observer);
    assert_eq!(sent, Sent::Ended(Outcome::Accepted));
    bench.until("the PPDU ended", |bench| {
        !bench.mpdus(EtherType::IPV4.value()).is_empty() && !bench.atoms.on_air(2)
    });

    // The neighbor's question went first, on the same path.
    assert_eq!(bench.mpdus(EtherType::ARP.value()).len(), 1);
    let datagrams = bench.mpdus(EtherType::IPV4.value());
    assert_eq!(datagrams.len(), 1);
    assert_eq!(datagram_of(&datagrams[0]), b"out through the ring");
    assert!(
        bench
            .seen
            .iter()
            .all(|frame| frame.format == AirFormat::Legacy)
    );

    // The completion brought every buffer back to the network.
    bench.settle();
    assert_eq!(bench.buffers_home(), NETWORK_BUFFERS);
    assert_eq!(bench.atoms.rule_breaks(), []);
    let end = bench.work();
    end.report("tx", &start);
    assert!(end.actor_polls > start.actor_polls);
    assert_eq!(end.moved.tx_frames - start.moved.tx_frames, 2);
}

#[test]
fn a_datagram_leaves_as_a_psdu_the_air_reads_and_ends_with_its_completion() {
    struct Network;
    joined::<Network>(|bench| one_datagram_out(bench));
}

/// The station path over the real S31 port: a terminal the actor's full
/// return queue refused stays with the port through the confirmed stop; a
/// freed cell lets the actor's `take_unconsumed` give it once; nothing
/// comes again, and the retire's cleanup and release then end the port.
#[test]
fn a_terminal_the_s31_port_keeps_through_the_confirmed_stop_comes_once_and_the_port_retires() {
    struct Network;
    joined_with::<Network>(0, Setup::ReturnsFull, |bench| {
        assert_eq!(bench.configured, 0, "the full queue refused the terminal");
        let deadline = bench.clock.now().saturating_add(STOP_BOUND);
        bench.send(ToActor::Stop {
            deadline,
            network: NetworkEnd::Stops,
        });
        bench.until("the stop answers", |bench| bench.stopped.is_some());
        assert_eq!(bench.stopped, Some(StopOutcome::Confirmed));
        assert_eq!(bench.configured, 0, "kept through the stop");
        // The lifecycle reads what it held back: every refused command, then
        // the kept terminal once, through the stopped port's
        // `take_unconsumed`.
        bench.hold_returns = false;
        while bench.unread_returns > 0 {
            match bench.returns.try_receive() {
                Some(Returned::Command { .. }) => bench.commands_back += 1,
                Some(_) => panic!("only the refused commands were held"),
                None => panic!("an announced return is in its queue"),
            }
            bench.unread_returns -= 1;
        }
        assert_eq!(bench.commands_back, RETURNS);
        bench.until("the kept terminal comes", |bench| bench.configured == 1);
        for _ in 0..4 {
            bench.clock.pass(1_000);
            bench.settle();
        }
        assert_eq!(bench.configured, 1, "it comes once");
        assert!(bench.returns.try_receive().is_none(), "nothing comes again");
        bench.send(ToActor::Retire);
        bench.until("the actor ends", |bench| {
            bench.ends.borrow().actor.is_some()
        });
        let ends = bench.ends.borrow();
        assert_eq!(ends.actor, Some(ActorEndSeen::Retired));
        assert_eq!(
            ends.released,
            Some(true),
            "the cleanup finished and the port retired"
        );
        assert!(!bench.faulted);
    });
}

/// The cost of a poll is an input of a cost scenario: it changes no outcome.
#[test]
fn a_poll_that_costs_time_changes_no_outcome() {
    struct Network;
    joined_at::<Network>(POLL_MICROS, |bench| one_datagram_out(bench));
}

/// A start needs no time of its own: no poll moves the clock, and it waits on
/// no timer.
#[test]
fn the_radio_starts_when_polls_take_no_time() {
    struct Network;
    joined::<Network>(|bench| {
        assert_eq!(bench.poll_micros, 0);
        assert_eq!(bench.start_took, Duration::from_micros(0));
    });
}

#[test]
fn a_datagram_comes_in_through_the_ring_to_the_socket() {
    struct Network;
    joined::<Network>(|bench| {
        let start = bench.work();
        bench.land_from_peer(&udp_from_peer(b"in through the ring"));
        let datagram = bench.receive_datagram();
        assert_eq!(datagram.remote.address, PEER_IP);
        assert_eq!(datagram.remote.port, PEER_PORT);
        assert_eq!(datagram.block.octets(), b"in through the ring");
        let end = bench.work();
        end.report("rx", &start);
        assert_eq!(end.moved.rx_frames - start.moved.rx_frames, 1);
        assert_eq!(end.atoms.units_landed - start.atoms.units_landed, 1);

        // Units that land before anyone looks stay apart, in their order.
        bench.land_from_peer(&udp_from_peer(b"the first of two"));
        bench.land_from_peer(&udp_from_peer(b"the second of two"));
        let first = bench.receive_datagram();
        let second = bench.receive_datagram();
        assert_eq!(first.block.octets(), b"the first of two");
        assert_eq!(second.block.octets(), b"the second of two");

        // The units go back to the ring.
        let mut socket = bench.socket.take().unwrap();
        socket
            .release(datagram.block)
            .ok()
            .expect("the block goes home");
        bench.socket = Some(socket);
        bench.settle();
        assert_eq!(bench.rx.detached(), 0);
        assert_eq!(bench.atoms.rule_breaks(), []);
    });
}

/// How the PPDU on the air ends around a stop: the test's input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PpduEnd {
    /// Delivered before the stop is asked, and the next one as well.
    BeforeStop,
    /// Delivered while the stop waits for it.
    DuringStop,
    /// Never delivered: the port's forced abort detaches it.
    Aborted,
    /// Never delivered, and the abort finds the hardware took it.
    AbortFindsItOnAir,
}

/// Work in every owner, a stop asked: the datagram the application still
/// holds comes back, for the case to account for.
fn stop_with_work_in_flight<T: 'static>(
    bench: &mut Bench<'_, '_, T>,
    end: PpduEnd,
    lent_unread: usize,
) -> (NetDatagram<T>, u8) {
    // The neighbor is known: what follows goes straight to the air.
    let first = bench.send_datagram(b"first");
    assert_eq!(bench.result(first), Sent::Ended(Outcome::Accepted));
    bench.until("the first PPDU ended", |bench| {
        !bench.mpdus(EtherType::IPV4.value()).is_empty()
    });
    bench.settle();
    bench.air = Air::Holds;

    // A frame on the air, and one the standby arena holds.
    let on_air = bench.send_datagram(b"on the air");
    assert_eq!(bench.result(on_air), Sent::Ended(Outcome::Accepted));
    bench.until("the PPDU is on the air", |bench| {
        bench.mpdus(EtherType::IPV4.value()).len() == 2
    });
    let queue = bench.seen.last().expect("a PPDU").queue;
    assert!(bench.atoms.on_air(queue));
    let standby = bench.send_datagram(b"in the standby arena");
    assert_eq!(bench.result(standby), Sent::Ended(Outcome::Accepted));
    bench.settle();
    assert_eq!(bench.mpdus(EtherType::IPV4.value()).len(), 2);
    assert_eq!(
        NETWORK_BUFFERS - bench.buffers_home(),
        2,
        "two frames hold a buffer"
    );

    // A block the application holds, a block its lane holds, a unit lent to
    // a network that has not run, and a unit nobody served in the ring.
    bench.land_from_peer(&udp_from_peer(b"held by the application"));
    let held = bench.receive_datagram();
    bench.land_from_peer(&udp_from_peer(b"unread in the lane"));
    bench.settle();
    bench.network_paused = true;
    for _ in 0..lent_unread {
        bench.land_from_peer(&udp_from_peer(b"lent, not yet taken"));
        bench.settle();
    }
    bench.land_from_peer(&udp_from_peer(b"unserved in the ring"));

    match end {
        PpduEnd::BeforeStop => {
            bench.air = Air::Delivers;
            bench.line.latch(bench.atoms.end_ppdu(queue, 0, 0));
        }
        PpduEnd::AbortFindsItOnAir => bench.atoms.abort_finds_on_air(queue),
        PpduEnd::DuringStop | PpduEnd::Aborted => {}
    }
    let deadline = bench.clock.now().saturating_add(STOP_BOUND);
    bench.send(ToActor::Stop {
        deadline,
        network: NetworkEnd::Stops,
    });
    bench.network_paused = false;
    bench.settle();
    if end == PpduEnd::DuringStop {
        assert!(bench.stopped.is_none(), "the stop waits for the PPDU");
        bench.line.latch(bench.atoms.end_ppdu(queue, 0, 0));
        // A unit lands behind the mask: its event is the closing epoch's.
        bench.until("the banks are masked", |bench| bench.line.masked());
        bench.land_from_peer(&udp_from_peer(b"behind the mask"));
        assert!(bench.line.latched(), "the event waits under the mask");
    }
    bench.until("the stop answers", |bench| bench.stopped.is_some());
    (held, queue)
}

/// Retire the stopped radio and account for every owner of a confirmed stop.
fn account_for_every_owner<T: 'static>(bench: &mut Bench<'_, '_, T>, held: NetDatagram<T>) {
    assert_eq!(bench.stopped, Some(StopOutcome::Confirmed));
    bench.send(ToActor::Retire);
    bench.until("the actor ends", |bench| {
        bench.ends.borrow().actor.is_some()
    });

    // The radio's owners.
    {
        let ends = bench.ends.borrow();
        assert_eq!(ends.cause, Some(EndCause::Stopped));
        assert_eq!(ends.actor, Some(ActorEndSeen::Retired));
        assert_eq!(ends.released, Some(true), "the port gave its device");
        assert!(ends.frontier.is_some(), "the device gave its frontier");
    }
    assert!(!bench.faulted);
    assert!(!bench.atoms.running(), "the MAC stopped");
    assert!(!bench.atoms.promiscuous(), "the open policy closed");
    assert!(bench.line.masked(), "the banks are masked");
    assert!(!bench.line.latched(), "the epoch closed its events");
    assert!(!bench.walker.borrow().walker_enabled, "the walker stopped");
    assert_eq!(bench.rx.detached(), 0, "every unit is back in the ring");
    assert_eq!(bench.stage.claimed_slots(), 0, "no unit stays staged");
    assert_eq!(bench.atoms.rule_breaks(), []);

    // The network's owners: every transmit buffer, every block.
    assert_eq!(bench.buffers_home(), NETWORK_BUFFERS, "the buffers");
    let mut socket = bench.socket.take().expect("the socket");
    let mut ends = bench.ends.borrow_mut();
    let ended = ends.service.as_mut().expect("the network ended");
    if let Err(refused) = socket.release(held.block) {
        ended.retired.pool.give(refused.block);
    }
    while let Some(block) = socket.reclaim() {
        ended.retired.pool.give(block);
    }
    ended.collect();
    ended.close_returns();
    assert_eq!(ended.retired.pool.free(), BLOCKS, "the blocks");
}

#[test]
fn a_stop_after_the_last_completion_accounts_for_every_owner() {
    struct Network;
    joined::<Network>(|bench| {
        let start = bench.work();
        let (held, _) = stop_with_work_in_flight(bench, PpduEnd::BeforeStop, LENT_UNREAD);
        // The completion put the standby on the air, and that one ended too.
        assert_eq!(bench.mpdus(EtherType::IPV4.value()).len(), 3);
        account_for_every_owner(bench, held);
        bench.work().report("stop, ended before", &start);
    });
}

#[test]
fn a_stop_with_a_ppdu_on_the_air_waits_for_its_completion() {
    struct Network;
    joined::<Network>(|bench| {
        let start = bench.work();
        bench.atoms.set_stop_reads(3);
        let (held, _) = stop_with_work_in_flight(bench, PpduEnd::DuringStop, LENT_UNREAD);
        // The standby never went: the stop gave its frame back.
        assert_eq!(bench.mpdus(EtherType::IPV4.value()).len(), 2);
        assert_eq!(bench.atoms.counts().aborts, 0);
        account_for_every_owner(bench, held);
        bench.work().report("stop, ended during", &start);
    });
}

#[test]
fn a_stop_with_a_ppdu_that_never_ends_aborts_it_at_the_publication_bound() {
    struct Network;
    joined::<Network>(|bench| {
        let start = bench.work();
        let asked = bench.clock.clock.now();
        let (held, queue) = stop_with_work_in_flight(bench, PpduEnd::Aborted, LENT_UNREAD);
        assert_eq!(bench.atoms.counts().aborts, 1, "the port forced one abort");
        assert!(!bench.atoms.on_air(queue), "the abort ended it");
        assert!(
            bench.clock.clock.now().saturating_duration_since(asked) >= PUBLICATION_BOUND,
            "the abort waits for the bound"
        );
        account_for_every_owner(bench, held);
        bench.work().report("stop, aborted", &start);
    });
}

/// The network's stop ends with more lent units in its queue than one of its
/// turns took. Dropping the receiving end leaves them in the channel, so the
/// stop gives them back first, or the ring's pause waits for them and the stop
/// ends unconfirmed at its bound.
#[test]
fn a_stop_gets_back_the_units_the_network_had_not_taken() {
    struct Network;
    joined::<Network>(|bench| {
        let (held, _) = stop_with_work_in_flight(bench, PpduEnd::DuringStop, LENT_PAST_A_TURN);
        account_for_every_owner(bench, held);
    });
}

#[test]
fn a_stop_the_hardware_does_not_confirm_keeps_the_owners_it_cannot_prove() {
    struct Network;
    joined::<Network>(|bench| {
        let start = bench.work();
        let (_held, queue) =
            stop_with_work_in_flight(bench, PpduEnd::AbortFindsItOnAir, LENT_UNREAD);
        let Some(StopOutcome::Unconfirmed(observed)) = bench.stopped else {
            panic!("a PPDU that never ends proves no stop");
        };
        assert!(observed.arena_on_air, "the stop names what is out");
        assert!(bench.atoms.on_air(queue), "the hardware keeps the PPDU");
        assert!(
            bench.buffers_home() < NETWORK_BUFFERS,
            "its buffer stays with the hardware"
        );
        assert!(bench.ends.borrow().actor.is_none(), "nothing retired");
        bench.work().report("stop, unconfirmed", &start);
    });
}

/// A data frame of another BSS, for another station: the ring takes it, the
/// station drops it, and the actor gets no unit of it.
fn foreign_data(sequence: u16) -> Vec<u8> {
    let mut frame = data_from_ap(sequence, &udp_from_peer(b"not for this station"));
    frame[4..10].copy_from_slice(&[0x02, 0, 0, 0x77, 0x77, 0x77]);
    frame[10..16].copy_from_slice(&[0x30, 0x05, 0x5c, 0x99, 0x99, 0x99]);
    frame
}

impl<T: 'static> Bench<'_, '_, T> {
    /// Land `mpdus` one each as the actor's next turns end: its edge runs the
    /// handler then, after the turn's last pull.
    fn land_as_turns_end(&mut self, mpdus: Vec<Vec<u8>>) {
        for mpdu in mpdus {
            let rx = ModelRx::<N, B, S>::new(self.storage, self.walker, self.atoms.clone());
            let line = self.line.clone();
            TURN_ENDS.with_borrow_mut(|edges| {
                edges.push_back(Box::new(move || {
                    let (landing, event) = rx
                        .land(&RxPrefix::new(), &mpdu)
                        .expect("the unit fits its buffer");
                    assert!(
                        matches!(landing, RxWalkerLanding::Landed(_)),
                        "the walker takes the unit: {landing:?}"
                    );
                    if let Some(event) = event {
                        line.latch(event);
                    }
                }));
            });
        }
    }

    /// Land `mpdu` now; its edge runs the handler.
    fn land_now(&mut self, mpdu: &[u8]) {
        let (landing, event) = self
            .rx
            .land(&RxPrefix::new(), mpdu)
            .expect("the unit fits its buffer");
        assert!(
            matches!(landing, RxWalkerLanding::Landed(_)),
            "the walker takes the unit: {landing:?}"
        );
        if let Some(event) = event {
            self.line.latch(event);
        }
    }

    /// The next Sequence Number of the access point.
    fn next_sequence(&mut self) -> u16 {
        let sequence = self.sequence;
        self.sequence += 1;
        sequence
    }
}

/// Turns in a row that took no unit, and one more: past the reposts the actor
/// makes for facts that served none of the port's work.
const TURNS_WITHOUT_UNITS: usize = DUE_REPOSTS as usize + 2;

/// Each of a run of turns ends with a unit landing: its edge masks RX
/// delivery and raises the receive fact after the turn's last pull. The
/// next turn serves it, but the station drops it, so no turn takes a unit.
/// Every landed unit is still served, the last one the AP's datagram.
#[test]
fn a_unit_that_lands_as_a_turn_ends_is_served_after_turns_that_took_none() {
    struct Network;
    joined::<Network>(|bench| {
        let start = bench.work();
        let mut units: Vec<_> = (0..TURNS_WITHOUT_UNITS)
            .map(|_| foreign_data(bench.next_sequence()))
            .collect();
        let sequence = bench.next_sequence();
        units.push(data_from_ap(sequence, &udp_from_peer(b"after the run")));
        bench.land_as_turns_end(units);
        // The first unit starts the run of turns.
        let sequence = bench.next_sequence();
        bench.land_now(&foreign_data(sequence));
        let datagram = bench.receive_datagram();
        assert_eq!(datagram.block.octets(), b"after the run");
        assert!(TURN_ENDS.with_borrow(std::collections::VecDeque::is_empty));
        // Drained: RX delivery is back, and the next unit raises its edge.
        bench.settle();
        assert!(bench.line.rx_delivery_enabled());
        bench.land_from_peer(&udp_from_peer(b"the next one"));
        assert_eq!(bench.receive_datagram().block.octets(), b"the next one");
        assert_eq!(bench.atoms.rule_breaks(), []);
        let end = bench.work();
        end.report("rx after turns without units", &start);
        assert_eq!(
            end.atoms.units_landed - start.atoms.units_landed,
            TURNS_WITHOUT_UNITS as u32 + 3
        );
    });
}

/// The same run of turns, the AP's units landing between polls once it
/// ended: the ring the run left is served, and so is everything after it.
#[test]
fn units_after_a_run_of_turns_without_units_reach_the_socket() {
    struct Network;
    joined::<Network>(|bench| {
        let units = (0..TURNS_WITHOUT_UNITS)
            .map(|_| foreign_data(bench.next_sequence()))
            .collect();
        bench.land_as_turns_end(units);
        let sequence = bench.next_sequence();
        bench.land_now(&foreign_data(sequence));
        bench.until("the run of turns ends", |_| {
            TURN_ENDS.with_borrow(std::collections::VecDeque::is_empty)
        });
        for n in 0..4_u8 {
            bench.land_from_peer(&udp_from_peer(&[n; 8]));
        }
        for n in 0..4_u8 {
            assert_eq!(bench.receive_datagram().block.octets(), [n; 8]);
        }
        bench.settle();
        assert_eq!(bench.atoms.rule_breaks(), []);
    });
}

/// Run `edge` as the actor's turn ends, after the edges queued before it.
fn then_as_a_turn_ends(edge: impl FnOnce() + 'static) {
    TURN_ENDS.with_borrow_mut(|edges| edges.push_back(Box::new(edge)));
}

/// A PPDU on the air with the next one prepared behind it ends as the last
/// of a run of turns without units ends: its completion's turn puts the
/// prepared one on the air, with no other wake and no time passing.
#[test]
fn a_completion_after_turns_that_took_none_puts_the_prepared_ppdu_on_the_air() {
    struct Network;
    joined::<Network>(|bench| {
        let first = bench.send_datagram(b"first");
        assert_eq!(bench.result(first), Sent::Ended(Outcome::Accepted));
        bench.until("the first PPDU ended", |bench| {
            bench.mpdus(EtherType::IPV4.value()).len() == 1
        });
        bench.settle();
        bench.air = Air::Holds;
        let on_air = bench.send_datagram(b"on the air");
        assert_eq!(bench.result(on_air), Sent::Ended(Outcome::Accepted));
        bench.until("the PPDU is on the air", |bench| {
            bench.mpdus(EtherType::IPV4.value()).len() == 2
        });
        let queue = bench.seen.last().expect("on the air").queue;
        let prepared = bench.send_datagram(b"prepared behind it");
        assert_eq!(bench.result(prepared), Sent::Ended(Outcome::Accepted));
        bench.settle();
        assert_eq!(bench.mpdus(EtherType::IPV4.value()).len(), 2);

        let units = (0..TURNS_WITHOUT_UNITS)
            .map(|_| foreign_data(bench.next_sequence()))
            .collect();
        bench.land_as_turns_end(units);
        let (atoms, line) = (bench.atoms.clone(), bench.line.clone());
        then_as_a_turn_ends(move || line.latch(atoms.end_ppdu(queue, 0, 0)));
        let sequence = bench.next_sequence();
        bench.land_now(&foreign_data(sequence));
        let now = bench.clock.now();
        bench.settle();
        assert!(TURN_ENDS.with_borrow(std::collections::VecDeque::is_empty));
        assert_eq!(bench.clock.now(), now, "no timer moved time");
        assert_eq!(
            bench.mpdus(EtherType::IPV4.value()).len(),
            3,
            "the completion's turn published the prepared PPDU"
        );
        assert_eq!(bench.atoms.rule_breaks(), []);
    });
}
