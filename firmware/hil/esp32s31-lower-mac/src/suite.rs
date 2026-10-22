//! P2 owns the real lower-MAC port, DMA, RX and cooperative hardware.
use crate::{
    Answer, Request, irq,
    irq::{B, N, Q, S, TX},
};
use core::{
    convert::Infallible,
    future::{Future, poll_fn},
    pin::{Pin, pin},
    task::{Context as PollContext, Poll},
};
use embassy_futures::select::{Either, select};
use esp_hal::rng::RngOwner;
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_board as board;
use oer_esp32s31_hal::{
    ieee80211::{
        dma::{space::ChipDma, tx_storage::TxDmaStorage},
        init::{
            MAC_COLD_HANDSHAKE_DEADLINE, MacDelayEntropy, MacSlowClockCalibration,
            MacSlowClockCalibrationSource,
        },
    },
    root::WifiPartition,
};
use oer_esp32s31_ieee80211_driver::{
    lower_mac::{ConnectionFramePriorities, LowerMacConfig, RadioCoexPriorities},
    ordinary_tx::{OrdinaryTxOwner, WifiTxEntropy},
    port::{
        self,
        device::{Composition, RadioDevice, RadioParts},
        *,
    },
    rx::arena::WifiIrqSignals,
    startup::{
        RadioStartConfig, WifiColdStartConfig, WifiMacPlatform, WifiMacStartConfig,
        start_esp32s31_radio,
    },
    tx::{CappedTxPowerProfile, WifiTxResources},
};
use oer_esp32s31_ieee80211_mac::tx::{TxSlot, runtime::WifiTxRuntimePolicy};
use oer_esp32s31_phy::{PhyConfig, PhyTxTargetPowerProfile};
use oer_esp32s31_radio_client::{MembershipCell, wifi::Wifi};
use oer_hil_protocol::{
    base::RejectReason,
    lower_mac::{CaseTrace, OutcomeTrace, Prepare, Prepared, outcome_trace},
};
use oer_ieee80211_port::{
    backend_protection::BasicRates,
    conformance::{MpduState, catalog, outcomes},
    *,
};
use oer_ieee80211_upper_mac::rate_selection::management_rate;
use oer_ieee80211_wire::{
    address::IndividualAddress,
    channel::{WifiChannel, WifiChannelWidth},
    frame::FrameControl,
    phy::{LegacyRate, PhyRate},
    scan::parse_management,
    sequence::SequenceNumber,
};
use oer_radio_port::PortPhase;

use oer_radio_port::{self as radio, Disabled, Ended, Owner, PortBackend, PortBody};
use oer_radio_regulatory::{Country, RegulatoryDomain};
use oer_time::{Duration, Timer};
use oer_trace::Event as _;
use sha2::{Digest as _, Sha256};
use static_cell::StaticCell;

mod barrier;
mod configure;
mod edca;
mod group;
mod ownership;
use ownership::{Context, FixtureBody};

type Device = RadioDevice<
    Fixture,
    EmbassyClock,
    Platform,
    irq::ImageRoutes,
    irq::ImageDoorbell,
    N,
    B,
    S,
    Q,
    irq::RadioExecutor,
>;
type Backend<Id> = Esp32s31Backend<Id, Device, TX>;
oer_espressif_executor::declare_port_slot!(slot: super::Backend<Id>);

/// The unique static backing lent to the conformance task.
pub struct Slot(&'static mut radio::PortSlot<slot::Id, Backend<slot::Id>>);
impl Slot {
    /// Claim before starting P2; every later claim refuses permanently.
    #[inline(never)]
    pub fn take_static() -> Option<Self> {
        Some(Self(slot::take_static()?))
    }
}

/// Plain HIL output, retained outside futures while a case disables the port.
struct TraceScratch(Option<oer_hil_protocol::lower_mac::Trace>);
struct PendingTrace<'a>(&'a mut TraceScratch);
impl Drop for PendingTrace<'_> {
    fn drop(&mut self) {
        // Cancellation discards the same plain output the old local held.
        self.0.0 = None;
    }
}

