//! Image-owned mapping; the station service and IP workload run on P1.
use crate::{
    W3Config,
    irq::TX,
    radio::{self, Backend, Caps, Device, Family},
};
use core::{
    convert::Infallible,
    future::Future,
    marker::PhantomData,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, Ordering},
};
use embassy_futures::join::join;
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_ieee80211_driver::port::{self, device::Composition};
use oer_espressif_ieee80211_policy::rate_selection::EspressifRateSelection;
use oer_ieee80211_port::*;
use oer_ieee80211_runtime::station_service::*;
use oer_ieee80211_sta::{
    data::StaData,
    epoch::{EpochContext, LinkEpochSource},
    machine::{self, StaConfig, Station, StationResources},
};
use oer_ieee80211_upper_mac::{
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
use oer_time::{Clock, Duration, Instant};
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
pub static TARGET: oer_memory::OnceValue<[u8; 6]> = oer_memory::OnceValue::new();
pub static AUTHENTICATED: AtomicBool = AtomicBool::new(false);
pub static LAST_BEACON_MS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
pub static DEPARTING: AtomicBool = AtomicBool::new(false);
pub static DEAUTH: AtomicBool = AtomicBool::new(false);
static NETWORK_RX: ConstStaticCell<
    MsduQueue<EthernetFrame, { oer_firmware_apps_w3::policy::RX_DEPTH }>,
> = ConstStaticCell::new(MsduQueue::new());
static NETWORK_TX: ConstStaticCell<MsduQueue<TxMsdu, 2>> = ConstStaticCell::new(MsduQueue::new());
static RX_COUNTS: oer_memory::OnceValue<rx_counts::ServiceRxCounts> = oer_memory::OnceValue::new();

/// Emit after the traffic workload, keeping snapshots outside its report/future.
pub async fn receive_notes<E>(
    mut emit: impl AsyncFnMut(&oer_diagnostics::Note<'static>) -> Result<(), E>,
) -> Result<(), E> {
    if let Some(counters) = radio::RX_COUNTERS.get() {
        let report = counters.loss.report();
        for (name, counts) in [
            (oer_diagnostics::name!("dma"; 32), report.dma),
            (oer_diagnostics::name!("staged"; 32), report.staged),
            (oer_diagnostics::name!("dma-chained"; 32), report.chained),
            (oer_diagnostics::name!("dma-length"; 32), report.length),
            (oer_diagnostics::name!("rx-pool"; 32), report.pool),
            (oer_diagnostics::name!("prefix"; 32), report.prefix),
            (oer_diagnostics::name!("decrypt-mic"; 32), report.integrity),
            (
                oer_diagnostics::name!("filter-address"; 32),
                report.filtered,
            ),
            (oer_diagnostics::name!("port"; 32), report.delivered),
        ] {
            emit(&oer_firmware_apps_w3::loss::traffic_note(name, counts)).await?;
        }
        let [
            mpdu,
            end,
            data,
            fcs,
            abort,
            other_unicast,
            buffer_full,
            fifo_overflow,
        ] = report.hardware;
        emit(&oer_diagnostics::Note::w3_rx_hardware(
            mpdu,
            end,
            data,
            fcs,
            abort,
            other_unicast,
            buffer_full,
            fifo_overflow,
        ))
        .await?;
        emit(&oer_diagnostics::Note::w3_rx_retry(
            report.retry_echo,
            u32::from(report.retired),
            report.pending,
            report.reserved,
            u32::from(report.scoped),
        ))
        .await?;
    }
    if let Some(counts) = RX_COUNTS.get() {
        for (name, row) in [
            (oer_diagnostics::name!("service"; 32), counts.input),
            (oer_diagnostics::name!("station"; 32), counts.delivered),
            (
                oer_diagnostics::name!("reorder-duplicate"; 32),
                counts.reorder_dropped,
            ),
            (oer_diagnostics::name!("drain"; 32), counts.drained),
            (oer_diagnostics::name!("handled"; 32), counts.handled),
            (oer_diagnostics::name!("queue-full"; 32), counts.queue_full),
            (oer_diagnostics::name!("queue-admitted"; 32), counts.queued),
            (oer_diagnostics::name!("msdu-length"; 32), counts.too_long),
        ] {
            emit(&oer_firmware_apps_w3::loss::traffic_note(name, row)).await?;
        }
        for (name, row) in [
            oer_diagnostics::name!("header"; 32),
            oer_diagnostics::name!("frame-kind"; 32),
            oer_diagnostics::name!("address"; 32),
            oer_diagnostics::name!("fragmented"; 32),
            oer_diagnostics::name!("protection"; 32),
            oer_diagnostics::name!("integrity"; 32),
            oer_diagnostics::name!("replay"; 32),
            oer_diagnostics::name!("payload"; 32),
            oer_diagnostics::name!("not-connected"; 32),
            oer_diagnostics::name!("security"; 32),
        ]
        .into_iter()
        .zip(&counts.rejections)
        {
            emit(&oer_firmware_apps_w3::loss::traffic_note(name, *row)).await?;
        }
    }
    Ok(())
}

fn observe_terminal<'id, Id: SlotIdentity + 'id>(terminal: &LowerMacTerminal<'id, Family<Id>>) {
    if let LowerMacTerminal::TxCompleted(TxCompleted::Single(completed)) = terminal
        && matches!(completed.outcome(), TxOutcome::Indeterminate(_))
    {
        radio::TIMEOUTS.fetch_add(1, Ordering::Relaxed);
    }
}
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
        observe_terminal(&terminal);
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
        if let LowerMacEvent::Terminal(terminal) = &event {
            observe_terminal(terminal);
        }
        if let LowerMacEvent::Received { frame, .. } = &event {
            let bytes = frame.bytes();
            let config = crate::CONFIG;
            if let Some(local) = TARGET.get()
                && oer_ieee80211_wire::station::parse_open_authentication_response(
                    bytes,
                    local,
                    config.stand,
                )
                .is_some_and(|response| response.status_code == 0)
            {
                AUTHENTICATED.store(true, Ordering::Relaxed);
            }
            if oer_ieee80211_wire::frame::DecodedHeader::decode(bytes)
                .is_some_and(|h| h.control().subtype() == 8)
                && oer_ieee80211_wire::scan::parse_management(bytes, config.channel, 0)
                    .is_some_and(|record| record.bssid.get() == config.stand)
            {
                LAST_BEACON_MS.store(
                    (EmbassyClock.now().as_micros() / 1000) as u32,
                    Ordering::Relaxed,
                );
            }
        }
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

struct Rates(EspressifRateSelection<Caps>);
impl<M: SignalMetadata> RateSelection<M, port::Ack> for Rates {
    fn packet_number_step(&self) -> oer_ieee80211_wire::ccmp::CcmpPacketNumberStep {
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
        // W3 has no PS, scan or control traffic once the explicit Disconnect
        // starts. Its sole Control completion is the final deauthentication.
        if DEPARTING.load(Ordering::Relaxed)
            && frame == FrameRates::Control
            && matches!(completion, TxOutcome::Ended(SingleEnd::Delivered { .. }))
        {
            DEAUTH.store(true, Ordering::Relaxed);
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
    pub config: W3Config,
    pub address: [u8; 6],
    pub seed: u64,
}

struct Callbacks;
impl machine::StaCallbacks for Callbacks {
    // W3 admits open associations only; protected handshakes never use this.
    fn nonce(&mut self) -> [u8; 32] {
        [0; 32]
    }
    fn he_packet_padding(&self, _: &[u8]) -> backend_protection::HePacketPadding {
        backend_protection::HePacketPadding::None
    }
}

struct Output<'a> {
    link: &'a core::cell::Cell<bool>,
    disconnected: &'a core::cell::Cell<bool>,
    failed: &'a core::cell::Cell<bool>,
    rx_reasons: &'a core::cell::Cell<u16>,
}
impl<'id, Path: rp::PoolPath> ServiceOutputSink<'id, Caps, Path> for Output<'_> {
    fn emit(&mut self, event: ServiceOutput<'id, Caps, Path>) {
        match event {
            ServiceOutput::RxFrameDropped(reason) => {
                let bit = match reason {
                    machine::StaRxError::Frame(reason) => 1u16 << reason as u16,
                    machine::StaRxError::Security(_) => 1u16 << 9,
                };
                self.rx_reasons.set(self.rx_reasons.get() | bit);
            }
            ServiceOutput::RxMsduRefused(_) => {
                self.rx_reasons.set(self.rx_reasons.get() | (1u16 << 10))
            }
            ServiceOutput::Notice(machine::StaNotice::Connected { .. }) => self.link.set(true),
            ServiceOutput::Notice(machine::StaNotice::Disconnected(_)) => {
                self.link.set(false);
                self.disconnected.set(true);
            }
            ServiceOutput::CommandRefused(_) | ServiceOutput::TxRefused { .. } => {
                self.failed.set(true)
            }
            _ => {}
        }
    }
}

type Service<'id, Id> = StationService<'id, StationComposition<Id>, Callbacks, Rates, 1>;
type Completed<'p, 'id, Id> =
    ServiceCompleted<'p, 'id, StationComposition<Id>, Callbacks, Rates, 1>;
type Io<'c, 'id, Id> = ServiceIo<'c, 'id, Caps, Output<'c>, 2, 8, 2, Child<Root<Id>, 8>>;
type Running<'s, 'c: 's, 'p: 's, 'id: 'p, Id: SlotIdentity + 'id> =
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
) -> Running<'s, 'c, 'p, 'id, Id>
where
    'id: 'p,
    'p: 's,
    'c: 's,
{
    StationService::run_placed(service, output, owner, io, &EmbassyClock)
}

fn running<'s, 'c, 'p, 'id, Id: SlotIdentity + 'id>(
    service: Placed<'s, Service<'id, Id>>,
    output: &'s mut MaybeUninit<Completed<'p, 'id, Id>>,
    owner: rp::Owner<'p, 'id, Id, Backend<Id>, rp::Disabled>,
    io: &'s mut Io<'c, 'id, Id>,
) -> PlacedFuture<'s, Running<'s, 'c, 'p, 'id, Id>>
where
    'id: 'p,
    'p: 's,
    'c: 's,
{
    RUNNING
        .take()
        .future(|| running_future(service, output, owner, io))
}

