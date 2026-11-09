//! The S31 access point: the S31 data port under the access point's
//! runner (`oer_protocols_ieee80211_runtime::access_point`).
//!
//! The composition's runner waits for the application's start, which names
//! the channel and the BSS, then starts the radio and installs the port as
//! the station's composition does, gives the runner the installation's access-point
//! tokens, its tickets and the first [`AP_BANK`] transmit buffers, and runs
//! it with the access point's own engine. The station's tokens of the
//! installation stay unused: an access-point-only image runs no station.
//! Once the runner stopped the radio, the composition releases the radio's
//! membership as the station's does.

use crate::chip::{Binding, Config, InitializationError, Resources};
use crate::radio::{self, Port};
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
    AP_BUFFERS, AP_KEYS, ApEnd, ApNonces, ApPolicy, ApResources, ApRunner, ApShared, ApStatus,
    ApUnsupported, GROUP_KEY_ID,
    network::{ApNetDevice, ApNetTransmit, ApNetworkEnd, ApNetworkStorage},
    serves_beacons,
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
};

/// The transmissions of an individually addressed management frame of the
/// access point (a response, an EAPOL-Key frame, a teardown): a named
/// product policy of this composition, chosen, not taken from a source or
/// a measurement.
pub const MANAGEMENT_TRANSMISSIONS: u8 = 4;

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

/// The storage of one access point, placed once by the image.
pub struct AccessPointStorage {
    peers: AccessPointPeerStorage,
    template: [u8; AP_BEACON_CAPACITY],
    network: ApNetworkStorage,
    shared: ApShared,
    start: StartSlot,
}

