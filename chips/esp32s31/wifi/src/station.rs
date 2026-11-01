//! Image-owned mapping; the station service and IP workload run on P1.
use crate::chip::{
    Binding, Config, InitializationError, Output, Profile, Security, ServiceRunner, StationExit,
    StationRunner, TX, UnavailableCause,
};
use crate::radio::{self, Backend, Caps, Device, Family};
use core::{convert::Infallible, future::Future, marker::PhantomData, mem::MaybeUninit};
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_ieee80211_driver::port::{self, device::Composition};
use oer_espressif_ieee80211_policy::rate_selection::EspressifRateSelection;
use oer_ieee80211_contracts_port::*;
use oer_ieee80211_runtime::station::*;

use oer_ieee80211_core_roles_sta::{
    data::StaData,
    epoch::{EpochContext, LinkEpochSource},
    machine::{self, Station, StationResources},
};
use oer_ieee80211_core_upper_mac::{
    rate_control::RatePeer,
    rate_selection::{FrameRates, RateSelection},
};
use oer_ieee80211_wire::{
    ht::{HtAmpduParameters, HtCapabilityInfo, HtLocalCapabilities, ht_capability_ie},
    station::AssociationCapabilities,
};
use oer_memory::{Placed, StaticPlace};
use oer_radio_port::{self as rp, Child, PortBackend, Root, SlotIdentity};
#[cfg(oer_trace)]
use oer_time::Clock;
use oer_time::{Duration, Instant};

