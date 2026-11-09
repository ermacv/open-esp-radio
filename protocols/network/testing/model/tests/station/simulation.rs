//! One host composition of the station and the network, as the
//! `hil-wifi-network` image composes them (`chips/esp32s31/app` over the
//! S31 station facade), minus the hardware: the real station lifecycle,
//! station and arbiter cores, the datapath actor, the station's neutral
//! network end, the network core, its service and a channel socket, over
//! the model's data port (`oer-protocols-ieee80211-testing-model`), under
//! one virtual clock. A simulated access point ([`access_point`]) answers on
//! the model's air, with a router that leases the address by DHCP and a
//! peer host behind it ([`wired`]).
//!
//! What stands in for the product: the model's port for the S31 backend,
//! the software AES for the accelerator, a one-channel scan of the test
//! domain, no transmit Block Ack, and the facade's network stop and the
//! application's transmit memory ([`composition`]) without their statics.
//!
//! A case is a script over [`World`]: the application's control, its
//! socket and the address watch, the access point and the clock. Tasks run
//! only once woken; with nothing woken the script's waits look again, and
//! then time moves to the next deadline. After the script, the harness
//! stops the station gracefully and runs every task to its end, then
//! accounts for the network's blocks and transmit memory ([`Report`]). A
//! run of several sessions starts each after the first from the owners the
//! last stop returned ([`simulate_sessions`]).
//!
//! The boundary: the protocol composition over the model's port, with the
//! real handshake, DHCP, UDP, echo and task interaction. The S31 hardware's
//! wakes, its HE path and transmit timing are outside it.

pub mod access_point;
pub mod composition;
pub mod wired;

use std::cell::{Cell, RefCell, RefMut};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use embassy_futures::select::{Either3, select, select3};
use oer_chips_family_radio_ieee80211_policy::rate_selection::EspressifRateSelection;
use oer_portable_memory::{WakingSlotQueue, WakingSpscQueue};
use oer_portable_time::{Clock, Duration, Instant, Timer};
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_ieee80211_contracts_port::TxEstimate;
use oer_protocols_ieee80211_contracts_port::{RegulatedChannel, SupportedChannel};
use oer_protocols_ieee80211_core_arbiter::ArbiterPolicy;
use oer_protocols_ieee80211_core_roles_sta::data::EthernetFrame as StationFrame;
use oer_protocols_ieee80211_core_roles_sta::machine::{
    GroupCapacity, StaCallbacks, StaConfig, StaProfile, StaSecurity, Station, StationResources,
};
use oer_protocols_ieee80211_core_security_cipher::aes::SoftwareAes128;
use oer_protocols_ieee80211_core_security_rsn::Pmk;
use oer_protocols_ieee80211_core_upper_mac::airtime::AirtimeStorage;
use oer_protocols_ieee80211_core_upper_mac::ccmp::CcmpPacketNumberStep;
use oer_protocols_ieee80211_runtime::datapath::{
    Actor, ActorEnd, ActorEpochPath, ActorLink, FromActor, GroupPending, LifecycleEnds,
    NetworkEnds, PARCELS, Parcel, Returned, ToActor,
};
use oer_protocols_ieee80211_runtime::egress::{AirtimeEgress, AirtimeFault};
use oer_protocols_ieee80211_runtime::lifecycle::{
    ActorEnds, Lifecycle, LifecycleArbiter, LifecycleEnd, LifecycleIo, LifecycleTxContext,
};
use oer_protocols_ieee80211_runtime::station::contract::{CommandOutcome, Notice};
use oer_protocols_ieee80211_runtime::station::{
    self as runtime, Control, DeviceTransmit, FrameCost, OperationEnd, Outgoing, RX_COPIED_DEPTH,
    Received as StationReceived, ReconnectPolicy, RxBlockAckStorage, ServiceCommand, ServiceOutput,
    State, StationAir, StopPolicy, TxEnded,
};
use oer_protocols_ieee80211_testing_model::{
    MODEL_PUBLICATION_BOUND, MODEL_PUBLICATION_COST, MODEL_TICKETS, ModelAir, ModelCaps24,
    ModelCommands, ModelInstall, ModelLentRx, ModelPort, ModelPortFault, ModelResources,
};
use oer_protocols_ieee80211_wire::address::{IndividualAddress, OwnAddress};
use oer_protocols_ieee80211_wire::station::AssociationCapabilities;
use oer_protocols_network_core::{
    Configuration, Datagram, Group, Interface, NetworkOwner, Remote, claim,
};
use oer_protocols_network_port::{
    EthernetFrameView, FramePort, FrameSource, LinkSnapshot, Unpublished,
};
use oer_protocols_network_runtime::{
    ConfigurationWatch, DevicePort, EndCause, Ended, LendingLanes, LendingSocket, MemoryPolicy,
    Observer, PortRefusal, Received as Delivered, Sent, Service, Socket, SocketLanes, StopLane,
};
use oer_protocols_network_wire::ip::Dscp;
use oer_protocols_radio_port::{Installations, LocalId, with_local_slot};

use access_point::{AccessPoint, ApConfig, PMK, SSID};
use composition::{
    Buffers, Counts, FreeMemory, Memory, NetworkStopState, Observed, StationPort, Supply,
    WrittenFrame,
};
use wired::{DhcpServer, Wired};

/// The station's own address.
pub const STATION: [u8; 6] = [0x02, 0, 0, 0x12, 0x34, 0x56];
/// The address of the model's access point interface beside the station,
/// which this composition never starts.
const LOCAL_ACCESS_POINT: [u8; 6] = [0x02, 0, 0, 0x12, 0x34, 0x57];
/// The port the application's socket holds.
pub const PORT: u16 = 5001;

/// The lifecycle's and the actor's queues, as the lifecycle's own tests
/// size them on the model.
const TO: usize = 24;
const FROM: usize = 64;
const RETURNS: usize = 40;
/// The facade's network queues and command depth.
pub const TX_DEPTH: usize = 16;
pub const RX_DEPTH: usize = 32;
const COMMANDS: usize = 2;
/// The facade's group key slots.
const GROUPS: usize = 1;

/// The application's network geometry (`chips/esp32s31/app/src/wifi/network.rs`),
/// with the channel socket alone.
pub const BLOCK: usize = 1472;
pub const BLOCKS: usize = 64;
const SOCKETS: usize = 1;
pub const CREDITS: usize = 32;
const RECEIVED: usize = 8;
pub const LANE: usize = 32;
pub const DEPTH: usize = 32;
const NEIGHBORS: usize = 4;
const MTU: u16 = 1500;
/// The network's transmit memory: this composition's choice, fewer buffers
/// than the S31 pool gives the network.
pub const NETWORK_BUFFERS: usize = 32;

