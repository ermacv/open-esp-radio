//! The S31 access point: the S31 data port under the access point's
//! runner (`oer_protocols_ieee80211_runtime::access_point`).
//!
//! The composition's runner waits for the application's start, which names
//! the channel and the BSS, then starts the radio and installs the port as
//! the station's composition does, gives the runner the installation's access-point
//! tokens, its tickets and the first [`AP_BANK`] transmit buffers, and runs
//! it with the access point's own engine. Every other pool buffer is the
//! network's transmit memory, held by the station's transmit owner over the
//! station's network channels ([`NetworkChannels`]): the network writes each
//! frame into one, the
//! runner writes its MPDU in place and sends that buffer, and gives it back
//! once its frame ended. The station's tokens of the
//! installation stay unused: an access-point-only image runs no station.
//! Once the runner stopped the radio, the composition releases the radio's
//! membership as the station's does.

use crate::access_point_end::{Retired, Stop};
use crate::chip::{Binding, Config, InitializationError, Resources};
use crate::radio::{self, Port};
use crate::supply::{
    LocalEnd, LocalTransmit, LocalTxState, NETWORK_BUFFERS, NetworkTx, NetworkTxEnd, PORT_BUFFERS,
    TxMemory,
};
use crate::{Ch, LentRx, Network, NetworkChannels, NetworkStopState, NetworkStopper, Notifies};
use oer_chips_esp32s31_radio_ieee80211_backend::port::{BufferClass, PortInstallation, TX_TICKETS};
use oer_chips_family_radio_ieee80211_policy::{access_point::ADVERTISEMENT, ccmp, lmac};
use oer_portable_embassy_time::EmbassyClock;
use oer_protocols_ieee80211_contracts_port::{GroupKeyId, PhyRate, TxBufferClass as _};
use oer_protocols_ieee80211_core_roles_ap::{
    AccessPointClientLimit, AccessPointInactiveTimeout, AccessPointPeerStorage, AccessPointService,
    beacon::ApBeacon, engine::ApEngine, security_policy::ApSecurityPolicy,
};
use oer_protocols_ieee80211_core_security_rsn::{PmkDerivation, frames::RsnGtk};
use oer_protocols_ieee80211_runtime::access_point::{
    AP_BUFFERS, AP_KEYS, AP_TX_DEPTH, ApEnd, ApNonces, ApPolicy, ApResources, ApRunner, ApShared,
    ApStatus, ApUnsupported, GROUP_KEY_ID, serves_beacons,
};
use oer_protocols_ieee80211_wire::{
    beacon::AP_BEACON_CAPACITY, phy::LegacyRate, security::spp::Interface,
    sequence::SequenceNumber, ssid::Ssid,
};

/// The transmit buffers the access point holds: the port keeps them from
/// the pool's first places.
pub const AP_BANK: u8 = AP_BUFFERS as u8;

const _: () = {
    if AP_BUFFERS > oer_chips_esp32s31_radio_ieee80211_backend::port::TX_BUFFERS {
        panic!("the access point's buffers come from the transmit pool");
    }
    if AP_BUFFERS > u8::MAX as usize {
        panic!("the bank counts the access point's buffers in a u8");
    }
    if <BufferClass<{ crate::chip::TX }>>::CAPACITY < AP_BEACON_CAPACITY {
        panic!("a beacon fits one transmit buffer");
    }
    // The network's memory is every place past the bank, as many as the
    // runner's queue back holds, so each one always finds its way home.
    if AP_BANK < PORT_BUFFERS {
        panic!("the bank holds at least the station's port buffers");
    }
    if NETWORK_BUFFERS > AP_TX_DEPTH {
        panic!("the runner's way home holds every network memory");
    }
};

/// The access point's runner's network end: the station's, whole.
pub type ApNetworkEnds<W> = oer_protocols_ieee80211_runtime::datapath::NetworkEnds<
    'static,
    TxMemory,
    LentRx<<W as Binding>::Notifies>,
    NetworkStopState<<W as Binding>::Notifies>,
    { crate::TX_DEPTH },
    { crate::RX_DEPTH },
    Ch<<W as Binding>::Notifies>,
>;

/// The link the access point's runner publishes and its network reads.
type ApLink<W> = oer_protocols_ieee80211_runtime::station::NetworkLink<
    <Ch<<W as Binding>::Notifies> as oer_protocols_ieee80211_runtime::station::Notifies>::NetworkWaker,
>;

