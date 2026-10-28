//! Image-owned mapping; the station service and IP workload run on P1.
use super::{
    Attempt,
    irq::TX,
    radio::{self, Backend, Caps, Device, Family},
};
use core::{convert::Infallible, future::Future, marker::PhantomData, mem::MaybeUninit};
use embassy_futures::select::{Either, select};
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_ieee80211_driver::port::{self, device::Composition};
use oer_espressif_ieee80211_policy::rate_selection::EspressifRateSelection;
use oer_ieee80211_control::{StationLinkState, StationServiceEnd};
use oer_ieee80211_port::*;
use oer_ieee80211_rsn::{Pmk, nonce::SNonceSource, sae::SaePassword};
use oer_ieee80211_runtime::station_service::*;
use oer_ieee80211_cipher::aes::{SoftwareAes128, SoftwareKeyUnwrap};
use oer_ieee80211_sta::data::{EthernetFrame, TxMsdu};
use oer_memory::{WakingSpscQueue, WakingSpscSender};
use oer_ieee80211_sta::{
    data::StaData,
    epoch::{EpochContext, LinkEpochSource},
    machine::{self, StaConfig, Station, StationResources},
};
use oer_ieee80211_upper_mac::{
    phy_mode::PhyMode,
    protection::HePacketPadding,
    rate_control::RatePeer,
    rate_selection::{FrameRates, RateSelection},
};
use oer_ieee80211_wire::{
    address::{IndividualAddress, OwnAddress},
    ht::{HtLocalCapabilities, ht_capability_ie},
    station::AssociationCapabilities,
};
use oer_memory::{Placed, PlacedFuture, StaticPlace};
use oer_radio_port::{self as rp, Child, PortBackend, Root, SlotIdentity};
use oer_time::{Duration, Instant};
use static_cell::ConstStaticCell;

pub type Epoch<Id> = Child<Root<Id>, 4>;
pub struct ImageComposition;
impl Composition for ImageComposition {
    type Coex = radio::Coex;
    type Entropy = radio::Entropy;
    type Body = Infallible;
    type OwnedContext<Id: SlotIdentity> = TxOwnedContext<Caps, port::BufferClass<TX>, Epoch<Id>>;
    type Context<'p, Id: SlotIdentity + 'p> = TxContext<'p, Caps, port::BufferClass<TX>, Epoch<Id>>;
    type OwnedConfigureContext<Id: SlotIdentity> =
        rp::ContextChoice<(), EpochContext<Epoch<Id>, ()>>;
    type ConfigureContext<'p, Id: SlotIdentity + 'p> =
        machine::StaConfigurationContext<'p, Epoch<Id>>;
}
pub struct StationComposition<Id: SlotIdentity>(PhantomData<Id>);
const TERMINALS: usize = <Family<radio::slot::Id> as PhaseTypes<'static>>::TERMINAL_CAPACITY;
const MANAGEMENT: u8 = 4;
static NETWORK_RX: ConstStaticCell<WakingSpscQueue<EthernetFrame, { super::network::RX }>> =
    ConstStaticCell::new(WakingSpscQueue::new());
static NETWORK_TX: ConstStaticCell<WakingSpscQueue<TxMsdu, 2>> = ConstStaticCell::new(WakingSpscQueue::new());