/// Executor passes before a run counts as spinning.
const PASSES: usize = 5_000_000;

type P<'s> = StationPort<'s>;
type Issuer<'s> = ModelCommands<'s, LocalId<'s>, ModelCaps24, composition::Contexts>;
type TxContexts<'s> = [Option<LifecycleTxContext<'s, P<'s>>>; MODEL_TICKETS];
type StationLifecycle<'s> =
    Lifecycle<'s, P<'s>, Callbacks, Issuer<'s>, TxContexts<'s>, GROUPS, Observed>;
pub type StationControl<'c> = Control<'c, ModelCaps24, COMMANDS>;
type Device<'c> = runtime::Device<'c, ModelLentRx, RX_DEPTH>;
type StationTransmit<'c> = DeviceTransmit<'c, WrittenFrame, Supply, TX_DEPTH>;
type Inner<'c> = DevicePort<Device<'c>, StationTransmit<'c>, Buffers<'c>>;
/// The queued egress, as the product's `Queued` network: the station's air
/// as one account per association, frames in flight bounded by the
/// network's buffers.
type Queued<'c> = AirtimeEgress<
    'static,
    StationAir<'c, <runtime::SameHart as runtime::Notifies>::NetworkWaker, ModelCeiling>,
    composition::Ticket,
    PEERS,
    NETWORK_BUFFERS,
>;
/// Accounts the egress keeps: this association's, and earlier ones whose
/// frames have not settled.
const PEERS: usize = 4;

/// What a frame of unknown cost is charged here: the model's publications
/// cost [`MODEL_PUBLICATION_COST`] each, and no plan of the model's makes
/// more than 64.
struct ModelCeiling;

impl FrameCost for ModelCeiling {
    fn least(&self, _frame_octets: usize) -> core::num::NonZeroU32 {
        let TxEstimate::Estimated { micros } = MODEL_PUBLICATION_COST else {
            return core::num::NonZeroU32::MIN;
        };
        core::num::NonZeroU32::new(micros).unwrap_or(core::num::NonZeroU32::MIN)
    }
    fn ceiling(&self, _frame_octets: usize) -> core::num::NonZeroU32 {
        let TxEstimate::Estimated { micros } = MODEL_PUBLICATION_COST else {
            return core::num::NonZeroU32::MAX;
        };
        core::num::NonZeroU32::new(micros.saturating_mul(64)).unwrap_or(core::num::NonZeroU32::MAX)
    }
}
type NetService<'c, T> = Service<
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
    Held<Inner<'c>>,
    Queued<'c>,
>;
type NetEnded<'c, T> = Ended<
    'static,
    'static,
    T,
    BLOCK,
    SOCKETS,
    CREDITS,
    BLOCKS,
    DEPTH,
    LANE,
    Held<Inner<'c>>,
    Queued<'c>,
    AirtimeFault,
>;
/// The application's socket.
pub type Sock<T> = Socket<'static, 'static, T, BLOCK, DEPTH, LANE, CREDITS>;
/// A received frame's buffer the station lends the network.
type LentBuffer = oer_protocols_network_port::LentFrame<
    <StationReceived<ModelLentRx> as oer_protocols_network_port::ReceivedFrame>::Storage,
>;
/// The application's socket when it lends ([`Setup::lending`]).
type LendSock<T> = LendingSocket<'static, 'static, T, BLOCK, DEPTH, LANE, CREDITS, LentBuffer>;
/// A datagram the application received: a copy, or the station's buffer.
pub type NetDatagram<T> = Delivered<'static, 'static, T, BLOCK, LentBuffer>;
/// The lending socket's credit: half the model's lend count, the rest kept
/// for frames on their way.
pub const LEND_CREDIT: usize = oer_protocols_ieee80211_testing_model::MODEL_RX_EVENTS / 2;

/// The application's socket, as the session's setup has it receive.
enum AppSocket<T: 'static> {
    Copying(Sock<T>),
    Lending(LendSock<T>),
}

impl<T> AppSocket<T> {
    fn socket(&mut self) -> &mut Sock<T> {
        match self {
            Self::Copying(socket) => socket,
            Self::Lending(lending) => lending.socket(),
        }
    }

    async fn receive(&mut self) -> Option<NetDatagram<T>> {
        match self {
            Self::Copying(socket) => socket.receive().await.ok().map(Delivered::Copied),
            Self::Lending(lending) => lending.receive().await.ok(),
        }
    }
}

/// Who sent `datagram`.
pub fn remote_of<T>(datagram: &NetDatagram<T>) -> Remote {
    match datagram {
        Delivered::Copied(datagram) => datagram.remote,
        Delivered::Lent(lent) => lent.remote(),
    }
}

/// The data of `datagram`.
pub fn data_of<T>(datagram: &mut NetDatagram<T>) -> Vec<u8> {
    match datagram {
        Delivered::Copied(datagram) => datagram.block.octets().to_vec(),
        Delivered::Lent(lent) => lent.data().expect("the lent buffer holds it").to_vec(),
    }
}

/// The station's callbacks: nonces from a counter, the software AES.
struct Callbacks {
    aes: SoftwareAes128,
    nonces: u8,
}

impl StaCallbacks for Callbacks {
    type AesEngine = SoftwareAes128;
    fn nonce(&mut self) -> [u8; 32] {
        self.nonces = self.nonces.wrapping_add(1);
        [self.nonces.max(1); 32]
    }
    fn aes_engine(&mut self) -> &mut SoftwareAes128 {
        &mut self.aes
    }
    fn he_packet_padding(
        &self,
        _: &[u8],
    ) -> oer_protocols_ieee80211_core_upper_mac::protection::HePacketPadding {
        oer_protocols_ieee80211_core_upper_mac::protection::HePacketPadding::None
    }
}

/// A legacy station: no HT, HE or WMM advertisement.
static CAPABILITIES: AssociationCapabilities = AssociationCapabilities {
    ht20: [0; 28],
    ht40: [0; 28],
    he20_ht: [0; 28],
    he20: [0; 24],
    he20_extended: None,
    wmm: [0; 9],
};

