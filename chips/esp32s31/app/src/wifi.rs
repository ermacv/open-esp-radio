//! The Wi-Fi roles on the S31 and their network: the radio executor (P2)
//! with the radio actor and the MAC driver, the station's runner or the
//! access point's on the application executor, the network core in the
//! image's group, their interrupts and storage.
//!
//! [`Station::start`] and `AccessPoint::start` run in the image's boot,
//! [`Network::start`] in the group's. They check everything a value can
//! carry (regulatory configuration, client seats, storage) before the first
//! binding; a later refusal leaves the bound owners where they are and the
//! boot's error records and resets. Once the executor runs, the library's
//! tasks serve the role and its network: a station exit other than an
//! ordinary stop records and resets.
//!
//! Cancellation of the application's waits: a dropped [`Station::connect`]
//! erases a profile still being prepared; once the request was admitted,
//! the connection attempt continues and the next connect waits for its end
//! (`disconnect` leaves the network). A [`Device`] no [`Network::start`]
//! took tells its role it has no network: the role returns each received
//! frame's buffer at once.

use crate::pilot;
use embassy_futures::select::{Either, select};
use oer_chips_esp32s31_hal::peripherals::{
    efuse::EfuseRegisters, software_interrupt::FromCpu3Irq, timg0_alarm::Timg0AlarmParts,
};
use oer_chips_esp32s31_platform_runtime::{Esp32s31, platform::DeviceIdentity};
use oer_chips_esp32s31_radio_ieee80211_station::{self as wifi, InterruptTypes};
use oer_chips_family_platform_executor::{Doorbell, DoorbellLine, InterruptExecutor, UrgentSource};
use oer_chips_family_platform_interrupt_table::signal::{FromHandler, FromOwnTask, IrqSignal};
use oer_portable_embassy_time::EmbassyClock;
#[cfg(feature = "station")]
use oer_protocols_ieee80211_runtime::station::contract::{CommandOutcome, Notice};
use static_cell::StaticCell;

#[cfg(feature = "access-point")]
mod access_point;
#[cfg(feature = "access-point")]
pub use access_point::*;
mod network;
pub use network::*;
#[cfg(feature = "station")]
mod station;
#[cfg(feature = "station")]
pub use station::*;

pub use oer_protocols_ieee80211_runtime::station::contract::{
    CommandRefusal, DisconnectReason, LinkPhy,
};
pub use oer_protocols_network_port::{LinkSnapshot, LinkState};
pub use wifi::Restarts;

#[cfg(feature = "station")]
use super::AesPeripheral;
use super::{Boot, Error, GroupBoot, PhyToken, RadioAlarmToken, RadioToken, WakeToken, WifiToken};

type RadioExecutor = InterruptExecutor<Esp32s31, 2>;

oer_portable_memory::zeroed_static! {
    static LINE: wifi::Interrupts = zeroed in ".critical.bss.wifi_rx_line";
}
oer_chips_family_platform_runtime::placed_static! {
    static MAC_SIGNAL: IrqSignal<wifi::DriverExecutor> = IrqSignal::new::<FromHandler<WifiToken>, WakeToken>(), in critical_data "wifi_station";
    static MAC_RX_SIGNAL: IrqSignal<wifi::DriverExecutor> = IrqSignal::new::<(FromHandler<WifiToken>, FromOwnTask<wifi::DriverExecutor, WakeToken>), WakeToken>(), in critical_data "wifi_station";
    static POWER_SIGNAL: IrqSignal<wifi::DriverExecutor> = IrqSignal::new::<FromHandler<WifiToken>, WakeToken>(), in critical_data "wifi_station";
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
        Err(raw) => oer_chips_esp32s31_platform_runtime::wifi_fault::reset(raw),
    }
}

/// The SAR/PWDET pilot's interrupt handler.
pub(crate) fn on_pilot(
    part: &mut <Esp32s31 as oer_chips_esp32s31_platform_runtime::platform::InterruptTypes>::Phy,
) {
    PILOT.interrupt(part);
}

type Actor = wifi::RadioActor<pilot::Client<PhyToken>, StationObserver>;
static ACTOR: StaticCell<Actor> = StaticCell::new();
static NONCES: StaticCell<wifi::NonceSource> = StaticCell::new();