impl<Id: SlotIdentity> StationPort for StationComposition<Id> {
    type Backend = Backend<Id>;
    type SlotId = Id;
    type Caps = Caps;
    type Types<'id>
        = Family<Id>
    where
        Self: 'id,
        Id: 'id;
    type TerminalStorage<T> = heapless::Deque<T, TERMINALS>;
    type Small = port::BufferClass<TX>;
    type Large = port::BufferClass<TX>;
    fn lend_phase<'a, 'id, 'r>(
        phase: &'r mut <Backend<Id> as PortBackend<Id>>::Phase<'a, 'id>,
    ) -> &'r mut impl LowerMacPhase<'a, 'id, Caps = Caps, Types = Family<Id>>
    where
        Self: 'id,
        Id: 'id,
        'id: 'a,
    {
        phase
    }
    fn terminal<'id>(
        terminal: LowerMacTerminal<'id, Family<Id>>,
    ) -> LowerMacTerminal<'id, Family<Id>>
    where
        Self: 'id,
        Id: 'id,
    {
        terminal
    }
    fn port_terminal<'id>(
        terminal: LowerMacTerminal<'id, Family<Id>>,
    ) -> LowerMacTerminal<'id, Family<Id>>
    where
        Self: 'id,
        Id: 'id,
    {
        terminal
    }
    fn tune<'a, 'id>(
        setup: &mut <Backend<Id> as PortBackend<Id>>::Setup<'a, 'id>,
        channel: SupportedChannel<Caps>,
    ) where
        Self: 'id,
        Id: 'id,
        'id: 'a,
    {
        setup.set_channel(channel);
    }
    fn event<'id>(event: LowerMacEvent<'id, Family<Id>>) -> LowerMacEvent<'id, Family<Id>>
    where
        Self: 'id,
        Id: 'id,
    {
        event
    }
    fn tx_buffer<'id>(buffer: port::AnyBuffer<'id, Id, Device, TX>) -> StationTxBuffer<'id, Self>
    where
        Self: 'id,
        Id: 'id,
    {
        // The eight identical DMA allocations are split once by their affine
        // pool indices: four remain with Station, four with StaData.
        let (small, mpdu) = match buffer {
            port::AnyBuffer::Fresh(buffer) => {
                (buffer.index() < MANAGEMENT, ReturnedMpdu::Fresh(buffer))
            }
            port::AnyBuffer::Sent(buffer) => {
                (buffer.index() < MANAGEMENT, ReturnedMpdu::Sent(buffer))
            }
        };
        if small {
            StationTxBuffer::Small(mpdu)
        } else {
            StationTxBuffer::Large(mpdu)
        }
    }
}

struct Rates(EspressifRateSelection<Caps>, Attempt);
impl<M: SignalMetadata> RateSelection<M, port::Ack> for Rates {
    fn packet_number_step(&self) -> oer_ieee80211_upper_mac::ccmp::CcmpPacketNumberStep {
        <EspressifRateSelection<Caps> as RateSelection<M, port::Ack>>::packet_number_step(&self.0)
    }
    fn plan<R>(
        &self,
        frame: FrameRates,
        carried: impl Fn(oer_ieee80211_wire::phy::PhyRate) -> Option<R>,
    ) -> Option<RetryPlan<R>> {
        <EspressifRateSelection<Caps> as RateSelection<M, port::Ack>>::plan(&self.0, frame, carried)
    }
    fn associated(&mut self, peer: &RatePeer, response: &M) {
        self.1.publish(StationLinkState::Associated);
        <EspressifRateSelection<Caps> as RateSelection<M, port::Ack>>::associated(
            &mut self.0,
            peer,
            response,
        );
    }
    fn disconnected(&mut self) {
        <EspressifRateSelection<Caps> as RateSelection<M, port::Ack>>::disconnected(&mut self.0);
    }
    fn completed<D>(
        &mut self,
        now: Instant,
        frame: FrameRates,
        completion: &SingleOutcome<port::Ack, D>,
    ) {
        use core::sync::atomic::Ordering::Relaxed;
        if matches!(
            completion,
            TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout)
        ) {
            super::observation::TIMEOUTS.fetch_add(1, Relaxed);
        }
        if super::observation::DEPARTING.load(Relaxed)
            && frame == FrameRates::Control
            && matches!(completion, TxOutcome::Ended(SingleEnd::Delivered { .. }))
        {
            super::observation::DEAUTH.fetch_add(1, Relaxed);
        }
        <EspressifRateSelection<Caps> as RateSelection<M, port::Ack>>::completed(
            &mut self.0,
            now,
            frame,
            completion,
        );
    }
}