/// The station's configuration: the facade's values on the test domain's
/// channel 6.
fn station_config() -> StaConfig<ModelCaps24, Callbacks> {
    let channel = SupportedChannel::new(
        oer_protocols_radio_regulatory::TEST_DOMAIN
            .permit(RegulatedChannel::from(access_point::channel()))
            .expect("channel 6 is permitted"),
    )
    .expect("the model supports channel 6");
    StaConfig {
        address: OwnAddress::new(IndividualAddress::new(STATION).expect("an individual address")),
        channels: heapless::Vec::from_slice(&[channel]).expect("one channel"),
        wide_channels: heapless::Vec::new(),
        dwell: Duration::from_millis(250),
        skipped_absences: 3,
        access_point_margin_db: 6,
        supported_rates: [0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24],
        association: &CAPABILITIES,
        listen_interval:
            oer_protocols_ieee80211_core_roles_sta::request::StationListenInterval::DEFAULT,
        phy: oer_protocols_ieee80211_core_roles_sta::association::StaPhyRequest::Legacy,
        tx_block_ack: None,
        callbacks: Callbacks {
            aes: SoftwareAes128,
            nonces: 0,
        },
    }
}

/// The network a case's connect names.
fn profile(protected: bool, pmk: [u8; 32]) -> StaProfile {
    StaProfile {
        ssid: heapless::Vec::from_slice(SSID).expect("an SSID"),
        security: if protected {
            StaSecurity::Wpa2Personal {
                pmk: Pmk::from_bytes(pmk),
            }
        } else {
            StaSecurity::Open
        },
        prefer_he20: false,
    }
}

/// What the application heard of the station.
#[derive(Default)]
pub struct Heard {
    pub notices: RefCell<Vec<(Instant, Notice)>>,
    pub refused_transmissions: Cell<usize>,
}

impl Heard {
    /// How many Connected notices came.
    pub fn connections(&self) -> usize {
        self.notices
            .borrow()
            .iter()
            .filter(|(_, notice)| matches!(notice, Notice::Connected { .. }))
            .count()
    }

    pub fn disconnections(&self) -> usize {
        self.notices
            .borrow()
            .iter()
            .filter(|(_, notice)| matches!(notice, Notice::Disconnected(_)))
            .count()
    }
}

/// A case's setup.
pub struct Setup {
    pub access_point: ApConfig,
    pub dhcp: DhcpServer,
    /// The station's reconnect interval; `None` keeps the facade's default,
    /// disabled.
    pub reconnect: Option<Duration>,
    /// The DHCP client's seed.
    pub seed: u64,
    /// The PMK the station's profile names: the network's by default.
    pub pmk: [u8; 32],
    /// The application's socket lends ([`LEND_CREDIT`]) instead of copying.
    pub lending: bool,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            access_point: ApConfig::default(),
            dhcp: DhcpServer::default(),
            reconnect: None,
            seed: 0x5eed,
            pmk: PMK,
            lending: false,
        }
    }
}

/// A case's script.
pub type Script<'a> = Pin<Box<dyn Future<Output = ()> + 'a>>;

/// How a run ended, and what came home.
pub struct Report {
    pub lifecycle: Option<LifecycleEnd<ModelPortFault>>,
    pub actor: Option<ActorEnd<ModelPortFault>>,
    /// Why the network's service ended.
    pub network: Option<EndCause>,
    /// The actor saw the network's stop participant lost.
    pub network_lost: bool,
    /// The network's blocks in its pool after the run.
    pub blocks_free: usize,
    /// The network's transmit memory free after the run, of its total.
    pub memory_free: usize,
    /// The network's memory left in the station's transmit queue: MSDUs the
    /// station never took.
    pub memory_queued: usize,
    pub memory: usize,
    /// Frames the egress policy still holds reservations of at its end:
    /// accepted by the port, their ends not handed on.
    pub in_flight: usize,
    /// Frames' ends the port handed on, with a known cost and without.
    pub settled: (usize, usize),
    pub counts: Counts,
    pub heard: Rc<Heard>,
    pub access_point: Rc<RefCell<AccessPoint>>,
    /// Receive loans out in the network's account after the run, every held
    /// datagram dropped.
    pub loans_out: usize,
}

impl Report {
    /// The station stopped as the application asked, and every owner the
    /// network and the station hold came home.
    pub fn assert_stopped_with_every_owner(&self) {
        self.assert_stopped();
        self.assert_transmit_memory_home();
    }

    /// The station stopped as the application asked: the lifecycle ended,
    /// the actor retired, the network's service stopped with every block in
    /// its pool, and no station token was lost.
    pub fn assert_stopped(&self) {
        assert!(
            matches!(self.lifecycle, Some(LifecycleEnd::Stopped)),
            "the lifecycle stopped: {:?}",
            self.lifecycle
        );
        self.assert_owners_home();
    }

    /// The actor retired, the network's service stopped with every block in
    /// its pool, and no station token was lost.
    pub fn assert_owners_home(&self) {
        assert!(
            matches!(self.actor, Some(ActorEnd::Retired)),
            "the actor retired: {:?}",
            self.actor
        );
        assert_eq!(self.network, Some(EndCause::Stopped), "the network stopped");
        assert_eq!(self.blocks_free, BLOCKS, "every block is in the pool");
        assert_eq!(self.loans_out, 0, "every loan came back");
        assert_eq!(self.counts.tokens_lost, 0, "no station token is lost");
    }

    /// Every transmit buffer of the network is free again.
    pub fn assert_transmit_memory_home(&self) {
        assert_eq!(
            (self.memory_free, self.memory_queued),
            (self.memory, 0),
            "every transmit buffer is free, none left in the station's queue"
        );
    }
}

/// A wake that marks its task to poll.
#[derive(Default)]
struct Flag(AtomicBool);

impl Wake for Flag {
    fn wake(self: Arc<Self>) {
        self.0.store(true, Ordering::SeqCst);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// One task of the executor.
struct Task<'t> {
    name: &'static str,
    future: Pin<&'t mut dyn Future<Output = ()>>,
    flag: Arc<Flag>,
    waker: Waker,
    done: bool,
    polls: u64,
}

impl<'t> Task<'t> {
    fn new(name: &'static str, future: Pin<&'t mut dyn Future<Output = ()>>) -> Self {
        let flag = Arc::new(Flag(AtomicBool::new(true)));
        let waker = Waker::from(flag.clone());
        Self {
            name,
            future,
            flag,
            waker,
            done: false,
            polls: 0,
        }
    }

