//! The Wi-Fi station and its IP service on the S31: the radio executor (P2)
//! with the radio actor and the MAC driver, the station runner and the IP
//! service on the application executor, their interrupts and storage.
//!
//! [`Station::start`] and [`Net::start`] run in the image's boot. They check
//! everything a value can carry (regulatory configuration, client seats,
//! storage) before the first binding; a later refusal leaves the bound
//! owners where they are and the boot's error records and resets. Once the
//! executor runs, the library's tasks serve the station and the stack:
//!
//! - a station exit other than an ordinary stop records and resets;
//! - an IP service that is refused its context records and resets; a
//!   stopped service ends its waits with [`NetError::Stopped`].
//!
//! Cancellation of the application's waits:
//!
//! - a dropped [`Station::connect`] erases a profile still being prepared;
//!   once the request was admitted, the connection attempt continues and
//!   the next connect waits for its end (`disconnect` leaves the network);
//! - a dropped [`Net::wait_ipv4`] or [`Net::wait_link_down`] frees its
//!   waiter slot;
//! - a [`Device`] no [`Net::start`] took tells the station it has no
//!   network: the station returns each received frame's buffer at once.

use embassy_futures::select::{Either, select};
use oer_embassy_net::{
    NetworkInterfaceId,
    ethernet::LentSlots,
    ip::{self, Configuration, Resources, Service},
};
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_compose::{Esp32s31, pilot, platform::DeviceIdentity};
use oer_esp32s31_hal::peripherals::{
    efuse::EfuseRegisters, software_interrupt::FromCpu3Irq, timg0_alarm::Timg0AlarmParts,
};
use oer_esp32s31_wifi::{self as wifi, InterruptTypes};
use oer_espressif_executor::{
    Doorbell, DoorbellLine, Executor, InterruptExecutor, ThreadContext, UrgentSource,
};
use oer_espressif_interrupt_table::signal::{FromHandler, FromTask, IrqSignal};
use oer_ieee80211_runtime::station::contract::{CommandOutcome, Notice};
use static_cell::StaticCell;

pub use embassy_net;
pub use oer_embassy_net::ip::{Ipv4Configuration, Ipv4Status, Lifecycle, Status};
pub use oer_ieee80211_runtime::station::contract::{CommandRefusal, DisconnectReason};
pub use oer_network_port::{LinkSnapshot, LinkState};

use super::{
    AesClient, Boot, Error, Hart1Boot, PhyToken, RadioAlarmToken, RadioToken, WakeToken, WifiToken,
};
use crate::net::{NetError, Published, Watchers};

type RadioExecutor = InterruptExecutor<Esp32s31, 2>;

oer_memory::zeroed_static! {
    static LINE: wifi::Interrupts = zeroed in ".critical.bss.wifi_rx_line";
}
oer_espressif_staged_runtime::placed_static! {
    static MAC_SIGNAL: IrqSignal<RadioExecutor> = IrqSignal::new::<(FromHandler<WifiToken>, FromTask<Executor<Esp32s31, 0>, WakeToken>), RadioToken>(), in critical_data "wifi_station";
    static POWER_SIGNAL: IrqSignal<RadioExecutor> = IrqSignal::new::<FromHandler<WifiToken>, RadioToken>(), in critical_data "wifi_station";
    static PILOT: pilot::Storage<PhyToken> = pilot::Storage::new::<RadioToken>(), in critical_data "phy_pilot";
}

/// The Wi-Fi MAC's interrupt part.
pub type WifiPart = <wifi::Interrupts as InterruptTypes>::Part;

/// The radio interrupt handler (P3): the doorbell's requests, the MAC bank,
/// then the WDEVPWR bank. A fatal MAC assertion records its raw word and
/// resets.
pub(crate) fn on_wifi(part: &mut WifiPart) {
    match LINE.service(part) {
        Ok(_installed) => {}
        Err(raw) => oer_esp32s31_compose::wifi_fault::reset(raw),
    }
}

/// The SAR/PWDET pilot's interrupt handler.
pub(crate) fn on_pilot(
    part: &mut <Esp32s31 as oer_esp32s31_compose::platform::InterruptTypes>::Phy,
) {
    PILOT.interrupt(part);
}

type Actor = wifi::RadioActor<pilot::Client<PhyToken>, StationObserver>;
static ACTOR: StaticCell<Actor> = StaticCell::new();
static NONCES: StaticCell<wifi::NonceSource> = StaticCell::new();