oer_chips_family_platform_executor::tasks! {
    /// The radio executor's tasks (P2).
    pub RadioTasks: InterruptExecutor<Esp32s31, 2> {
        @observer: super::executor_observer::<1>();
        radio_actor(actor: &'static mut Actor) = radio_actor;
    }
}

/// The image keeps no trace observations.
#[derive(Clone, Copy, Default)]
pub struct Unobserved;
impl oer_portable_trace::Observer for Unobserved {
    fn event<E: oer_portable_trace::Event + Copy + 'static>(&self, _: E) {}
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
#[cfg(all(feature = "station", feature = "observe"))]
const STATION_SINK: StationSink = super::observed::STATION_SINK;
#[cfg(all(feature = "station", not(feature = "observe")))]
const STATION_SINK: StationSink = ();

/// The boot-seeded nonce stream the RSN handshake draws from.
pub struct Callbacks(&'static mut wifi::NonceSource);
impl wifi::Callbacks for Callbacks {
    fn nonce(&mut self) -> [u8; 32] {
        self.0.next_nonce().unwrap_or_else(|wifi::NonceExhausted| {
            super::fail(oer_portable_diagnostics::name!("wifi-nonce-exhausted"; 32))
        })
    }
}

/// The one-shot fault of a diagnostic image.
#[cfg(oer_fault_injection)]
pub mod debug {
    pub use super::wifi::refuse_next_publication;
}

/// The last cause in `restarts` as its console line reads, written to `out`;
/// the bytes written.
pub fn restart_cause_text(restarts: &wifi::Restarts, out: &mut [u8]) -> usize {
    let path = restarts
        .last
        .as_ref()
        .map(oer_portable_diagnostics::DiagPath::of)
        .unwrap_or_default();
    oer_chips_esp32s31_platform_runtime::diagnostic_text::write(b"wifi-restarted", &path, out)
}

/// How the station ended (`ended`, as [`Station::ended`] reads it) as its
/// line reads, written to `out`: `wifi-stop` and the stop error's variant
/// path, or `wifi-stopped` after a stop that ended with it stopped; the
/// bytes written.
#[cfg(feature = "station")]
pub fn end_text(ended: &Result<(), StopError>, out: &mut [u8]) -> usize {
    let (prefix, path): (&[u8], _) = match ended {
        Ok(()) => (b"wifi-stopped", oer_portable_diagnostics::DiagPath::new()),
        Err(error) => (b"wifi-stop", oer_portable_diagnostics::DiagPath::of(error)),
    };
    oer_chips_esp32s31_platform_runtime::diagnostic_text::write(prefix, &path, out)
}

/// The station restarted after `failure`, which stayed inside the radio: the
/// console keeps its cause and the chip runs on.
fn restarted(failure: &wifi::failure::LifecycleFailure) {
    let path = oer_portable_diagnostics::DiagPath::of(failure);
    let mut text = [0; oer_portable_diagnostics::fault::PAYLOAD_BYTES];
    let length = oer_chips_esp32s31_platform_runtime::diagnostic_text::write(
        b"wifi-restarted",
        &path,
        &mut text,
    );
    // Once the application opened the console this line goes nowhere: the
    // cause stays readable through `Station::restarts`.
    if let Some(line) = text.get(..length) {
        let _written = super::whole(line).and_then(|()| super::whole(b"\n"));
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
            oer_chips_esp32s31_platform_runtime::platform::reset_request(request)
        }
        wifi::InitializationError::Phy(fault) => fault.reset(),
        wifi::InitializationError::MembershipUnavailable => {
            super::fail(oer_portable_diagnostics::name!("wifi-membership"; 32))
        }
        _ => super::fail(oer_portable_diagnostics::name!("wifi-initialization"; 32)),
    }
}

/// The station's transmit end, which the network's sending path owns.
type Transmit = wifi::StationTransmit<binding::Binding>;

#[cfg(not(feature = "group"))]
wifi::bind_station! {pub(crate) mod binding{wifi:WifiToken,driver:WakeToken,callbacks:Callbacks,observer:StationObserver,driver_observer:StationObserver,sink:StationSink,events:ProductEvents,tx:TxEnd,fail:failure,restarted:restarted}}
// The station's network end runs on hart 1, with the group's IP
// service.
#[cfg(feature = "group")]
wifi::bind_station! {pub(crate) mod binding{wifi:WifiToken,driver:WakeToken,callbacks:Callbacks,observer:StationObserver,driver_observer:StationObserver,sink:StationSink,events:ProductEvents,tx:TxEnd,fail:failure,restarted:restarted,across_harts:{network:NetworkNotify=&NETWORK,actor:ActorNotify=&DATAPATH}}}

/// Where the station's network end runs: on the station's hart, or on hart
/// 1 through the peers' signals.
type Notifies = <binding::Binding as wifi::Binding>::Notifies;

#[cfg(feature = "group")]
type NetworkNotify = &'static oer_chips_family_platform_interrupt_table::signal::PeerNotify<
    super::hart1::Hart1Executor,
>;
#[cfg(feature = "group")]
type ActorNotify =
    &'static oer_chips_family_platform_interrupt_table::signal::PeerNotify<wifi::DriverExecutor>;

// One signal wakes one task: the network task's on hart 1 (every network end
// and the network's stop) and the datapath actor's on hart 0 (every station
// end of the network's channels, the network's stop answer and the lent
// buffers' returns, which the network drops on hart 1 and the actor itself
// on its own). The station's lifecycle shares the actor's executor and
// hears it in same-executor queues; nothing on hart 1 wakes it.
#[cfg(feature = "group")]
oer_chips_family_platform_runtime::placed_static! {
    static NETWORK_SIGNAL: IrqSignal<super::hart1::Hart1Executor> = IrqSignal::new::<oer_chips_family_platform_interrupt_table::signal::FromPeerHart<super::hart1::AppExecutor, WakeToken>, super::Hart1WakeToken>(), in critical_data "hart_peers";
    static ACTOR_SIGNAL: IrqSignal<wifi::DriverExecutor> = IrqSignal::new::<(oer_chips_family_platform_interrupt_table::signal::FromPeerHart<super::hart1::Hart1Executor, super::Hart1WakeToken>, FromOwnTask<wifi::DriverExecutor, WakeToken>), WakeToken>(), in critical_data "hart_peers";
    static NETWORK: oer_chips_family_platform_interrupt_table::signal::PeerNotify<super::hart1::Hart1Executor> = oer_chips_family_platform_interrupt_table::signal::PeerNotify::new(&NETWORK_SIGNAL), in critical_data "hart_peers";
    static DATAPATH: oer_chips_family_platform_interrupt_table::signal::PeerNotify<wifi::DriverExecutor> = oer_chips_family_platform_interrupt_table::signal::PeerNotify::new(&ACTOR_SIGNAL), in critical_data "hart_peers";
}

/// Run the shared RF actor until its periodic PHY tracking fails; the PHY
/// may then be half changed, so the library records the failure and resets.
async fn radio_actor(actor: &'static mut Actor) {
    observed!(RadioActor, actor.run()).await.reset()
}

/// Where an ended task leaves its installation for the application: one
/// place, written once by the task and read once by the application's
/// handle.
type EndHome<T> = oer_portable_memory::WakingSpscQueue<T, 1>;
/// The task's side of an [`EndHome`].
type EndSender<T> = oer_portable_memory::WakingSpscSender<'static, T, 1>;
/// The application's side of an [`EndHome`].
pub(crate) type EndReceiver<T> = oer_portable_memory::WakingSpscReceiver<'static, T, 1>;

/// Split a home for an ended task's installation out of `cell`.
pub(crate) fn end_home<T>(
    cell: &'static StaticCell<EndHome<T>>,
) -> Result<(EndSender<T>, EndReceiver<T>), Error> {
    let home = cell.try_init(EndHome::new()).ok_or(Error::Storage)?;
    Ok(home.split())
}

/// Leave `end` in its home: the home's one place takes it, as the task
/// sends once.
pub(crate) async fn leave<T>(mut home: EndSender<T>, end: T) {
    home.ready().await.send(end);
}

/// Wait for the installation an ended task left in its home, once.
#[cfg(feature = "access-point")]
pub(crate) async fn ended<T>(home: &mut Option<EndReceiver<T>>) -> Option<T> {
    let receiver = home.as_mut()?;
    loop {
        if let Some(end) = receiver.try_receive() {
            *home = None;
            return Some(end);
        }
        receiver.ready().await;
    }
}

/// What a started radio hands the role that runs on it: the resources of
/// its composition, its handshake nonces and the network's seed.
pub(crate) struct RadioStarted {
    pub(crate) resources: wifi::Resources<binding::Binding>,
    pub(crate) nonces: &'static mut wifi::NonceSource,
    pub(crate) seed: u64,
}

/// Start the radio for a role on `wifi`: the radio executor and the radio
/// actor, the timers, the Wi-Fi interrupt with its doorbell, the boot's
/// identity and the handshake nonces.
pub(crate) fn start_radio(wifi: WifiPeripheral, boot: &mut Boot) -> Result<RadioStarted, Error> {
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
    let timers = oer_chips_esp32s31_platform_runtime::platform::timers(
        timer,
        boot.router(),
        app_alarm,
        radio.radio_alarm,
    );
    #[cfg(feature = "group")]
    {
        boot.timers = Some(timers);
    }
    #[cfg(not(feature = "group"))]
    let _ = timers;
    let (line, lower) = DoorbellLine::<Esp32s31, 3>::split(radio.line3);
    let bound = boot
        .router()
        .bind(
            radio.wifi,
            wifi::Interrupts::part(wifi.interrupts, radio.irq3, wifi::Lower(lower)),
        )
        .unwrap_or_else(|error| {
            oer_chips_family_platform_runtime::interrupts::reset_binding::<
                oer_chips_esp32s31_hal::system::platform::Port,
                _,
            >(error)
        });
    let (bell, urgent) = Doorbell::new::<FromCpu3Irq>(line, &bound);
    let (hart, router) = boot.adopted.parts();
    let executor = RadioExecutor::new(radio.line2)
        .start(hart, router, radio.radio, radio.irq2, urgent)
        .unwrap_or_else(|failure| failure.reset());
    let entropy = boot.entropy.take().ok_or(Error::Entropy)?;
    let mut rng =
        oer_chips_esp32s31_platform_runtime::platform::Entropy::new(entropy, EmbassyClock);
    if !rng.is_healthy() {
        return Err(Error::Entropy);
    }
    // Every draw happens here, at boot: the generator stays with no
    // runtime owner, so a failed health test is a typed boot error.
    boot.boot_id = Some(super::draw_boot_id(&mut rng).map_err(|_| Error::Entropy)?);
    let nonces =
        wifi::NonceSource::from_entropy(|seed| rng.fill(seed)).map_err(|_| Error::Entropy)?;
    let nonces = NONCES.try_init(nonces).ok_or(Error::Storage)?;
    let mut words = [0; 12];
    rng.fill(&mut words).map_err(|_| Error::Entropy)?;
    let [t0, t1, t2, t3, s0, s1, s2, s3, s4, s5, s6, s7] = words;
    let mut delay = [0; 4];
    rng.fill(&mut delay).map_err(|_| Error::Entropy)?;
    let resources = wifi::Resources {
        events: ProductEvents,
        storage: binding::storage(),
        hardware,
        delay_random: u32::from_le_bytes(delay),
        address,
        ap,
        bound,
        bell: wifi::Bell(bell),
        entropy: u32::from_le_bytes([t0, t1, t2, t3]),
        interrupts: &LINE,
        mac: &MAC_SIGNAL,
        receive: &MAC_RX_SIGNAL,
        power: &POWER_SIGNAL,
        dma,
    };
    if RadioTasks::radio_actor(&executor, actor).is_err() {
        return Err(Error::AlreadyStarted);
    }
    boot.urgent = Some(urgent);
    Ok(RadioStarted {
        resources,
        nonces,
        seed: u64::from_le_bytes([s0, s1, s2, s3, s4, s5, s6, s7]),
    })
}

/// The radio's owners: the station takes them all.
pub struct WifiPeripheral {
    pub(crate) radio: oer_chips_esp32s31_hal::system::root::RadioHardware,
    pub(crate) clocks: oer_chips_esp32s31_hal::power::ChipClocks,
    pub(crate) timg0_alarm: Timg0AlarmParts,
    pub(crate) interrupts: oer_chips_esp32s31_hal::system::owner::WifiInterruptBanks,
    pub(crate) efuse: EfuseRegisters,
}

/// The radio parts of the boot context: the radio executor's and the
/// doorbell's lines and the radio entries' tokens.
pub(crate) struct RadioBoot {
    pub(crate) line2: oer_chips_esp32s31_hal::peripherals::software_interrupt::FromCpu2,
    pub(crate) irq2: oer_chips_esp32s31_hal::peripherals::software_interrupt::FromCpu2Irq,
    pub(crate) line3: oer_chips_esp32s31_hal::peripherals::software_interrupt::FromCpu3,
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
    #[cfg_attr(
        not(feature = "station"),
        allow(dead_code, reason = "the access point's stop reads it from `chip`")
    )]
    drain: oer_portable_time::Duration,
}