    /// Poll once if woken: whether it ran.
    fn run(&mut self) -> bool {
        if self.done || !self.flag.0.swap(false, Ordering::SeqCst) {
            return false;
        }
        self.polls += 1;
        let mut cx = Context::from_waker(&self.waker);
        if self.future.as_mut().poll(&mut cx).is_ready() {
            self.done = true;
        }
        true
    }
}

/// A one-shot event a task waits for.
#[derive(Default)]
pub struct Signal {
    raised: Cell<bool>,
    waker: Cell<Option<Waker>>,
}

impl Signal {
    pub fn raise(&self) {
        self.raised.set(true);
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }

    async fn wait(&self) {
        std::future::poll_fn(|cx| {
            if self.raised.get() {
                return Poll::Ready(());
            }
            self.waker.set(Some(cx.waker().clone()));
            Poll::Pending
        })
        .await;
    }
}

/// The script's wait for the rest of the world to settle at this instant.
#[derive(Default)]
struct Idle {
    waiter: Cell<Option<Waker>>,
    /// Counts the settles the executor announced.
    rounds: Cell<u64>,
}

impl Idle {
    async fn settled(&self) {
        let start = self.rounds.get();
        std::future::poll_fn(|cx| {
            if self.rounds.get() != start {
                return Poll::Ready(());
            }
            self.waiter.set(Some(cx.waker().clone()));
            Poll::Pending
        })
        .await;
    }

    /// Wake the waiter: whether there was one.
    fn announce(&self) -> bool {
        match self.waiter.take() {
            Some(waker) => {
                self.rounds.set(self.rounds.get() + 1);
                waker.wake();
                true
            }
            None => false,
        }
    }
}

/// The service's port, which the network's task drops at its end while the
/// rest of the retired service stays for the harness's account.
pub struct Held<I>(Option<I>);

impl<I: FramePort<Refusal = PortRefusal>> FramePort for Held<I> {
    type Refusal = PortRefusal;
    type Ticket = I::Ticket;
    fn publish<E>(
        &mut self,
        length: usize,
        fill: impl FnOnce(&mut [u8]) -> Result<(), E>,
    ) -> Result<I::Ticket, Unpublished<PortRefusal, E>> {
        self.0
            .as_mut()
            .ok_or(Unpublished::Port(PortRefusal::LinkDown))?
            .publish(length, fill)
    }
    fn is_final(refusal: &PortRefusal) -> bool {
        I::is_final(refusal)
    }
    fn flush(&mut self) {
        if let Some(inner) = &mut self.0 {
            inner.flush();
        }
    }
}

impl<I: FrameSource> FrameSource for Held<I> {
    type Notice = I::Notice;
    type Held = I::Held;
    fn take_frame(
        &mut self,
        read: impl FnOnce(
            EthernetFrameView<'_>,
            oer_protocols_network_port::Holding,
        ) -> oer_protocols_network_port::Keep,
    ) -> oer_protocols_network_port::Taken<I::Held> {
        self.0
            .as_mut()
            .map_or(oer_protocols_network_port::Taken::Empty, |inner| {
                inner.take_frame(read)
            })
    }
    fn take_notice(&mut self) -> Option<I::Notice> {
        self.0.as_mut().and_then(FrameSource::take_notice)
    }
    fn link(&self) -> LinkSnapshot {
        self.0
            .as_ref()
            .map_or(LinkSnapshot::DOWN, FrameSource::link)
    }
    fn register(&mut self, cx: &mut Context<'_>) {
        if let Some(inner) = &mut self.0 {
            inner.register(cx);
        }
    }
}

/// What a script works with.
pub struct World<'b, T: 'static> {
    pub clock: Rc<VirtualClock>,
    pub air: Rc<ModelAir>,
    pub access_point: Rc<RefCell<AccessPoint>>,
    pub heard: Rc<Heard>,
    control: RefCell<Option<StationControl<'b>>>,
    socket: RefCell<Option<AppSocket<T>>>,
    /// Datagrams the application still holds at its end, for the pool's
    /// holder.
    held: RefCell<Vec<NetDatagram<T>>>,
    watch: &'static ConfigurationWatch,
    idle: Rc<Idle>,
    free: Rc<FreeMemory>,
    observed: Observed,
    protected: bool,
    pmk: [u8; 32],
    stopping: Cell<bool>,
    /// The network's task is suspended: not polled, its future kept.
    network_suspended: Cell<bool>,
    /// The network's task ends without reporting its stop.
    network_loses_its_stop: Cell<bool>,
    /// The network service's `run` future is dropped.
    cancel_run: Signal,
}

impl<'b, T: 'static> World<'b, T> {
    pub fn now(&self) -> Instant {
        self.clock.now()
    }