oer_espressif_executor::tasks! {
    /// The radio executor's tasks (P2).
    pub RadioTasks: InterruptExecutor<Esp32s31, 2> {
        @observer: super::executor_observer::<1>();
        radio_actor(actor: &'static mut Actor) = radio_actor;
        driver(driver: wifi::Driver<binding::Binding>) = drive;
    }
}

/// The station's start: what its runner needs from the boot.
pub(crate) struct StationStart {
    pub(crate) runner: wifi::StationRunner<binding::Binding>,
    config: wifi::Config,
    address: [u8; 6],
    nonces: &'static mut wifi::NonceSource,
}

/// The image keeps no trace observations.
#[derive(Clone, Copy, Default)]
pub struct Unobserved;
impl oer_trace::Observer for Unobserved {
    fn event<E: oer_trace::Event + Copy + 'static>(&self, _: E) {}
}

/// The observer of the station, its driver and the RF actor: the
/// library's with `observe`, none without.
#[cfg(feature = "observe")]
type StationObserver = super::observed::Observer;
#[cfg(not(feature = "observe"))]
type StationObserver = Unobserved;

/// The station service's event sink.
#[cfg(feature = "observe")]
type StationSink = super::observed::StationSink;
#[cfg(not(feature = "observe"))]
type StationSink = ();
#[cfg(feature = "observe")]
const STATION_SINK: StationSink = super::observed::STATION_SINK;
#[cfg(not(feature = "observe"))]
const STATION_SINK: StationSink = ();

/// The boot-seeded nonce stream the RSN handshake draws from.
pub struct Callbacks(&'static mut wifi::NonceSource);
impl wifi::Callbacks for Callbacks {
    fn nonce(&mut self) -> [u8; 32] {
        self.0.next_nonce().unwrap_or_else(|wifi::NonceExhausted| {
            super::fail(oer_diagnostics::name!("wifi-nonce-exhausted"; 32))
        })
    }
}

/// Product events have no consumer: the station's operations carry every
/// result, the link snapshot its state.
pub struct ProductEvents;
impl wifi::Events for ProductEvents {
    fn event(&mut self, _: wifi::Event) {}
}

/// A construction error the facade reports from inside the runner.
fn failure(error: wifi::InitializationError) -> ! {
    match error {
        wifi::InitializationError::Request(request) => {
            oer_esp32s31_compose::platform::reset_request(request)
        }
        wifi::InitializationError::Phy(fault) => fault.reset(),
        wifi::InitializationError::MembershipUnavailable => {
            super::fail(oer_diagnostics::name!("wifi-membership"; 32))
        }
        _ => super::fail(oer_diagnostics::name!("wifi-initialization"; 32)),
    }
}

#[cfg(not(feature = "hart1"))]
wifi::bind_station! {pub(crate) mod binding{wifi:WifiToken,radio:RadioToken,callbacks:Callbacks,observer:StationObserver,driver_observer:StationObserver,sink:StationSink,engine:AesClient,events:ProductEvents,fail:failure}}
// The station's network end runs on hart 1, with the second boot's IP
// service.
#[cfg(feature = "hart1")]
wifi::bind_station! {pub(crate) mod binding{wifi:WifiToken,radio:RadioToken,callbacks:Callbacks,observer:StationObserver,driver_observer:StationObserver,sink:StationSink,engine:AesClient,events:ProductEvents,fail:failure,across_harts:{network:NetworkNotify=&NETWORK,station:StationNotify=&STATION,driver:DriverNotify=&DRIVER}}}

/// Where the station's network end runs: on the station's hart, or on hart
/// 1 through the peers' signals.
type Notifies = <binding::Binding as wifi::Binding>::Notifies;

#[cfg(feature = "hart1")]
type NetworkNotify =
    &'static oer_espressif_interrupt_table::signal::PeerNotify<super::hart1::Hart1Executor>;
#[cfg(feature = "hart1")]
type StationNotify =
    &'static oer_espressif_interrupt_table::signal::PeerNotify<super::hart1::AppExecutor>;
#[cfg(feature = "hart1")]
type DriverNotify = &'static oer_espressif_interrupt_table::signal::PeerNotify<RadioExecutor>;

// One signal wakes one task: the IP service's on hart 1 (every network end
// and the network's stop), the station runner's on hart 0 (every station
// end, the stop's answer and the lent buffers' return) and the radio
// driver's (its request for its lent buffers).
#[cfg(feature = "hart1")]
oer_espressif_staged_runtime::placed_static! {
    static NETWORK_SIGNAL: IrqSignal<super::hart1::Hart1Executor> = IrqSignal::new::<oer_espressif_interrupt_table::signal::FromPeerHart<super::hart1::AppExecutor, WakeToken>, super::Hart1WakeToken>(), in critical_data "hart_peers";
    static STATION_SIGNAL: IrqSignal<super::hart1::AppExecutor> = IrqSignal::new::<oer_espressif_interrupt_table::signal::FromPeerHart<super::hart1::Hart1Executor, super::Hart1WakeToken>, WakeToken>(), in critical_data "hart_peers";
    static DRIVER_SIGNAL: IrqSignal<RadioExecutor> = IrqSignal::new::<(oer_espressif_interrupt_table::signal::FromPeerHart<super::hart1::Hart1Executor, super::Hart1WakeToken>, FromTask<Executor<Esp32s31, 0>, WakeToken>), RadioToken>(), in critical_data "hart_peers";
    static NETWORK: oer_espressif_interrupt_table::signal::PeerNotify<super::hart1::Hart1Executor> = oer_espressif_interrupt_table::signal::PeerNotify::new(&NETWORK_SIGNAL), in critical_data "hart_peers";
    static STATION: oer_espressif_interrupt_table::signal::PeerNotify<super::hart1::AppExecutor> = oer_espressif_interrupt_table::signal::PeerNotify::new(&STATION_SIGNAL), in critical_data "hart_peers";
    static DRIVER: oer_espressif_interrupt_table::signal::PeerNotify<RadioExecutor> = oer_espressif_interrupt_table::signal::PeerNotify::new(&DRIVER_SIGNAL), in critical_data "hart_peers";
}

/// Run the shared RF actor until its periodic PHY tracking fails; the PHY
/// may then be half changed, so the library records the failure and resets.
async fn radio_actor(actor: &'static mut Actor) {
    observed!(RadioActor, actor.run()).await.reset()
}

async fn drive(driver: wifi::Driver<binding::Binding>) {
    observed!(Driver, driver.run()).await;
}

/// The station runner's task: an ordinary stop ends it, every other exit
/// records and resets.
pub(crate) async fn serve_station(start: StationStart) {
    let StationStart {
        runner,
        config,
        address,
        nonces,
    } = start;
    let profile = wifi::Profile::new(
        &config,
        address,
        Callbacks(nonces),
        StationObserver::default(),
        STATION_SINK,
    )
    .unwrap_or_else(|error| failure(error));
    match observed!(Station, runner.run(profile)).await {
        wifi::StationExit::Stopped => {}
        wifi::StationExit::Unavailable(wifi::UnavailableCause::Initialization(error)) => {
            failure(error)
        }
        wifi::StationExit::Unavailable(wifi::UnavailableCause::Abandoned) => {
            super::fail(oer_diagnostics::name!("wifi-abandoned"; 32))
        }
        wifi::StationExit::Failed(_) => super::fail(oer_diagnostics::name!("wifi-failed"; 32)),
        wifi::StationExit::Poisoned(_) => super::fail(oer_diagnostics::name!("wifi-poisoned"; 32)),
        wifi::StationExit::Quarantined(_) => {
            super::fail(oer_diagnostics::name!("wifi-quarantined"; 32))
        }
        wifi::StationExit::RetirementFailed(_) => {
            super::fail(oer_diagnostics::name!("wifi-retirement"; 32))
        }
        wifi::StationExit::ReleaseFailed(_) => {
            super::fail(oer_diagnostics::name!("wifi-release"; 32))
        }
    }
}

/// The radio's owners: the station takes them all.
pub struct WifiPeripheral {
    pub(crate) radio: oer_esp32s31_hal::root::RadioHardware,
    pub(crate) clocks: oer_esp32s31_hal::power::ChipClocks,
    pub(crate) timg0_alarm: Timg0AlarmParts,
    pub(crate) interrupts: oer_esp32s31_hal::owner::WifiInterruptBanks,
    pub(crate) efuse: EfuseRegisters,
}

/// The radio parts of the boot context: the radio executor's and the
/// doorbell's lines and the radio entries' tokens.
pub(crate) struct RadioBoot {
    pub(crate) line2: oer_esp32s31_hal::peripherals::software_interrupt::FromCpu2,
    pub(crate) irq2: oer_esp32s31_hal::peripherals::software_interrupt::FromCpu2Irq,
    pub(crate) line3: oer_esp32s31_hal::peripherals::software_interrupt::FromCpu3,
    pub(crate) irq3: FromCpu3Irq,
    pub(crate) radio_alarm: RadioAlarmToken,
    pub(crate) phy: PhyToken,
    pub(crate) radio: RadioToken,
    pub(crate) wifi: WifiToken,
}

/// The station's regulatory configuration, reconnect policy and stop
/// deadline. The constructors give the defaults: a reconnect every second
/// after a lost association ([`RECONNECT`]) and two seconds for the IP
/// service to return its receive buffers at a stop ([`DRAIN`]), the
/// application's [`Station::stop`] or one after a failure; the `with_`
/// methods override them.
#[derive(Clone, Copy)]
pub struct Config {
    chip: wifi::Config,
    /// The stop deadline `chip` holds, for [`Station::stop`].
    drain: oer_time::Duration,
}

/// The default interval of a reconnect after a lost association.
pub const RECONNECT: embassy_time::Duration = embassy_time::Duration::from_secs(1);

/// The default bound of a stop: how long the IP service may take to return
/// the receive buffers it holds. The library's choice,
/// not a measured need.
pub const DRAIN: embassy_time::Duration = embassy_time::Duration::from_secs(2);

/// Why a configuration was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigError {
    /// The country is not a known regulatory domain.
    Country,
    /// The channel is not permitted in the domain.
    Channel,
    /// The reconnect interval is zero.
    Reconnect,
    /// The stop deadline is zero.
    Drain,
}

impl Config {
    /// The 2.4 GHz `channel` of `country` at `cap_dbm`, with the default
    /// reconnect and stop deadline.
    ///
    /// # Errors
    /// The country or channel is refused.
    pub fn new(country: [u8; 2], channel: u8, cap_dbm: i8) -> Result<Self, ConfigError> {
        Self::chip(country, channel, cap_dbm, false)
    }

    /// Every 2.4 GHz channel `country` permits, scanned at each connect, at
    /// `cap_dbm`, with the default reconnect and stop deadline: for an
    /// image that learns its network's channel only when it connects.
    ///
    /// # Errors
    /// The country is refused, or it permits no 2.4 GHz channel.
    pub fn all_channels(country: [u8; 2], cap_dbm: i8) -> Result<Self, ConfigError> {
        // The radio starts on the domain's lowest permitted channel.
        crate::boot::lowest_channel(
            |channel| Self::chip(country, channel, cap_dbm, true),
            |error| *error == ConfigError::Channel,
        )
        .map_err(|refused| refused.unwrap_or(ConfigError::Channel))
    }

    fn chip(
        country: [u8; 2],
        channel: u8,
        cap_dbm: i8,
        all_channels: bool,
    ) -> Result<Self, ConfigError> {
        let drain = stop(DRAIN)?;
        let config =
            wifi::Config::new(channel, country, cap_dbm, all_channels, drain).map_err(|error| {
                match error {
                    wifi::InitializationError::Country | wifi::InitializationError::Domain => {
                        ConfigError::Country
                    }
                    _ => ConfigError::Channel,
                }
            })?;
        Self {
            chip: config,
            drain: drain.drain,
        }
        .with_reconnect(RECONNECT)
    }

    /// Reconnect every `interval` after a lost association.
    ///
    /// # Errors
    /// [`ConfigError::Reconnect`] for a zero interval.
    pub fn with_reconnect(self, interval: embassy_time::Duration) -> Result<Self, ConfigError> {
        let policy = wifi::ReconnectPolicy::every(oer_duration(interval))
            .map_err(|wifi::ZeroReconnectInterval| ConfigError::Reconnect)?;
        Ok(Self {
            chip: self.chip.with_reconnect(policy),
            ..self
        })
    }

    /// Stay idle after a lost association until the next connect.
    pub fn without_reconnect(self) -> Self {
        Self {
            chip: self.chip.with_reconnect(wifi::ReconnectPolicy::DISABLED),
            ..self
        }
    }

    /// Give the IP service `drain` to return its receive buffers at a stop.
    ///
    /// # Errors
    /// [`ConfigError::Drain`] for a zero deadline.
    pub fn with_drain(self, drain: embassy_time::Duration) -> Result<Self, ConfigError> {
        let policy = stop(drain)?;
        Ok(Self {
            chip: self.chip.with_stop(policy),
            drain: policy.drain,
        })
    }
}

/// The stop policy with `drain`; zero is refused, since no network could
/// return a buffer by it.
fn stop(drain: embassy_time::Duration) -> Result<wifi::StopPolicy, ConfigError> {
    let micros = crate::boot::drain_micros(drain.as_micros()).ok_or(ConfigError::Drain)?;
    Ok(wifi::StopPolicy {
        drain: oer_time::Duration::from_micros(micros.get()),
    })
}

/// The station's duration of an application's.
fn oer_duration(duration: embassy_time::Duration) -> oer_time::Duration {
    oer_time::Duration::from_micros(duration.as_micros())
}

/// A network to join: its SSID and credentials, validated, secrets erased
/// on drop.
pub struct Profile(wifi::StationSettings);

/// Why a profile was refused.
pub use wifi::CredentialsError as ProfileError;

impl Profile {
    /// An open network.
    ///
    /// # Errors
    /// The SSID is empty or longer than 32 bytes.
    pub fn open(ssid: &[u8]) -> Result<Self, ProfileError> {
        wifi::StationSettings::open(ssid).map(Self)
    }

    /// A WPA2-Personal network and its passphrase (8 to 63 characters).
    ///
    /// # Errors
    /// The SSID or the passphrase has the wrong length.
    pub fn wpa2(ssid: &[u8], passphrase: &[u8]) -> Result<Self, ProfileError> {
        wifi::StationSettings::wpa2_passphrase(ssid, passphrase).map(Self)
    }
}

/// The network a connect joined.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Connected {
    pub bssid: [u8; 6],
    pub channel: u8,
}