/// The default interval of a reconnect after a lost association.
pub const RECONNECT: embassy_time::Duration = embassy_time::Duration::from_secs(1);

/// The default bound of a stop: how long the network may take to return
/// the receive buffers it holds and a frame a foreign transmitter keeps off
/// the air may take to end. The library's choice, not a measured need.
pub const DRAIN: embassy_time::Duration = embassy_time::Duration::from_secs(3);
const _: () = {
    if DRAIN.as_micros() <= wifi::STOP_FLOOR.as_micros() {
        panic!("a stop expires behind a frame that may still end");
    }
};

/// Why a configuration was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigError {
    /// The country is not a known regulatory domain.
    Country,
    /// The channel is not permitted in the domain.
    Channel,
    /// The reconnect interval is zero.
    Reconnect,
    /// The stop deadline is one a held frame could outlast.
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

    /// Join at HT 40 MHz where the access point's HT elements name a
    /// secondary channel the regulatory domain permits beside the scanned
    /// one; otherwise, and without this, the join stays at 20 MHz.
    pub fn with_ht40(self) -> Self {
        Self {
            chip: self.chip.with_ht40(),
            ..self
        }
    }

    /// Stay idle after a lost association until the next connect.
    pub fn without_reconnect(self) -> Self {
        Self {
            chip: self.chip.with_reconnect(wifi::ReconnectPolicy::DISABLED),
            ..self
        }
    }

    /// Give the network `drain` to return its receive buffers at a stop.
    ///
    /// # Errors
    /// [`ConfigError::Drain`] for a deadline a held frame could outlast.
    pub fn with_drain(self, drain: embassy_time::Duration) -> Result<Self, ConfigError> {
        let policy = stop(drain)?;
        Ok(Self {
            chip: self.chip.with_stop(policy),
            drain: policy.drain,
        })
    }
}

