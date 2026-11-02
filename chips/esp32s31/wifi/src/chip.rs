use crate::{Caps, Channels, Control, Network, ServiceConnection, radio, station as service};
use oer_chips_esp32s31_compose::Esp32s31;
use oer_chips_esp32s31_ieee80211_driver::irq::{
    DoorbellAcknowledge, RadioDoorbell, WifiIrqLine, WifiIrqPart, service_wifi_line,
};
use oer_chips_family_executor::{Doorbell, DoorbellAcknowledger};
use oer_chips_family_interrupt_table::{Bound, Entry, InterruptSources, signal::IrqSignal};
use oer_portable_memory::HandlerCell;
use oer_portable_time::Duration;
use oer_portable_trace::Observer;
use oer_protocols_ieee80211_core_roles_sta::machine::{StaCallbacks, StaConfig};
use oer_protocols_ieee80211_runtime::station as runtime;
use oer_protocols_ieee80211_wire::{
    address::{IndividualAddress, OwnAddress},
    channel::WifiChannelWidth,
};
use oer_protocols_radio_port::SlotIdentity;
use oer_protocols_radio_regulatory::{Country, RegulatoryDomain};

pub const N: usize = 96;
pub const B: usize = crate::FRAME_CAPACITY;
pub const S: usize = B;
/// The receive units outside the ring at once: the station's 16 (its
/// events and reorder window), the admission's critical reserve and the
/// network's lent units. The other 64 of the 96 stay armed with the
/// hardware, two Block Ack windows of margin, as in the archive.
pub const Q: usize = 32;
const _: () = {
    if crate::RX_DEPTH < 2 * oer_chips_esp32s31_ieee80211_driver::port::RX_HELD {
        panic!("the network queue holds two receive windows");
    }
    if Q >= N {
        panic!("the receive ring keeps descriptors armed beside the staged units");
    }
    if oer_chips_esp32s31_ieee80211_driver::port::lent_rx(Q) == 0 {
        panic!("the staged units leave the network none beside the station's");
    }
};
pub const TX: usize = crate::FRAME_CAPACITY;
/// The executor that polls the MAC driver: the station's, on the boot hart.
pub type DriverExecutor = oer_chips_family_executor::Executor<Esp32s31, 0>;
/// Image-selected entries, callbacks, observers and event sink.
pub trait Binding: Sized + 'static {
    type Id: SlotIdentity + 'static;
    type Wifi: Entry<Part = WifiIrqPart<Lower>>;
    /// The entry of the driver's executor ([`DriverExecutor`]): its wake
    /// entry, which rings the radio doorbell.
    type DriverEntry: Entry<Part: oer_chips_family_interrupt_table::InterruptSource>;
    type Callbacks: crate::Callbacks + 'static;
    /// Receives the facade's own trace events ([`crate::observation`]).
    type Observer: Observer + Clone + 'static;
    type DriverObserver: Observer + Default + 'static;
    /// The sink the station service lends its cores: the protocol's events.
    /// `()` keeps none.
    type Sink: oer_protocols_ieee80211_runtime::station::StationEvents + 'static;
    type Events: Events + 'static;
    /// Where the station's network end runs: [`crate::SameHart`], or
    /// [`crate::AcrossHarts`] with the executor signals the two harts raise
    /// for each other's tasks and the driver.
    type Notifies: crate::Notifies;
    /// The network end's transmit memory over the pool buffers the port
    /// does not hold, and the network stack's packets it takes.
    type TxSupply: crate::NetworkTxSupply;
    fn take_slot() -> Option<&'static mut radio::Slot<Self>>;
    /// Image-owned terminal policy, called with a handled construction error.
    fn fail(error: InitializationError) -> !;
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitializationError {
    Phy(oer_chips_esp32s31_compose::phy_fault::Failure),
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
    Request(oer_portable_memory::RequestError),
    Radio,
    Backend,
    Registration,
    Rx,
    Tx,
    Dma(crate::DmaError),
    /// The station's transmit Block Ack policy builds no originator.
    TxBlockAck,
}