/// Why a connect did not join its network. Every cause is the station's own
/// observation; none guesses at the credentials.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectError {
    /// The station refused the request in its state (already connected or
    /// connecting).
    Refused(CommandRefusal),
    /// The attempt ended with this cause: `NotFound`, `AuthenticationFailed`,
    /// `AssociationFailed`, `HandshakeFailed` (the four-way handshake failed
    /// or timed out), a peer's reason code and the others.
    Failed(DisconnectReason),
    /// The station's command queue is full.
    Busy,
    /// The station stopped or never started.
    Stopped,
    /// The profile's credentials could not be prepared.
    Profile(ProfileError),
}

/// A connect's cause as a diagnostic path, which [`println!`](crate::println)
/// prints whole: `Failed(Sae(Protocol(Malformed)))`.
impl oer_diagnostics::DiagnosticPath for ConnectError {
    fn push_path(&self, path: &mut oer_diagnostics::DiagPath) {
        match self {
            Self::Refused(refusal) => {
                path.name("Refused");
                oer_diagnostics::DiagnosticPath::push_path(refusal, path);
            }
            Self::Failed(reason) => {
                path.name("Failed");
                oer_diagnostics::DiagnosticPath::push_path(reason, path);
            }
            Self::Busy => path.name("Busy"),
            Self::Stopped => path.name("Stopped"),
            Self::Profile(error) => {
                path.name("Profile");
                oer_diagnostics::DiagnosticPath::push_path(error, path);
            }
        }
    }
}