type Family<Id> = Types<Id, Device, TX>;
type Caps = Esp32s31Caps<TX>;
static SHARED: StaticCell<PortSharedStorage<TX>> = StaticCell::new();
static STORAGE: StaticCell<PortStorage<Device, TX>> = StaticCell::new();
static DEVICE: StaticCell<Device> = StaticCell::new();
static RETAINED: oer_trace::Retained<256, 4, 33> = oer_trace::Retained::new();
static TRACE: oer_trace::Trace = oer_trace::Trace::new(&RETAINED);
oer_trace::clock!(oer_embassy_time::now_micros);

pub struct Boot {
    pub cell: MembershipCell<Wifi>,
    pub wifi: WifiPartition,
    pub rng: RngOwner<'static>,
    pub station: [u8; 6],
    pub ap: [u8; 6],
    pub routes: irq::ImageRoutes,
    pub bell: irq::ImageDoorbell,
}
struct Platform {
    rng: RngOwner<'static>,
    power: Option<PhyTxTargetPowerProfile>,
}
impl MacDelayEntropy for Platform {
    fn mac_delay_random(&mut self) -> u32 {
        self.rng.random()
    }
}
impl MacSlowClockCalibrationSource for Platform {
    fn mac_slow_clock_calibration(&mut self) -> MacSlowClockCalibration {
        MacSlowClockCalibration::Unavailable
    }
}
impl WifiMacPlatform for Platform {
    fn install_phy_tx_power_profile(&mut self, profile: PhyTxTargetPowerProfile) {
        self.power = Some(profile)
    }
}
struct Entropy;
impl WifiTxEntropy for Entropy {
    fn next_u32(&mut self) -> u32 {
        u32::MAX
    }
}
struct Fixture;
impl Composition for Fixture {
    type Coex = Coex;
    type Profile = oer_espressif_ieee80211_policy::backend::EspressifBackendTxProfile;
    type Entropy = Entropy;
    type Body = FixtureBody;
    type OwnedContext<Id: radio::SlotIdentity> = Context;
    type Context<'p, Id: radio::SlotIdentity + 'p> = Context;
    type OwnedConfigureContext<Id: radio::SlotIdentity> = configure::Witness;
    type ConfigureContext<'p, Id: radio::SlotIdentity + 'p> = configure::Witness;
}
struct Coex;
impl RadioCoexPriorities for Coex {
    fn beacon_window_pti(&self) -> oer_esp32s31_hal::types::MacPti {
        let event = oer_esp32s31_radio_coex::CoexEventId::new(0).expect("beacon event");
        oer_esp32s31_hal::types::MacPti::new(
            oer_esp32s31_radio_coex::CoexPtiTable::VENDOR
                .pti(event)
                .value()
                .into(),
        )
        .expect("four bit PTI")
    }
    fn connection_frame_priorities(&self, _: ConnectionFrame) -> Option<ConnectionFramePriorities> {
        None
    }
}
#[derive(Clone, Copy)]
struct Peer {
    address: IndividualAddress,
    rate: LegacyRate,
    station: [u8; 6],
}
mod futures {
    use super::*;
    pub(super) type InstallFuture =
        impl core::future::Future<Output = Result<PortParts<Device, TX>, &'static str>>;
    static INSTALL_FUTURE: StaticCell<InstallFuture> = StaticCell::new();
    pub(super) struct StaticFuture<F: 'static>(Pin<&'static mut F>);
    impl<F: core::future::Future> core::future::Future for StaticFuture<F> {
        type Output = F::Output;
        #[inline(never)]
        fn poll(
            mut self: Pin<&mut Self>,
            cx: &mut core::task::Context<'_>,
        ) -> core::task::Poll<Self::Output> {
            self.0.as_mut().poll(cx)
        }
    }
    #[inline(never)]
    pub(super) fn install_future(boot: Boot, config: Prepare) -> StaticFuture<InstallFuture> {
        StaticFuture(Pin::static_mut(
            INSTALL_FUTURE.init_with(|| installing(boot, config)),
        ))
    }
    #[define_opaque(InstallFuture)]
    fn installing(boot: Boot, config: Prepare) -> InstallFuture {
        install(boot, config)
    }
}
use futures::install_future;
type Front = radio::SlotFrontend<'static, slot::Id, Backend<slot::Id>>;
enum DriverRequest {
    Initial(Prepare),
    Resume {
        joined: radio::Joined<slot::Id>,
        epoch: u32,
    },
}
static DRIVER_DOORBELL: oer_memory::Doorbell = oer_memory::Doorbell::new();
type DriverSlot = oer_memory::RequestSlot<DriverRequest, Result<Front, RejectReason>>;
pub struct DriverChannel(&'static DriverSlot);
pub struct FrontendChannel(oer_memory::Poster<DriverRequest, Result<Front, RejectReason>>);
/// Place both channel endpoints before either executor starts polling them.
pub fn channels() -> (DriverChannel, FrontendChannel) {
    static REQUEST: StaticCell<DriverSlot> = StaticCell::new();
    let request = REQUEST.init_with(|| oer_memory::RequestSlot::new(&DRIVER_DOORBELL, 1));
    let poster = request.claim().expect("one P1 port requester");
    (DriverChannel(request), FrontendChannel(poster))
}

/// P1 owns portable work and the plain HIL result; no driver owner crosses here.
struct Server {
    driver: Option<oer_memory::Poster<DriverRequest, Result<Front, RejectReason>>>,
    started: bool,
    joined: Option<radio::Joined<slot::Id>>,
    peer: Option<Peer>,
    epoch: u32,
    bodies: &'static ownership::BodyPool,
    output: &'static mut TraceScratch,
}
impl Server {
    fn new(channel: FrontendChannel) -> Self {
        static OUTPUT: StaticCell<TraceScratch> = StaticCell::new();
        Self {
            driver: Some(channel.0),
            started: false,
            joined: None,
            peer: None,
            epoch: 1,
            bodies: ownership::BodyPool::new(),
            output: OUTPUT.init_with(|| TraceScratch(None)),
        }
    }
    async fn frontend(&mut self, request: DriverRequest) -> Result<Front, RejectReason> {
        let mut pending = self
            .driver
            .take()
            .expect("P1 serializes driver requests")
            .post(request);
        let (poster, frontend) = poll_fn(|cx| pending.poll_answer(cx)).await;
        self.driver = Some(poster);
        frontend
    }
    async fn request(&mut self, request: Request) -> Result<Answer, RejectReason> {
        match request {
            Request::Prepare(config) => run_request(pin!(self.prepare(config))).await,
            Request::Run(key) => run_request(pin!(self.exchange(key))).await,
        }
    }
    async fn prepare(&mut self, config: Prepare) -> Result<Answer, RejectReason> {
        if self.started {
            return Err(RejectReason::InvalidState);
        }
        self.started = true;
        let frontend = self.frontend(DriverRequest::Initial(config)).await?;
        let result = radio::run_port(frontend, Discover { config }).await;
        let radio::PortEnd::Uninstalled { output, joined, .. } = result else {
            return Err(RejectReason::Internal);
        };
        self.joined = Some(joined);
        let (peer, prepared) = output.map_err(|_| RejectReason::Internal)?;
        self.peer = Some(peer);
        Ok(Answer::Prepared(prepared))
    }
    async fn exchange(
        &mut self,
        key: oer_hil_protocol::lower_mac::RunCase,
    ) -> Result<Answer, RejectReason> {
        let case = key.case().ok_or(RejectReason::InvalidConfiguration)?;
        if case.extension().is_some() {
            return Err(RejectReason::Unsupported);
        }
        let peer = self.peer.ok_or(RejectReason::InvalidState)?;
        let joined = self.joined.take().ok_or(RejectReason::InvalidState)?;
        self.epoch = self.epoch.checked_add(1).ok_or(RejectReason::Internal)?;
        let frontend = self
            .frontend(DriverRequest::Resume {
                joined,
                epoch: self.epoch,
            })
            .await?;
        let result = radio::run_port(
            frontend,
            Exchange {
                peer,
                case,
                bodies: self.bodies,
                output: self.output,
            },
        )
        .await;
        let radio::PortEnd::Uninstalled { output, joined, .. } = result else {
            return Err(RejectReason::Internal);
        };
        self.joined = Some(joined);
        let trace = output.map_err(|_| RejectReason::Internal)?;
        Ok(Answer::Trace(CaseTrace { case: key, trace }))
    }
}

/// The permanent P2 task owns registration, ordinary finish, and every Drop.
/// P1 receives only Front; neither the manager nor a cleanup guard is sent.
/// Answer publication cannot poll P1 while this higher-priority task runs.
/// The completed registration future ends before unregistered publishes join.
pub async fn drive(boot: Boot, slot: Slot, channel: DriverChannel) {
    let mut slot = oer_espressif_executor::port_slot::StaticPortSlot::new(slot.0);
    let mut boot = Some(boot);
    loop {
        poll_fn(|cx| DRIVER_DOORBELL.poll_rung(cx)).await;
        let Some((request, response)) = channel.0.take_request() else {
            continue;
        };
        let registered = match request {
            DriverRequest::Initial(config) => {
                let Some(boot) = boot.take() else {
                    response.answer(Err(RejectReason::InvalidState));
                    continue;
                };
                let Ok(parts) = install_future(boot, config).await else {
                    response.answer(Err(RejectReason::Internal));
                    continue;
                };
                let backend = Esp32s31Backend::new(parts)
                    .ok()
                    .expect("validated conformance port parts");
                slot.register(backend)
                    .ok()
                    .expect("one initial registration")
            }
            DriverRequest::Resume { joined, epoch } => slot
                .reinstall(joined, |stopped| {
                    stopped
                        .reinstall(epoch)
                        .ok()
                        .expect("Joined retains stopped DMA and a distinct epoch")
                })
                .ok()
                .expect("Joined grants the next endpoint epoch"),
        };
        let (frontend, registration) = registered;
        response.answer(Ok(frontend));
        // The guard was armed before answering. Its only owner is this P2
        // frame, including cancellation before the driver's first poll.
        let completion = { registration.run().await };
        if let Some(completion) = completion {
            completion.unregistered();
        }
    }
}

async fn install(boot: Boot, config: Prepare) -> Result<PortParts<Device, TX>, &'static str> {
    let domain = RegulatoryDomain::of(Country::new(config.country).map_err(|_| "country")?)
        .ok_or("domain")?;
    let channel =
        WifiChannel::new_2_4_ghz(config.channel, WifiChannelWidth::Mhz20).map_err(|_| "channel")?;
    let initial = domain.permit(channel).map_err(|_| "channel refused")?;
    let limits = board::wifi_power_limits(
        i32::from(PhyConfig::production().highest_target_power_quarter_dbm()) * 25,
        Some(1000),
    );
    let ceiling = initial.tx_ceiling(limits);
    let start = RadioStartConfig::new(
        WifiColdStartConfig::new(initial, ceiling),
        WifiMacStartConfig::new(
            MAC_COLD_HANDSHAKE_DEADLINE,
            IndividualAddress::for_interface(boot.station).map_err(|_| "station")?,
            IndividualAddress::for_interface(boot.ap).map_err(|_| "AP")?,
        )
        .map_err(|_| "MAC")?,
    );
    let (cell, stopped) = start_esp32s31_radio(
        boot.cell,
        boot.wifi,
        Platform {
            rng: boot.rng,
            power: None,
        },
        start,
    )
    .await;
    let stopped = stopped.map_err(|_| "radio start")?;
    let power = stopped.start_report().track.tx_power;
    let arena = irq::RX.take().ok_or("RX claimed")?;
    let (rx, _, _) = irq::Arena::bind(
        arena,
        WifiIrqSignals {
            mac: &irq::MAC_SIGNAL,
            power: &irq::POWER_SIGNAL,
        },
    )
    .map_err(|_| "RX bind")?;
    let device = DEVICE.init_with(|| {
        RadioDevice::new(RadioParts {
            wifi: stopped,
            rx,
            interrupt_cell: &irq::LINE,
            actor: cell,
            timer: EmbassyClock,
            routes: boot.routes,
            doorbell: boot.bell,
            domain,
            power_limits: limits,
        })
    });
    macro_rules! slot {
        ($dma:ident, $slot:ident) => {{
            let dma = TxDmaStorage::pin::<ChipDma>(irq::$dma.take().ok_or("TX claimed")?)
                .map_err(|_| "TX bind")?;
            Pin::new(irq::$slot.init(TxSlot::from_dma(dma)))
        }};
    }
    let transmitter = OrdinaryTxOwner::new(WifiTxResources {
        slot: slot!(TX0, SLOT0),
        policy: WifiTxRuntimePolicy::<
            oer_espressif_ieee80211_policy::backend::EspressifBackendTxProfile,
        >::vendor_defaults(),
        power: CappedTxPowerProfile::new(power, ceiling),
        entropy: Entropy,
        timer: EmbassyClock,
    })
    .into_idle()
    .map_err(|_| "TX idle")?;
    let parts = PortParts {
        storage: STORAGE
            .init_with(|| PortStorage::new(device, SHARED.init_with(PortSharedStorage::new))),
        transmitter,
        buffers: [
            slot!(TX1, SLOT1),
            slot!(TX2, SLOT2),
            slot!(TX3, SLOT3),
            slot!(TX4, SLOT4),
            slot!(TX5, SLOT5),
            slot!(TX6, SLOT6),
            slot!(TX7, SLOT7),
            slot!(TX8, SLOT8),
        ],
        config: LowerMacConfig {
            station_address: boot.station,
            channel,
            publication_timeout: PUBLICATION_BOUND,
            tsf_epoch: 1,
        },
        access_point_address: IndividualAddress::for_interface(boot.ap).map_err(|_| "AP")?,
        coex: &Coex,
    };
    let _ = oer_trace::install(&TRACE);
    Ok(parts)
}
struct Discover {
    config: Prepare,
}
impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for Discover {
    type Output = Result<(Peer, Prepared), &'static str>;
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Id, Backend<Id>, Disabled>,
        resources: <Backend<Id> as PortBackend<Id>>::Resources<'id>,
    ) -> Ended<Id, Backend<Id>, Self::Output>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(failed) => return Ended::fail_stop(failed.fail_stop(), Err("enable")),
        };
        let output = {
            let mut phase = owner.phase();
            phase.station().configure(
                resources
                    .station_commands
                    .configure
                    .with_context(configure::Witness::Discovery),
                StationReceive::OtherBssManagement,
            );
            let station = phase.station().address().get();
            match select(
                discover(&mut phase, self.config, station),
                EmbassyClock.wait_for(Duration::from_secs(10)),
            )
            .await
            {
                Either::First(value) => value,
                Either::Second(_) => Err("peer discovery timeout"),
            }
        };
        let owner = match owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
        {
            Ok(owner) => owner,
            Err(failed) => return Ended::fail_stop(failed.fail_stop(), Err("disable")),
        };
        Ended::uninstall(owner, (), output)
    }
}
async fn discover<'p, 'id: 'p, Id: radio::SlotIdentity + 'id>(
    phase: &mut Esp32s31Phase<'p, 'id, Id, Device, TX>,
    config: Prepare,
    station: [u8; 6],
) -> Result<(Peer, Prepared), &'static str> {
    loop {
        match phase
            .next_event()
            .await
            .map_err(|_| "discovery poisoned")?
            .map_err(|_| "discovery drained")?
        {
            LowerMacEvent::Received { frame, .. } => {
                let record = parse_management(frame.bytes(), config.channel, 0);
                phase.recycle(frame);
                let Some(record) = record else { continue };
                let address = record.bssid.individual();
                if Sha256::digest(address.get()).as_slice() != config.peer_sha256 {
                    continue;
                }
                let mut basic_rates = [0; 16];
                let mut len = 0_usize;
                for &rate in record
                    .supported_rates_bytes()
                    .iter()
                    .chain(record.extended_supported_rates_bytes())
                {
                    *basic_rates.get_mut(len).ok_or("too many rates")? = rate;
                    len += 1;
                }
                let basic = BasicRates::from_rate_elements(
                    basic_rates.get(..len).ok_or("rate range")?,
                    &[],
                );
                let rate = management_rate(Band::Ghz2_4, Some(&basic)).ok_or("no basic rate")?;
                return Ok((
                    Peer {
                        address,
                        rate,
                        station,
                    },
                    Prepared {
                        ampdu: Backend::<Id>::AMPDU.is_some(),
                        basic_rates,
                        basic_rates_len: len as u8,
                    },
                ));
            }
            LowerMacEvent::Terminal(LowerMacTerminal::Configured(_)) | LowerMacEvent::Tbtt(_) => {}
            _ => return Err("unexpected discovery event"),
        }
    }
}
struct Exchange<'a> {
    peer: Peer,
    case: catalog::Case,
    bodies: &'static ownership::BodyPool,
    output: &'a mut TraceScratch,
}
impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for Exchange<'_> {
    type Output = Result<oer_hil_protocol::lower_mac::Trace, &'static str>;
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Id, Backend<Id>, Disabled>,
        resources: <Backend<Id> as PortBackend<Id>>::Resources<'id>,
    ) -> Ended<Id, Backend<Id>, Self::Output>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(failed) => return Ended::fail_stop(failed.fail_stop(), Err("enable")),
        };
        match self.case {
            catalog::Case::Ownership(scenario) => {
                run_case(pin!(ownership::run(
                    owner,
                    resources,
                    self.peer,
                    scenario,
                    self.bodies
                )))
                .await
            }
            catalog::Case::Barrier(case) => {
                run_case(pin!(barrier::run(
                    owner,
                    resources,
                    self.peer,
                    case,
                    self.output
                )))
                .await
            }
            catalog::Case::Configure(case) => {
                run_case(pin!(configure::run(owner, resources, case))).await
            }
            catalog::Case::Group(case) => {
                run_case(pin!(group::run(owner, resources, case, self.peer))).await
            }
            catalog::Case::Edca(case) => {
                run_case(pin!(edca::run(owner, resources, self.peer, case))).await
            }
            case => run_case(pin!(outcome(owner, resources, self.peer, case))).await,
        }
    }
}
fn run_case<F: Future>(mut future: Pin<&mut F>) -> impl Future<Output = F::Output> + '_ {
    poll_fn(move |cx| poll_case(future.as_mut(), cx))
}
#[inline(never)]
fn poll_case<F: Future>(future: Pin<&mut F>, cx: &mut PollContext<'_>) -> Poll<F::Output> {
    future.poll(cx)
}
async fn outcome<'p, 'id: 'p, Id: radio::SlotIdentity + 'id>(
    mut owner: Owner<'p, 'id, Id, Backend<Id>, radio::Enabled>,
    resources: <Backend<Id> as PortBackend<Id>>::Resources<'id>,
    peer: Peer,
    case: catalog::Case,
) -> Ended<Id, Backend<Id>, Result<oer_hil_protocol::lower_mac::Trace, &'static str>> {
    let output = match case {
        catalog::Case::Outcome(case) => exchange(&mut owner.phase(), resources, peer, case)
            .await
            .map(oer_hil_protocol::lower_mac::Trace::Outcome),
        _ => Err("fixture unavailable"),
    };
    let owner = match owner
        .disable(&mut |_| core::ops::ControlFlow::Continue(()))
        .await
    {
        Ok(owner) => owner,
        Err(failed) => return Ended::fail_stop(failed.fail_stop(), Err("disable")),
    };
    Ended::uninstall(owner, (), output)
}
async fn exchange<'p, 'id: 'p, Id: radio::SlotIdentity + 'id>(
    phase: &mut Esp32s31Phase<'p, 'id, Id, Device, TX>,
    resources: <Backend<Id> as PortBackend<Id>>::Resources<'id>,
    peer: Peer,
    case: outcomes::Case,
) -> Result<OutcomeTrace, &'static str> {
    let mut access = resources.terminal_access;
    let [ticket, ..] = resources.tickets;
    let [mut buffer, ..] = resources.buffers;
    let admitted = outcomes::Identity {
        ticket: ticket.index(),
        buffer: buffer.index(),
    };
    buffer.bytes_mut().fill(0);
    let rate = phase.rate(PhyRate::Legacy(peer.rate)).ok_or("rate")?;
    let receiver = match case {
        outcomes::Case::Ack => peer.address,
        outcomes::Case::NoResponse(outcomes::Form::Single) => {
            IndividualAddress::new([2, 0, 0, 0, 0, 254]).ok_or("absent address")?
        }
        _ => return Err("physical single outcome required"),
    };
    // SOURCE: IEEE 802.11 management Probe Request, subtype 4.
    let header = HeaderSpec::new(
        FrameControl::new(0x0040).ok_or("probe control")?,
        0,
        receiver,
        peer.station,
        peer.address.get(),
        0x1230,
        None,
        None,
        None,
    )
    .ok_or("header")?;
    let frame = ClassFrame::new(TxFrame::Individual {
        frame: SizedFrame::new(
            AckHeader::new(header).ok_or("ACK header")?,
            MpduLength::new(26).ok_or("length")?,
        )
        .ok_or("frame")?,
        plan: RetryPlan::new(RetryStage {
            rate,
            transmissions: core::num::NonZeroU8::MIN,
            protection: Protection::None,
        }),
    })
    .map_err(|_| "class")?;
    let source = TxSource::Fresh {
        buffer,
        frame,
        body: None,
        protection: TxProtection::Open,
    }
    .check()
    .map_err(|_| "layout")?;
    let source =
        <Family<Id> as TxBufferPool<'id, BufferClass<TX>>>::prepare::<Station, Caps>(source)
            .map_err(|_| "prepare")?;
    oer_trace::start(port::trace::Publication::CHANNEL.mask());
    phase
        .station()
        .submit(
            Attempt {
                context: Context::Plain,
                ticket,
                source,
                access_category: oer_ieee80211_wire::qos::WmmAccessCategory::BestEffort,
                protection: Protection::None,
                power: SupportedPower::new(TxPower::Calibrated).ok_or("power")?,
                coex_operation: TxOperation::Connection(ConnectionFrame::ProbeRequest),
            }
            .admit(),
        )
        .map_err(|_| "submit")?;
    let completed = loop {
        match phase
            .next_event()
            .await
            .map_err(|_| "TX poisoned")?
            .map_err(|_| "TX drained")?
        {
            LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(TxCompleted::Single(
                completed,
            ))) => break completed,
            LowerMacEvent::Received { frame, .. } => phase.recycle(frame),
            LowerMacEvent::Tbtt(_) => {}
            _ => return Err("unexpected TX event"),
        }
    };
    let parts = completed.into_parts(&mut access);
    let (buffer, _) = parts.payload.into_parts();
    let (mut buffer, state) = match buffer {
        AnyBuffer::Fresh(buffer) => (buffer, MpduState::Fresh),
        AnyBuffer::Sent(sent) => {
            let (buffer, _) = <Family<Id> as TxBufferPool<'id, BufferClass<TX>>>::renew(sent);
            (buffer, MpduState::Sent)
        }
    };
    let returned = outcomes::Identity {
        ticket: parts.ticket.index(),
        buffer: buffer.index(),
    };
    let sequence = SequenceNumber::from_sequence_control(u16::from_le_bytes(
        buffer
            .bytes_mut()
            .get(22..24)
            .ok_or("sequence")?
            .try_into()
            .map_err(|_| "sequence")?,
    ));
    let trace = outcomes::Trace::<Ack, Ack, Infallible> {
        admitted,
        returned,
        state,
        rate: PhyRate::Legacy(peer.rate),
        sequence,
        publications: TRACE
            .records()
            .filter(|record| record.decode::<port::trace::Publication>().is_some())
            .count(),
        outcome: outcomes::Outcome::Single(parts.outcome),
    };
    outcome_trace(&trace).ok_or("unrepresentable outcome")
}