/// The transmissions of an individually addressed management frame of the
/// access point (a response, an EAPOL-Key frame, a teardown): a named
/// product policy of this composition, chosen, not taken from a source or
/// a measurement.
pub const MANAGEMENT_TRANSMISSIONS: u8 = 4;

/// [`MANAGEMENT_TRANSMISSIONS`], checked at compile time.
const MANAGEMENT_TRANSMITTED: core::num::NonZeroU8 =
    match core::num::NonZeroU8::new(MANAGEMENT_TRANSMISSIONS) {
        Some(transmissions) => transmissions,
        None => panic!("a management frame is transmitted at least once"),
    };

/// The transmissions of a data frame: the lower MAC's short retry limit,
/// checked at compile time.
const DATA_TRANSMITTED: core::num::NonZeroU8 =
    match core::num::NonZeroU8::new(lmac::SHORT_RETRY_LIMIT) {
        Some(transmissions) => transmissions,
        None => panic!("a data frame is transmitted at least once"),
    };

/// The group key's KeyID, checked at compile time.
const GROUP_KEY: GroupKeyId = match GroupKeyId::new(GROUP_KEY_ID) {
    Some(key_id) => key_id,
    None => panic!("the group key's KeyID is one of the group's"),
};

/// The BSS the access point serves.
pub struct ApSettings {
    /// Its SSID.
    pub ssid: Ssid,
    /// Its WPA2-Personal passphrase: 8 to 63 characters, erased on drop.
    pub passphrase: zeroize::Zeroizing<heapless::Vec<u8, 63>>,
    /// The beacon interval, in TU.
    pub beacon_interval_tu: u16,
    /// The DTIM period, in beacons.
    pub dtim_period: u8,
    /// The stations it admits at most.
    pub client_limit: AccessPointClientLimit,
}

impl core::fmt::Debug for ApSettings {
    /// Every setting but the passphrase.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ApSettings")
            .field("ssid_octets", &self.ssid.as_bytes().len())
            .field("beacon_interval_tu", &self.beacon_interval_tu)
            .field("dtim_period", &self.dtim_period)
            .finish_non_exhaustive()
    }
}

/// Why the settings are not a BSS the access point serves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApSettingsError {
    /// The SSID is empty or longer than 32 octets.
    Ssid,
    /// The passphrase is not 8 to 63 characters.
    Passphrase,
    /// The client limit is zero or beyond the access point's peers.
    ClientLimit,
}

impl ApSettings {
    /// A WPA2-Personal BSS named `ssid` with `passphrase`, the vendor's
    /// default beacon interval of 100 TU and DTIM period of 1, admitting
    /// `clients` stations.
    ///
    /// # Errors
    ///
    /// The value that is not a BSS's.
    pub fn wpa2(ssid: &[u8], passphrase: &[u8], clients: u8) -> Result<Self, ApSettingsError> {
        let ssid = Ssid::new(ssid).map_err(|_| ApSettingsError::Ssid)?;
        if !(8..=63).contains(&passphrase.len()) {
            return Err(ApSettingsError::Passphrase);
        }
        let passphrase = zeroize::Zeroizing::new(
            heapless::Vec::from_slice(passphrase).map_err(|_| ApSettingsError::Passphrase)?,
        );
        let client_limit =
            AccessPointClientLimit::new(clients).map_err(|_| ApSettingsError::ClientLimit)?;
        Ok(Self {
            ssid,
            passphrase,
            beacon_interval_tu: 100,
            dtim_period: 1,
            client_limit,
        })
    }
}

/// The storage of one access point, placed once by the image. Its active
/// runner and completed end have separate permanent homes: an unfinished
/// end can keep the active runner while the application owns that end.
pub struct AccessPointStorage<W: Binding> {
    completed: oer_portable_memory::StaticPlace<AccessPointEnd<W>>,
    active: oer_portable_memory::StaticPlace<ApRunnerOf<W>>,
    peers: AccessPointPeerStorage,
    template: [u8; AP_BEACON_CAPACITY],
    /// The network's channels, built for the image's placement of the
    /// network ([`NetworkChannels::new`] or [`NetworkChannels::across_harts`]).
    network: NetworkChannels<W::Notifies, TxMemory>,
    /// What the network's transmit owner keeps of its buffers.
    transmit: LocalTxState,
    shared: ApShared,
    start: StartSlot,
}