/// A started Wi-Fi station: its control.
pub struct Station {
    control: wifi::Control<'static, Notifies>,
    started: bool,
    address: [u8; 6],
    /// How long a stop gives the network to return its receive buffers.
    drain: oer_time::Duration,
}

/// Why a stop did not end with the station stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopError {
    /// The station ended by a failure, which the library records before it
    /// resets the chip.
    Failed,
}

/// The station's network device: the boot hands it to the second boot
/// ([`super::Hart1Peripherals`]), where [`Net::start`] takes it. Dropped
/// without it, it tells the station that no network takes its frames.
pub struct Device {
    network: Option<(
        wifi::Network<'static, Notifies>,
        wifi::NetworkStopper<'static, Notifies>,
    )>,
    address: [u8; 6],
    seed: u64,
}

impl Drop for Device {
    fn drop(&mut self) {
        if let Some((network, stopper)) = self.network.take() {
            stopper.no_network(network);
        }
    }
}

impl Station {
    /// Start the station on `wifi`, with a client of `aes` for its key
    /// unwraps: the radio executor, the actor, the MAC driver and the
    /// station runner. Its tasks run once the executor does.
    ///
    /// # Errors
    /// The AES service has no free client, the station started already, or
    /// a binding or storage was refused.
    pub fn start(
        wifi: WifiPeripheral,
        aes: &super::Aes,
        config: Config,
        boot: &mut Boot,
    ) -> Result<(Self, NetStatus), Error> {
        let engine = aes.client()?;
        let radio = boot.radio.take().ok_or(Error::AlreadyStarted)?;
        let timer = boot.timer.take().ok_or(Error::AlreadyStarted)?;
        let app_alarm = boot.app_alarm.take().ok_or(Error::AlreadyStarted)?;
        let dma = wifi::DmaResources::claim().map_err(|_| Error::Storage)?;
        let identity = DeviceIdentity::read(wifi.efuse);
        let address = identity.station_address();
        let ap = identity.ap_address();
        let resources =
            wifi::RadioResources::new::<binding::Binding>(wifi.radio, wifi.clocks, &identity);
        let pilot = PILOT.init(wifi.timg0_alarm, boot.router(), radio.phy);
        let (actor, hardware) = resources.start(pilot, StationObserver::default());
        let actor = ACTOR.try_init(actor).ok_or(Error::Storage)?;
        // The radio executor writes the system timer; the application
        // executor publishes its deadlines to it.
        let timers = oer_esp32s31_compose::platform::timers(
            timer,
            boot.router(),
            app_alarm,
            radio.radio_alarm,
        );
        #[cfg(feature = "hart1")]
        {
            boot.timers = Some(timers);
        }
        #[cfg(not(feature = "hart1"))]
        let _ = timers;
        let (line, lower) = DoorbellLine::<Esp32s31, 3>::split(radio.line3);
        let bound = boot
            .router()
            .bind(
                radio.wifi,
                wifi::Interrupts::part(wifi.interrupts, radio.irq3, wifi::Lower(lower)),
            )
            .unwrap_or_else(|error| {
                oer_espressif_staged_runtime::interrupts::reset_binding::<
                    oer_esp32s31_hal::platform::Port,
                    _,
                >(error)
            });
        let (bell, urgent) = Doorbell::new::<FromCpu3Irq>(line, &bound);
        let (hart, router) = boot.adopted.parts();
        let executor = RadioExecutor::new(radio.line2)
            .start(hart, router, radio.radio, radio.irq2, urgent)
            .unwrap_or_else(|failure| failure.reset());
        let entropy = boot.entropy.take().ok_or(Error::Entropy)?;
        let mut rng = oer_esp32s31_compose::platform::Entropy::new(entropy, EmbassyClock);
        if !rng.is_healthy() {
            return Err(Error::Entropy);
        }
        boot.boot_id = Some(super::draw_boot_id(&mut rng));
        let nonces = NONCES
            .try_init(wifi::NonceSource::from_entropy(|seed| rng.fill(seed)))
            .ok_or(Error::Storage)?;
        let mut words = [0; 12];
        rng.fill(&mut words);
        let [t0, t1, t2, t3, s0, s1, s2, s3, s4, s5, s6, s7] = words;
        let (control, network, stopper, runners) = wifi::station(
            wifi::Resources {
                engine,
                events: ProductEvents,
                storage: binding::storage(),
                hardware,
                rng,
                address,
                ap,
                bound,
                bell: wifi::Bell(bell),
                entropy: u32::from_le_bytes([t0, t1, t2, t3]),
                interrupts: &LINE,
                mac: &MAC_SIGNAL,
                power: &POWER_SIGNAL,
                dma,
            },
            config.chip,
        );
        if RadioTasks::radio_actor(&executor, actor).is_err()
            || RadioTasks::driver(&executor, runners.driver).is_err()
        {
            return Err(Error::AlreadyStarted);
        }
        boot.urgent = Some(urgent);
        boot.pending.station = Some(StationStart {
            runner: runners.station,
            config: config.chip,
            address,
            nonces,
        });
        boot.device = Some(Device {
            network: Some((network, stopper)),
            address,
            seed: u64::from_le_bytes([s0, s1, s2, s3, s4, s5, s6, s7]),
        });
        Ok((
            Self {
                control,
                started: false,
                address,
                drain: config.drain,
            },
            NetStatus {
                _local: core::marker::PhantomData,
            },
        ))
    }