impl AccessPointStorage {
    pub const fn new() -> Self {
        Self {
            peers: AccessPointPeerStorage::new(),
            template: [0; AP_BEACON_CAPACITY],
            network: ApNetworkStorage::new(),
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

impl Default for AccessPointStorage {
    fn default() -> Self {
        Self::new()
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

/// Why the access point's run ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApEnded {
    /// The application stopped it.
    Stopped,
    /// Its radio did not start.
    StartRefused(
        oer_protocols_ieee80211_contracts_port::StartRefused<crate::failure::BackendFault>,
    ),
    /// The port reported its named fault.
    Faulted(crate::failure::BackendFault),
    /// The runner's own contract broke.
    Contract(oer_protocols_ieee80211_runtime::access_point::ApContract),
    /// The port refused one of its commands.
    Refused(oer_protocols_ieee80211_contracts_port::AccessPointRefusal),
    /// The port refused a received unit's recycle for its count; the unit
    /// stayed with the runner and ended with it after the release.
    Unrecycled(oer_protocols_ieee80211_contracts_port::ReceiveAccounting),
    /// The port's capabilities do not serve the access point; the run
    /// never asked the radio to start.
    Unsupported(ApUnsupported),
}

/// How the access point ended, or how far its end came.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApExit {
    /// It ended for its cause; its radio stopped, every owner came back
    /// and the radio was released.
    Ended(ApEnded),
    /// It ended for its cause and its radio's stop is not confirmed yet, as
    /// the port observed it last: the runner keeps every owner the hardware
    /// may still use and watches the stop on. Published while it does; the
    /// run never returns it.
    StopUnconfirmed {
        /// Why it ended.
        ended: ApEnded,
        /// What the port observed of the stop.
        observed: oer_protocols_ieee80211_contracts_port::StopUnconfirmed,
    },
    /// It ended for its cause, its radio's stop was confirmed, and the
    /// radio's release refused: published while the MAC refuses it as
    /// still active and the release tries again; returned for a refusal
    /// the release does not repeat (a leave never answered, a poisoned
    /// domain, a port with no stopped radio).
    ReleaseRefused {
        /// Why it ended.
        ended: ApEnded,
        /// Why the release refused.
        cause: crate::failure::ReleaseCause,
    },
    /// It ended for its cause, and the stop that followed found a received
    /// unit whose recycle the port refused for its count: the unit stayed
    /// with the runner and ended with it after the release.
    Unrecycled {
        /// Why it ended.
        ended: ApEnded,
        /// The count that refused the unit.
        fault: oer_protocols_ieee80211_contracts_port::ReceiveAccounting,
        /// How the release ended.
        released: Result<(), crate::failure::ReleaseCause>,
    },
    /// It never served: its refusal before the runner existed, and how the
    /// owners that refusal returned ended.
    Unavailable(crate::failure::Unavailable<InitializationError>),
}

/// The access point's task: it owns the radio.
pub struct AccessPointRunner<W: Binding> {
    boot: radio::Boot<W>,
    start: &'static StartSlot,
    network: ApNetworkEnd<'static>,
    peers: &'static mut AccessPointPeerStorage,
    template: &'static mut [u8; AP_BEACON_CAPACITY],
    shared: &'static ApShared,
    nonces: Nonces,
    /// Where a refused release keeps its cleanup's receipt.
    kept: &'static mut Option<radio::Removal<W>>,
}

/// The access point's network device and its sending path.
pub type ApNetwork = ApNetDevice<'static>;

/// Assemble the access point over `resources`: its control, its network
/// device and sending path for the stack's frames `F`, and its task, which
/// waits for the control's start. `config` holds the radio's regulatory
/// configuration until the start names the one it runs on.
pub fn access_point<W: Binding, F>(
    resources: Resources<W>,
    config: Config,
    storage: &'static mut AccessPointStorage,
    nonces: &'static mut crate::NonceSource,
) -> (
    AccessPointControl,
    ApNetwork,
    ApNetTransmit<'static, F>,
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
    let AccessPointStorage {
        peers,
        template,
        network,
        shared,
        start,
    } = storage;
    let (device, transmit, end) = network.split::<F>();
    let shared: &'static ApShared = shared;
    let start: &'static StartSlot = start;
    (
        AccessPointControl { shared, start },
        device,
        transmit,
        AccessPointRunner {
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
            network: end,
            peers,
            template,
            shared,
            nonces: Nonces(nonces),
            kept: &mut station.kept,
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
    /// radio started.
    pub async fn run(self) -> ApExit {
        let slot = self.start;
        let exit = self.serve().await;
        slot.ended.set(Some(exit));
        exit
    }

    async fn serve(self) -> ApExit {
        let Self {
            mut boot,
            start: slot,
            network,
            peers,
            template,
            shared,
            mut nonces,
            kept,
        } = self;
        let Some((config, settings)) = slot.take(shared).await else {
            return ApExit::Ended(ApEnded::Stopped);
        };
        boot.config = config;
        let Some(mut installations) = W::take_installations() else {
            return ApExit::Unavailable(crate::failure::Unavailable::took_nothing(
                InitializationError::Storage,
            ));
        };
        // The PMK of the passphrase, a step at a time between the
        // executor's other work.
        let Ok(mut derivation) = PmkDerivation::new(&settings.passphrase, settings.ssid.as_bytes())
        else {
            return ApExit::Unavailable(crate::failure::Unavailable::took_nothing(
                InitializationError::Ssid,
            ));
        };
        let pmk = loop {
            if let Some(pmk) = derivation.step() {
                break pmk;
            }
            embassy_futures::yield_now().await;
        };
        let Some(gtk_bytes) = nonces.nonce() else {
            return ApExit::Unavailable(crate::failure::Unavailable::took_nothing(
                InitializationError::Storage,
            ));
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
            return ApExit::Unavailable(crate::failure::Unavailable::took_nothing(
                InitializationError::Address,
            ));
        };
        let channel = config.supported;
        let Some(key_id) = GroupKeyId::new(GROUP_KEY_ID) else {
            return ApExit::Unavailable(crate::failure::Unavailable::took_nothing(
                InitializationError::Storage,
            ));
        };
        let Ok(service) = AccessPointService::<Interface>::new(
            address,
            pmk,
            RsnGtk::new(key_id, true, &gtk),
            1,
            settings.client_limit,
            AccessPointInactiveTimeout::default(),
            peers.lend(),
        ) else {
            return ApExit::Unavailable(crate::failure::Unavailable::took_nothing(
                InitializationError::Storage,
            ));
        };
        let wire_channel = channel.channel();
        let Ok(beacon) = ApBeacon::new(
            template,
            &ADVERTISEMENT,
            address,
            &settings.ssid,
            wire_channel,
            settings.beacon_interval_tu,
            settings.dtim_period,
            SequenceNumber::ZERO,
            ApSecurityPolicy::Wpa2Personal,
        ) else {
            return ApExit::Unavailable(crate::failure::Unavailable::took_nothing(
                InitializationError::Ssid,
            ));
        };
        let (Some(management_transmissions), Some(data_transmissions)) = (
            core::num::NonZeroU8::new(MANAGEMENT_TRANSMISSIONS),
            core::num::NonZeroU8::new(lmac::SHORT_RETRY_LIMIT),
        ) else {
            return ApExit::Unavailable(crate::failure::Unavailable::took_nothing(
                InitializationError::Tx,
            ));
        };
        let Some(minted) = installations.first() else {
            return ApExit::Unavailable(crate::failure::Unavailable::took_nothing(
                InitializationError::Registration,
            ));
        };
        let (parts, driver) = match radio::install(boot, config, AP_BANK).await {
            Ok(installed) => installed,
            Err(refused) => {
                // A started radio leaves before the refusal is told, its
                // outcome told with it.
                return ApExit::Unavailable(
                    crate::release::refused(refused.cause, refused.owners).await,
                );
            }
        };
        let PortInstallation {
            port,
            commands,
            resources,
        } = match Port::<W>::install(parts, minted, driver) {
            Ok(installation) => installation,
            Err((parts, _minted)) => {
                // The started radio leaves from the refused parts' device,
                // which the port never started; its outcome is the cleanup.
                return ApExit::Unavailable(
                    radio::port_refused::<W>(InitializationError::Backend, parts.device).await,
                );
            }
        };
        let mut tickets = heapless::Vec::new();
        for ticket in resources.tickets {
            // The access point holds every ticket: `TX_TICKETS` is its
            // `AP_TICKETS`.
            if let Err(ticket) = tickets.push(ticket) {
                drop(ticket);
            }
        }
        let mut buffers = heapless::Vec::new();
        for buffer in resources.buffers.into_iter().take(AP_BUFFERS) {
            if let Err(buffer) = buffers.push(buffer) {
                drop(buffer);
            }
        }
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
        let engine = ApEngine::new(service, beacon, ADVERTISEMENT, wire_channel, key_id);
        let policy = ApPolicy {
            // The BSS's lowest basic rate, the archive's beacon rate.
            management_rate: PhyRate::Legacy(LegacyRate::Dsss1M),
            management_transmissions,
            data_transmissions,
            packet_numbers: ccmp::TX_PACKET_NUMBER_STEP,
            group_generation: 1,
        };
        let runner = ApRunner::new(
            port,
            commands,
            ap_resources,
            engine,
            policy,
            channel,
            gtk,
            network,
            (),
            nonces,
        );
        let mut runner = runner;
        let end = runner.run(shared, &EmbassyClock).await;
        let (ended, stop) = match end {
            ApEnd::Stopped => (ApEnded::Stopped, Stop::Confirmed),
            ApEnd::StopUnconfirmed(observed) => (ApEnded::Stopped, Stop::Unconfirmed(observed)),
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
        };
        retire::<W>(slot, runner, kept, ended, stop).await
    }
}

/// What a run's end left of the radio's stop.
enum Stop {
    /// The port confirmed it.
    Confirmed,
    /// The port did not confirm it yet, as it observed.
    Unconfirmed(oer_protocols_ieee80211_contracts_port::StopUnconfirmed),
    /// The run ended before it.
    Owed,
}

/// The access point's runner over the S31 port.
type ApRunnerOf<W> = ApRunner<
    'static,
    'static,
    'static,
    Port<W>,
    radio::Commands<W>,
    BufferClass<{ crate::chip::TX }>,
    Interface,
    Nonces,
>;

/// Stop the radio of an ended run unless the port confirmed its stop, then
/// release it. Until the port confirms the stop, the runner keeps every
/// owner and watches the stop on its facts and deadline, each observation
/// published as [`ApExit::StopUnconfirmed`]; until the release ends, a
/// release the MAC refused as still active is published as
/// [`ApExit::ReleaseRefused`] and tried again. The owners end with the
/// runner once the stop is confirmed and the release ended; a refused
/// release's cleanup receipt stays in the composition's storage.
async fn retire<W: Binding>(
    slot: &StartSlot,
    mut runner: ApRunnerOf<W>,
    kept: &mut Option<radio::Removal<W>>,
    ended: ApEnded,
    stop: Stop,
) -> ApExit {
    let unconfirmed = |observed| {
        slot.ended
            .set(Some(ApExit::StopUnconfirmed { ended, observed }))
    };
    let finished = match stop {
        Stop::Confirmed => Ok(()),
        Stop::Unconfirmed(observed) => {
            unconfirmed(observed);
            runner.finish_stop(&EmbassyClock, unconfirmed).await
        }
        Stop::Owed => runner.finish_stop(&EmbassyClock, unconfirmed).await,
    };
    // A unit the port refused stays here until the runner ends, after the
    // release.
    let unrecycled = runner.take_unrecycled();
    let refused = |cause| {
        slot.ended
            .set(Some(ApExit::ReleaseRefused { ended, cause }))
    };
    // The cleanup's removal receipt: a refused release keeps it in the
    // composition's place for good; after a release that succeeded it ends
    // with the retired installation.
    let (released, returned) = radio::release::<W>(runner.port_mut(), refused).await;
    drop(unrecycled);
    crate::release::settle(kept, released.is_err(), returned);
    match (ended, finished, released) {
        // A refusal the stop found after another cause.
        (ended, Err(fault), released) if !matches!(ended, ApEnded::Unrecycled(_)) => {
            ApExit::Unrecycled {
                ended,
                fault,
                released,
            }
        }
        (ended, _, Ok(())) => ApExit::Ended(ended),
        (ended, _, Err(cause)) => ApExit::ReleaseRefused { ended, cause },
    }
}

const _: () = {
    if TX_TICKETS > oer_protocols_ieee80211_runtime::access_point::AP_TICKETS {
        panic!("the access point holds every ticket of the installation");
    }
};