/// The station's transmit Block Ack agreements: the widest window the S31
/// serves, which holds two aggregates of the port's subframes, so that the
/// next aggregate waits in the port while one is on the air; three requests
/// a second apart, each answered within
/// a second, ended after five idle seconds. A missing aggregate member goes
/// again until its MSDU's aggregate lifetime or the LMAC's long retry limit
/// of aggregates on the air.
///
/// SOURCE(esp32s31): member lifetime `oer_chips_family_ieee80211_policy::lmac::
/// AMPDU_MSDU_LIFETIME_MICROS` and attempts `lmac::LONG_RETRY_LIMIT`
/// (`libpp[lmac.o]::lmacInit`). The window is this composition's policy:
/// the driver's `TX_BLOCK_ACK_MAX_WINDOW`, as the vendor's HT ADDBA Request
/// asks 64 (`libnet80211[ieee80211_ht.o]::ieee80211_ampdu_request`);
/// negotiation timing and inactivity are this composition's policy too.
const TX_BLOCK_ACK: oer_protocols_ieee80211_core_roles_sta::machine::StaTxBlockAckPolicy =
    oer_protocols_ieee80211_core_roles_sta::machine::StaTxBlockAckPolicy {
        window: oer_chips_esp32s31_ieee80211_driver::mac::tx::ampdu::TX_BLOCK_ACK_MAX_WINDOW,
        negotiation_timeout: Duration::from_secs(1),
        retry: oer_protocols_ieee80211_core_upper_mac::block_ack::TxBlockAckRetry {
            attempts: 3,
            interval: Duration::from_secs(1),
        },
        inactivity: Duration::from_secs(5),
        retry_members: oer_protocols_ieee80211_core_roles_sta::data::AggregateRetry {
            lifetime: Duration::from_micros(
                oer_chips_family_ieee80211_policy::lmac::AMPDU_MSDU_LIFETIME_MICROS as u64,
            ),
            attempts: oer_chips_family_ieee80211_policy::lmac::LONG_RETRY_LIMIT,
        },
    };
pub struct Lower(pub DoorbellAcknowledger<Esp32s31, 3>);
impl DoorbellAcknowledge for Lower {
    fn acknowledge(&mut self) {
        self.0.acknowledge();
    }
}
/// The radio doorbell, rung by the driver's executor through its entry `R`.
pub struct Bell<E: Entry, R: Entry>(pub Doorbell<Esp32s31, 3, E, DriverExecutor, R>)
where
    R::Part: oer_chips_family_interrupt_table::InterruptSource;