    /// Join the network `profile` names; the future ends with the result of
    /// this connection attempt. Once linked, the station keeps the network
    /// and rejoins it after a lost association; an attempt that ends without
    /// a link leaves the station idle for the next connect.
    ///
    /// # Errors
    /// The station refused the request or the attempt failed, with its
    /// cause.
    pub async fn connect(&mut self, profile: Profile) -> Result<Connected, ConnectError> {
        if !self.started {
            self.control
                .wait_started()
                .await
                .map_err(|wifi::StartError::Unavailable| ConnectError::Stopped)?;
            self.started = true;
        }
        let credentials = profile.0.prepare().await.map_err(ConnectError::Profile)?;
        let operation = self.control.connect(credentials).await.map_err(control)?;
        match operation.await {
            wifi::OperationEnd::Completed(CommandOutcome::Connected { bssid, channel }) => {
                Ok(Connected { bssid, channel })
            }
            wifi::OperationEnd::Completed(
                CommandOutcome::ConnectFailed(reason) | CommandOutcome::Disconnected(reason),
            ) => Err(ConnectError::Failed(reason)),
            wifi::OperationEnd::Completed(CommandOutcome::BackgroundScan { .. })
            | wifi::OperationEnd::ServiceEnded => Err(ConnectError::Stopped),
            wifi::OperationEnd::Refused(refusal) => Err(ConnectError::Refused(refusal)),
        }
    }