    /// The access point and the wired side.
    pub fn ap(&self) -> RefMut<'_, AccessPoint> {
        self.access_point.borrow_mut()
    }

    pub fn counts(&self) -> Counts {
        self.observed.counts()
    }

    /// The network's transmit memory free now.
    pub fn memory_free(&self) -> usize {
        self.free.free()
    }

    /// Wait until `done` holds, looking again whenever the world settled,
    /// for at most `within` of virtual time.
    pub async fn until(&self, what: &str, within: Duration, mut done: impl FnMut(&Self) -> bool) {
        let deadline = self.now().saturating_add(within);
        loop {
            if done(self) {
                return;
            }
            assert!(
                self.now() < deadline,
                "{what}: not within {} ms",
                within.as_micros() / 1000
            );
            self.idle.settled().await;
        }
    }

    /// Run `f` on the application's control.
    pub fn with_control<R>(&self, f: impl FnOnce(&mut StationControl<'b>) -> R) -> R {
        let mut control = self.control.borrow_mut();
        f(control.as_mut().expect("the control"))
    }

    pub fn link(&self) -> LinkSnapshot {
        self.with_control(|control| control.status().link)
    }

    /// Ask the station to connect to the access point's network; the
    /// connect's terminal.
    pub async fn connect(&self) -> OperationEnd {
        let mut control = self.control.borrow_mut().take().expect("the control");
        let operation = control.connect(profile(self.protected, self.pmk)).await;
        *self.control.borrow_mut() = Some(control);
        operation.expect("the connect is admitted").await
    }

    /// Connect and wait for the address: the lease the network holds.
    pub async fn join(&self) -> Configuration {
        let end = self.connect().await;
        assert!(
            matches!(
                end,
                OperationEnd::Completed(CommandOutcome::Connected { .. })
            ),
            "the connect links: {end:?}"
        );
        self.leased().await
    }

    /// The network's configuration as the service last published it.
    pub fn configuration(&self) -> Option<Configuration> {
        self.watch
            .read()
            .expect("the network's hart publishes between reads")
            .configuration
    }

    pub fn address(&self) -> Option<[u8; 4]> {
        self.configuration()
            .and_then(|configuration| configuration.ipv4)
            .map(|ipv4| ipv4.address())
    }

    /// Wait for a leased address.
    pub async fn leased(&self) -> Configuration {
        self.until("the address is leased", Duration::from_secs(30), |world| {
            world.address().is_some()
        })
        .await;
        self.configuration().expect("a configuration")
    }

    /// Run `f` on the application's socket.
    pub fn with_socket<R>(&self, f: impl FnOnce(&mut Sock<T>) -> R) -> R {
        let mut socket = self.socket.borrow_mut();
        f(socket.as_mut().expect("the socket").socket())
    }

    /// The socket's next datagram, waited for.
    pub async fn receive(&self) -> NetDatagram<T> {
        let mut socket = self.socket.borrow_mut().take().expect("the socket");
        let datagram = socket.receive().await;
        *self.socket.borrow_mut() = Some(socket);
        datagram.expect("the socket is open")
    }

    /// Give a received datagram's block back to the network, or its lent
    /// buffer to the station.
    pub fn release(&self, datagram: NetDatagram<T>) {
        let Delivered::Copied(datagram) = datagram else {
            return;
        };
        self.with_socket(|socket| socket.release(datagram.block))
            .map_err(|refused| refused.reason)
            .expect("the lane takes the block");
    }

    /// Send `payload` to `to`: the command's observer, once the socket took
    /// it.
    pub async fn send(&self, to: Remote, payload: &[u8]) -> Observer<'static> {
        let mut socket = self.socket.borrow_mut().take().expect("the socket");
        let mut block = socket.socket().reserve().await.expect("a block");
        block.fill(payload).expect("the block holds it");
        socket.socket().credit().await.expect("a credit");
        let observer = socket
            .socket()
            .send(to, block)
            .map_err(|refused| refused.reason)
            .expect("the socket takes the datagram");
        *self.socket.borrow_mut() = Some(socket);
        observer
    }

    /// The result of `observer`'s command.
    pub async fn result(&self, mut observer: Observer<'static>) -> Sent {
        observer.finished().await;
        let Ok(sent) = observer.take() else {
            panic!("a finished command has a result");
        };
        sent
    }

    /// Stop the station gracefully, by `within` from now.
    /// Suspend the network's task, keeping its future and owners, or
    /// resume it: a software consumer that is late.
    pub fn suspend_network(&self, suspended: bool) {
        self.network_suspended.set(suspended);
    }

    /// Make the network's stop participant go without reporting, as the
    /// facade's `NetworkStopper` dropped unreported: the network's owners
    /// stay with the composition, which keeps serving their returns.
    pub fn lose_network_stop(&self) {
        self.network_loses_its_stop.set(true);
    }

    /// Drop the network service's `run` future now, mid-traffic, keeping
    /// the service: its owner, the composition, retires it and serves the
    /// port's returns from then on.
    pub fn cancel_network_run(&self) {
        self.cancel_run.raise();
    }

    /// Let `duration` of virtual time pass.
    pub async fn sleep(&self, duration: Duration) {
        let at = self.now().saturating_add(duration);
        self.clock.wait_until(at).await;
    }

    /// Keep `datagram` in the application's hands to its end.
    pub fn hold(&self, datagram: NetDatagram<T>) {
        self.held.borrow_mut().push(datagram);
    }

    /// Whether the station's service stopped.
    pub fn stopped(&self) -> bool {
        self.with_control(|control| control.status().stopped)
    }

    pub fn stop(&self, within: Duration) {
        self.stopping.set(true);
        let deadline = self.now().saturating_add(within);
        self.with_control(|control| control.stop_gracefully(deadline));
    }
}

/// One session's script, boxed.
pub type Session<T> = Box<dyn for<'a, 'b> FnOnce(&'a World<'b, T>) -> Script<'a>>;

/// Box `script` as a session.
pub fn session<T: 'static>(
    script: impl for<'a, 'b> FnOnce(&'a World<'b, T>) -> Script<'a> + 'static,
) -> Session<T> {
    Box::new(script)
}

/// Run the station and the network of `setup` on a fresh model with
/// `script` as the application, the access point and the peer, then stop
/// gracefully. `T` names the network: each case brings its own type.
pub fn simulate<T: 'static>(
    setup: Setup,
    script: impl for<'a, 'b> FnOnce(&'a World<'b, T>) -> Script<'a> + 'static,
) -> Report {
    let mut reports = simulate_sessions::<T>(setup, vec![session(script)]);
    reports.pop().expect("one session's report")
}

/// [`simulate`], one session per script: each after the first starts again
/// from the owners the last stop returned. The actor reinstalls the
/// model's port, and a station on the new installation's tokens takes the
/// retired station's configuration and the events' sink; the control
/// opens a new epoch over the same command queue and state; the network's
/// core takes the retired pool and brand, the same device port and
/// transmit memory. Each session's report accounts for what its own stop
/// returned.
pub fn simulate_sessions<T: 'static>(setup: Setup, sessions: Vec<Session<T>>) -> Vec<Report> {
    embassy_futures::block_on(with_local_slot(async |slot| {
        run::<T>(slot, setup, sessions).await
    }))
}