impl<W: Binding> AccessPointStorage<W> {
    /// The storage over the network's channels `network`.
    pub const fn new(network: NetworkChannels<W::Notifies, TxMemory>) -> Self {
        Self {
            completed: oer_portable_memory::StaticPlace::new(),
            active: oer_portable_memory::StaticPlace::new(),
            peers: AccessPointPeerStorage::new(),
            template: [0; AP_BEACON_CAPACITY],
            network,
            transmit: LocalTxState::new(),
            shared: ApShared::new(),
            start: StartSlot::new(),
        }
    }
}

/// The application's one start of the access point, until its runner takes
/// it.
struct StartSlot {
    start: core::cell::Cell<Option<(Config, ApSettings)>>,
    /// Whether a start was given: a second one is refused.
    given: core::cell::Cell<bool>,
    /// How the runner ended, once it did.
    ended: core::cell::Cell<Option<ApExit>>,
    waker: oer_portable_sync::PriorityWaker,
}

impl StartSlot {
    const fn new() -> Self {
        Self {
            start: core::cell::Cell::new(None),
            given: core::cell::Cell::new(false),
            ended: core::cell::Cell::new(None),
            waker: oer_portable_sync::PriorityWaker::new(),
        }
    }

    /// Wait for the start, or for a stop asked before it.
    async fn take(&self, shared: &ApShared) -> Option<(Config, ApSettings)> {
        let start = core::future::poll_fn(|cx| {
            self.waker.register(cx.waker());
            match self.start.take() {
                Some(start) => core::task::Poll::Ready(start),
                None => core::task::Poll::Pending,
            }
        });
        match embassy_futures::select::select(start, shared.stop_requested()).await {
            embassy_futures::select::Either::First(start) => Some(start),
            embassy_futures::select::Either::Second(()) => None,
        }
    }
}

/// The application's end of the access point.
pub struct AccessPointControl {
    shared: &'static ApShared,
    start: &'static StartSlot,
}

/// Why the access point refused a start.
#[derive(Debug)]
pub enum StartRefused {
    /// It was started once already; the settings come back.
    AlreadyStarted(ApSettings),
    /// The S31 port's capabilities do not serve the runner's beacons: no
    /// beacon Timestamp rule is declared for the S31
    /// ([`serves_beacons`]). Nothing started, the radio included; the
    /// settings come back, and the access point was not started (a stop
    /// ends its runner without a radio).
    Unsupported {
        /// The settings, unchanged.
        settings: ApSettings,
        /// What the port lacks.
        cause: ApUnsupported,
    },
}

/// Whether the S31 port serves the access point: refused before anything
/// starts, the settings returned.
fn check_start(settings: ApSettings) -> Result<ApSettings, StartRefused> {
    match serves_beacons::<radio::Caps>() {
        Ok(()) => Ok(settings),
        Err(cause) => Err(StartRefused::Unsupported { settings, cause }),
    }
}

impl AccessPointControl {
    /// Start the access point on `config`'s channel with `settings`: its
    /// runner derives the PMK, starts the radio and beacons. The start is
    /// the access point's one; a stop asked before it ends the runner
    /// without a radio.
    ///
    /// # Errors
    /// A port that cannot beacon refuses before anything starts
    /// ([`StartRefused::Unsupported`]); a second start returns its
    /// settings. Either way the settings come back.
    pub fn start(&self, config: Config, settings: ApSettings) -> Result<(), StartRefused> {
        let settings = check_start(settings)?;
        if self.start.given.replace(true) {
            return Err(StartRefused::AlreadyStarted(settings));
        }
        self.start.start.set(Some((config, settings)));
        self.start.waker.wake();
        Ok(())
    }

    /// The access point's status now.
    pub fn status(&self) -> ApStatus {
        self.shared.status()
    }

    /// How the access point's runner ended, or how far its end came.
    pub fn exit(&self) -> Option<ApExit> {
        self.start.ended.get()
    }

    /// Wait for a status other than `seen`.
    pub async fn changed(&self, seen: ApStatus) -> ApStatus {
        self.shared.changed(seen).await
    }

    /// Ask the access point to stop: its runner tears its peers down and
    /// stops the radio.
    pub fn stop(&self) {
        self.shared.request_stop();
    }
}