macro_rules! plain_context {
    ($ty:ty) => {
        impl<Id: radio::SlotIdentity> radio::ContextProjection<Id> for $ty {
            type View<'a, 'p>
                = &'a Self
            where
                Self: 'p,
                Id: 'p,
                'p: 'a;
            type Parts<'p>
                = Self
            where
                Self: 'p,
                Id: 'p;
            fn view<'a, 'p>(&'a self, _: radio::ContextLoan<'p, Id>) -> Self::View<'a, 'p>
            where
                Self: 'p,
                Id: 'p,
                'p: 'a,
            {
                self
            }
            fn into_parts<'p>(self, _: radio::ContextLoan<'p, Id>) -> Self
            where
                Self: 'p,
                Id: 'p,
            {
                self
            }
        }
        impl<'p, Id: radio::SlotIdentity + 'p> radio::LoanedContext<'p, Id> for $ty {
            type Owned = Self;
            fn enclose(self) -> radio::PortContext<'p, Id, Self> {
                radio::PortContext::new(self)
            }
            fn restore(context: radio::PortContext<'p, Id, Self>) -> Self {
                context.into_parts()
            }
        }
    };
}
plain_context!(Context);
plain_context!(configure::Witness);

pub async fn serve(channel: FrontendChannel) {
    let mut server = Server::new(channel);
    loop {
        core::future::poll_fn(|cx| crate::DOORBELL.poll_rung(cx)).await;
        if let Some((request, response)) = crate::SLOT.take_request() {
            let answer = server
                .request(request)
                .await
                .unwrap_or_else(Answer::Rejected);
            response.answer(answer);
        }
    }
}

fn run_request<F: Future>(mut future: Pin<&mut F>) -> impl Future<Output = F::Output> + '_ {
    poll_fn(move |cx| poll_request(future.as_mut(), cx))
}
#[inline(never)]
fn poll_request<F: Future>(future: Pin<&mut F>, cx: &mut PollContext<'_>) -> Poll<F::Output> {
    future.poll(cx)
}