    /// Leave the network; the station stays idle until the next connect.
    ///
    /// # Errors
    /// As [`Self::connect`].
    pub async fn disconnect(&mut self) -> Result<(), ConnectError> {
        let operation = self.control.disconnect().await.map_err(control)?;
        match operation.await {
            wifi::OperationEnd::Completed(_) => Ok(()),
            wifi::OperationEnd::Refused(refusal) => Err(ConnectError::Refused(refusal)),
            wifi::OperationEnd::ServiceEnded => Err(ConnectError::Stopped),
        }
    }

    /// Leave the network and stop the station for good: the IP service
    /// stops and returns every receive buffer it holds within the
    /// configuration's drain ([`Config::with_drain`]), then the radio
    /// retires. The future ends once the station stopped; a later connect
    /// answers [`ConnectError::Stopped`], and the IP service's waits end
    /// with its stop. A second stop ends with the first one's result.
    ///
    /// # Errors
    /// [`StopError::Failed`]: the station ended by a failure instead.
    pub async fn stop(&mut self) -> Result<(), StopError> {
        let deadline = {
            use oer_time::Clock as _;
            EmbassyClock.now().saturating_add(self.drain)
        };
        self.control.stop(deadline);
        match self.control.wait_stopped().await {
            Ok(wifi::StationExit::Stopped) => Ok(()),
            Ok(_) | Err(wifi::RunnerLost) => Err(StopError::Failed),
        }
    }

    /// The link's state and epoch now.
    pub fn link(&self) -> LinkSnapshot {
        self.control.link()
    }

    /// The network the station is linked to now: its link is up and its last
    /// notice was the connection.
    pub fn linked(&self) -> Option<Connected> {
        let status = self.control.status();
        match (status.link.state, status.notice) {
            (LinkState::Up, Some(Notice::Connected { bssid, channel })) => {
                Some(Connected { bssid, channel })
            }
            _ => None,
        }
    }