pub type Epoch<Id> = Child<Root<Id>, 4>;
pub struct ImageComposition<W: Binding>(PhantomData<W>);
impl<W: Binding> Composition for ImageComposition<W> {
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
pub struct StationComposition<W: Binding, Id: SlotIdentity>(PhantomData<(W, Id)>);
/// The pool places the station's management frames take; the data machine
/// takes the next [`DATA_BUFFERS`].
const MANAGEMENT: u8 = 4;
/// The buffers the data machine holds: as many as its queues take
/// (`PortData`'s capacity). The pool's other buffers stay unused by this
/// image until the data machine takes more.
const DATA_BUFFERS: usize = 8;
#[cfg(oer_trace)]
pub(crate) static TARGET: oer_memory::OnceValue<([u8; 6], Option<[u8; 6]>)> =
    oer_memory::OnceValue::new();
#[cfg(oer_trace)]
fn observe_terminal<'id, W: Binding, Id: SlotIdentity + 'id>(
    terminal: &LowerMacTerminal<'id, Family<Id, W>>,
) {
    if let LowerMacTerminal::TxCompleted(TxCompleted::Single(completed)) = terminal
        && matches!(completed.outcome(), TxOutcome::Indeterminate(_))
    {
        let observer = W::DriverObserver::default();
        oer_trace::observe!(observer, crate::observation::TxIndeterminate);
    }
}
impl<W: Binding, Id: SlotIdentity> StationPort for StationComposition<W, Id> {
    type Backend = Backend<Id, W>;
    type SlotId = Id;
    type Caps = Caps;
    type Types<'id>
        = Family<Id, W>
    where
        Self: 'id,
        Id: 'id;
    type LentRx = crate::LentRx<W::Notifies>;
    type TerminalStorage<T> = heapless::Deque<T, { port::TERMINAL_CAPACITY }>;
    type Small = port::BufferClass<TX>;
    type Large = port::BufferClass<TX>;
    fn lend_phase<'a, 'id, 'r>(
        phase: &'r mut <Backend<Id, W> as PortBackend<Id>>::Phase<'a, 'id>,
    ) -> &'r mut impl LowerMacPhase<'a, 'id, Caps = Caps, Types = Family<Id, W>>
    where
        Self: 'id,
        Id: 'id,
        'id: 'a,
    {
        phase
    }
    fn terminal<'id>(
        terminal: LowerMacTerminal<'id, Family<Id, W>>,
    ) -> LowerMacTerminal<'id, Family<Id, W>>
    where
        Self: 'id,
        Id: 'id,
    {
        #[cfg(oer_trace)]
        observe_terminal::<W, Id>(&terminal);
        terminal
    }
    fn port_terminal<'id>(
        terminal: LowerMacTerminal<'id, Family<Id, W>>,
    ) -> LowerMacTerminal<'id, Family<Id, W>>
    where
        Self: 'id,
        Id: 'id,
    {
        terminal
    }
    fn tune<'a, 'id>(
        setup: &mut <Backend<Id, W> as PortBackend<Id>>::Setup<'a, 'id>,
        channel: SupportedChannel<Caps>,
    ) where
        Self: 'id,
        Id: 'id,
        'id: 'a,
    {
        setup.set_channel(channel);
    }
    fn event<'id>(event: LowerMacEvent<'id, Family<Id, W>>) -> LowerMacEvent<'id, Family<Id, W>>
    where
        Self: 'id,
        Id: 'id,
    {
        #[cfg(oer_trace)]
        if let LowerMacEvent::Terminal(terminal) = &event {
            observe_terminal::<W, Id>(terminal);
        }
        #[cfg(oer_trace)]
        if let LowerMacEvent::Received { frame, .. } = &event {
            let bytes = frame.bytes();
            if let Some((local, Some(target))) = TARGET.get() {
                if oer_ieee80211_wire::station::parse_open_authentication_response(
                    bytes, local, target,
                )
                .is_some_and(|r| r.status_code == 0)
                {
                    #[cfg(oer_trace)]
                    {
                        let observer = W::DriverObserver::default();
                        oer_trace::observe!(observer, crate::observation::StationAuthenticated);
                    }
                }
                if oer_ieee80211_wire::frame::DecodedHeader::decode(bytes)
                    .is_some_and(|h| h.control().subtype() == 8)
                    && oer_ieee80211_wire::scan::parse_management(bytes)
                        .is_some_and(|r| r.bssid.get() == target)
                {
                    #[cfg(oer_trace)]
                    {
                        let observer = W::DriverObserver::default();
                        oer_trace::observe!(
                            observer,
                            crate::observation::StationBeacon {
                                at_millis: (EmbassyClock.now().as_micros() / 1000) as u32
                            }
                        );
                    }
                }
            }
        }
        event
    }
    #[cfg_attr(
        not(oer_trace),
        allow(unused_variables, reason = "only a trace build observes the span")
    )]
    fn span(span: ServiceSpan, end: bool) {
        let observer = W::DriverObserver::default();
        oer_trace::observe!(observer, crate::observation::ServiceSpan { span, end });
    }
    fn tx_buffer<'id>(
        buffer: port::AnyBuffer<'id, Id, Device<W>, TX, ()>,
    ) -> StationTxBuffer<'id, Self>
    where
        Self: 'id,
        Id: 'id,
    {
        // The identical pool buffers are split once by their affine pool
        // indices: the first four remain with Station, the next with StaData.
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

pub struct Rates<W: Binding>(EspressifRateSelection<Caps>, W::Observer);
impl<W: Binding, M: SignalMetadata> RateSelection<M, port::Ack> for Rates<W> {
    fn packet_number_step(&self) -> oer_ieee80211_core_upper_mac::ccmp::CcmpPacketNumberStep {
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
        oer_trace::observe!(self.1, crate::observation::StationAssociated);
        <EspressifRateSelection<Caps> as RateSelection<M, port::Ack>>::associated(
            &mut self.0,
            peer,
            response,
        );
        oer_trace::observe!(self.1, selected_rate::<M>(&self.0));
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
        #[cfg(oer_trace)]
        if frame == FrameRates::Control
            && matches!(completion, TxOutcome::Ended(SingleEnd::Delivered { .. }))
        {
            oer_trace::observe!(self.1, crate::observation::ControlDelivered);
        }
        #[cfg(oer_trace)]
        if matches!(
            completion,
            TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout)
        ) {
            oer_trace::observe!(self.1, crate::observation::TxHardwareTimeout);
        }
        <EspressifRateSelection<Caps> as RateSelection<M, port::Ack>>::completed(
            &mut self.0,
            now,
            frame,
            completion,
        );
        #[cfg(oer_trace)]
        if frame == FrameRates::Data {
            oer_trace::observe!(self.1, selected_rate::<M>(&self.0));
        }
    }
}

