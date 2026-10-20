//! P2 owns the real lower-MAC port, DMA, RX and cooperative hardware.
use crate::{
    Answer, Request, irq,
    irq::{B, N, Q, S, TX},
};
use core::{convert::Infallible, pin::Pin};
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
type Backend = Esp32s31Backend<Device, TX>;
type Family = Types<Device, TX>;
type Caps = Esp32s31Caps<TX>;
static STORAGE: StaticCell<PortStorage<Device, TX>> = StaticCell::new();
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
    type Profile = oer_espressif_ieee80211_policy::backend::EspressifBackendTxProfile;
    type Entropy = Entropy;
    type Body = FixtureBody;
    type Context<'id> = Context;
    type ConfigureContext<'id> = configure::Witness;
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
        impl core::future::Future<Output = Result<radio::Installed<Backend>, &'static str>>;
    pub(super) type DiscoverFuture = impl core::future::Future<
            Output = radio::PortEnd<Backend, Result<(Peer, Prepared), &'static str>>,
        >;
    pub(super) type ExchangeFuture = impl core::future::Future<
            Output = radio::PortEnd<
                Backend,
                Result<oer_hil_protocol::lower_mac::Trace, &'static str>,
            >,
        >;
    pub(super) static INSTALL_FUTURE: StaticCell<InstallFuture> = StaticCell::new();
    pub(super) static DISCOVER_FUTURE: StaticCell<DiscoverFuture> = StaticCell::new();
    pub(super) static EXCHANGE_FUTURE: StaticCell<Option<ExchangeFuture>> = StaticCell::new();
    pub(super) struct StaticFuture<F: 'static>(pub Pin<&'static mut F>);
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
    #[inline(never)]
    pub(super) fn discover_future(
        installed: radio::Installed<Backend>,
        body: Discover,
    ) -> StaticFuture<DiscoverFuture> {
        StaticFuture(Pin::static_mut(
            DISCOVER_FUTURE.init_with(|| discovering(installed, body)),
        ))
    }
    #[inline(never)]
    pub(super) fn exchange_future(
        future: &mut Pin<&'static mut Option<ExchangeFuture>>,
        installed: radio::Installed<Backend>,
        body: Exchange,
    ) {
        future.set(None);
        future.set(Some(exchanging(installed, body)));
    }
    #[define_opaque(InstallFuture)]
    pub(super) fn installing(boot: Boot, config: Prepare) -> InstallFuture {
        install(boot, config)
    }
    #[define_opaque(DiscoverFuture)]
    pub(super) fn discovering(
        installed: radio::Installed<Backend>,
        body: Discover,
    ) -> DiscoverFuture {
        radio::run_port(installed, body, oer_radio_port::LocalExecution)
    }
    #[define_opaque(ExchangeFuture)]
    pub(super) fn exchanging(
        installed: radio::Installed<Backend>,
        body: Exchange,
    ) -> ExchangeFuture {
        radio::run_port(installed, body, oer_radio_port::LocalExecution)
    }
}
use futures::*;
#[inline(never)]
fn poll_exchange(
    mut future: Pin<&mut ExchangeFuture>,
) -> impl core::future::Future<
    Output = radio::PortEnd<Backend, Result<oer_hil_protocol::lower_mac::Trace, &'static str>>,
> {
    core::future::poll_fn(move |cx| poll_exchange_once(future.as_mut(), cx))
}
#[inline(never)]
fn poll_exchange_once(
    future: Pin<&mut ExchangeFuture>,
    cx: &mut core::task::Context<'_>,
) -> core::task::Poll<
    radio::PortEnd<Backend, Result<oer_hil_protocol::lower_mac::Trace, &'static str>>,