// IEEE 802.11 HT20, one spatial stream, MCS 0..7; no HE advertisement.
// WMM Information: QoS station, U-APSD disabled (IEEE 802.11 WMM OUI type 2).
const HT20: [u8; 28] = ht_capability_ie(
    HtLocalCapabilities::new(0, 0, 0xff, 1),
    match Channel::ghz2_4(1, oer_ieee80211_wire::channel::WifiChannelWidth::Mhz20) {
        Ok(channel) => channel,
        Err(_) => panic!("HT20 advertisement geometry"),
    },
);
static ASSOCIATION: AssociationCapabilities = AssociationCapabilities {
    ht20: HT20,
    ht40: [0; 28],
    he20_ht: [0; 28],
    he20: [0; 24],
    he20_extended: [0; 14],
    wmm: [221, 7, 0, 0x50, 0xf2, 2, 0, 1, 0],
};
pub struct Body {
    pub address: [u8; 6],
    pub seed: u64,
    pub ssid: heapless::Vec<u8, 32>,
    pub password: SaePassword,
    pub pmk: Pmk,
    pub nonces: Placed<'static, SNonceSource>,
    pub attempt: Attempt,
}

struct Callbacks(Placed<'static, SNonceSource>);
impl machine::StaCallbacks for Callbacks {
    fn nonce(&mut self) -> [u8; 32] {
        self.0
            .next_nonce()
            .expect("SNonce stream exhausted; reset required")
    }
    fn he_packet_padding(&self, _: &[u8]) -> HePacketPadding {
        HePacketPadding::None
    }
}

struct Output<'a, 'id, Path: rp::PoolPath> {
    link: &'a core::cell::Cell<bool>,
    retry: &'a core::cell::Cell<bool>,
    ready: &'a core::cell::Cell<Option<machine::StaRequest<'id, Path>>>,
    attempt: Attempt,
}
impl<'id, Path: rp::PoolPath> ServiceOutputSink<'id, Caps, Path> for Output<'_, 'id, Path> {
    fn emit(&mut self, event: ServiceOutput<'id, Caps, Path>) {
        match event {
            ServiceOutput::Notice(machine::StaNotice::Connected { .. }) => {
                self.link.set(true);
                self.attempt.publish(StationLinkState::Authorized);
            }
            ServiceOutput::Notice(machine::StaNotice::Disconnected(reason)) => {
                self.link.set(false);
                self.retry.set(true);
                self.attempt.publish(StationLinkState::Disconnected {
                    reason: super::report::disconnect(reason),
                });
            }
            ServiceOutput::CommandCompleted(terminal) => self.ready.set(Some(terminal.request)),
            ServiceOutput::CommandRefused(refused) => {
                self.ready.set(Some(refused.request));
                self.retry.set(true);
            }
            _ => {}
        }
    }
}
/// The observer of the station, its data machine and its service.
type Observed = super::observation::StationObserver;
type Service<'id, Id> =
    StationService<'id, StationComposition<Id>, Callbacks, Rates, 1, Observed>;
type Completed<'p, 'id, Id> =
    ServiceCompleted<'p, 'id, StationComposition<Id>, Callbacks, Rates, 1, Observed>;
type Io<'c, 'id, Id> =
    ServiceIo<'c, 'id, Caps, Output<'c, 'id, Child<Root<Id>, 8>>, 2, 8, 2, Child<Root<Id>, 8>>;
type Running<'s, 'c: 's, 'p: 's, 'id: 'p + 'c, Id: SlotIdentity + 'id> =
    impl Future<Output = Placed<'s, Completed<'p, 'id, Id>>>;

// These layouts name static body loans only to size uninitialized backing.
// StaticPlace lends each allocation with the actual body/input lifetimes.
static SERVICE: ConstStaticCell<StaticPlace<Service<'static, radio::slot::Id>>> =
    ConstStaticCell::new(StaticPlace::new());
// Station agreements use the installation's shared eight-bank pool. The
// independent frame budget is the port's eight outstanding RX credits.
const RX_BA_BANKS: usize = 8;
const RX_BA_FRAMES: usize = port::TX_TICKETS;
const _: () = assert!(
    core::mem::size_of::<
        RxBlockAckStorage<'static, Family<radio::slot::Id>, RX_BA_BANKS, RX_BA_FRAMES>,
    >() <= 2048,
    "the station RX BA owner including pending responses fits its 2 KiB PSRAM budget",
);
static RECEIVE: ConstStaticCell<
    StaticPlace<RxBlockAckStorage<'static, Family<radio::slot::Id>, RX_BA_BANKS, RX_BA_FRAMES>>,
> = ConstStaticCell::new(StaticPlace::new());

static COMPLETED: ConstStaticCell<StaticPlace<Completed<'static, 'static, radio::slot::Id>>> =
    ConstStaticCell::new(StaticPlace::new());
static RUNNING: ConstStaticCell<
    StaticPlace<Running<'static, 'static, 'static, 'static, radio::slot::Id>>,
> = ConstStaticCell::new(StaticPlace::new());

#[define_opaque(Running)]
fn running_future<'s, 'c, 'p, 'id, Id: SlotIdentity + 'id>(
    service: Placed<'s, Service<'id, Id>>,
    output: &'s mut MaybeUninit<Completed<'p, 'id, Id>>,
    owner: rp::Owner<'p, 'id, Id, Backend<Id>, rp::Disabled>,
    io: &'s mut Io<'c, 'id, Id>,
    unwrap: &'s mut KeyUnwrapSlot<SoftwareKeyUnwrap>,
) -> Running<'s, 'c, 'p, 'id, Id>
where
    'id: 'p + 'c,
    'p: 's,
    'c: 's,
{
    StationService::run_placed(service, output, owner, io, &EmbassyClock, unwrap)
}