/// The stop policy with `drain`; one a published frame could outlast is
/// refused, since the stop would end before the frame.
fn stop(drain: embassy_time::Duration) -> Result<wifi::StopPolicy, ConfigError> {
    let micros = crate::boot::drain_micros(drain.as_micros(), wifi::STOP_FLOOR.as_micros())
        .ok_or(ConfigError::Drain)?;
    Ok(wifi::StopPolicy {
        drain: oer_portable_time::Duration::from_micros(micros.get()),
    })
}

/// The station's duration of an application's.
fn oer_duration(duration: embassy_time::Duration) -> oer_portable_time::Duration {
    oer_portable_time::Duration::from_micros(duration.as_micros())
}

/// A role's network device, the station's or the access point's: the boot
/// hands it to the group ([`super::GroupPeripherals`]), where
/// [`Network::start`] takes it. Dropped without it, it tells its role that
/// no network takes its frames.
pub struct Device {
    network: Option<(
        wifi::Network<'static, Notifies>,
        Transmit,
        wifi::NetworkStopper<'static, Notifies>,
    )>,
    address: [u8; 6],
    seed: u64,
}

impl Drop for Device {
    fn drop(&mut self) {
        if let Some((network, transmit, stopper)) = self.network.take() {
            stopper.no_network(network, transmit);
        }
    }
}

/// The urgent source the doorbell admits, for both executors.
pub(crate) type Urgent = UrgentSource<FromCpu3Irq>;