> {
    core::future::Future::poll(future, cx)
}
pub struct Server {
    boot: Option<Boot>,
    stopped: Option<StoppedPort<Device, TX>>,
    peer: Option<Peer>,
    epoch: u32,
    exchange: Pin<&'static mut Option<ExchangeFuture>>,
    bodies: &'static ownership::BodyPool,
}
impl Server {
    pub fn new(boot: Boot) -> Self {
        Self {
            boot: Some(boot),
            stopped: None,
            peer: None,
            epoch: 1,
            exchange: Pin::static_mut(EXCHANGE_FUTURE.init(None)),
            bodies: ownership::BodyPool::new(),
        }
    }
    pub async fn request(&mut self, request: Request) -> Result<Answer, RejectReason> {
        match request {
            Request::Prepare(config) => {
                let boot = self.boot.take().ok_or(RejectReason::InvalidState)?;
                let station = boot.station;
                let installed = install_future(boot, config)
                    .await
                    .map_err(|_| RejectReason::Internal)?;
                let result = discover_future(installed, Discover { config, station }).await;
                let radio::PortEnd::Uninstalled {
                    output,
                    uninstalled: Uninstalled::Stopped(stopped),
                    ..
                } = result
                else {
                    return Err(RejectReason::Internal);
                };
                self.stopped = Some(stopped);
                let (peer, prepared) = output.map_err(|_| RejectReason::Internal)?;
                self.peer = Some(peer);
                Ok(Answer::Prepared(prepared))
            }
            Request::Run(key) => {
                let case = key.case().ok_or(RejectReason::InvalidConfiguration)?;
                if case.extension().is_some() {
                    return Err(RejectReason::Unsupported);
                }
                let peer = self.peer.ok_or(RejectReason::InvalidState)?;
                let stopped = self.stopped.take().ok_or(RejectReason::InvalidState)?;
                self.epoch = self.epoch.checked_add(1).ok_or(RejectReason::Internal)?;
                let installed = stopped
                    .reinstall(self.epoch)
                    .map_err(|_| RejectReason::Internal)?;
                exchange_future(
                    &mut self.exchange,
                    installed,
                    Exchange {
                        peer,
                        case,
                        bodies: self.bodies,
                    },
                );
                let result = poll_exchange(
                    self.exchange
                        .as_mut()
                        .as_pin_mut()
                        .ok_or(RejectReason::Internal)?,
                )
                .await;
                let radio::PortEnd::Uninstalled {
                    output,
                    uninstalled: Uninstalled::Stopped(stopped),
                    ..
                } = result
                else {
                    return Err(RejectReason::Internal);
                };
                self.stopped = Some(stopped);
                let trace = output.map_err(|_| RejectReason::Internal)?;
                Ok(Answer::Trace(CaseTrace { case: key, trace }))
            }
        }
    }
}
async fn install(boot: Boot, config: Prepare) -> Result<radio::Installed<Backend>, &'static str> {
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
    let device = RadioDevice::new(RadioParts {
        wifi: stopped,
        rx,
        interrupt_cell: &irq::LINE,
        actor: cell,
        timer: EmbassyClock,
        routes: boot.routes,
        doorbell: boot.bell,
        domain,
        power_limits: limits,
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
        storage: STORAGE.init(PortStorage::new(device)),
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
    Backend::new(parts).map_err(|_| "port construction")
}
struct Discover {
    config: Prepare,
    station: [u8; 6],
}
impl PortBody<Backend> for Discover {
    type Output = Result<(Peer, Prepared), &'static str>;
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Backend, Disabled>,
        resources: <Backend as PortBackend>::Resources<'id>,
    ) -> Ended<'id, Backend, Self::Output> {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(failed) => return Ended::fail_stopped(failed.fail_stop(), Err("enable")),
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
            match select(
                discover(&mut phase, self.config, self.station),
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
            Err(failed) => return Ended::fail_stopped(failed.fail_stop(), Err("disable")),
        };
        Ended::uninstall(owner, (), output)
    }
}
async fn discover<'p, 'id: 'p>(
    phase: &mut Esp32s31Phase<'p, 'id, Device, TX>,
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
                        ampdu: Backend::AMPDU.is_some(),
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
struct Exchange {
    peer: Peer,
    case: catalog::Case,
    bodies: &'static ownership::BodyPool,
}
impl PortBody<Backend> for Exchange {
    type Output = Result<oer_hil_protocol::lower_mac::Trace, &'static str>;
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Backend, Disabled>,
        resources: <Backend as PortBackend>::Resources<'id>,
    ) -> Ended<'id, Backend, Self::Output> {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(failed) => return Ended::fail_stopped(failed.fail_stop(), Err("enable")),
        };
        if let catalog::Case::Ownership(scenario) = self.case {
            return ownership::run(owner, resources, self.peer, scenario, self.bodies).await;
        }
        if let catalog::Case::Barrier(case) = self.case {
            return barrier::run(owner, resources, self.peer, case).await;
        }
        if let catalog::Case::Configure(case) = self.case {
            return configure::run(owner, resources, case).await;
        }
        if let catalog::Case::Group(case) = self.case {
            return group::run(owner, resources, case, self.peer).await;
        }
        if let catalog::Case::Edca(case) = self.case {
            return edca::run(owner, resources, self.peer, case).await;
        }
        let output = match self.case {
            catalog::Case::Outcome(case) => {
                exchange(&mut owner.phase(), resources, self.peer, case)
                    .await
                    .map(oer_hil_protocol::lower_mac::Trace::Outcome)
            }
            _ => Err("fixture unavailable"),
        };
        let owner = match owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
        {
            Ok(owner) => owner,
            Err(failed) => return Ended::fail_stopped(failed.fail_stop(), Err("disable")),
        };
        Ended::uninstall(owner, (), output)
    }
}
async fn exchange<'p, 'id: 'p>(
    phase: &mut Esp32s31Phase<'p, 'id, Device, TX>,
    resources: <Backend as PortBackend>::Resources<'id>,
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
    let source = <Family as TxBufferPool<'id, BufferClass<TX>>>::prepare::<Station, Caps>(source)
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
            let (buffer, _) = <Family as TxBufferPool<'id, BufferClass<TX>>>::renew(sent);
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
