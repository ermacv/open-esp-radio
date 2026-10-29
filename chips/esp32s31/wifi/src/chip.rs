use crate::{Caps, Channels, Connection, Control, Network, radio, station as service};
use oer_esp32s31_compose::Esp32s31;
use oer_esp32s31_hal::{ieee80211::dma::tx_storage::TxDmaStorage, root::WifiPartition};
use oer_esp32s31_ieee80211_driver::{
    irq::{DoorbellAcknowledge, RadioDoorbell, WifiIrqLine, WifiIrqPart, service_wifi_line},
    rx::arena::WifiRxStatic,
};
use oer_espressif_executor::{Doorbell, DoorbellAcknowledger, InterruptExecutor};
use oer_espressif_interrupt_table::{Bound, Entry, InterruptSources, signal::IrqSignal};
use oer_ieee80211_runtime::station as runtime;
use oer_ieee80211_sta::machine::{StaCallbacks, StaConfig};
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
pub type Rng = oer_esp32s31_hal::peripherals::entropy::Entropy<oer_embassy_time::EmbassyClock>;
/// Image-selected entries, callbacks, observers and key-unwrap engine.
pub trait Binding: Sized + 'static {
    type Id: SlotIdentity + 'static;
    type Wifi: Entry<Part = WifiIrqPart<Lower>>;
    type Radio: Entry;
    type Callbacks: StaCallbacks + 'static;
    type Observer: Observer + Clone + 'static;
    type DriverObserver: Observer + Default + 'static;
    type Engine: oer_crypto::KeyUnwrap<{ oer_ieee80211_sta::machine::KEY_DATA_CAPACITY }, Operation: Unpin>
        + 'static;
    type Events: Events + 'static;
    fn take_slot() -> Option<&'static mut radio::Slot<Self>>;
    /// Image-owned terminal policy, called with a handled construction error.
    fn fail(error: InitializationError) -> !;
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitializationError {
    Phy(oer_esp32s31_hal::domain::transaction::Fault),
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
            .map_err(|oer_esp32s31_ieee80211_mac::irq::Fault::FatalAssertion { raw }| raw)
    }
}
pub trait DmaTypes {
    type Rx: bytemuck::Zeroable;
    type Tx: bytemuck::Zeroable;
}
impl DmaTypes for Interrupts {
    type Rx = WifiRxStatic<N, B, S, Q, RadioExecutor>;
    type Tx = TxDmaStorage<TX>;
}
#[derive(Clone, Copy)]
pub struct Config {
    pub(crate) channel: u8,
    pub(crate) country: [u8; 2],
    pub(crate) cap_dbm: i8,
    pub(crate) target: Option<[u8; 6]>,
    pub(crate) all_channels: bool,
    pub(crate) domain: RegulatoryDomain,
    pub(crate) supported: oer_ieee80211_port::SupportedChannel<Caps>,
}
impl Config {
    pub fn new(
        channel: u8,
        country: [u8; 2],
        cap_dbm: i8,
        target: Option<[u8; 6]>,
        all_channels: bool,
    ) -> Result<Self, InitializationError> {
        let domain =
            RegulatoryDomain::of(Country::new(country).map_err(|_| InitializationError::Country)?)
                .ok_or(InitializationError::Domain)?;
        let channel_value = oer_ieee80211_port::Channel::ghz2_4(channel, WifiChannelWidth::Mhz20)
            .map_err(|_| InitializationError::Channel)?;
        let supported = oer_ieee80211_port::SupportedChannel::new(
            domain
                .permit(channel_value)
                .map_err(|_| InitializationError::Channel)?,
        )
        .ok_or(InitializationError::Channel)?;
        Ok(Self {
            channel,
            country,
            cap_dbm,
            target,
            all_channels,
            domain,
            supported,
        })
    }
}
pub struct OpenCallbacks;
impl StaCallbacks for OpenCallbacks {
    fn nonce(&mut self) -> [u8; 32] {
        [0; 32]
    }
    fn he_packet_padding(&self, _: &[u8]) -> oer_ieee80211_upper_mac::protection::HePacketPadding {
        oer_ieee80211_upper_mac::protection::HePacketPadding::None
    }
}
pub(crate) enum Security {
    Open,
    Personal {
        oer_password: oer_ieee80211_rsn::sae::SaePassword,
        pmk: oer_ieee80211_rsn::Pmk,
    },
}
pub struct Profile<W: Binding> {
    pub(crate) station: StaConfig<Caps, W::Callbacks, W::Observer>,
    pub(crate) security: Security,
}
impl<W: Binding> Profile<W> {
    pub fn open(
        config: &Config,
        address: [u8; 6],
        ssid: &[u8],
        callbacks: W::Callbacks,
        observer: W::Observer,
    ) -> Result<Self, InitializationError> {
        let mut channels = heapless::Vec::new();
        if config.all_channels {
            for n in 1..=13 {
                let Ok(ch) = oer_ieee80211_port::Channel::ghz2_4(n, WifiChannelWidth::Mhz20) else {
                    continue;
                };
                if let Ok(p) = config.domain.permit(ch)
                    && let Some(c) = oer_ieee80211_port::SupportedChannel::<Caps>::new(p)
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
                ssid: heapless::Vec::from_slice(ssid).map_err(|_| InitializationError::Ssid)?,
                channels,
                dwell: Duration::from_millis(250),
                skipped_absences: 3,
                access_point_margin_db: 6,
                supported_rates: [0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24],
                association: &service::ASSOCIATION,
                listen_interval: oer_ieee80211_sta::request::StationListenInterval::DEFAULT,
                phy: oer_ieee80211_upper_mac::phy_mode::PhyMode::Ht20,
                callbacks,
                observer,
            },
            security: Security::Open,
        })
    }
    pub fn personal(
        config: &Config,
        address: [u8; 6],
        ssid: &[u8],
        credentials: (oer_ieee80211_rsn::sae::SaePassword, oer_ieee80211_rsn::Pmk),
        callbacks: W::Callbacks,
        observer: W::Observer,
    ) -> Result<Self, InitializationError> {
        let mut p = Self::open(config, address, ssid, callbacks, observer)?;
        let (password, pmk) = credentials;
        p.security = Security::Personal {
            oer_password: password,
            pmk,
        };
        Ok(p)
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
    CommandInFlight(runtime::Request),
    TxRefused {
        operation: runtime::TxOperation,
        reason: runtime::contract::TransmitRefusal,
    },
    Other(runtime::ServiceOutput<Caps>),
}
pub(crate) struct Output<E: Events> {
    pub events: E,
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
            runtime::ServiceOutput::CommandInFlight(r) => Event::CommandInFlight(r),
            runtime::ServiceOutput::TxRefused { operation, reason } => {
                Event::TxRefused { operation, reason }
            }
            other => Event::Other(other),
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
}
impl<W: Binding> Storage<W> {
    pub const fn new() -> Self {
        Self {
            channels: Channels::new(),
            driver: radio::DriverStorage::new(),
            service: service::ServiceStorage::new(),
            bell: oer_memory::Doorbell::new(),
            ready: static_cell::StaticCell::new(),
        }
    }
}
impl<W: Binding> Default for Storage<W> {
    fn default() -> Self {
        Self::new()
    }
}
pub struct Resources<W: Binding> {
    pub events: W::Events,
    pub storage: &'static mut Storage<W>,
    pub cell: oer_esp32s31_radio_client::MembershipCell<oer_esp32s31_radio_client::wifi::Wifi>,
    pub wifi: WifiPartition,
    pub rng: Rng,
    pub address: [u8; 6],
    pub ap: [u8; 6],
    pub bound: Bound<W::Wifi>,
    pub bell: Bell<W::Wifi, W::Radio>,
    pub entropy: u32,
    pub interrupts: &'static Interrupts,
    pub mac: &'static IrqSignal<RadioExecutor>,
    pub power: &'static IrqSignal<RadioExecutor>,
    pub rx: oer_esp32s31_hal::memory::dma_static::Placed<WifiRxStatic<N, B, S, Q, RadioExecutor>>,
    pub tx: [oer_esp32s31_hal::memory::dma_static::Placed<TxDmaStorage<TX>>; 9],
}
pub struct Runners<W: Binding> {
    pub driver: Driver<W>,
    pub station: StationRunner<W>,
}
pub struct Driver<W: Binding> {
    pub(crate) boot: radio::Boot<W>,
    pub(crate) ready: &'static radio::ReadySlot<W>,
    pub(crate) bell: &'static oer_memory::Doorbell,
}
pub struct StationRunner<W: Binding> {
    pub(crate) frontend: oer_memory::Poster<(), Result<radio::Front<W>, InitializationError>>,
    pub(crate) config: Config,
    pub(crate) connection: Connection<'static>,
    pub(crate) storage: &'static mut service::ServiceStorage<W>,
    pub(crate) output: runtime::Events<'static, Output<W::Events>>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndKind {
    Stopped,
    Failed,
    Poisoned,
    Quarantined,
    Unavailable,
}
pub struct Outcome<W: Binding> {
    pub kind: EndKind,
    pub fault: Option<runtime::ServiceFault<service::StationComposition<W, W::Id>>>,
}
pub fn station<W: Binding>(
    resources: Resources<W>,
    config: Config,
) -> (Control<'static>, Network<'static>, Runners<W>) {
    let Resources {
        events,
        storage,
        cell,
        wifi,
        rng,
        address,
        ap,
        bound,
        bell,
        entropy,
        interrupts,
        mac,
        power,
        rx,
        tx,
    } = resources;
    let (control, network, connection) = storage
        .channels
        .split()
        .unwrap_or_else(|_| W::fail(InitializationError::Control));
    let _ = service::TARGET.set((address, config.target, config.channel));
    let output = control.inner.events(Output { events });
    let ready = storage
        .ready
        .try_init_with(|| radio::ReadySlot::<W>::new(&storage.bell, 1, 0x100))
        .unwrap_or_else(|| W::fail(InitializationError::Storage));
    let frontend = ready
        .claim()
        .unwrap_or_else(|| W::fail(InitializationError::Frontend));
    (
        control,
        network,
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
                bell: &storage.bell,
            },
            station: StationRunner {
                frontend,
                config,
                connection,
                storage: &mut storage.service,
                output,
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
    pub async fn run(self, profile: Profile<W>, engine: W::Engine) -> Outcome<W> {
        service::serve(self, profile, engine).await
    }
}