/// Asks a running access point to stop.
pub struct AccessPointStop(&'static ApShared);

impl AccessPointStop {
    /// Ask the access point to stop: its runner tears its peers down and
    /// stops the radio.
    pub fn request_stop(&self) {
        self.0.request_stop();
    }
}

/// The ANonces of the access point's handshakes.
pub struct Nonces(&'static mut crate::NonceSource);

impl ApNonces for Nonces {
    fn nonce(&mut self) -> Option<[u8; 32]> {
        self.0.next_nonce().ok()
    }
}

pub use crate::access_point_end::{
    ApEnded, ApExitOf, UnfinishedAccessPoint, UnkeptItems, UnkeptStop,
};
pub use crate::unfinished::{Phase, UNKEPT_ITEMS, Unfinished};
/// How the access point ended, or how far its end came.
pub type ApExit = ApExitOf<InitializationError>;

/// The access point's task: it owns the radio.
pub struct AccessPointRunner<W: Binding> {
    completed: &'static mut oer_portable_memory::StaticPlace<AccessPointEnd<W>>,
    active: &'static mut oer_portable_memory::StaticPlace<ApRunnerOf<W>>,
    boot: radio::Boot<W>,
    start: &'static StartSlot,
    network: ApNetworkEnds<W>,
    link: &'static ApLink<W>,
    peers: &'static mut AccessPointPeerStorage,
    template: &'static mut [u8; AP_BEACON_CAPACITY],
    shared: &'static ApShared,
    nonces: Nonces,
}

/// How the access point ended: a completed retire (or a run that never
/// installed), or an installation that did not retire, whole, which the
/// caller holds from here.
#[allow(
    clippy::large_enum_variant,
    reason = "no-alloc end returns every owner by value, as the install's refusal does"
)]
#[must_use = "an unfinished installation holds the runner, its port and their owners"]
pub enum AccessPointEnd<W: Binding> {
    /// Its exit; nothing of it is left.
    Ended(ApExit),
    /// The installation that did not retire: its runner with the port,
    /// the items a port call could not keep, the cleanup's receipt, its
    /// exit as the cause, and its phase.
    Unfinished(UnfinishedAccessPointOf<W>),
    /// The port refused its installation: its exit, every input the port
    /// was handed, and every other input of the runner, whole (its boot is
    /// in the port's parts).
    Refused {
        exit: ApExit,
        inputs: radio::RefusedInstall<W>,
        runner: ApInputs<W>,
    },
    /// A stage before the port's installation refused, or a stop came
    /// before the start: its exit, the registry and the minted
    /// installation it had taken, and every other input of the runner
    /// ([`ApInputs`]), whole.
    NotInstalled {
        exit: ApExit,
        inputs: radio::NotInstalled<W>,
        runner: ApInputs<W>,
    },
}

/// Ownership of an access point's end in its permanent home. Moving this
/// handle moves no installation; dropping it destroys the end and every
/// retained owner exactly once. The end can be inspected through `Deref`.
pub type AccessPointEndOwner<W> = oer_portable_memory::Placed<'static, AccessPointEnd<W>>;

/// What the access point's runner was handed that a refusal before its
/// port's installation did not take, whole: its boot, or what a refused
/// install left of it once the radio left ([`radio::BootLeft`]), its
/// network end, its nonces, the settings it was started with, and its
/// peers' and beacon's storage, or the service and beacon built on them.
#[must_use = "the inputs hold the radio's owners"]
pub struct ApInputs<W: Binding> {
    /// The boot, or what is left of it; `None` once the port's parts hold
    /// it ([`AccessPointEnd::Refused`]).
    pub boot: Option<radio::BootLeft<W>>,
    /// The runner's network end.
    pub network: ApNetworkEnds<W>,
    /// The link the runner publishes.
    pub link: &'static ApLink<W>,
    /// The handshakes' nonce source.
    pub nonces: Nonces,
    /// The settings of the start, when one came.
    pub settings: Option<ApSettings>,
    /// The peers' storage, or the service built on it.
    pub peers: ApPeers,
    /// The beacon's storage, or the beacon built on it.
    pub template: ApTemplate,
}