fn running<'s, 'c, 'p, 'id, Id: SlotIdentity + 'id>(
    service: Placed<'s, Service<'id, Id>>,
    output: &'s mut MaybeUninit<Completed<'p, 'id, Id>>,
    owner: rp::Owner<'p, 'id, Id, Backend<Id>, rp::Disabled>,
    io: &'s mut Io<'c, 'id, Id>,
    unwrap: &'s mut KeyUnwrapSlot<SoftwareKeyUnwrap>,
) -> PlacedFuture<'s, Running<'s, 'c, 'p, 'id, Id>>
where
    'id: 'p + 'c,
    'p: 's,
    'c: 's,
{
    RUNNING
        .take()
        .future(|| running_future(service, output, owner, io, unwrap))
}

// Keep construction's resource and capability temporaries out of the body poll.
#[inline(never)]
fn initialize_service<'id, Id: SlotIdentity + 'id>(
    body: Body,
    resources: <Backend<Id> as PortBackend<Id>>::Resources<'id>,
) -> (
    Placed<'id, Service<'id, Id>>,
    machine::StaCommandCredits<'id, Child<Root<Id>, 8>>,
) {
    let config = super::CONFIG;
    let domain = oer_radio_regulatory::RegulatoryDomain::of(
        oer_radio_regulatory::Country::new(config.country).expect("validated station country"),
    )
    .expect("supported station domain");
    let channel = SupportedChannel::<Caps>::new(
        domain
            .permit(
                Channel::ghz2_4(
                    config.channel,
                    oer_ieee80211_wire::channel::WifiChannelWidth::Mhz20,
                )
                .expect("validated channel"),
            )
            .expect("station permitted channel"),
    )
    .expect("S31 channel");
    let (arbiter, place, absence_place) = PortArbiter::<StationComposition<Id>>::new(
        super::arbiter(port::PUBLICATION_BOUND).expect("station arbiter bound"),
        channel,
        resources.arbiter_authority,
        resources.gate,
    );
    let [t0, t1, t2, t3, t4, t5, t6, t7] = resources.tickets;
    let [b0, b1, b2, b3, b4, b5, b6, b7] = resources.buffers;
    let service_storage = SERVICE.take().uninit();
    let credits = core::cell::Cell::new(None);
    let receive = RxBlockAckStorage::<Family<Id>, RX_BA_BANKS, RX_BA_FRAMES>::init_in_place(
        RECEIVE.take().uninit(),
        resources.block_acks,
        Duration::from_millis(100),
    )
    .view();
    let service = StationService::<StationComposition<Id>, _, _, 1, Observed>::init_in_place(
        service_storage,
        |station_storage| {
            let (station, granted) = Station::init_personal(
                station_storage,
                StaConfig {
                    address: OwnAddress::new(
                        IndividualAddress::for_interface(body.address)
                            .expect("eFuse station address"),
                    ),
                    ssid: body.ssid,
                    channels: channels(domain),
                    dwell: Duration::from_millis(250),
                    skipped_absences: 3,
                    access_point_margin_db: 6,
                    supported_rates: [0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24],
                    association: &ASSOCIATION,
                    listen_interval: oer_ieee80211_sta::request::StationListenInterval::DEFAULT,
                    phy: PhyMode::Ht20,
                    callbacks: Callbacks(body.nonces),
                    observer: super::observation::StationObserver(body.attempt),
                },
                body.password,
                body.pmk,
                StationResources::<_, 4, 4, 1> {
                    configure: resources.station_commands.configure,
                    epochs: LinkEpochSource::new(resources.connection_epoch),
                    tickets: [t0, t1, t2, t3],
                    buffers: [b0, b1, b2, b3],
                    place,
                    absence_place,
                    keys: resources.station_association,
                    group_permission: resources.station_group_permission,
                },
                machine::GroupCapacity::new(),
            );
            credits.set(Some(granted));
            station
        },
        |place| {
            StaData::init_in_place(
                place,
                [t4, t5, t6, t7],
                [b4, b5, b6, b7],
                Backend::<Id>::TX_QUEUE_DEPTH,
                super::observation::StationObserver(body.attempt),
            )
        },
        arbiter,
        Rates(EspressifRateSelection::<Caps>::new(), body.attempt),
        resources.terminal_access,
        receive,
        super::observation::StationObserver(body.attempt),
    );
    let credits = credits.take().expect("station grants command credits");
    (service, credits)
}