/// A station's lifecycle on an installation's `resources` and `issuer`, as
/// the facade's `initialize` builds it, and its receive agreements' storage.
fn lifecycle_on<'s>(
    config: StaConfig<ModelCaps24, Callbacks>,
    resources: ModelResources<'s, composition::Backend<'s>>,
    issuer: Issuer<'s>,
    events: Observed,
) -> (
    StationLifecycle<'s>,
    RxBlockAckStorage<'s, composition::Types<'s>, 2, 4>,
) {
    let ModelResources {
        shared:
            oer_protocols_ieee80211_testing_model::ModelShared {
                block_acks,
                terminal_access,
                tickets,
                small,
                gate,
                arbiter_authority,
                ..
            },
        station:
            oer_protocols_ieee80211_testing_model::ModelStationPermissions {
                connection_epoch,
                commands: station_commands,
                association: station_association,
                pairwise: station_pairwise,
                group: station_group,
                group_permission: station_group_permission,
            },
        ..
    } = resources;
    let mut tickets = tickets.into_iter();
    let station_tickets: [_; 4] = core::array::from_fn(|_| tickets.next().expect("a ticket"));
    let Ok(small) = <[_; 4]>::try_from(small) else {
        panic!("four station buffers");
    };
    let channel = config.channels[0];
    let (arbiter, place, absence) = LifecycleArbiter::<P<'s>>::new(
        ArbiterPolicy::DEFAULT
            .checked(MODEL_PUBLICATION_BOUND)
            .expect("the arbiter's policy"),
        channel,
        arbiter_authority,
        gate,
    );
    let station_resources = StationResources::<_, 4, 4, GROUPS> {
        configure: station_commands.configure,
        epochs: oer_protocols_ieee80211_core_roles_sta::epoch::LinkEpochSource::new(
            connection_epoch,
        ),
        tickets: station_tickets,
        buffers: small,
        place,
        absence_place: absence,
        keys: oer_protocols_ieee80211_contracts_port::association::reunite_vacant(
            station_association,
            oer_protocols_ieee80211_contracts_port::association::VacantKeyBank::new(
                station_pairwise,
                station_group,
            ),
        ),
        group_permission: station_group_permission,
    };
    let station = Station::new(config, station_resources, GroupCapacity::new());
    let lifecycle = Lifecycle::new(
        station,
        arbiter,
        issuer,
        CcmpPacketNumberStep::ONE,
        channel,
        terminal_access,
        events,
    );
    let storage = RxBlockAckStorage::<_, 2, 4>::new(block_acks, Duration::from_millis(100));
    (lifecycle, storage)
}