    /// The station's own MAC address.
    pub fn address(&self) -> [u8; 6] {
        self.address
    }
}

fn control(error: wifi::ControlError) -> ConnectError {
    match error {
        wifi::ControlError::QueueFull => ConnectError::Busy,
        wifi::ControlError::Stopped => ConnectError::Stopped,
    }
}

/// The default packet pools of the IP service: receive slots, transmit
/// credits and general slots, each holding one 1514-byte Ethernet frame.
pub const RX: usize = 8;
pub const TX: usize = 8;
pub const GENERAL: usize = 8;

#[cfg(not(feature = "hart1"))]
type Context = ThreadContext<0>;
/// The IP service's executor: hart 1's with the second boot there.
#[cfg(feature = "hart1")]
type Context = ThreadContext<1>;
type IpResources<const RX: usize, const TX: usize, const GENERAL: usize> =
    Resources<Context, RX, TX, GENERAL>;
type IpService<const RX: usize, const TX: usize> =
    Service<Context, RX, TX, wifi::Network<'static, Notifies>, { wifi::LENT_RX }>;

/// The IP service's packet pools: `RX` receive slots, `TX` transmit
/// credits and `GENERAL` general slots, each one 1514-byte frame. A static
/// of this type is the storage [`Net::start_with`] takes once; [`Net::start`]
/// takes the library's own of [`RX`], [`TX`] and [`GENERAL`].
pub struct Pools<const RX: usize, const TX: usize, const GENERAL: usize> {
    resources: StaticCell<IpResources<RX, TX, GENERAL>>,
    control: StaticCell<ip::Control<'static>>,
}

impl<const RX: usize, const TX: usize, const GENERAL: usize> Pools<RX, TX, GENERAL> {
    pub const fn new() -> Self {
        Self {
            resources: StaticCell::new(),
            control: StaticCell::new(),
        }
    }
}

impl<const RX: usize, const TX: usize, const GENERAL: usize> Default for Pools<RX, TX, GENERAL> {
    fn default() -> Self {
        Self::new()
    }
}

static POOLS: Pools<RX, TX, GENERAL> = Pools::new();

/// What the IP service runs: the service, the station's stop request, and
/// the service's control to stop it with, at the image's pool sizes.
#[doc(hidden)]
pub struct IpStart<const RX: usize, const TX: usize> {
    service: IpService<RX, TX>,
    stopper: wifi::NetworkStopper<'static, Notifies>,
    control: &'static ip::Control<'static>,
}

impl<const RX: usize, const TX: usize> super::NetTask for IpStart<RX, TX> {
    fn serve(self) -> impl core::future::Future<Output = ()> {
        serve_ip(self)
    }
}

/// The packets of the frames the station lends in place: as many as its
/// receive buffers the network may hold.
static LENT: LentSlots<wifi::ReceivedStorage<Notifies>, { wifi::LENT_RX }> = LentSlots::new();

/// The IP stack's device, which sockets name, over pools of `RX` and `TX`.
pub type IpDevice<const RX: usize = { self::RX }, const TX: usize = { self::TX }> =
    oer_embassy_net::OwnedNetworkDevice<'static, Context, RX, TX>;

/// The IP stack sockets open on.
pub type Stack<const RX: usize = { self::RX }, const TX: usize = { self::TX }> =
    embassy_net::Stack<'static, IpDevice<RX, TX>>;

static WATCHERS: Watchers<Context, 4> = Watchers::new();

/// The IP service's status for the application, and the application's
/// notify, which the service tells after each publication.
static STATUS: Published = Published::new();

/// The IP service's status as the application sees it: the boot's
/// [`Station::start`] returns it, before the second boot starts the
/// service. It stays with the application's future (`!Send`), the one task
/// its waits wake; until the service starts it reads a running service
/// without link or address.
pub struct NetStatus {
    _local: core::marker::PhantomData<*mut ()>,
}

impl NetStatus {
    /// The service's link, IPv4 and lifecycle now.
    pub async fn status(&self) -> Status {
        crate::net::published_status(&STATUS, &super::APPLICATION).await
    }

    /// Wait for an IPv4 configuration over an up link; across a link loss
    /// the wait goes on to the next lease.
    ///
    /// # Errors
    /// The service stopped or failed.
    pub async fn wait_ipv4(&self) -> Result<Ipv4Configuration, NetError> {
        crate::net::published_ipv4(&STATUS, &super::APPLICATION).await
    }

    /// Wait until the link that is up now goes down or is replaced.
    ///
    /// # Errors
    /// As [`Self::wait_ipv4`].
    pub async fn wait_link_down(&self) -> Result<(), NetError> {
        crate::net::published_link_down(&STATUS, &super::APPLICATION).await
    }
}