impl<Id: SlotIdentity> rp::PortBody<Id, Backend<Id>> for Body {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: rp::Owner<'p, 'id, Id, Backend<Id>, rp::Disabled>,
        resources: <Backend<Id> as PortBackend<Id>>::Resources<'id>,
    ) -> rp::Ended<Id, Backend<Id>, Self::Output>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let (address, seed, attempt) = (self.address, self.seed, self.attempt);
        let (service, credits) = initialize_service::<Id>(self, resources);
        let mut commands = WakingSpscQueue::<ServiceCommand<'id, Caps, Child<Root<Id>, 8>>, 2>::new();
        let tx = NETWORK_TX.take();
        let rx = NETWORK_RX.take();
        let (command_send, command_receive) = commands.split();
        let (tx_send, tx_receive) = tx.split();
        let (rx_send, rx_receive) = rx.split();
        let link = core::cell::Cell::new(false);
        let retry = core::cell::Cell::new(true);
        let stopped = core::cell::Cell::new(None);
        let ready = core::cell::Cell::new(Some(machine::StaRequest::new(1, credits.connect)));
        let mut output = Output {
            link: &link,
            retry: &retry,
            ready: &ready,
            attempt,
        };
        let mut io = ServiceIo {
            commands: command_receive,
            transmit: tx_receive,
            receive: rx_send,
            output: &mut output,
        };
        // The image's key-unwrap engine: software AES, ready on its first poll.
        let mut unwrap = KeyUnwrapSlot::new(SoftwareKeyUnwrap::new(SoftwareAes128));
        let completed_storage = COMPLETED.take().uninit();
        let mut running = core::pin::pin!(running(
            service,
            completed_storage,
            owner,
            &mut io,
            &mut unwrap,
        ));
        let mut controls = core::pin::pin!(reconnect(
            command_send,
            &retry,
            &ready,
            attempt,
            credits.disconnect,
            &stopped,
            &link
        ));
        let mut workload = core::pin::pin!(async {
            embassy_futures::join::join(
                super::network::run(address, seed, tx_send, rx_receive, &link, attempt),
                super::future::Boundary(controls.as_mut()),
            )
            .await
        });
        let completed = match select(
            super::future::Boundary(running.as_mut()),
            super::future::Boundary(workload.as_mut()),
        )
        .await
        {
            Either::First(completed) => completed,
            Either::Second(_) => unreachable!("permanent station network tasks"),
        };
        let (end, _retired) =
            oer_memory::unpack_placed!(completed, ServiceCompleted { end, retired });
        let ended = oer_memory::match_placed!(end, ServiceEnd {
            Stopped(owner) => {
                if let Some(request)=stopped.get() {
                    super::emit(request,&oer_ieee80211_control::StationStopped(super::observation::counters()));
                }
                rp::Ended::uninstall(owner.read(), (), ())
            },
            // A Disconnected the runtime published precedes the service
            // end; the image adds none of its own.
            Failed { owner, fault, custody } => {
                super::end(attempt, StationServiceEnd::Failed(super::report::service(fault.read())));
                drop(custody);
                owner.read().uninstall((), ())
            },
            Poisoned { owner, fault, custody } => {
                let owner = owner.read();
                super::end(attempt, StationServiceEnd::Poisoned(super::report::service(fault.read())));
                drop(custody);
                rp::Ended::fail_stop(owner.fail_stop(), ())
            },
            Quarantined { request, fault, custody } => {
                super::end(attempt, StationServiceEnd::Quarantined(super::report::service(fault.read())));
                drop(custody);
                rp::Ended::fail_stop(request.read(), ())
            },
        });
        ended
    }
}