#[allow(clippy::too_many_lines, reason = "one composition, built in order")]
async fn run<'s, T: 'static>(
    slot: oer_protocols_radio_port::SlotClaim<LocalId<'s>>,
    setup: Setup,
    sessions: Vec<Session<T>>,
) -> Vec<Report> {
    let clock = Rc::new(VirtualClock::new());
    let mut installations = Installations::claim(slot);
    let (install, air) = ModelPort::install(
        &mut installations,
        access_point::channel(),
        [
            IndividualAddress::new(STATION).expect("an individual address"),
            IndividualAddress::new(LOCAL_ACCESS_POINT).expect("an individual address"),
        ],
        clock.clone(),
    )
    .expect("the first installation");
    let ModelInstall {
        port,
        commands: issuer,
        resources,
    } = install;
    let observed = Observed::default();
    let (lifecycle, storage) = lifecycle_on(station_config(), resources, issuer, observed.clone());
    let block_ack_storage = Box::leak(Box::new(storage));
    let mut actor = Actor::<P<'s>, _, _>::new(
        port,
        EspressifRateSelection::<ModelCaps24>::new(),
        block_ack_storage.view(),
        observed.clone(),
    );
    let mut lifecycle = Some(lifecycle);

    // The queues between the lifecycle and the actor.
    let mut to_queue = WakingSpscQueue::<ToActor<'s, P<'s>>, TO>::new();
    let mut parcel_queue = WakingSpscQueue::<Parcel<'s, P<'s>>, PARCELS>::new();
    let mut from_queue = WakingSpscQueue::<FromActor<'s, P<'s>>, FROM>::new();
    let mut return_queue = WakingSpscQueue::<Returned<'s, P<'s>>, RETURNS>::new();
    let mut link_queue = WakingSpscQueue::<ActorLink<'s, P<'s>>, 1>::new();
    let mut group_queue = WakingSpscQueue::<GroupPending<'s, ActorEpochPath<'s, P<'s>>>, 1>::new();
    let mut link_return_queue =
        WakingSpscQueue::<oer_protocols_ieee80211_core_roles_sta::link::LinkReturn, 1>::new();
    let (to_actor, to_actor_end) = to_queue.split();
    let (parcels, parcels_end) = parcel_queue.split();
    let (from_actor_end, from_actor) = from_queue.split();
    let (returns_end, returns) = return_queue.split();
    let (link, link_end) = link_queue.split();
    let (groups_end, groups) = group_queue.split();
    let (link_return_end, link_return) = link_return_queue.split();
    let mut actor_ends = LifecycleEnds {
        to_actor: to_actor_end,
        parcels: parcels_end,
        from_actor: from_actor_end,
        returns: returns_end,
        groups: groups_end,
        link: link_end,
        link_return: link_return_end,
    };
    let mut lifecycle_ends = ActorEnds {
        to_actor,
        parcels,
        from_actor,
        returns,
        groups,
        link,
        link_return,
    };

    // The station's channels to the network, as the facade's `Channels`
    // split them.
    let mut transmit_queue = WakingSlotQueue::<Option<Outgoing<Memory>>, TX_DEPTH>::new();
    let mut returned_queue = WakingSlotQueue::<Option<TxEnded<Memory>>, TX_DEPTH>::new();
    let mut receive_queue = WakingSpscQueue::<StationReceived<ModelLentRx>, RX_DEPTH>::new();
    let mut copies_queue = WakingSpscQueue::<StationFrame, RX_COPIED_DEPTH>::new();
    let mut command_queue = WakingSpscQueue::<ServiceCommand<ModelCaps24>, COMMANDS>::new();
    let (stack, transmit_end) = transmit_queue.split(|| None);
    let (returned_end, returned) = returned_queue.split(|| None);
    let (receive_end, delivered) = receive_queue.split();
    let (copies_end, copies) = copies_queue.split();
    let (application, commands) = command_queue.split();
    let state = State::new();
    let network_stop = NetworkStopState::default();
    let mut network_ends = NetworkEnds::<_, _, _, TX_DEPTH, RX_DEPTH, runtime::SameHart> {
        transmit: transmit_end,
        returned: returned_end,
        receive: receive_end,
        receive_copied: copies_end,
        network: &network_stop,
    };
    let free = FreeMemory::new(NETWORK_BUFFERS);
    let device = Device::new(delivered, copies, &state);
    let transmit = DeviceTransmit::new(stack, returned, Supply(free.clone()), &state);
    let link = device.link_source();
    let mut port = Some(DevicePort::new(
        device,
        transmit,
        Buffers {
            free: free.clone(),
            link,
        },
    ));

    // The network's memory, claimed once for every session's core.
    let memory = Box::leak(Box::new([[0_u8; BLOCK]; BLOCKS]));
    let mut network = Some(
        claim::<T, BLOCK, BLOCKS>(memory)
            .map_err(|refused| refused.reason)
            .expect("the network's brand"),
    );

    let access_point = Rc::new(RefCell::new(AccessPoint::new(
        air.clone(),
        clock.clone(),
        setup.access_point,
        Wired::new(setup.dhcp),
    )));
    let idle = Rc::new(Idle::default());
    let mut reports = Vec::new();
    // The lifecycle's ends to the application, which every session's
    // lifecycle takes in turn: the command queue's receiver, what the
    // application hears and the control epoch's runner end.
    let (first_control, runner) = Control::new(application, &state)
        .map_err(|refused| refused.reason)
        .expect("the control");
    let mut application = None;
    let mut first = Some(first_control);
    let hearing: Rc<RefCell<Rc<Heard>>> = Rc::default();
    let record = {
        let hearing = hearing.clone();
        let clock = clock.clone();
        move |output: ServiceOutput<ModelCaps24>| {
            let heard = hearing.borrow().clone();
            match output {
                ServiceOutput::Notice(notice) => {
                    heard.notices.borrow_mut().push((clock.now(), notice));
                }
                ServiceOutput::TxRefused { .. } => {
                    heard
                        .refused_transmissions
                        .set(heard.refused_transmissions.get() + 1);
                }
                _ => {}
            }
        }
    };
    let mut output = first.as_ref().expect("the first control").events(record);
    let mut io = LifecycleIo {
        commands,
        output: &mut output,
        control: runner,
        stop: StopPolicy {
            drain: Duration::from_secs(1),
        },
    };
    let mut retired_config = None;
    for (index, script) in sessions.into_iter().enumerate() {
        if index != 0 {
            // The next installation under the same actor, and a station on
            // its tokens with the retired station's configuration.
            let mut minted = None;
            #[expect(
                clippy::result_large_err,
                reason = "the model's refusal returns its port"
            )]
            let reinstalled = actor.reinstall(|port| {
                port.reinstall(&mut installations)
                    .map(|install| {
                        let ModelInstall {
                            port,
                            commands,
                            resources,
                        } = install;
                        minted = Some((commands, resources));
                        port
                    })
                    .map_err(|refused| {
                        let (port, reason) = *refused;
                        (port, reason)
                    })
            });
            actor = match reinstalled {
                Ok(actor) => actor,
                Err((_, reason)) => panic!("the stopped port reinstalls: {reason:?}"),
            };
            let (issuer, resources) = minted.expect("the new installation");
            let config = retired_config
                .take()
                .expect("the retired station's configuration");
            let (next, storage) = lifecycle_on(config, resources, issuer, observed.clone());
            // The actor keeps its receive agreements' storage; the new
            // installation's stays owned.
            let _kept = Box::leak(Box::new(storage));
            lifecycle = Some(next);
            network_stop.reset();
        }

        // A control epoch over the command queue and the state; the first
        // one's runner end is already the lifecycle's.
        let mut control = match first.take() {
            Some(control) => control,
            None => {
                let (control, runner) =
                    Control::new(application.take().expect("the producer"), &state)
                        .map_err(|refused| refused.reason)
                        .expect("the control");
                io.control = runner;
                control
            }
        };
        if let Some(interval) = setup.reconnect {
            control.configure_reconnect(ReconnectPolicy::every(interval).expect("an interval"));
        }

        // The network, as the application's `Network::start` builds it.
        let (pool, brand) = network.take().expect("the network's pool and brand");
        let seed = setup.seed.wrapping_add(index as u64);
        let interface = Interface::dhcp(STATION, MTU, seed).expect("the interface");
        let owner = NetworkOwner::new(brand, interface, pool, Held(port.take()));
        let (stop_lane, stopping) = Box::leak(Box::new(StopLane::new())).split();
        let airtime = Box::leak(Box::new(AirtimeStorage::new(
            core::num::NonZeroU32::new(1_000).expect("a quantum"),
        )));
        let egress = AirtimeEgress::new(airtime, StationAir::new(link, ModelCeiling));
        let mut service: NetService<'_, T> =
            Service::new(owner, egress, MemoryPolicy::KEEP, stopping);
        let watch: &'static ConfigurationWatch = Box::leak(Box::new(ConfigurationWatch::new()));
        service.watch(watch).expect("the first publication");
        let socket = if setup.lending {
            let lent = oer_protocols_ieee80211_testing_model::MODEL_RX_EVENTS;
            service
                .lend_from(lent, lent - LEND_CREDIT)
                .expect("the account fits the model's lend count");
            AppSocket::Lending(
                service
                    .bind_lending::<LEND_CREDIT>(
                        PORT,
                        Dscp::default(),
                        Group::shared(),
                        Box::leak(Box::new(LendingLanes::new())),
                    )
                    .map_err(|refused| refused.reason)
                    .expect("the socket opens"),
            )
        } else {
            AppSocket::Copying(
                service
                    .bind(
                        PORT,
                        Dscp::default(),
                        Group::shared(),
                        Box::leak(Box::new(SocketLanes::new())),
                    )
                    .map_err(|refused| refused.reason)
                    .expect("the socket opens"),
            )
        };

        // What the application hears.
        let heard = Rc::new(Heard::default());
        *hearing.borrow_mut() = heard.clone();
        let world = World {
            clock: clock.clone(),
            air: air.clone(),
            access_point: access_point.clone(),
            heard: heard.clone(),
            control: RefCell::new(Some(control)),
            socket: RefCell::new(Some(socket)),
            watch,
            idle: idle.clone(),
            free: free.clone(),
            observed: observed.clone(),
            protected: setup.access_point.protected,
            pmk: setup.pmk,
            held: RefCell::new(Vec::new()),
            stopping: Cell::new(false),
            network_suspended: Cell::new(false),
            network_loses_its_stop: Cell::new(false),
            cancel_run: Signal::default(),
        };

        // The four tasks.
        let actor_end = RefCell::new(None);
        let lifecycle_end = RefCell::new(None);
        let network_end: RefCell<Option<NetEnded<'_, T>>> = RefCell::new(None);
        let station_done = Signal::default();
        {
            let running = lifecycle.take().expect("the session's lifecycle");
            let actor_task = async {
                let end = actor.run(&mut actor_ends, &mut network_ends, &*clock).await;
                *actor_end.borrow_mut() = Some(end);
                station_done.raise();
            };
            let lifecycle_task = async {
                let completed = running.run(&mut lifecycle_ends, &mut io, &*clock).await;
                *lifecycle_end.borrow_mut() = Some(completed);
            };
            let network_task = async {
                // `None`: the composition dropped the run future.
                let cause = {
                    let mut events = observed.clone();
                    let mut run = core::pin::pin!(service.run(&*clock, &mut events));
                    match select3(
                        run.as_mut(),
                        network_stop.requested(),
                        world.cancel_run.wait(),
                    )
                    .await
                    {
                        Either3::First(cause) => Some(cause),
                        Either3::Second(()) => {
                            // The lane holds this one word.
                            let _sent = stop_lane.try_send(());
                            Some(run.await)
                        }
                        Either3::Third(()) => None,
                    }
                };
                // The composition owns the service, whose run ended or was
                // dropped: retiring it keeps every owner, the port included.
                let mut ended = service.retire();
                if let Some(cause) = cause {
                    assert_eq!(ended.cause, cause);
                }
                if let Some(port) = ended.retired.port.0.as_mut() {
                    // A dropped run: the composition serves the port's
                    // returns until the station asks the network to stop.
                    if cause.is_none() {
                        let serving = std::future::poll_fn(|cx| {
                            port.serve_returns(cx);
                            Poll::<()>::Pending
                        });
                        let _asked = select(serving, network_stop.requested()).await;
                    }
                    // The queues outlive the port, so its frames go back and
                    // the returned memory comes home first, as the
                    // application's stop does.
                    port.give_back();
                    if world.network_loses_its_stop.get() {
                        // The participant goes unreported; the composition
                        // still serves the returns until the station ended.
                        network_stop.lost();
                        let serving = std::future::poll_fn(|cx| {
                            port.serve_returns(cx);
                            Poll::<()>::Pending
                        });
                        let _done = select(serving, station_done.wait()).await;
                        port.give_back();
                    } else {
                        network_stop.stopped();
                    }
                } else {
                    network_stop.stopped();
                }
                *network_end.borrow_mut() = Some(ended);
            };
            let script_task = async {
                script(&world).await;
                if !world.stopping.get() {
                    world.stop(Duration::from_secs(10));
                }
            };
            let mut actor_task = core::pin::pin!(actor_task);
            let mut lifecycle_task = core::pin::pin!(lifecycle_task);
            let mut network_task = core::pin::pin!(network_task);
            let mut script_task = core::pin::pin!(script_task);
            let mut tasks = [
                Task::new("actor", actor_task.as_mut()),
                Task::new("lifecycle", lifecycle_task.as_mut()),
                Task::new("network", network_task.as_mut()),
                Task::new("script", script_task.as_mut()),
            ];
            execute(
                &mut tasks,
                &clock,
                &air,
                &access_point,
                &idle,
                &world.network_suspended,
            );
        }

        // The account: what the station gave back, and every block.
        let mut ended = network_end.borrow_mut().take().expect("the network ended");
        let mut socket = world.socket.borrow_mut().take().expect("the socket");
        // A held lent buffer goes back to the station only now, after the
        // stop: its loan comes home to the pool's holder late.
        for datagram in world.held.borrow_mut().drain(..) {
            if let Delivered::Copied(datagram) = datagram {
                ended.retired.pool.give(datagram.block);
            }
        }
        while let Some(block) = socket.socket().reclaim() {
            ended.retired.pool.give(block);
        }
        drop(socket);
        ended.collect();
        ended.close_returns();
        let (end, retired) = match lifecycle_end.borrow_mut().take() {
            Some(completed) => (Some(completed.end), Some(completed.retired)),
            None => (None, None),
        };
        reports.push(Report {
            lifecycle: end,
            actor: actor_end.borrow_mut().take(),
            network: Some(ended.cause),
            network_lost: actor.network_lost(),
            blocks_free: ended.retired.pool.free(),
            memory_free: free.free(),
            memory_queued: network_ends.transmit.len(),
            memory: free.total(),
            in_flight: ended.egress.in_flight(),
            settled: free.settled(),
            counts: observed.counts(),
            heard,
            access_point: access_point.clone(),
            loans_out: ended.retired.pool.loans().outstanding(),
        });

        // What the next session starts from.
        let Ended {
            retired:
                oer_protocols_network_core::Retired {
                    pool,
                    brand,
                    port: held,
                    ..
                },
            ..
        } = ended;
        network = Some((pool, brand));
        port = held.0;
        retired_config = retired.map(|retired| retired.station.into_config());
        let mut control = world.control.borrow_mut().take().expect("the control");
        // The ended epoch's stop is observed before its producer comes back.
        control.wait_stopped().await;
        application = Some(
            control
                .try_finish()
                .ok()
                .expect("the stopped epoch gives its producer back"),
        );
    }
    reports
}

