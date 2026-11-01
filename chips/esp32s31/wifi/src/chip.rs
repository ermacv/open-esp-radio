use crate::{Caps, Channels, Control, Network, ServiceConnection, radio, station as service};
use oer_esp32s31_compose::Esp32s31;
use oer_esp32s31_ieee80211_driver::irq::{
    DoorbellAcknowledge, RadioDoorbell, WifiIrqLine, WifiIrqPart, service_wifi_line,
};
use oer_espressif_executor::{Doorbell, DoorbellAcknowledger, InterruptExecutor};
use oer_espressif_interrupt_table::{Bound, Entry, InterruptSources, signal::IrqSignal};
use oer_ieee80211_core_roles_sta::machine::{StaCallbacks, StaConfig};
use oer_ieee80211_runtime::station as runtime;
use oer_ieee80211_wire::{
    address::{IndividualAddress, OwnAddress},
    channel::WifiChannelWidth,
};
use oer_memory::HandlerCell;
use oer_radio_port::SlotIdentity;
use oer_radio_regulatory::{Country, RegulatoryDomain};
use oer_time::Duration;
use oer_trace::Observer;

pub const N: usize = 32;
pub const B: usize = crate::FRAME_CAPACITY;
pub const S: usize = B;
pub const Q: usize = 32;
pub const TX: usize = crate::FRAME_CAPACITY;
pub type RadioExecutor = InterruptExecutor<Esp32s31, 2>;
pub(crate) type Rng = oer_esp32s31_compose::platform::Entropy;
/// Image-selected entries, callbacks, observers, event sink and key-unwrap engine.
pub trait Binding: Sized + 'static {
    type Id: SlotIdentity + 'static;
    type Wifi: Entry<Part = WifiIrqPart<Lower>>;
    type Radio: Entry;
    type Callbacks: crate::Callbacks + 'static;
    /// Receives the facade's own trace events ([`crate::observation`]).
    type Observer: Observer + Clone + 'static;
    type DriverObserver: Observer + Default + 'static;
    /// The sink the station service lends its cores: the protocol's events.
    /// `()` keeps none.
    type Sink: oer_ieee80211_runtime::station::StationEvents + 'static;
    type Engine: oer_crypto::KeyUnwrap<
            { oer_ieee80211_core_roles_sta::machine::KEY_DATA_CAPACITY },
            Operation: Unpin,
        > + 'static;
    type Events: Events + 'static;
    fn take_slot() -> Option<&'static mut radio::Slot<Self>>;
    /// Image-owned terminal policy, called with a handled construction error.
    fn fail(error: InitializationError) -> !;
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitializationError {
    Phy(oer_esp32s31_compose::phy_fault::Failure),
    Country,
    Domain,
    Channel,
    Address,
    Ssid,
    Arbiter,
    Storage,
    Control,
    Frontend,
    MembershipUnavailable,
    Request(oer_memory::RequestError),
    Radio,
    Backend,
    Registration,
    Rx,
    Tx,
    Dma(crate::DmaError),
}
pub struct Lower(pub DoorbellAcknowledger<Esp32s31, 3>);
impl DoorbellAcknowledge for Lower {
    fn acknowledge(&mut self) {
        self.0.acknowledge();
    }
}
pub struct Bell<E: Entry, X: Entry>(pub Doorbell<Esp32s31, 3, E, X>);
impl<E: Entry, X: Entry> RadioDoorbell for Bell<E, X> {
    const HANDLER: oer_espressif_interrupt_table::Context = Doorbell::<Esp32s31, 3, E, X>::HANDLER;
    const EXECUTOR: oer_espressif_interrupt_table::Context =
        Doorbell::<Esp32s31, 3, E, X>::EXECUTOR;
    fn ring(&self) {
        self.0.ring();
    }
}
/// Types and operations of the permanently bound interrupt side.
pub trait InterruptTypes {
    type Part: InterruptSources;
    fn part(
        banks: oer_esp32s31_hal::owner::WifiInterruptBanks,
        irq: oer_esp32s31_hal::peripherals::software_interrupt::FromCpu3Irq,
        lower: Lower,
    ) -> Self::Part;
    fn service(&self, part: &mut Self::Part) -> Result<bool, u32>;
}
#[derive(bytemuck::Zeroable)]
pub struct Interrupts {
    pub(crate) line: HandlerCell<WifiIrqLine<RadioExecutor>>,
}
impl InterruptTypes for Interrupts {
    type Part = WifiIrqPart<Lower>;
    fn part(
        banks: oer_esp32s31_hal::owner::WifiInterruptBanks,
        irq: oer_esp32s31_hal::peripherals::software_interrupt::FromCpu3Irq,
        lower: Lower,
    ) -> Self::Part {
        WifiIrqPart::new(banks, irq, lower)
    }
    fn service(&self, part: &mut Self::Part) -> Result<bool, u32> {
        service_wifi_line(&self.line, part)
            .map_err(|oer_esp32s31_ieee80211_driver::mac::irq::Fault::FatalAssertion { raw }| raw)
    }
}
#[derive(Clone, Copy)]
pub struct Config {
    pub(crate) reconnect: crate::ReconnectPolicy,
    pub(crate) stop: runtime::StopPolicy,
    pub(crate) channel: u8,
    pub(crate) country: [u8; 2],
    pub(crate) cap_dbm: i8,
    #[cfg(oer_trace)]
    pub(crate) target: Option<[u8; 6]>,
    pub(crate) all_channels: bool,
    pub(crate) domain: RegulatoryDomain,
    pub(crate) supported: oer_ieee80211_contracts_port::SupportedChannel<Caps>,
}
impl Config {
    pub fn with_reconnect(mut self, policy: crate::ReconnectPolicy) -> Self {
        self.reconnect = policy;
        self
    }
    /// Replace the deadline of a stop no request timed, which
    /// [`Self::new`] took.
    pub fn with_stop(mut self, stop: runtime::StopPolicy) -> Self {
        self.stop = stop;
        self
    }
    /// Name the access point whose frames a trace build observes; without
    /// it, none is selected.
    #[cfg(oer_trace)]
    pub fn with_trace_target(mut self, target: [u8; 6]) -> Self {
        self.target = Some(target);
        self
    }
    /// The same signature in every build; a trace build's target is chosen
    /// with [`Self::with_trace_target`].
    /// `stop` bounds the network's return of the station's lent receive
    /// buffers for a stop no request timed: a stop after a failure.
    pub fn new(
        channel: u8,
        country: [u8; 2],
        cap_dbm: i8,
        all_channels: bool,
        stop: runtime::StopPolicy,
    ) -> Result<Self, InitializationError> {
        let domain =
            RegulatoryDomain::of(Country::new(country).map_err(|_| InitializationError::Country)?)
                .ok_or(InitializationError::Domain)?;
        let channel_value =
            oer_ieee80211_contracts_port::Channel::ghz2_4(channel, WifiChannelWidth::Mhz20)
                .map_err(|_| InitializationError::Channel)?;
        let supported = oer_ieee80211_contracts_port::SupportedChannel::new(
            domain
                .permit(oer_ieee80211_contracts_port::RegulatedChannel::from(
                    channel_value,
                ))
                .map_err(|_| InitializationError::Channel)?,
        )
        .ok_or(InitializationError::Channel)?;
        Ok(Self {
            reconnect: crate::ReconnectPolicy::DISABLED,
            stop,
            channel,
            country,
            cap_dbm,
            #[cfg(oer_trace)]
            target: None,
            all_channels,
            domain,
            supported,
        })
    }
}
pub(crate) struct CoreCallbacks<C>(C);
impl<C: crate::Callbacks> StaCallbacks for CoreCallbacks<C> {
    fn nonce(&mut self) -> [u8; 32] {
        self.0.nonce()
    }
    fn he_packet_padding(
        &self,
        _: &[u8],
    ) -> oer_ieee80211_core_upper_mac::protection::HePacketPadding {
        oer_ieee80211_core_upper_mac::protection::HePacketPadding::None
    }
}
/// What the station runner starts with: the station's own address, scan
/// channels and callbacks. The network each connection joins comes later,
/// with its connect ([`crate::Control::connect`]).
pub struct Profile<W: Binding> {
    pub(crate) station: StaConfig<Caps, CoreCallbacks<W::Callbacks>>,
    pub(crate) observer: W::Observer,
    pub(crate) sink: W::Sink,
}
impl<W: Binding> Profile<W> {
    pub fn new(
        config: &Config,
        address: [u8; 6],
        callbacks: W::Callbacks,
        observer: W::Observer,
        sink: W::Sink,
    ) -> Result<Self, InitializationError> {
        let mut channels = heapless::Vec::new();
        if config.all_channels {
            for n in 1..=13 {
                let Ok(ch) =
                    oer_ieee80211_contracts_port::Channel::ghz2_4(n, WifiChannelWidth::Mhz20)
                else {
                    continue;
                };
                if let Ok(p) = config
                    .domain
                    .permit(oer_ieee80211_contracts_port::RegulatedChannel::from(ch))
                    && let Some(c) = oer_ieee80211_contracts_port::SupportedChannel::<Caps>::new(p)
                {
                    channels.push(c).map_err(|_| InitializationError::Channel)?;
                }
            }
        } else {
            channels
                .push(config.supported)
                .map_err(|_| InitializationError::Channel)?;
        }
        Ok(Self {
            station: StaConfig {
                address: OwnAddress::new(
                    IndividualAddress::for_interface(address)
                        .map_err(|_| InitializationError::Address)?,
                ),
                channels,
                dwell: Duration::from_millis(250),
                skipped_absences: 3,
                access_point_margin_db: 6,
                supported_rates: [0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24],
                association: &service::ASSOCIATION,
                listen_interval:
                    oer_ieee80211_core_roles_sta::request::StationListenInterval::DEFAULT,
                phy: oer_ieee80211_core_upper_mac::phy_mode::PhyMode::Ht20,
                callbacks: CoreCallbacks(callbacks),
            },
            observer,
            sink,
        })
    }
}
/// Product callbacks are immediate, concrete, and never queued or coalesced.
pub trait Events {
    fn event(&mut self, event: Event);
}
impl Events for () {
    fn event(&mut self, _: Event) {}
}
#[expect(
    clippy::large_enum_variant,
    reason = "a refused original frame reaches its callback without allocation or loss"
)]
pub enum Event {
    Connected {
        bssid: [u8; 6],
        channel: u8,
    },
    Disconnected(runtime::contract::DisconnectReason),
    CommandRefused(runtime::CommandRefused),
    CommandCompleted(runtime::CommandTerminal),
    TxRefused {
        operation: runtime::TxOperation,
        reason: runtime::contract::TransmitRefusal,
    },
    CommandStarted {
        request: runtime::Request,
        status: runtime::Status,
    },
    MsduRefused {
        packet: runtime::contract::TransmitPacket,
        reason: runtime::contract::MsduRefusal,
    },
    AccessPoint(runtime::contract::AccessPointCommand<Caps>),
}
pub(crate) struct Output<E: Events> {
    pub events: E,
    pub status: crate::StatusReader<'static>,
}
impl<E: Events> runtime::ServiceOutputSink<Caps> for Output<E> {
    fn emit(&mut self, event: runtime::ServiceOutput<Caps>) {
        let event = match event {
            runtime::ServiceOutput::Notice(runtime::contract::Notice::Connected {
                bssid,
                channel,
            }) => Event::Connected { bssid, channel },
            runtime::ServiceOutput::Notice(runtime::contract::Notice::Disconnected(r)) => {
                Event::Disconnected(r)
            }
            runtime::ServiceOutput::CommandRefused(r) => Event::CommandRefused(r),
            runtime::ServiceOutput::CommandCompleted(r) => Event::CommandCompleted(r),
            runtime::ServiceOutput::TxRefused { operation, reason } => {
                Event::TxRefused { operation, reason }
            }
            runtime::ServiceOutput::CommandStarted(request) => Event::CommandStarted {
                request,
                status: self.status.status(),
            },
            runtime::ServiceOutput::MsduRefused { packet, reason } => {
                Event::MsduRefused { packet, reason }
            }
            runtime::ServiceOutput::AccessPoint(command) => Event::AccessPoint(command),
        };
        self.events.event(event)
    }
}
pub struct Storage<W: Binding> {
    pub(crate) channels: Channels,
    pub(crate) driver: radio::DriverStorage<W>,
    pub(crate) service: service::ServiceStorage<W>,
    pub(crate) bell: oer_memory::Doorbell,
    pub(crate) ready: static_cell::StaticCell<radio::ReadySlot<W>>,
    pub(crate) retire: static_cell::StaticCell<radio::RetireSlot<W>>,
}
impl<W: Binding> Storage<W> {
    pub const fn new() -> Self {
        Self {
            channels: Channels::new(),
            driver: radio::DriverStorage::new(),
            service: service::ServiceStorage::new(),
            bell: oer_memory::Doorbell::new(),
            ready: static_cell::StaticCell::new(),
            retire: static_cell::StaticCell::new(),
        }
    }
}
impl<W: Binding> Default for Storage<W> {
    fn default() -> Self {
        Self::new()
    }
}
pub struct Resources<W: Binding> {
    pub engine: W::Engine,
    pub events: W::Events,
    pub storage: &'static mut Storage<W>,
    pub hardware: crate::WifiHardware,
    pub rng: Rng,
    pub address: [u8; 6],
    pub ap: [u8; 6],
    pub bound: Bound<W::Wifi>,
    pub bell: Bell<W::Wifi, W::Radio>,
    pub entropy: u32,
    pub interrupts: &'static Interrupts,
    pub mac: &'static IrqSignal<RadioExecutor>,
    pub power: &'static IrqSignal<RadioExecutor>,
    pub dma: crate::DmaResources,
}
pub struct Runners<W: Binding> {
    pub driver: Driver<W>,
    pub station: StationRunner<W>,
}
pub struct Driver<W: Binding> {
    pub(crate) boot: radio::Boot<W>,
    pub(crate) ready: &'static radio::ReadySlot<W>,
    pub(crate) retire: &'static radio::RetireSlot<W>,
    pub(crate) bell: &'static oer_memory::Doorbell,
}
pub struct StationRunner<W: Binding> {
    pub(crate) service: ServiceRunner<W>,
    /// Hands the ordinary join to the driver task, which answers once its
    /// radio release has ended.
    pub(crate) retire:
        oer_memory::Poster<radio::Retire<W>, Result<(), crate::failure::ReleaseCause>>,
    /// Publishes the exit to Control once it is final.
    pub(crate) stop: crate::StopGuard<'static, StationExit>,
}
pub(crate) struct ServiceRunner<W: Binding> {
    pub(crate) engine: W::Engine,
    pub(crate) frontend: oer_memory::Poster<(), Result<radio::Front<W>, InitializationError>>,
    pub(crate) config: Config,
    pub(crate) connection: ServiceConnection<'static>,
    pub(crate) storage: &'static mut service::ServiceStorage<W>,
    pub(crate) output: runtime::Events<'static, Output<W::Events>>,
}
/// Terminal result owns every typed cause, with no backend identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationExit {
    Stopped,
    Failed(crate::failure::FailedStation),
    Poisoned(crate::failure::FailedStation),
    Quarantined(crate::failure::FailedStation),
    /// Ended for uninstall, but the installation fail-stopped instead.
    RetirementFailed(crate::failure::RetirementFailure),
    /// Retired, but the Wi-Fi radio release did not complete.
    ReleaseFailed(crate::failure::ReleaseFailure),
    Unavailable(UnavailableCause),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnavailableCause {
    Abandoned,
    Initialization(InitializationError),
}
pub fn station<W: Binding>(
    resources: Resources<W>,
    config: Config,
) -> (
    Control<'static>,
    Network<'static>,
    crate::NetworkStopper<'static>,
    Runners<W>,
) {
    let Resources {
        engine,
        events,
        storage,
        hardware,
        rng,
        address,
        ap,
        bound,
        bell,
        entropy,
        interrupts,
        mac,
        power,
        dma,
    } = resources;
    let crate::WifiHardware { cell, wifi } = hardware;
    let crate::DmaResources { rx, tx } = dma;
    let (mut control, network, stopper, connection) = storage
        .channels
        .split()
        .unwrap_or_else(|_| W::fail(InitializationError::Control));
    let crate::Connection {
        service: connection,
        stop,
    } = connection;
    #[cfg(oer_trace)]
    let _ = service::TARGET.set((address, config.target));
    control
        .inner
        .configure_reconnect(config.reconnect.runtime());
    let output = control.inner.events(Output {
        events,
        status: control.status_reader(),
    });
    let ready = storage
        .ready
        .try_init_with(|| radio::ReadySlot::<W>::new(&storage.bell, 1, 0x100))
        .unwrap_or_else(|| W::fail(InitializationError::Storage));
    let frontend = ready
        .claim()
        .unwrap_or_else(|| W::fail(InitializationError::Frontend));
    let retire = storage
        .retire
        .try_init_with(|| radio::RetireSlot::<W>::new(&storage.bell, 2, 0x101))
        .unwrap_or_else(|| W::fail(InitializationError::Storage));
    let retire_poster = retire
        .claim()
        .unwrap_or_else(|| W::fail(InitializationError::Frontend));
    (
        control,
        network,
        stopper,
        Runners {
            driver: Driver {
                boot: radio::Boot {
                    cell,
                    wifi,
                    rng,
                    station: address,
                    ap,
                    bound,
                    bell,
                    config,
                    entropy,
                    interrupts,
                    mac,
                    power,
                    rx,
                    tx,
                    storage: &mut storage.driver,
                },
                ready,
                retire,
                bell: &storage.bell,
            },
            station: StationRunner {
                service: ServiceRunner {
                    engine,
                    frontend,
                    config,
                    connection,
                    storage: &mut storage.service,
                    output,
                },
                retire: retire_poster,
                stop,
            },
        },
    )
}
impl<W: Binding> Driver<W> {
    pub async fn run(self) {
        radio::drive(self).await
    }
}
impl<W: Binding> StationRunner<W> {
    pub async fn run(self, profile: Profile<W>) -> StationExit {
        service::serve(self, profile).await
    }
}