impl<E: Entry, R: Entry> RadioDoorbell for Bell<E, R>
where
    R::Part: oer_chips_family_interrupt_table::InterruptSource,
{
    const HANDLER: oer_chips_family_interrupt_table::Context =
        Doorbell::<Esp32s31, 3, E, DriverExecutor, R>::HANDLER;
    const EXECUTOR: oer_chips_family_interrupt_table::Context =
        Doorbell::<Esp32s31, 3, E, DriverExecutor, R>::EXECUTOR;
    fn ring(&self) {
        self.0.ring();
    }
}
/// Types and operations of the permanently bound interrupt side.
pub trait InterruptTypes {
    type Part: InterruptSources;
    fn part(
        banks: oer_chips_esp32s31_hal::owner::WifiInterruptBanks,
        irq: oer_chips_esp32s31_hal::peripherals::software_interrupt::FromCpu3Irq,
        lower: Lower,
    ) -> Self::Part;
    fn service(&self, part: &mut Self::Part) -> Result<bool, u32>;
}
#[derive(bytemuck::Zeroable)]
pub struct Interrupts {
    pub(crate) line: HandlerCell<WifiIrqLine<DriverExecutor>>,
}
impl InterruptTypes for Interrupts {
    type Part = WifiIrqPart<Lower>;
    fn part(
        banks: oer_chips_esp32s31_hal::owner::WifiInterruptBanks,
        irq: oer_chips_esp32s31_hal::peripherals::software_interrupt::FromCpu3Irq,
        lower: Lower,
    ) -> Self::Part {
        WifiIrqPart::new(banks, irq, lower)
    }
    fn service(&self, part: &mut Self::Part) -> Result<bool, u32> {
        service_wifi_line(&self.line, part).map_err(
            |oer_chips_esp32s31_ieee80211_driver::mac::irq::Fault::FatalAssertion { raw }| raw,
        )
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
    /// Whether a join may tune HT 40 MHz where the access point offers it.
    pub(crate) ht40: bool,
    pub(crate) domain: RegulatoryDomain,
    pub(crate) supported: oer_protocols_ieee80211_contracts_port::SupportedChannel<Caps>,
}
impl Config {
    pub fn with_reconnect(mut self, policy: crate::ReconnectPolicy) -> Self {
        self.reconnect = policy;
        self
    }
    /// Join at HT 40 MHz where the access point's HT elements name a
    /// secondary channel the regulatory domain permits beside the scanned
    /// one; otherwise, and without it, the join stays at 20 MHz.
    pub fn with_ht40(mut self) -> Self {
        self.ht40 = true;
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
        let channel_value = oer_protocols_ieee80211_contracts_port::Channel::ghz2_4(
            channel,
            WifiChannelWidth::Mhz20,
        )
        .map_err(|_| InitializationError::Channel)?;
        let supported = oer_protocols_ieee80211_contracts_port::SupportedChannel::new(
            domain
                .permit(
                    oer_protocols_ieee80211_contracts_port::RegulatedChannel::from(channel_value),
                )
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
            ht40: false,
            domain,
            supported,
        })
    }
}
/// The image's callbacks and the engine the station unwraps EAPOL-Key data
/// with in its step: the AES accelerator, each block waited for on its
/// `STATE` flag.
pub(crate) struct CoreCallbacks<C> {
    callbacks: C,
    aes: oer_chips_esp32s31_compose::crypto::AesEngine,
}
impl<C: crate::Callbacks> StaCallbacks for CoreCallbacks<C> {
    type AesEngine = oer_chips_esp32s31_compose::crypto::AesEngine;
    fn nonce(&mut self) -> [u8; 32] {
        self.callbacks.nonce()
    }
    fn aes_engine(&mut self) -> &mut Self::AesEngine {
        &mut self.aes
    }
    fn he_packet_padding(
        &self,
        _: &[u8],
    ) -> oer_protocols_ieee80211_core_upper_mac::protection::HePacketPadding {
        oer_protocols_ieee80211_core_upper_mac::protection::HePacketPadding::None
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
    /// The station's profile; `aes` is the accelerator the station unwraps
    /// its EAPOL-Key data with.
    pub fn new(
        config: &Config,
        address: [u8; 6],
        callbacks: W::Callbacks,
        aes: oer_chips_esp32s31_compose::crypto::AesEngine,
        observer: W::Observer,
        sink: W::Sink,
    ) -> Result<Self, InitializationError> {
        let mut channels = heapless::Vec::new();
        if config.all_channels {
            for n in 1..=13 {
                let Ok(ch) = oer_protocols_ieee80211_contracts_port::Channel::ghz2_4(
                    n,
                    WifiChannelWidth::Mhz20,
                ) else {
                    continue;
                };
                if let Ok(p) = config
                    .domain
                    .permit(oer_protocols_ieee80211_contracts_port::RegulatedChannel::from(ch))
                    && let Some(c) =
                        oer_protocols_ieee80211_contracts_port::SupportedChannel::<Caps>::new(p)
                {
                    channels.push(c).map_err(|_| InitializationError::Channel)?;
                }
            }
        } else {
            channels
                .push(config.supported)
                .map_err(|_| InitializationError::Channel)?;
        }
        let mut wide_channels = heapless::Vec::new();
        if config.ht40 {
            for scanned in &channels {
                let number = scanned.channel().number();
                for width in [WifiChannelWidth::Mhz40Above, WifiChannelWidth::Mhz40Below] {
                    // A secondary outside the band or the domain is no
                    // 40 MHz channel of this primary.
                    let Ok(wide) =
                        oer_protocols_ieee80211_contracts_port::Channel::ghz2_4(number, width)
                    else {
                        continue;
                    };
                    if let Ok(permitted) = config.domain.permit(
                        oer_protocols_ieee80211_contracts_port::RegulatedChannel::from(wide),
                    ) && let Some(wide) =
                        oer_protocols_ieee80211_contracts_port::SupportedChannel::<Caps>::new(
                            permitted,
                        )
                    {
                        wide_channels
                            .push(wide)
                            .map_err(|_| InitializationError::Channel)?;
                    }
                }
            }
        }
        Ok(Self {
            station: StaConfig {
                address: OwnAddress::new(
                    IndividualAddress::for_interface(address)
                        .map_err(|_| InitializationError::Address)?,
                ),
                channels,
                wide_channels,
                dwell: Duration::from_millis(250),
                skipped_absences: 3,
                access_point_margin_db: 6,
                supported_rates: [0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24],
                association: &service::ASSOCIATION,
                listen_interval:
                    oer_protocols_ieee80211_core_roles_sta::request::StationListenInterval::DEFAULT,
                phy: if config.ht40 {
                    oer_protocols_ieee80211_core_upper_mac::phy_mode::PhyMode::Ht40
                } else {
                    oer_protocols_ieee80211_core_upper_mac::phy_mode::PhyMode::Ht20
                },
                tx_block_ack: Some(
                    oer_protocols_ieee80211_core_roles_sta::machine::StaTxBlockAck::new(
                        TX_BLOCK_ACK,
                    )
                    .map_err(|_| InitializationError::TxBlockAck)?,
                ),
                callbacks: CoreCallbacks { callbacks, aes },
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
    /// The packet itself went back to the network stack.
    MsduRefused {
        reason: runtime::contract::MsduRefusal,
    },
    AccessPoint(runtime::contract::AccessPointCommand<Caps>),
}
pub(crate) struct Output<E: Events, N: crate::Notifies> {
    pub events: E,
    pub status: crate::StatusReader<'static, N>,
}
impl<E: Events, N: crate::Notifies> runtime::ServiceOutputSink<Caps> for Output<E, N> {
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
            runtime::ServiceOutput::MsduRefused { reason } => Event::MsduRefused { reason },
            runtime::ServiceOutput::AccessPoint(command) => Event::AccessPoint(command),
        };
        self.events.event(event)
    }
}
pub struct Storage<W: Binding> {
    pub(crate) channels: Channels<W::Notifies, crate::TxMemory>,
    /// The transmit pool, pinned once: the port holds its first buffers,
    /// the network the rest ([`crate::NetworkTx`]).
    pub(crate) pool: static_cell::StaticCell<oer_chips_esp32s31_ieee80211_driver::port::TxPool<TX>>,
    pub(crate) driver: radio::DriverStorage<W>,
    pub(crate) service: service::ServiceStorage<W>,
    pub(crate) bell: oer_portable_memory::Doorbell,
    pub(crate) ready: static_cell::StaticCell<radio::ReadySlot<W>>,
    pub(crate) retire: static_cell::StaticCell<radio::RetireSlot<W>>,
}
impl<W, N, S, D> Storage<W>
where
    W: Binding<Notifies = crate::AcrossHarts<N, S, D>>,
    N: oer_portable_memory::QueueNotify + Copy + 'static,
    S: oer_portable_memory::QueueNotify + Copy + 'static,
    D: oer_portable_memory::QueueNotify + Copy + 'static,
{
    /// The storage of a station whose network end runs on another hart
    /// ([`Channels::across_harts`]): `network` raises the network task's
    /// signal, `station` the station task's, `driver` the radio driver's.
    pub const fn across_harts(network: N, station: S, driver: D) -> Self {
        Self {
            channels: Channels::across_harts(network, station),
            driver: radio::DriverStorage::new(
                oer_chips_esp32s31_ieee80211_driver::port::NetworkPeers {
                    driver,
                    returned: station,
                },
            ),
            service: service::ServiceStorage::new(),
            pool: static_cell::StaticCell::new(),
            bell: oer_portable_memory::Doorbell::new(),
            ready: static_cell::StaticCell::new(),
            retire: static_cell::StaticCell::new(),
        }
    }
}
impl<W: Binding<Notifies = crate::SameHart>> Storage<W> {
    pub const fn new() -> Self {
        Self {
            channels: Channels::new(),
            driver: radio::DriverStorage::new(
                oer_chips_esp32s31_ieee80211_driver::port::SameHart::new(),
            ),
            service: service::ServiceStorage::new(),
            pool: static_cell::StaticCell::new(),
            bell: oer_portable_memory::Doorbell::new(),
            ready: static_cell::StaticCell::new(),
            retire: static_cell::StaticCell::new(),
        }
    }
}
impl<W: Binding<Notifies = crate::SameHart>> Default for Storage<W> {
    fn default() -> Self {
        Self::new()
    }
}
pub struct Resources<W: Binding> {
    pub events: W::Events,
    pub storage: &'static mut Storage<W>,
    pub hardware: crate::WifiHardware,
    /// The random word of the MAC's delay slot, drawn from the hardware
    /// generator before the station starts.
    pub delay_random: u32,
    pub address: [u8; 6],
    pub ap: [u8; 6],
    pub bound: Bound<W::Wifi>,
    pub bell: Bell<W::Wifi, W::DriverEntry>,
    pub entropy: u32,
    pub interrupts: &'static Interrupts,
    pub mac: &'static IrqSignal<DriverExecutor>,
    /// The MAC's receive facts, which wake the station's task.
    pub receive: &'static IrqSignal<DriverExecutor>,
    pub power: &'static IrqSignal<DriverExecutor>,
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
    pub(crate) bell: &'static oer_portable_memory::Doorbell,
}
pub struct StationRunner<W: Binding> {
    pub(crate) service: ServiceRunner<W>,
    /// Hands the ordinary join to the driver task, which answers once its
    /// radio release has ended.
    pub(crate) retire:
        oer_portable_memory::Poster<radio::Retire<W>, Result<(), crate::failure::ReleaseCause>>,
    /// Publishes the exit to Control once it is final.
    pub(crate) stop: crate::StopGuard<'static, StationExit>,
}
pub(crate) struct ServiceRunner<W: Binding> {
    pub(crate) frontend:
        oer_portable_memory::Poster<(), Result<radio::Front<W>, InitializationError>>,
    pub(crate) config: Config,
    pub(crate) connection: ServiceConnection<'static, W::Notifies, crate::TxMemory>,
    pub(crate) storage: &'static mut service::ServiceStorage<W>,
    pub(crate) output: runtime::Events<
        'static,
        Output<W::Events, W::Notifies>,
        <W::Notifies as crate::Notifies>::Channels,
    >,
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
/// The station's network end over `W`'s transmit memory and its packets.
pub type StationNetwork<W> = Network<
    'static,
    <W as Binding>::Notifies,
    <<W as Binding>::TxSupply as crate::NetworkTxSupply>::Packet,
    <W as Binding>::TxSupply,
>;
#[allow(
    clippy::type_complexity,
    reason = "the four ends of the station's start"
)]
pub fn station<W: Binding>(
    resources: Resources<W>,
    config: Config,
) -> (
    Control<'static, W::Notifies>,
    StationNetwork<W>,
    crate::NetworkStopper<'static, W::Notifies>,
    Runners<W>,
) {
    let Resources {
        events,
        storage,
        hardware,
        delay_random,
        address,
        ap,
        bound,
        bell,
        entropy,
        interrupts,
        mac,
        receive,
        power,
        dma,
    } = resources;
    let crate::WifiHardware { cell, wifi } = hardware;
    let crate::DmaResources { rx, tx } = dma;
    let crate::dma::TxDma {
        pool,
        descriptors,
        chains,
    } = tx;
    // The pool is pinned here, before the network's end exists: the
    // network holds every buffer the port does not.
    let pool = storage
        .pool
        .try_init(
            oer_chips_esp32s31_ieee80211_driver::port::TxPool::pin(pool)
                .unwrap_or_else(|_| W::fail(InitializationError::Tx)),
        )
        .unwrap_or_else(|| W::fail(InitializationError::Storage));
    let pool: &'static _ = pool;
    let supply = <W::TxSupply as crate::NetworkTxSupply>::new(
        crate::NetworkTx::claim(pool).unwrap_or_else(|_| W::fail(InitializationError::Tx)),
    );
    let (mut control, network, stopper, connection) = storage
        .channels
        .split(supply)
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
                    delay_random,
                    station: address,
                    ap,
                    bound,
                    bell,
                    config,
                    entropy,
                    interrupts,
                    mac,
                    receive,
                    power,
                    rx,
                    pool,
                    descriptors,
                    chains,
                    storage: &mut storage.driver,
                },
                ready,
                retire,
                bell: &storage.bell,
            },
            station: StationRunner {
                service: ServiceRunner {
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