/// Run `tasks` to their ends: woken tasks are polled, the access point
/// answers what the station sent, a waiting script (the last task) looks
/// again once the others settled, and only then time moves to the next
/// deadline.
fn execute(
    tasks: &mut [Task<'_>],
    clock: &VirtualClock,
    air: &ModelAir,
    access_point: &RefCell<AccessPoint>,
    idle: &Idle,
    suspended: &Cell<bool>,
) {
    let mut announced = false;
    // The network's task is not polled while the script suspends it: its
    // future and owners stay as they are, its wakes wait.
    let held = |task: &Task<'_>| suspended.get() && task.name == "network";
    for _ in 0..PASSES {
        let mut progress = false;
        let script = tasks.len() - 1;
        for (index, task) in tasks.iter_mut().enumerate() {
            if held(task) {
                continue;
            }
            let ran = task.run();
            // The script's own look after a settle is no progress of the
            // world's.
            progress |= ran && !(announced && index == script);
        }
        progress |= access_point.borrow_mut().serve();
        if tasks.iter().all(|task| task.done) {
            return;
        }
        if progress {
            announced = false;
            continue;
        }
        if tasks
            .iter()
            .any(|task| !task.done && !held(task) && task.flag.0.load(Ordering::SeqCst))
        {
            continue;
        }
        if !announced && idle.announce() {
            announced = true;
            continue;
        }
        announced = false;
        if clock.advance_to_next_deadline().is_none() {
            let running: Vec<_> = tasks
                .iter()
                .filter(|task| !task.done)
                .map(|task| task.name)
                .collect();
            panic!("nothing is woken and no time is due: {running:?} wait for a lost wake");
        }
        air.clock_advanced();
    }
    let polls: Vec<_> = tasks
        .iter()
        .map(|task| (task.name, task.polls, task.done))
        .collect();
    panic!(
        "{PASSES} passes without an end at {:?}: a task spins: {polls:?}",
        clock.now()
    );
}