// Keep construction's resource and capability temporaries out of the body poll.
#[inline(never)]
fn initialize_service<'id, Id: SlotIdentity + 'id>(
    body: &Body,
    resources: <Backend<Id> as PortBackend<Id>>::Resources<'id>,
) -> (
    Placed<'id, Service<'id, Id>>,
    machine::StaCommandCredits<'id, Child<Root<Id>, 8>>,
) {
    let _ = TARGET.set(body.address);
    let domain = oer_radio_regulatory::RegulatoryDomain::of(
        oer_radio_regulatory::Country::new(body.config.country).expect("validated W3 country"),
    )
    .expect("supported W3 domain");
    let channel = SupportedChannel::<Caps>::new(
        domain
            .permit(
                Channel::ghz2_4(
                    body.config.channel,
                    oer_ieee80211_wire::channel::WifiChannelWidth::Mhz20,
                )
                .expect("validated channel"),
            )
            .expect("W3 permitted channel"),
    )
    .expect("S31 channel");
    let (arbiter, place, absence_place) = PortArbiter::<StationComposition<Id>>::new(
        oer_firmware_apps_w3::policy::arbiter(port::PUBLICATION_BOUND).expect("W3 arbiter bound"),
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
    let service = StationService::<StationComposition<Id>, _, _, 1>::init_in_place(
        service_storage,
        |station_storage| {
            let (station, granted) = Station::init_open(
                station_storage,
                StaConfig {
                    address: OwnAddress::new(
                        IndividualAddress::for_interface(body.address)
                            .expect("eFuse station address"),
                    ),
                    ssid: heapless::Vec::from_slice(body.config.ssid)
                        .expect("validated SSID length"),
                    channels: heapless::Vec::from_slice(&[channel]).expect("one channel"),
                    dwell: Duration::from_millis(250),
                    skipped_absences: 3,
                    access_point_margin_db: 6,
                    supported_rates: [0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24],
                    association: &ASSOCIATION,
                    listen_interval: oer_ieee80211_sta::request::StationListenInterval::DEFAULT,
                    phy: PhyMode::Ht20,
                    callbacks: Callbacks,
                },
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
            )
        },
        arbiter,
        Rates(EspressifRateSelection::<Caps>::new()),
        resources.terminal_access,
        receive,
    );
    let credits = credits.take().expect("station grants command credits");
    (service, credits)
}

impl<Id: SlotIdentity> rp::PortBody<Id, Backend<Id>> for Body {
    type Output = oer_firmware_apps_w3::report::Report;
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
        let (service, credits) = initialize_service::<Id>(&self, resources);
        let mut commands = MsduQueue::<ServiceCommand<'id, Caps, Child<Root<Id>, 8>>, 2>::new();
        let tx = NETWORK_TX.take();
        let rx = NETWORK_RX.take();
        let (command_send, command_receive) = commands.split();
        let (tx_send, tx_receive) = tx.split();
        let (rx_send, rx_receive) = rx.split();
        let link = core::cell::Cell::new(false);
        let disconnected = core::cell::Cell::new(false);
        let failed = core::cell::Cell::new(false);
        let rx_reasons = core::cell::Cell::new(0u16);
        let mut output = Output {
            link: &link,
            disconnected: &disconnected,
            failed: &failed,
            rx_reasons: &rx_reasons,
        };
        let mut io = ServiceIo {
            commands: command_receive,
            transmit: tx_receive,
            receive: rx_send,
            output: &mut output,
        };
        let completed_storage = COMPLETED.take().uninit();
        let mut running = core::pin::pin!(running(service, completed_storage, owner, &mut io,));
        let mut workload = core::pin::pin!(crate::network::workload(
            self.config,
            self.address,
            self.seed,
            credits,
            command_send,
            tx_send,
            rx_receive,
            &link,
            &disconnected,
            &failed
        ));
        let mut joined = core::pin::pin!(join(
            crate::future::Boundary(running.as_mut()),
            crate::future::Boundary(workload.as_mut()),
        ));
        let joined = joined.as_mut().await;
        let mut report = joined.1;
        let _ = RX_COUNTS.set(joined.0.retired.counters.rx);
        report.service_rx_queue_full = joined.0.retired.counters.rx_msdus_queue_full.get();
        report.service_rx_rejected = joined
            .0
            .retired
            .counters
            .rx_frames_dropped
            .get()
            .saturating_add(joined.0.retired.counters.rx_msdus_too_long.get());
        report.service_rx_reasons = rx_reasons.get();
        report.port_events_lost = joined.0.retired.counters.events_lost.get();
        let (end, _retired) =
            oer_memory::unpack_placed!(joined.0, ServiceCompleted { end, retired });
        let ended = oer_memory::match_placed!(end, ServiceEnd {
            Stopped(owner) => rp::Ended::uninstall(owner.read(), (), report),
            Failed { owner, fault, custody } => { drop((fault, custody)); owner.read().uninstall((), report) },
            Poisoned { owner, fault, custody } => { drop((fault, custody)); rp::Ended::fail_stop(owner.read().fail_stop(), report) },
            Quarantined { request, fault, custody } => { drop((fault, custody)); rp::Ended::fail_stop(request.read(), report) },
        });
        ended
    }
}