/// The access point's peers' storage, as a refusal left it.
pub enum ApPeers {
    /// The storage: nothing was built on it.
    Storage(&'static mut AccessPointPeerStorage),
    /// The service built on it.
    Service(AccessPointService<'static, Interface>),
    /// The service's construction refused: its loan of the storage, back
    /// as it was.
    Lent(oer_protocols_ieee80211_core_roles_ap::AccessPointPeers<'static>),
}

/// The access point's beacon storage, as a refusal left it.
pub enum ApTemplate {
    /// The storage: nothing was built on it.
    Storage(&'static mut [u8; AP_BEACON_CAPACITY]),
    /// The beacon built on it.
    Beacon(ApBeacon<'static>),
}

impl<W: Binding> AccessPointEnd<W> {
    /// The exit the access point ended with.
    pub fn exit(&self) -> ApExit {
        match self {
            Self::Ended(exit) | Self::Refused { exit, .. } | Self::NotInstalled { exit, .. } => {
                *exit
            }
            Self::Unfinished(unfinished) => unfinished.cause,
        }
    }
}

/// An access point's installation of `W` that did not retire.
pub type UnfinishedAccessPointOf<W> = crate::access_point_end::UnfinishedAccessPoint<
    oer_portable_memory::Placed<'static, ApRunnerOf<W>>,
    oer_protocols_ieee80211_contracts_port::Unkept<
        'static,
        <Port<W> as oer_protocols_ieee80211_contracts_port::DataPort<'static>>::Types,
    >,
    <Port<W> as oer_protocols_ieee80211_contracts_port::DataPort<'static>>::Admitted,
    radio::Freed<W>,
    InitializationError,
>;

/// Assemble the access point over `resources`: its control, the network's
/// ends of the station's network channels (its device, its transmit owner
/// over every pool buffer past the bank, and its stop), and its task, which
/// waits for the control's start. `config` holds
/// the radio's regulatory configuration until the start names the one it
/// runs on.
pub fn access_point<W: Binding>(
    resources: Resources<W>,
    config: Config,
    storage: &'static mut AccessPointStorage<W>,
    nonces: &'static mut crate::NonceSource,
) -> (
    AccessPointControl,
    Network<'static, W::Notifies>,
    LocalTransmit<'static, W::Notifies>,
    NetworkStopper<'static, W::Notifies>,
    AccessPointRunner<W>,
) {
    let Resources {
        events: _,
        storage: station,
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
    let pool = station
        .pool
        .try_init(
            oer_chips_esp32s31_radio_ieee80211_backend::port::TxPool::pin(pool)
                .unwrap_or_else(|_| W::fail(InitializationError::Tx)),
        )
        .unwrap_or_else(|| W::fail(InitializationError::Storage));
    let pool: &'static _ = pool;
    // The network holds every buffer past the bank, before its end exists.
    let memory =
        NetworkTx::claim(pool, AP_BANK).unwrap_or_else(|_| W::fail(InitializationError::Tx));
    let AccessPointStorage {
        completed,
        active,
        peers,
        template,
        network,
        transmit,
        shared,
        start,
    } = storage;
    let (device, owner, stopper, ends, state) = network
        .split(|queues| <LocalEnd as NetworkTxEnd<W::Notifies>>::end(queues, memory, transmit));
    let shared: &'static ApShared = shared;
    let start: &'static StartSlot = start;
    (
        AccessPointControl { shared, start },
        device,
        owner,
        stopper,
        AccessPointRunner {
            completed,
            active,
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
                storage: &mut station.driver,
            },
            start,
            network: ends.ends(),
            link: state.network_link(),
            peers,
            template,
            shared,
            nonces: Nonces(nonces),
        },
    )
}

impl<W: Binding> AccessPointRunner<W> {
    /// A handle that asks the running access point to stop, as its
    /// control does.
    pub fn stop_handle(&self) -> AccessPointStop {
        AccessPointStop(self.shared)
    }

    /// Wait for the start, run the access point until its stop, then
    /// release the radio. A stop before the start ends it at once, with no
    /// radio started. The returned owner keeps the end in this access
    /// point's storage, including an unfinished installation; it moves only
    /// the handle to that storage.
    pub async fn run(self) -> AccessPointEndOwner<W> {
        let slot = self.start;
        let end = self.serve().await;
        slot.ended.set(Some(end.exit()));
        end
    }

    async fn serve(self) -> AccessPointEndOwner<W> {
        let Self {
            completed,
            active,
            mut boot,
            start: slot,
            network,
            link,
            peers,
            template,
            shared,
            mut nonces,
        } = self;
        // The end owns large refused/unfinished installations. Construct it
        // in its permanent home; only its destruction right crosses the
        // serve/run/task/mailbox boundaries.
        macro_rules! finish {
            ($end:expr) => {
                return oer_portable_memory::Placed::init(completed.uninit(), |place| {
                    place.write($end)
                })
            };
        }
        // A refusal before the port's installation returns every input.
        macro_rules! refuse {
            ($exit:expr, $installations:expr, $minted:expr, $boot:expr, $settings:expr, $peers:expr, $template:expr) => {
                finish!(AccessPointEnd::NotInstalled {
                    exit: $exit,
                    inputs: radio::NotInstalled::<W> {
                        installations: $installations,
                        minted: $minted,
                    },
                    runner: ApInputs {
                        boot: Some($boot),
                        network,
                        link,
                        nonces,
                        settings: $settings,
                        peers: $peers,
                        template: $template,
                    },
                })
            };
        }
        let took_nothing =
            |cause| ApExit::Unavailable(crate::failure::Unavailable::took_nothing(cause));
        let Some((config, settings)) = slot.take(shared).await else {
            refuse!(
                ApExit::Ended(ApEnded::Stopped),
                None,
                None,
                radio::BootLeft::Boot(boot),
                None,
                ApPeers::Storage(peers),
                ApTemplate::Storage(template)
            );
        };
        boot.config = config;
        let Some(installations) = W::take_installations() else {
            refuse!(
                took_nothing(InitializationError::Storage),
                None,
                None,
                radio::BootLeft::Boot(boot),
                Some(settings),
                ApPeers::Storage(peers),
                ApTemplate::Storage(template)
            );
        };
        let (installations, minted) = match crate::station_end::first_installation(
            installations,
            radio::Installations::<W>::first,
        ) {
            Ok(first) => first,
            Err(refused) => {
                refuse!(
                    took_nothing(InitializationError::Registration),
                    refused.installations,
                    refused.minted,
                    radio::BootLeft::Boot(boot),
                    Some(settings),
                    ApPeers::Storage(peers),
                    ApTemplate::Storage(template)
                );
            }
        };
        // The PMK of the passphrase, a step at a time between the
        // executor's other work.
        let Ok(mut derivation) = PmkDerivation::new(&settings.passphrase, settings.ssid.as_bytes())
        else {
            refuse!(
                took_nothing(InitializationError::Ssid),
                Some(installations),
                Some(minted),
                radio::BootLeft::Boot(boot),
                Some(settings),
                ApPeers::Storage(peers),
                ApTemplate::Storage(template)
            );
        };
        let pmk = loop {
            if let Some(pmk) = derivation.step() {
                break pmk;
            }
            embassy_futures::yield_now().await;
        };
        let Some(gtk_bytes) = nonces.nonce() else {
            refuse!(
                took_nothing(InitializationError::Storage),
                Some(installations),
                Some(minted),
                radio::BootLeft::Boot(boot),
                Some(settings),
                ApPeers::Storage(peers),
                ApTemplate::Storage(template)
            );
        };
        let mut gtk = [0; 16];
        for (octet, source) in gtk.iter_mut().zip(gtk_bytes) {
            *octet = source;
        }
        // Everything the access point builds from its settings is built
        // before the radio starts: a refusal here takes nothing.
        let Ok(address) =
            oer_protocols_ieee80211_wire::address::IndividualAddress::for_interface(boot.ap)
                .map(|address| address.get())
        else {
            refuse!(
                took_nothing(InitializationError::Address),
                Some(installations),
                Some(minted),
                radio::BootLeft::Boot(boot),
                Some(settings),
                ApPeers::Storage(peers),
                ApTemplate::Storage(template)
            );
        };
        let channel = config.supported;
        let service = match AccessPointService::<Interface>::new(
            address,
            pmk,
            RsnGtk::new(GROUP_KEY, true, &gtk),
            1,
            settings.client_limit,
            AccessPointInactiveTimeout::default(),
            peers.lend(),
        ) {
            Ok(service) => service,
            // The PMK and GTK it returns were derived here from the
            // settings and a nonce: they end with the refusal; the storage
            // comes back.
            Err(refused) => refuse!(
                took_nothing(InitializationError::Storage),
                Some(installations),
                Some(minted),
                radio::BootLeft::Boot(boot),
                Some(settings),
                ApPeers::Lent(refused.peer_storage),
                ApTemplate::Storage(template)
            ),
        };
        let wire_channel = channel.channel();
        let beacon = match ApBeacon::new(
            template,
            &ADVERTISEMENT,
            address,
            &settings.ssid,
            wire_channel,
            settings.beacon_interval_tu,
            settings.dtim_period,
            SequenceNumber::ZERO,
            ApSecurityPolicy::Wpa2Personal,
        ) {
            Ok(beacon) => beacon,
            Err(refused) => refuse!(
                took_nothing(InitializationError::Ssid),
                Some(installations),
                Some(minted),
                radio::BootLeft::Boot(boot),
                Some(settings),
                ApPeers::Service(service),
                ApTemplate::Storage(refused.storage)
            ),
        };
        let (parts, driver) = match radio::install(boot, config, AP_BANK).await {
            Ok(installed) => installed,
            Err(refused) => {
                // A started radio leaves before the refusal is told, its
                // outcome told with it; every other owner comes back.
                let (unavailable, left) =
                    crate::release::refused(refused.cause, refused.owners).await;
                refuse!(
                    ApExit::Unavailable(unavailable),
                    Some(installations),
                    Some(minted),
                    left,
                    Some(settings),
                    ApPeers::Service(service),
                    ApTemplate::Beacon(beacon)
                );
            }
        };
        let PortInstallation {
            port,
            commands,
            resources,
        } = match Port::<W>::install(parts, minted, driver) {
            Ok(installation) => installation,
            Err((parts, minted, driver)) => {
                // The started radio leaves from the refused parts' device,
                // which the port never started; its outcome is the cleanup,
                // and every input comes back.
                let unavailable =
                    radio::port_refused::<W>(InitializationError::Backend, &mut *parts.device)
                        .await;
                finish!(AccessPointEnd::Refused {
                    exit: ApExit::Unavailable(unavailable),
                    inputs: radio::RefusedInstall {
                        parts,
                        minted,
                        driver,
                        installations,
                    },
                    runner: ApInputs {
                        boot: None,
                        network,
                        link,
                        nonces,
                        settings: Some(settings),
                        peers: ApPeers::Service(service),
                        template: ApTemplate::Beacon(beacon),
                    },
                });
            }
        };
        // The access point holds every ticket (`TX_TICKETS` is at most its
        // `AP_TICKETS`) and the first `AP_BUFFERS` buffer places, which
        // hold the port's bank; the other places hold no memory and have
        // no owner here, as the station's.
        let tickets = heapless::Vec::from_array(resources.tickets);
        let [b0, b1, b2, b3, b4, b5, b6, b7, ..] = resources.buffers;
        const {
            if AP_BUFFERS != 8 {
                panic!("the access point takes eight buffer places");
            }
        }
        let buffers = heapless::Vec::from_array([b0, b1, b2, b3, b4, b5, b6, b7]);
        let (start, bank) = resources.access_point_association.into_parts();
        let (pairwise_slots, group_slots) = bank.into_parts();
        let mut pairwise = heapless::Vec::new();
        for slot in pairwise_slots {
            if pairwise.len() < AP_KEYS && pairwise.push(slot).is_err() {
                break;
            }
        }
        let [group, ..] = group_slots;
        let ap_resources = ApResources::<'static, Port<W>, BufferClass<{ crate::chip::TX }>> {
            tickets,
            access: resources.terminal_access,
            buffers,
            configure: resources.access_point_commands.configure.with_context(
                oer_protocols_ieee80211_core_roles_sta::machine::StaConfigurationContext::detached(
                ),
            ),
            association: start,
            pairwise,
            group,
            group_permission: resources.access_point_group_permission,
            beacon: resources.access_point_beacon,
        };
        let engine = ApEngine::new(service, beacon, ADVERTISEMENT, wire_channel, GROUP_KEY);
        let policy = ApPolicy {
            // The BSS's lowest basic rate, the archive's beacon rate.
            management_rate: PhyRate::Legacy(LegacyRate::Dsss1M),
            management_transmissions: MANAGEMENT_TRANSMITTED,
            data_transmissions: DATA_TRANSMITTED,
            packet_numbers: ccmp::TX_PACKET_NUMBER_STEP,
            group_generation: 1,
            network_stop: config.stop.drain,
        };
        let runner = oer_portable_memory::Placed::init(active.uninit(), |place| {
            place.write(ApRunner::new(
                port,
                commands,
                ap_resources,
                engine,
                policy,
                channel,
                gtk,
                network,
                link,
                (),
                nonces,
            ))
        });
        let mut runner = runner;
        let end = runner.run(shared, &EmbassyClock).await;
        let (ended, stop) = match end {
            ApEnd::Stopped => (ApEnded::Stopped, Stop::Confirmed),
            ApEnd::NetworkLost(observed) => (ApEnded::NetworkLost(observed), Stop::Confirmed),
            // The radio's stop was confirmed; the retire's watch goes on
            // with the network's return.
            ApEnd::NetworkReturning(_) => (ApEnded::Stopped, Stop::Owed),
            ApEnd::StopUnconfirmed(observed) => (
                ApEnded::Stopped,
                Stop::Unconfirmed(crate::failure::convert::stop_unconfirmed(observed)),
            ),
            ApEnd::StartRefused(refused) => (
                ApEnded::StartRefused(crate::failure::convert::start_refused(refused)),
                Stop::Owed,
            ),
            ApEnd::Faulted(fault) => (
                ApEnded::Faulted(crate::failure::convert::backend_fault(fault)),
                Stop::Owed,
            ),
            ApEnd::Contract(contract) => (ApEnded::Contract(contract), Stop::Owed),
            ApEnd::Refused(refusal) => (ApEnded::Refused(refusal), Stop::Owed),
            ApEnd::Unrecycled(fault) => (ApEnded::Unrecycled(fault), Stop::Owed),
            // The start's check refuses such a port before the radio
            // starts; a run that still ended here never asked it to start.
            ApEnd::Unsupported(cause) => (ApEnded::Unsupported(cause), Stop::Owed),
            // The retire takes the item from the runner, and the stop's
            // watch goes on.
            ApEnd::Unkept => (ApEnded::Unkept, Stop::Owed),
            ApEnd::Unaccepted => (ApEnded::Unaccepted, Stop::Owed),
        };
        match crate::access_point_end::retire(
            &slot.ended,
            runner,
            ended,
            stop,
            &EmbassyClock,
            radio::RELEASE_RETRY,
            radio::StoppedRadio::release,
        )
        .await
        {
            Retired::Completed(exit) => finish!(AccessPointEnd::Ended(exit)),
            Retired::Unfinished(unfinished) => finish!(AccessPointEnd::Unfinished(unfinished)),
        }
    }
}

/// The access point's runner over the S31 port.
pub type ApRunnerOf<W> = ApRunner<
    'static,
    'static,
    'static,
    Port<W>,
    radio::Commands<W>,
    BufferClass<{ crate::chip::TX }>,
    Interface,
    Nonces,
    LentRx<<W as Binding>::Notifies>,
    NetworkStopState<<W as Binding>::Notifies>,
    { crate::TX_DEPTH },
    { crate::RX_DEPTH },
    Ch<<W as Binding>::Notifies>,
>;

impl<W: Binding> crate::access_point_end::Retiring for ApRunnerOf<W> {
    type Port = Port<W>;
    type Unkept = oer_protocols_ieee80211_contracts_port::Unkept<
        'static,
        <Port<W> as oer_protocols_ieee80211_contracts_port::DataPort<'static>>::Types,
    >;
    fn end_transmit_tokens(
        &mut self,
    ) -> oer_protocols_ieee80211_runtime::datapath::TransmitTokensEnded {
        ApRunner::end_transmit_tokens(self)
    }
    fn finish_stop(
        &mut self,
        mut unconfirmed: impl FnMut(
            oer_protocols_ieee80211_contracts_port::StopUnconfirmed<crate::failure::BackendFault>,
        ),
    ) -> impl core::future::Future<
        Output = Result<
            oer_protocols_ieee80211_runtime::access_point::StopWatch<crate::failure::BackendFault>,
            oer_protocols_ieee80211_contracts_port::ReceiveAccounting,
        >,
    > {
        use oer_protocols_ieee80211_runtime::access_point::StopWatch;
        let watch = ApRunner::finish_stop(self, &EmbassyClock, move |observed| {
            unconfirmed(crate::failure::convert::stop_unconfirmed(observed))
        });
        async {
            watch.await.map(|watched| match watched {
                StopWatch::Confirmed => StopWatch::Confirmed,
                StopWatch::NetworkLost(observed) => StopWatch::NetworkLost(observed),
                StopWatch::NetworkReturning(observed) => StopWatch::NetworkReturning(observed),
                StopWatch::Unkept => StopWatch::Unkept,
                StopWatch::Faulted { fault, observed } => StopWatch::Faulted {
                    fault: fault.map(crate::failure::convert::backend_fault),
                    observed: crate::failure::convert::stop_unconfirmed(observed),
                },
                StopWatch::DeviceFailed {
                    fault,
                    observations,
                } => StopWatch::DeviceFailed {
                    fault: crate::failure::convert::backend_fault(fault),
                    observations,
                },
            })
        }
    }
    fn port_mut(&mut self) -> &mut Port<W> {
        ApRunner::port_mut(self)
    }
    fn take_unkept(&mut self) -> Option<Self::Unkept> {
        ApRunner::take_unkept(self)
    }
    type Unaccepted =
        <Port<W> as oer_protocols_ieee80211_contracts_port::DataPort<'static>>::Admitted;
    fn take_unaccepted(&mut self) -> Option<Self::Unaccepted> {
        ApRunner::take_unaccepted(self)
    }
}

const _: () = {
    if TX_TICKETS > oer_protocols_ieee80211_runtime::access_point::AP_TICKETS {
        panic!("the access point holds every ticket of the installation");
    }
};