fn channels(
    domain: oer_radio_regulatory::RegulatoryDomain,
) -> heapless::Vec<SupportedChannel<Caps>, 16> {
    let mut channels = heapless::Vec::new();
    for number in 1..=13 {
        let Ok(channel) =
            Channel::ghz2_4(number, oer_ieee80211_wire::channel::WifiChannelWidth::Mhz20)
        else {
            continue;
        };
        if let Ok(permitted) = domain.permit(channel)
            && let Some(channel) = SupportedChannel::<Caps>::new(permitted)
        {
            channels
                .push(channel)
                .expect("at most thirteen scan channels");
        }
    }
    channels
}
async fn reconnect<'a, 'id: 'a, Path: rp::PoolPath>(
    mut commands: WakingSpscSender<'a, ServiceCommand<'id, Caps, Path>, 2>,
    retry: &core::cell::Cell<bool>,
    ready: &core::cell::Cell<Option<machine::StaRequest<'id, Path>>>,
    attempt: Attempt,
    disconnect: machine::StaCommandCredit<'id, Path, machine::DisconnectCommand>,
    stopped: &core::cell::Cell<Option<oer_hil_protocol::RequestIdentity>>,
    link: &core::cell::Cell<bool>,
) -> ! {
    use oer_time::Timer;
    let mut first = true;
    let mut disconnect = Some(disconnect);
    loop {
        if let Ok(request) = super::STOPS.try_receive()
            && let Some(disconnect) = disconnect.take()
        {
            stopped.set(Some(request));
            super::observation::DEPARTING.store(true, core::sync::atomic::Ordering::Relaxed);
            retry.set(false);
            commands
                .ready()
                .await
                .send(ServiceCommand::Station(machine::StaRequest::new(
                    0xffff, disconnect,
                )));
        }
        if stopped.get().is_some() {
            if !link.get() {
                commands.ready().await.send(ServiceCommand::Stop);
                core::future::pending::<()>().await;
            }
            let _ = EmbassyClock.wait_for(Duration::from_millis(10)).await;
            continue;
        }
        if retry.get()
            && let Some(request) = ready.take()
        {
            if !first {
                attempt.advance();
            }
            first = false;
            retry.set(false);
            commands
                .ready()
                .await
                .send(ServiceCommand::Station(request));
        }
        let _ = EmbassyClock.wait_for(Duration::from_secs(1)).await;
    }
}