/// Publish the service's status to the application and wake every wait.
fn publish(status: Status) {
    STATUS.publish(status);
    oer_memory::QueueNotify::notify(&super::APPLICATION);
    WATCHERS.wake_all();
}

/// How the stack takes its IPv4 configuration.
pub use oer_embassy_net::ip::Configuration as NetConfig;

/// The IP service over the station's device, with pools of `RX` receive
/// slots and `TX` transmit credits; the defaults are the library's.
pub struct Net<const RX: usize = { self::RX }, const TX: usize = { self::TX }> {
    control: &'static ip::Control<'static>,
    stack: Stack<RX, TX>,
}

impl Net {
    /// Start the IP service over `device` with `configuration`
    /// (`NetConfig::Dhcpv4` or a static address) and the library's pools,
    /// in the second boot; it returns with the service, whose task runs
    /// once the executor does.
    ///
    /// # Errors
    /// The service started already or refused its configuration.
    pub fn start(
        device: Device,
        configuration: Configuration,
        boot: Hart1Boot,
    ) -> Result<(Self, Hart1Boot<IpStart<RX, TX>>), Error> {
        Self::start_with(device, &POOLS, configuration, boot)
    }
}

impl<const RX: usize, const TX: usize> Net<RX, TX> {
    /// As [`Net::start`], with the image's own `pools`.
    ///
    /// # Errors
    /// As [`Net::start`]; the pools were taken already.
    pub fn start_with<const GENERAL: usize>(
        mut device: Device,
        pools: &'static Pools<RX, TX, GENERAL>,
        configuration: Configuration,
        boot: Hart1Boot,
    ) -> Result<(Self, Hart1Boot<IpStart<RX, TX>>), Error> {
        let (network, stopper) = device.network.take().ok_or(Error::AlreadyStarted)?;
        let resources = pools
            .resources
            .try_uninit()
            .map(|place| place.write(const { IpResources::<RX, TX, GENERAL>::new() }))
            .ok_or(Error::AlreadyStarted)?;
        let (control, service) = resources
            .start(
                network,
                Some(&LENT),
                NetworkInterfaceId::new(0),
                device.address,
                device.seed,
                configuration,
            )
            .map_err(|refused| Error::Net(refused.reason))?;
        let control: &'static ip::Control<'static> =
            pools.control.try_init(control).ok_or(Error::Storage)?;
        let stack = service.stack();
        Ok((
            Self { control, stack },
            boot.with_net(IpStart {
                service,
                stopper,
                control,
            }),
        ))
    }

    /// The stack sockets open on.
    pub fn stack(&self) -> Stack<RX, TX> {
        self.stack
    }

    /// The service's link, IPv4 and lifecycle now.
    pub fn status(&self) -> Status {
        self.control.status()
    }

    /// Wait for an IPv4 configuration over an up link; across a link loss
    /// the wait goes on to the next lease.
    ///
    /// # Errors
    /// The service stopped or failed, or too many tasks wait at once.
    pub async fn wait_ipv4(&self) -> Result<Ipv4Configuration, NetError> {
        crate::net::wait_ipv4(&WATCHERS, || self.control.status()).await
    }

    /// Wait until the link that is up now goes down or is replaced.
    ///
    /// # Errors
    /// As [`Self::wait_ipv4`].
    pub async fn wait_link_down(&self) -> Result<(), NetError> {
        crate::net::wait_link_down(&WATCHERS, || self.control.status()).await
    }
}

#[cfg(feature = "observe")]
use super::observed::{bridge_copied, bridge_received, bridge_refused};
/// The network bridge's frames: counted with `observe`.
#[cfg(not(feature = "observe"))]
fn bridge_received(_: oer_embassy_net::EthernetFrameView<'_>) {}
#[cfg(not(feature = "observe"))]
fn bridge_refused(_: oer_embassy_net::RxEnqueueError) {}
#[cfg(not(feature = "observe"))]
fn bridge_copied(_: oer_embassy_net::ethernet::LentCopy) {}

/// The IP service's task: every status change wakes the waits. Once the
/// station asks its network to stop, the service stops and returns every
/// receive buffer it holds, then the station hears that it stopped.
async fn serve_ip<const RX: usize, const TX: usize>(start: IpStart<RX, TX>) {
    let IpStart {
        mut service,
        stopper,
        control,
    } = start;
    let result = {
        let mut run = core::pin::pin!(service.run(
            |changed| publish(changed.current),
            bridge_received,
            bridge_refused,
            bridge_copied,
            |_| {}
        ));
        match select(run.as_mut(), stopper.requested()).await {
            Either::First(result) => result,
            Either::Second(()) => {
                control.stop();
                run.await
            }
        }
    };
    publish(control.status());
    if result.is_err() {
        super::fail(oer_diagnostics::name!("net-context"; 32));
    }
    stopper.stopped();
}

/// The urgent source the doorbell admits, for both executors.
pub(crate) type Urgent = UrgentSource<FromCpu3Irq>;