#[cfg(oer_trace)]
fn selected_rate<M: SignalMetadata>(
    selection: &EspressifRateSelection<Caps>,
) -> crate::observation::TxRateSelected {
    use oer_ieee80211_wire::phy::PhyRate;
    crate::observation::TxRateSelected {
        kbps: <EspressifRateSelection<Caps> as RateSelection<M, port::Ack>>::plan(
            selection,
            FrameRates::Data,
            Some::<PhyRate>,
        )
        .map(|plan| plan.first().rate.nominal_kbps()),
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
// IEEE 802.11 HT40, one spatial stream, MCS 0..7 with both short guard
// intervals; the encoder sets the width and guard-interval bits from the
// 40 MHz geometry. SM power save is disabled and the station receives
// A-MPDUs of up to 65 535 octets with MPDUs at least 4 us apart: an AP's
// 32-MPDU aggregate of 1 500-octet MSDUs needs the 65 535-octet limit.
//
// SOURCE: the vendor station's HT Capabilities beside its HE elements,
// `libnet80211.a[ieee80211_ht.o]::ieee80211_add_htcap_body`: SM power save
// disabled (bits 2..3 = 0b11), A-MPDU Parameters 0x17 (exponent 3, spacing
// code 5). Recovered in open-esp-radio-rs da26f64c3
// `crates/roles/esp32s31/ieee80211/sta/src/profile.rs` (SOURCE
// HIL_VENDOR_HE20_NDPA_CBF_2026_07_24, frame 7624), where a static SM power
// save HT40 request was observed to be dropped by a Linux HT40 AP.
const HT40: [u8; 28] = ht_capability_ie(
    HtLocalCapabilities::new(
        u16::from_le_bytes(
            HtCapabilityInfo::from_bytes([0; 2])
                .with_sm_power_save(HtCapabilityInfo::SM_POWER_SAVE_DISABLED)
                .to_bytes(),
        ),
        {
            let [parameters] = HtAmpduParameters::from_bytes([0])
                .with_exponent(HtAmpduParameters::EXPONENT_65535_OCTETS)
                .with_spacing(HtAmpduParameters::SPACING_4_US)
                .to_bytes();
            parameters
        },
        0xff,
        1,
    ),
    match Channel::ghz2_4(
        13,
        oer_ieee80211_wire::channel::WifiChannelWidth::Mhz40Below,
    ) {
        Ok(channel) => channel,
        Err(_) => panic!("HT40 advertisement geometry"),
    },
);
pub(crate) static ASSOCIATION: AssociationCapabilities = AssociationCapabilities {
    ht20: HT20,
    ht40: HT40,
    he20_ht: [0; 28],
    he20: [0; 24],
    he20_extended: [0; 14],
    wmm: [221, 7, 0, 0x50, 0xf2, 2, 0, 1, 0],
};
pub struct Body<W: Binding> {
    runner: ServiceRunner<W>,
    profile: Profile<W>,
}

type Service<'id, W> = Runner<
    'id,
    StationComposition<W, <W as Binding>::Id>,
    crate::chip::CoreCallbacks<<W as Binding>::Callbacks>,
    Rates<W>,
    1,
    <W as Binding>::Sink,
>;
type Completed<'p, 'id, W> = ServiceCompleted<
    'p,
    'id,
    StationComposition<W, <W as Binding>::Id>,
    crate::chip::CoreCallbacks<<W as Binding>::Callbacks>,
    Rates<W>,
    1,
    <W as Binding>::Sink,
>;
type Io<'c, W> = ServiceIo<
    'c,
    Caps,
    Events<'static, Output<<W as Binding>::Events, <W as Binding>::Notifies>, Ch<W>>,
    crate::LentRx<<W as Binding>::Notifies>,
    2,
    { crate::RX_DEPTH },
    2,
    Ch<W>,
>;
/// The channels' notify types of `W`'s station.
type Ch<W> = <<W as Binding>::Notifies as crate::Notifies>::Channels;
type Running<'s, 'c: 's, 'p: 's, 'id: 'p, W: Binding> =
    impl Future<Output = Placed<'s, Completed<'p, 'id, W>>>;
const RX_BA_BANKS: usize = 8;
const RX_BA_FRAMES: usize = port::RX_HELD;
pub(crate) struct ServiceStorage<W: Binding> {
    service: StaticPlace<Service<'static, W>>,
    receive: StaticPlace<RxBlockAckStorage<'static, Family<W::Id, W>, RX_BA_BANKS, RX_BA_FRAMES>>,
    completed: StaticPlace<Completed<'static, 'static, W>>,
    running: StaticPlace<Running<'static, 'static, 'static, 'static, W>>,
}
impl<W: Binding> ServiceStorage<W> {
    pub(crate) const fn new() -> Self {
        Self {
            service: StaticPlace::new(),
            receive: StaticPlace::new(),
            completed: StaticPlace::new(),
            running: StaticPlace::new(),
        }
    }
}
#[define_opaque(Running)]
fn running_future<'s, 'c, 'p, 'id, W: Binding>(
    service: Placed<'s, Service<'id, W>>,
    output: &'s mut MaybeUninit<Completed<'p, 'id, W>>,
    owner: rp::Owner<'p, 'id, W::Id, Backend<W::Id, W>, rp::Disabled>,
    io: &'s mut Io<'c, W>,
    unwrap: &'s mut KeyUnwrapSlot<W::Engine>,
) -> Running<'s, 'c, 'p, 'id, W>
where
    'id: 'p,
    'p: 's,
    'c: 's,
{
    Runner::run_placed(service, output, owner, io, &EmbassyClock, unwrap)
}
fn initialize_service<'id, W: Binding>(
    profile: Profile<W>,
    config: Config,
    resources: <Backend<W::Id, W> as PortBackend<W::Id>>::Resources<'id>,
    storage: &'static mut StaticPlace<Service<'static, W>>,
    receive: &'static mut StaticPlace<
        RxBlockAckStorage<'static, Family<W::Id, W>, RX_BA_BANKS, RX_BA_FRAMES>,
    >,
) -> Placed<'id, Service<'id, W>> {
    const {
        if core::mem::size_of::<
            RxBlockAckStorage<'static, Family<W::Id, W>, RX_BA_BANKS, RX_BA_FRAMES>,
        >() > 8192
        {
            // A 32-MPDU window's reorder slots: placed static storage,
            // never a stack value.
            panic!("RX block-ack owner exceeds 8 KiB");
        }
    }
    let (arbiter, place, absence_place) = PortArbiter::<StationComposition<W, W::Id>>::new(
        crate::policy::arbiter(port::PUBLICATION_BOUND)
            .unwrap_or_else(|_| W::fail(InitializationError::Arbiter)),
        config.supported,
        resources.arbiter_authority,
        resources.gate,
    );
    let [t0, t1, t2, t3, t4, t5, t6, t7] = resources.tickets;
    let [b0, b1, b2, b3, b4, b5, b6, b7, b8, b9, b10, b11, ..] = resources.buffers;
    const {
        if MANAGEMENT != 4 || DATA_BUFFERS != 8 || port::TX_BUFFERS < 12 {
            panic!("the split names four management and eight data buffers of the pool");
        }
    }
    let Profile {
        station,
        security,
        observer,
        sink,
    } = profile;
    let receive = RxBlockAckStorage::<Family<W::Id, W>, RX_BA_BANKS, RX_BA_FRAMES>::init_in_place(
        receive.uninit(),
        resources.block_acks,
        Duration::from_millis(100),
    )
    .view();
    Runner::<StationComposition<W, W::Id>, _, _, 1, W::Sink>::init_in_place(
        storage.uninit(),
        |station_storage| {
            let station_resources = StationResources::<_, 4, 4, 1> {
                configure: resources.station_commands.configure,
                epochs: LinkEpochSource::new(resources.connection_epoch),
                tickets: [t0, t1, t2, t3],
                buffers: [b0, b1, b2, b3],
                place,
                absence_place,
                keys: resources.station_association,
                group_permission: resources.station_group_permission,
            };
            match security {
                Security::Open => Station::init_open(station_storage, station, station_resources),
                Security::Wpa2Personal(pmk) => Station::init_wpa2_personal(
                    station_storage,
                    station,
                    pmk,
                    station_resources,
                    machine::GroupCapacity::new(),
                ),
            }
        },
        |place| {
            StaData::init_in_place(
                place,
                [t4, t5, t6, t7],
                [b4, b5, b6, b7, b8, b9, b10, b11],
                Backend::<W::Id, W>::TX_QUEUE_DEPTH,
            )
        },
        arbiter,
        Rates(EspressifRateSelection::new(), observer),
        resources.terminal_access,
        receive,
        sink,
    )
}
impl<W: Binding> rp::PortBody<W::Id, Backend<W::Id, W>> for Body<W> {
    type Output = StationExit;
    async fn run<'p, 'id>(
        self,
        owner: rp::Owner<'p, 'id, W::Id, Backend<W::Id, W>, rp::Disabled>,
        resources: <Backend<W::Id, W> as PortBackend<W::Id>>::Resources<'id>,
    ) -> rp::Ended<W::Id, Backend<W::Id, W>, StationExit>
    where
        Backend<W::Id, W>: 'id,
        W::Id: 'id,
        'id: 'p,
    {
        let Body { runner, profile } = self;
        let ServiceRunner {
            engine,
            config,
            connection,
            storage,
            mut output,
            ..
        } = runner;
        let crate::ServiceConnection {
            network_stop,
            start,
            commands,
            transmit,
            receive,
            receive_copied,
            runner: control,
        } = connection;
        let ServiceStorage {
            service,
            receive: receive_storage,
            completed,
            running,
        } = storage;
        let stop = config.stop;
        let service = initialize_service::<W>(profile, config, resources, service, receive_storage);
        start.complete(Ok(()));
        let mut io = ServiceIo {
            commands,
            transmit,
            receive,
            receive_copied,
            control,
            output: &mut output,
            network: network_stop,
            stop,
        };
        let mut unwrap = KeyUnwrapSlot::new(engine);
        let done = running
            .future(|| {
                running_future::<W>(service, completed.uninit(), owner, &mut io, &mut unwrap)
            })
            .await;
        let (end, _retired, network) = oer_memory::unpack_placed!(
            done,
            ServiceCompleted {
                end,
                retired,
                network
            }
        );
        let network = network.read().map(crate::failure::convert::network_drain);
        let failed = |fault| crate::failure::FailedStation {
            failure: crate::failure::convert::service::<StationComposition<W, W::Id>>(fault),
            network,
        };
        oer_memory::match_placed!(end,ServiceEnd{
        Stopped(owner)=>rp::Ended::uninstall(owner.read(),(),StationExit::Stopped),
        Failed{owner,fault,custody}=>{drop(custody);owner.read().uninstall((),StationExit::Failed(failed(fault.read())))},
        Poisoned{owner,fault,custody}=>{drop(custody);rp::Ended::fail_stop(owner.read().fail_stop(),StationExit::Poisoned(failed(fault.read())))},
        Quarantined{request,fault,custody}=>{drop(custody);rp::Ended::fail_stop(request.read(),StationExit::Quarantined(failed(fault.read())))}
        })
    }
}
pub async fn serve<W: Binding>(runner: StationRunner<W>, profile: Profile<W>) -> StationExit {
    let StationRunner {
        service: mut runner,
        retire,
        stop,
    } = runner;
    let mut pending = runner.frontend.post(());
    let (poster, front) = core::future::poll_fn(|cx| pending.poll_answer(cx))
        .await
        .unwrap_or_else(|error| W::fail(InitializationError::Request(error)));
    runner.frontend = poster;
    let exit = match front {
        Ok(front) => match rp::run_port(front, Body { runner, profile }).await {
            rp::PortEnd::Uninstalled {
                joined,
                output,
                deadline,
            } => {
                // The exit is published only after the radio release ended,
                // or once the stop's deadline passed first.
                let mut pending = retire.post(joined);
                let released = release_answer::<W>(&mut pending, deadline).await;
                released_exit(output, released)
            }
            rp::PortEnd::FailStopped { outcome, output } => {
                // A body that ended for uninstall reports the fail-stop; a
                // body that requested one already reports its own cause.
                let retirement = |failure| {
                    StationExit::RetirementFailed(crate::failure::convert::retirement(
                        failure,
                        outcome.cause(),
                        matches!(outcome.outcome(), rp::Abandon::Stopped(_)),
                    ))
                };
                match output {
                    StationExit::Stopped => retirement(None),
                    StationExit::Failed(failed) => retirement(Some(failed)),
                    StationExit::Poisoned(_)
                    | StationExit::Quarantined(_)
                    | StationExit::RetirementFailed(_)
                    | StationExit::ReleaseFailed(_)
                    | StationExit::Unavailable(_) => output,
                }
            }
            rp::PortEnd::Abandoned { .. } => StationExit::Unavailable(UnavailableCause::Abandoned),
        },
        Err(error) => {
            runner
                .connection
                .start
                .complete(Err(crate::StartError::Unavailable));
            StationExit::Unavailable(UnavailableCause::Initialization(error))
        }
    };
    // Every release and retirement step ended: Control reads this copy.
    stop.publish(exit);
    exit
}

/// The radio driver task's release answer, waited for until `deadline`
/// ([`crate::release::within`]): an answer after it is never read, the
/// release reports `Deadline`, and the spent poster is never reused.
async fn release_answer<W: Binding>(
    pending: &mut oer_memory::Awaiting<radio::Retire<W>, Result<(), crate::failure::ReleaseCause>>,
    deadline: rp::StopDeadline,
) -> Result<(), crate::failure::ReleaseCause> {
    use crate::failure::ReleaseCause;
    match crate::release::within(&EmbassyClock, deadline, |cx| pending.poll_answer(cx)).await {
        Some(answer) => answer
            .map_err(|lost| ReleaseCause::HandoffLost(lost.observations()))
            .and_then(|(_, released)| released),
        None => Err(ReleaseCause::Deadline),
    }
}

/// An uninstalled station's exit once its radio release ended: a failed
/// release never reports the ordinary Stopped.
fn released_exit(
    output: StationExit,
    released: Result<(), crate::failure::ReleaseCause>,
) -> StationExit {
    let Err(cause) = released else {
        return output;
    };
    let failed = match output {
        StationExit::Stopped => None,
        StationExit::Failed(failed) => Some(failed),
        StationExit::Poisoned(_)
        | StationExit::Quarantined(_)
        | StationExit::RetirementFailed(_)
        | StationExit::ReleaseFailed(_)
        | StationExit::Unavailable(_) => return output,
    };
    StationExit::ReleaseFailed(crate::failure::ReleaseFailure {
        failure: failed.map(|failed| failed.failure),
        network: failed.and_then(|failed| failed.network),
        cause,
    })
}
